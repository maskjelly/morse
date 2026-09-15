use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};

use crate::llm::{Block, ChatMessage, ChatRequest, ChatResponse, Provider, Role};

pub struct Anthropic {
    http: reqwest::Client,
    key: String,
    model: String,
}

impl Anthropic {
    pub fn from_env() -> Option<Self> {
        let key = std::env::var("MORSE_API_KEY")
            .or_else(|_| std::env::var("ANTHROPIC_API_KEY"))
            .ok()?;
        let model =
            std::env::var("MORSE_MODEL").unwrap_or_else(|_| "claude-sonnet-4-5".to_string());
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(300))
            .build()
            .ok()?;
        Some(Self { http, key, model })
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
        let mut body = json!({
            "model": self.model,
            "max_tokens": req.max_tokens,
            "system": req.system,
            "messages": Self::messages_json(&req.messages),
        });
        if !req.tools.is_empty() {
            body["tools"] = Self::tools_json(&req.tools);
        }
        let resp = self
            .http
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.key)
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .await
            .context("anthropic request failed")?;
        let status = resp.status();
        let payload: Value = resp.json().await.context("parsing anthropic response")?;
        if !status.is_success() {
            let msg = payload["error"]["message"]
                .as_str()
                .unwrap_or("unknown error");
            return Err(anyhow!("anthropic api error {status}: {msg}"));
        }
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
        Ok(ChatResponse {
            blocks,
            stop_reason: payload["stop_reason"]
                .as_str()
                .unwrap_or("end_turn")
                .to_string(),
        })
    }
}
