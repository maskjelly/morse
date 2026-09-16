use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};

use crate::llm::{
    retry_request, Block, ChatMessage, ChatRequest, ChatResponse, Provider, Role, Usage,
};
use crate::sse;

pub const DEFAULT_MODEL: &str = "claude-sonnet-4-5";
pub const API_URL: &str = "https://api.anthropic.com/v1/messages";

pub struct Anthropic {
    http: reqwest::Client,
    key: String,
    model: String,
    url: String,
}

impl Anthropic {
    pub fn new(key: String, model: String, url: String) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(300))
            .connect_timeout(Duration::from_secs(15))
            .build()
            .expect("building http client");
        Self {
            http,
            key,
            model,
            url,
        }
    }

    pub fn from_env() -> Option<Self> {
        let key = std::env::var("MORSE_API_KEY")
            .or_else(|_| std::env::var("ANTHROPIC_API_KEY"))
            .ok()?;
        let model = std::env::var("MORSE_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());
        let url = std::env::var("MORSE_BASE_URL").unwrap_or_else(|_| API_URL.to_string());
        Some(Self::new(key, model, url))
    }

    fn messages_json(messages: &[ChatMessage]) -> Value {
        messages
            .iter()
            .map(|m| {
                let role = match m.role {
                    Role::User => "user",
                    Role::Assistant => "assistant",
                };
                let content: Vec<Value> = m
                    .blocks
                    .iter()
                    .filter(|b| match b {
                        Block::Text { text } => !text.trim().is_empty(),
                        _ => true,
                    })
                    .map(|b| match b {
                        Block::Text { text } => json!({"type": "text", "text": text}),
                        Block::ToolUse { id, name, input } => {
                            json!({"type": "tool_use", "id": id, "name": name, "input": input})
                        }
                        Block::ToolResult {
                            tool_use_id,
                            content,
                            is_error,
                        } => json!({
                            "type": "tool_result",
                            "tool_use_id": tool_use_id,
                            "content": content,
                            "is_error": is_error,
                        }),
                    })
                    .collect();
                json!({"role": role, "content": content})
            })
            .collect::<Vec<_>>()
            .into()
    }

    fn tools_json(tools: &[crate::llm::ToolSpec]) -> Value {
        tools
            .iter()
            .map(|t| {
                json!({
                    "name": t.name,
                    "description": t.description,
                    "input_schema": t.schema,
                })
            })
            .collect::<Vec<_>>()
            .into()
    }

    fn body(&self, req: &ChatRequest, stream: bool) -> Value {
        let mut body = json!({
            "model": self.model,
            "max_tokens": req.max_tokens,
            "system": req.system,
            "messages": Self::messages_json(&req.messages),
        });
        if !req.tools.is_empty() {
            body["tools"] = Self::tools_json(&req.tools);
        }
        if stream {
            body["stream"] = json!(true);
        }
        body
    }

    async fn post(&self, body: &Value) -> Result<reqwest::Response> {
        retry_request(|| {
            let request = self
                .http
                .post(&self.url)
                .header("x-api-key", &self.key)
                .header("anthropic-version", "2023-06-01")
                .json(body);
            async move { request.send().await.context("anthropic request failed") }
        })
        .await
    }

    async fn parse_error(resp: reqwest::Response) -> anyhow::Error {
        let status = resp.status();
        let payload: Value = resp.json().await.unwrap_or(Value::Null);
        let msg = payload["error"]["message"]
            .as_str()
            .unwrap_or("unknown error");
        anyhow!("anthropic api error {status}: {msg}")
    }

    fn parse_response(payload: &Value) -> ChatResponse {
        let mut blocks = Vec::new();
        for b in payload["content"].as_array().cloned().unwrap_or_default() {
            match b["type"].as_str().unwrap_or_default() {
                "text" => blocks.push(Block::Text {
                    text: b["text"].as_str().unwrap_or_default().to_string(),
                }),
                "tool_use" => blocks.push(Block::ToolUse {
                    id: b["id"].as_str().unwrap_or_default().to_string(),
                    name: b["name"].as_str().unwrap_or_default().to_string(),
                    input: b.get("input").cloned().unwrap_or(Value::Null),
                }),
                _ => {}
            }
        }
        ChatResponse {
            blocks,
            stop_reason: payload["stop_reason"]
                .as_str()
                .unwrap_or("end_turn")
                .to_string(),
            usage: Usage {
                input_tokens: payload["usage"]["input_tokens"].as_u64().unwrap_or(0),
                output_tokens: payload["usage"]["output_tokens"].as_u64().unwrap_or(0),
            },
        }
    }
}

#[derive(Default)]
struct PendingBlock {
    text: String,
    tool_id: String,
    tool_name: String,
    tool_json: String,
}

fn apply_event(
    event: &Value,
    blocks: &mut BTreeMap<u64, PendingBlock>,
    usage: &mut Usage,
    stop_reason: &mut String,
    on_text: &mut (dyn FnMut(String) + Send),
) {
    match event["type"].as_str().unwrap_or_default() {
        "message_start" => {
            usage.input_tokens = event["message"]["usage"]["input_tokens"]
                .as_u64()
                .unwrap_or(0);
        }
        "content_block_start" => {
            let index = event["index"].as_u64().unwrap_or(0);
            let block = blocks.entry(index).or_default();
            if event["content_block"]["type"] == "tool_use" {
                block.tool_id = event["content_block"]["id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                block.tool_name = event["content_block"]["name"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
            }
        }
        "content_block_delta" => {
            let index = event["index"].as_u64().unwrap_or(0);
            let block = blocks.entry(index).or_default();
            match event["delta"]["type"].as_str().unwrap_or_default() {
                "text_delta" => {
                    let text = event["delta"]["text"].as_str().unwrap_or_default();
                    block.text.push_str(text);
                    on_text(text.to_string());
                }
                "input_json_delta" => {
                    block
                        .tool_json
                        .push_str(event["delta"]["partial_json"].as_str().unwrap_or_default());
                }
                _ => {}
            }
        }
        "message_delta" => {
            if let Some(reason) = event["delta"]["stop_reason"].as_str() {
                *stop_reason = reason.to_string();
            }
            if let Some(out) = event["usage"]["output_tokens"].as_u64() {
                usage.output_tokens = out;
            }
        }
        _ => {}
    }
}

fn finish_stream(
    blocks: BTreeMap<u64, PendingBlock>,
    stop_reason: String,
    usage: Usage,
) -> ChatResponse {
    let mut out = Vec::new();
    for (_, pending) in blocks {
        if !pending.tool_name.is_empty() {
            out.push(Block::ToolUse {
                id: pending.tool_id,
                name: pending.tool_name,
                input: serde_json::from_str(&pending.tool_json).unwrap_or_else(|_| json!({})),
            });
        } else if !pending.text.trim().is_empty() {
            out.push(Block::Text { text: pending.text });
        }
    }
    ChatResponse {
        blocks: out,
        stop_reason,
        usage,
    }
}

#[async_trait]
impl Provider for Anthropic {
    fn name(&self) -> &'static str {
        "anthropic"
    }

    fn model(&self) -> Option<String> {
        Some(self.model.clone())
    }

    async fn complete(&self, req: &ChatRequest) -> Result<ChatResponse> {
        let resp = self.post(&self.body(req, false)).await?;
        if !resp.status().is_success() {
            return Err(Self::parse_error(resp).await);
        }
        let payload: Value = resp.json().await.context("parsing anthropic response")?;
        Ok(Self::parse_response(&payload))
    }

    async fn complete_stream(
        &self,
        req: &ChatRequest,
        on_text: &mut (dyn FnMut(String) + Send),
    ) -> Result<ChatResponse> {
        let resp = self.post(&self.body(req, true)).await?;
        if !resp.status().is_success() {
            return Err(Self::parse_error(resp).await);
        }
        let mut blocks: BTreeMap<u64, PendingBlock> = BTreeMap::new();
        let mut usage = Usage::default();
        let mut stop_reason = String::new();
        sse::for_each_data(resp, |data| {
            let Ok(event) = serde_json::from_str::<Value>(data) else {
                return;
            };
            apply_event(&event, &mut blocks, &mut usage, &mut stop_reason, on_text);
        })
        .await?;
        if stop_reason.is_empty() {
            stop_reason = "end_turn".to_string();
        }
        Ok(finish_stream(blocks, stop_reason, usage))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_events_build_text_and_tool_blocks() {
        let mut blocks = BTreeMap::new();
        let mut usage = Usage::default();
        let mut stop_reason = String::new();
        let mut streamed = String::new();
        let events = [
            json!({"type":"message_start","message":{"usage":{"input_tokens":12,"output_tokens":1}}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"on it"}}),
            json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"c1","name":"bash"}}),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"command\":"}}),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"\"ls\"}"}}),
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":9}}),
        ];
        for e in events {
            apply_event(&e, &mut blocks, &mut usage, &mut stop_reason, &mut |t| {
                streamed.push_str(&t)
            });
        }
        let resp = finish_stream(blocks, stop_reason, usage);
        assert_eq!(streamed, "on it");
        assert_eq!(resp.usage.input_tokens, 12);
        assert_eq!(resp.usage.output_tokens, 9);
        assert!(matches!(&resp.blocks[0], Block::Text { text } if text == "on it"));
        match &resp.blocks[1] {
            Block::ToolUse { id, name, input } => {
                assert_eq!(id, "c1");
                assert_eq!(name, "bash");
                assert_eq!(input["command"], "ls");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(resp.stop_reason, "tool_use");
    }

    #[test]
    fn parses_message_response_with_usage() {
        let payload = json!({
            "content": [{"type": "text", "text": "hi"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 4, "output_tokens": 1}
        });
        let resp = Anthropic::parse_response(&payload);
        assert_eq!(resp.usage.input_tokens, 4);
        assert_eq!(resp.text(), "hi");
    }
}
