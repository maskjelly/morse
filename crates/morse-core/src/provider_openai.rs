use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};

use crate::llm::{
    retry_request, Block, ChatMessage, ChatRequest, ChatResponse, Provider, Role, Usage,
};
use crate::sse;

pub const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";

pub struct OpenAi {
    http: reqwest::Client,
    key: String,
    model: String,
    base_url: String,
}

impl OpenAi {
    pub fn new(key: String, model: String, base_url: String) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(300))
            .connect_timeout(Duration::from_secs(15))
            .build()
            .expect("building http client");
        Self {
            http,
            key,
            model,
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    /// Works with any OpenAI-compatible `/chat/completions` endpoint,
    /// including local servers such as Ollama that need no API key.
    pub fn from_env() -> Option<Self> {
        let base_url = std::env::var("MORSE_BASE_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.into());
        let key = std::env::var("MORSE_API_KEY")
            .or_else(|_| std::env::var("OPENAI_API_KEY"))
            .unwrap_or_default();
        let local = !base_url.contains("api.openai.com");
        if key.is_empty() && !local {
            return None;
        }
        let model = std::env::var("MORSE_MODEL").unwrap_or_else(|_| "gpt-4o-mini".to_string());
        Some(Self::new(key, model, base_url))
    }

    fn messages_json(messages: &[ChatMessage]) -> Value {
        let mut out: Vec<Value> = Vec::new();
        for m in messages {
            match m.role {
                Role::Assistant => {
                    let text: String = m
                        .blocks
                        .iter()
                        .filter_map(|b| match b {
                            Block::Text { text } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("");
                    let calls: Vec<Value> = m
                        .blocks
                        .iter()
                        .filter_map(|b| match b {
                            Block::ToolUse { id, name, input } => Some(json!({
                                "id": id,
                                "type": "function",
                                "function": {"name": name, "arguments": input.to_string()}
                            })),
                            _ => None,
                        })
                        .collect();
                    let mut msg = json!({"role": "assistant"});
                    if !text.is_empty() {
                        msg["content"] = json!(text);
                    }
                    if !calls.is_empty() {
                        msg["tool_calls"] = Value::Array(calls);
                    }
                    out.push(msg);
                }
                Role::User => {
                    let text: String = m
                        .blocks
                        .iter()
                        .filter_map(|b| match b {
                            Block::Text { text } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("");
                    if !text.is_empty() {
                        out.push(json!({"role": "user", "content": text}));
                    }
                    for b in &m.blocks {
                        if let Block::ToolResult {
                            tool_use_id,
                            content,
                            ..
                        } = b
                        {
                            out.push(json!({
                                "role": "tool",
                                "tool_call_id": tool_use_id,
                                "content": content,
                            }));
                        }
                    }
                }
            }
        }
        Value::Array(out)
    }

    fn tools_json(tools: &[crate::llm::ToolSpec]) -> Value {
        tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.schema,
                    }
                })
            })
            .collect::<Vec<_>>()
            .into()
    }

    fn body(&self, req: &ChatRequest, stream: bool) -> Value {
        let mut messages = vec![json!({"role": "system", "content": req.system})];
        if let Value::Array(rest) = Self::messages_json(&req.messages) {
            messages.extend(rest);
        }
        let mut body = json!({
            "model": self.model,
            "max_tokens": req.max_tokens,
            "messages": messages,
        });
        if !req.tools.is_empty() {
            body["tools"] = Self::tools_json(&req.tools);
            body["tool_choice"] = json!("auto");
        }
        if stream {
            body["stream"] = json!(true);
            body["stream_options"] = json!({"include_usage": true});
        }
        body
    }

    async fn post(&self, body: &Value) -> Result<reqwest::Response> {
        let url = format!("{}/chat/completions", self.base_url);
        retry_request(|| {
            let mut request = self.http.post(&url).json(body);
            if !self.key.is_empty() {
                request = request.bearer_auth(&self.key);
            }
            async move { request.send().await.context("openai request failed") }
        })
        .await
    }

    async fn error_for(resp: reqwest::Response) -> anyhow::Error {
        let status = resp.status();
        let detail = resp
            .text()
            .await
            .unwrap_or_default()
            .chars()
            .take(400)
            .collect::<String>();
        anyhow!("openai api error {status}: {detail}")
    }

    fn parse_message(message: &Value) -> (Vec<Block>, Option<Usage>) {
        let mut blocks = Vec::new();
        if let Some(text) = message["content"].as_str() {
            if !text.trim().is_empty() {
                blocks.push(Block::Text {
                    text: text.to_string(),
                });
            }
        }
        for call in message["tool_calls"]
            .as_array()
            .cloned()
            .unwrap_or_default()
        {
            let args = call["function"]["arguments"].as_str().unwrap_or("{}");
            blocks.push(Block::ToolUse {
                id: call["id"].as_str().unwrap_or_default().to_string(),
                name: call["function"]["name"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                input: serde_json::from_str(args).unwrap_or_else(|_| json!({"_raw": args})),
            });
        }
        (blocks, None)
    }

    fn parse_response(payload: &Value) -> Result<ChatResponse> {
        let choice = payload["choices"]
            .as_array()
            .and_then(|c| c.first())
            .ok_or_else(|| anyhow!("openai response has no choices: {payload}"))?;
        let (blocks, _) = Self::parse_message(&choice["message"]);
        Ok(ChatResponse {
            blocks,
            stop_reason: choice["finish_reason"]
                .as_str()
                .unwrap_or("stop")
                .to_string(),
            usage: parse_usage(&payload["usage"]),
        })
    }
}

pub fn parse_usage(usage: &Value) -> Usage {
    Usage {
        input_tokens: usage["prompt_tokens"].as_u64().unwrap_or(0),
        output_tokens: usage["completion_tokens"].as_u64().unwrap_or(0),
    }
}

#[derive(Default)]
struct PendingCall {
    id: String,
    name: String,
    args: String,
}

fn apply_stream_delta(delta: &Value, calls: &mut BTreeMap<u64, PendingCall>) -> String {
    let mut text = String::new();
    if let Some(chunk) = delta["content"].as_str() {
        text.push_str(chunk);
    }
    for call in delta["tool_calls"].as_array().cloned().unwrap_or_default() {
        let index = call["index"].as_u64().unwrap_or(0);
        let entry = calls.entry(index).or_default();
        if let Some(id) = call["id"].as_str() {
            entry.id = id.to_string();
        }
        if let Some(name) = call["function"]["name"].as_str() {
            entry.name.push_str(name);
        }
        if let Some(args) = call["function"]["arguments"].as_str() {
            entry.args.push_str(args);
        }
    }
    text
}

fn finish_stream(
    text: String,
    calls: BTreeMap<u64, PendingCall>,
    stop_reason: String,
    usage: Usage,
) -> ChatResponse {
    let mut blocks = Vec::new();
    if !text.trim().is_empty() {
        blocks.push(Block::Text { text });
    }
    for (_, call) in calls {
        blocks.push(Block::ToolUse {
            id: call.id,
            name: call.name,
            input: serde_json::from_str(&call.args).unwrap_or_else(|_| json!({"_raw": call.args})),
        });
    }
    ChatResponse {
        blocks,
        stop_reason,
        usage,
    }
}

#[async_trait]
impl Provider for OpenAi {
    fn name(&self) -> &'static str {
        "openai"
    }

    fn model(&self) -> Option<String> {
        Some(self.model.clone())
    }

    async fn complete(&self, req: &ChatRequest) -> Result<ChatResponse> {
        let resp = self.post(&self.body(req, false)).await?;
        if !resp.status().is_success() {
            return Err(Self::error_for(resp).await);
        }
        let payload: Value = resp.json().await.context("parsing openai response")?;
        Self::parse_response(&payload)
    }

    async fn complete_stream(
        &self,
        req: &ChatRequest,
        on_text: &mut (dyn FnMut(String) + Send),
    ) -> Result<ChatResponse> {
        let resp = self.post(&self.body(req, true)).await?;
        if !resp.status().is_success() {
            return Err(Self::error_for(resp).await);
        }
        let mut text = String::new();
        let mut calls: BTreeMap<u64, PendingCall> = BTreeMap::new();
        let mut stop_reason = String::new();
        let mut usage = Usage::default();
        sse::for_each_data(resp, |data| {
            let Ok(event) = serde_json::from_str::<Value>(data) else {
                return;
            };
            if let Some(u) = event.get("usage") {
                if !u.is_null() {
                    usage = parse_usage(u);
                }
            }
            let Some(choice) = event["choices"].as_array().and_then(|c| c.first()) else {
                return;
            };
            if let Some(reason) = choice["finish_reason"].as_str() {
                stop_reason = reason.to_string();
            }
            let delta_text = apply_stream_delta(&choice["delta"], &mut calls);
            if !delta_text.is_empty() {
                text.push_str(&delta_text);
                on_text(delta_text);
            }
        })
        .await?;
        if stop_reason.is_empty() {
            stop_reason = "stop".to_string();
        }
        Ok(finish_stream(text, calls, stop_reason, usage))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::ToolSpec;

    fn req() -> ChatRequest {
        ChatRequest {
            purpose: crate::llm::Purpose::Main,
            session_id: "s".into(),
            system: "sys".into(),
            messages: vec![
                ChatMessage::user_text("hi"),
                ChatMessage::assistant(vec![Block::ToolUse {
                    id: "c1".into(),
                    name: "bash".into(),
                    input: json!({"command": "ls"}),
                }]),
                ChatMessage::user_results(vec![Block::ToolResult {
                    tool_use_id: "c1".into(),
                    content: "file.txt".into(),
                    is_error: false,
                }]),
            ],
            tools: vec![ToolSpec {
                name: "bash",
                description: "run".into(),
                schema: json!({"type": "object"}),
            }],
            max_tokens: 100,
        }
    }

    #[test]
    fn body_has_system_tools_and_tool_messages() {
        let p = OpenAi::new("k".into(), "m".into(), DEFAULT_BASE_URL.into());
        let body = p.body(&req(), false);
        assert_eq!(body["model"], "m");
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["content"], "hi");
        assert_eq!(body["messages"][2]["tool_calls"][0]["id"], "c1");
        assert_eq!(
            body["messages"][2]["tool_calls"][0]["function"]["name"],
            "bash"
        );
        assert_eq!(body["messages"][3]["role"], "tool");
        assert_eq!(body["tools"][0]["function"]["name"], "bash");
        assert_eq!(body["stream"], Value::Null);
    }

    #[test]
    fn streaming_body_sets_stream_options() {
        let p = OpenAi::new("k".into(), "m".into(), DEFAULT_BASE_URL.into());
        let body = p.body(&req(), true);
        assert_eq!(body["stream"], true);
        assert_eq!(body["stream_options"]["include_usage"], true);
    }

    #[test]
    fn parses_non_streaming_response() {
        let payload = json!({
            "choices": [{
                "message": {"content": "hello", "tool_calls": [{
                    "id": "c9", "function": {"name": "bash", "arguments": "{\"command\":\"ls\"}"}
                }]},
                "finish_reason": "tool_calls"
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 3}
        });
        let resp = OpenAi::parse_response(&payload).unwrap();
        assert_eq!(resp.stop_reason, "tool_calls");
        assert_eq!(resp.usage.input_tokens, 10);
        assert!(matches!(&resp.blocks[0], Block::Text { text } if text == "hello"));
        assert!(matches!(&resp.blocks[1], Block::ToolUse { name, .. } if name == "bash"));
    }

    #[test]
    fn accumulates_tool_call_fragments_across_deltas() {
        let mut text = String::new();
        let mut calls = BTreeMap::new();
        for delta in [
            json!({"content": "thin"}),
            json!({"content": "king"}),
            json!({"tool_calls": [{"index": 0, "id": "c1", "function": {"name": "ba", "arguments": "{\"comm"}}]}),
            json!({"tool_calls": [{"index": 0, "function": {"name": "sh", "arguments": "and\":\"ls\"}"}}]}),
        ] {
            text.push_str(&apply_stream_delta(&delta, &mut calls));
        }
        let resp = finish_stream(text, calls, "tool_calls".into(), Usage::default());
        assert_eq!(resp.text(), "thinking");
        match &resp.blocks[1] {
            Block::ToolUse { id, name, input } => {
                assert_eq!(id, "c1");
                assert_eq!(name, "bash");
                assert_eq!(input["command"], "ls");
            }
            other => panic!("{other:?}"),
        }
    }
}
