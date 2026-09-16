use std::future::Future;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, serde::Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Block {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
}

#[derive(Debug, Clone, serde::Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    pub blocks: Vec<Block>,
}

impl ChatMessage {
    pub fn user_text(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            blocks: vec![Block::Text { text: text.into() }],
        }
    }

    pub fn user_results(results: Vec<Block>) -> Self {
        Self {
            role: Role::User,
            blocks: results,
        }
    }

    pub fn assistant(blocks: Vec<Block>) -> Self {
        Self {
            role: Role::Assistant,
            blocks,
        }
    }

    pub fn text_content(&self) -> Option<&str> {
        self.blocks.iter().find_map(|b| match b {
            Block::Text { text } => Some(text.as_str()),
            _ => None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    Main,
    Side,
}

#[derive(Debug, Clone)]
pub struct ToolSpec {
    pub name: &'static str,
    pub description: String,
    pub schema: Value,
}

pub struct ChatRequest {
    pub purpose: Purpose,
    pub session_id: String,
    pub system: String,
    pub messages: Vec<ChatMessage>,
    pub tools: Vec<ToolSpec>,
    pub max_tokens: u32,
}

#[derive(Debug, Clone)]
pub struct ChatResponse {
    pub blocks: Vec<Block>,
    pub stop_reason: String,
    pub usage: Usage,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl Usage {
    pub fn is_zero(&self) -> bool {
        self.input_tokens == 0 && self.output_tokens == 0
    }
}

impl ChatResponse {
    pub fn text(&self) -> String {
        self.blocks
            .iter()
            .filter_map(|b| match b {
                Block::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }

    pub fn tool_uses(&self) -> Vec<(&String, &String, &Value)> {
        self.blocks
            .iter()
            .filter_map(|b| match b {
                Block::ToolUse { id, name, input } => Some((id, name, input)),
                _ => None,
            })
            .collect()
    }
}

#[async_trait]
pub trait Provider: Send + Sync {
    fn name(&self) -> &'static str;
    fn model(&self) -> Option<String>;
    fn is_mock(&self) -> bool {
        false
    }
    async fn complete(&self, req: &ChatRequest) -> anyhow::Result<ChatResponse>;

    /// Complete a request while streaming text deltas to `on_text`.
    /// Providers that support live streaming override this; the default
    /// buffers the full response and emits each text block once.
    async fn complete_stream(
        &self,
        req: &ChatRequest,
        on_text: &mut (dyn FnMut(String) + Send),
    ) -> anyhow::Result<ChatResponse> {
        let resp = self.complete(req).await?;
        for block in &resp.blocks {
            if let Block::Text { text } = block {
                on_text(text.clone());
            }
        }
        Ok(resp)
    }
}

pub const HTTP_ATTEMPTS: u32 = 3;

/// Retry transient HTTP failures (network errors, 408/409/429/5xx) with backoff.
/// The final attempt always returns the response or error so callers can report it.
pub async fn retry_request<F, Fut>(mut send: F) -> anyhow::Result<reqwest::Response>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = anyhow::Result<reqwest::Response>>,
{
    let mut attempt = 0;
    loop {
        attempt += 1;
        match send().await {
            Ok(resp) => {
                let retryable = matches!(
                    resp.status().as_u16(),
                    408 | 409 | 425 | 429 | 500 | 502 | 503 | 504 | 529
                );
                if !retryable || attempt >= HTTP_ATTEMPTS {
                    return Ok(resp);
                }
                tracing::warn!(
                    "provider http {}: retrying ({attempt}/{HTTP_ATTEMPTS})",
                    resp.status()
                );
            }
            Err(e) => {
                if attempt >= HTTP_ATTEMPTS {
                    return Err(e);
                }
                tracing::warn!(
                    "provider request failed: {e:#}; retrying ({attempt}/{HTTP_ATTEMPTS})"
                );
            }
        }
        tokio::time::sleep(Duration::from_millis(250 * 3u64.pow(attempt - 1))).await;
    }
}

pub fn tool_specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "bash",
            description: "Run a shell command in the session workspace. Output streams to the user in real time.".into(),
            schema: json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "shell command"},
                    "timeout_ms": {"type": "integer", "description": "timeout in ms, default 120000"}
                },
                "required": ["command"]
            }),
        },
        ToolSpec {
            name: "read_file",
            description: "Read a text file from the workspace.".into(),
            schema: json!({
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "required": ["path"]
            }),
        },
        ToolSpec {
            name: "write_file",
            description: "Create or overwrite a file in the workspace. A diff is shown to the user.".into(),
            schema: json!({
                "type": "object",
                "properties": {"path": {"type": "string"}, "content": {"type": "string"}},
                "required": ["path", "content"]
            }),
        },
        ToolSpec {
            name: "edit_file",
            description: "Replace an exact string in a file. old_string must occur exactly once.".into(),
            schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "old_string": {"type": "string"},
                    "new_string": {"type": "string"}
                },
                "required": ["path", "old_string", "new_string"]
            }),
        },
        ToolSpec {
            name: "list_files",
            description: "List files in a workspace directory (recursive, capped).".into(),
            schema: json!({
                "type": "object",
                "properties": {"path": {"type": "string", "description": "relative dir, default ."}, "max": {"type": "integer"}}
            }),
        },
        ToolSpec {
            name: "glob",
            description: "Find files by glob pattern, e.g. '**/*.rs' or 'src/**/*.toml'.".into(),
            schema: json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string"},
                    "path": {"type": "string", "description": "relative dir to search from, default ."},
                    "max": {"type": "integer", "description": "max results, default 200"}
                },
                "required": ["pattern"]
            }),
        },
        ToolSpec {
            name: "grep",
            description: "Search file contents with a regular expression.".into(),
            schema: json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "regex"},
                    "path": {"type": "string", "description": "relative dir to search from, default ."},
                    "glob": {"type": "string", "description": "only files matching this glob"},
                    "ignore_case": {"type": "boolean"},
                    "max": {"type": "integer", "description": "max matches, default 100"}
                },
                "required": ["pattern"]
            }),
        },
        ToolSpec {
            name: "plan",
            description: "Publish or update the user-visible task checklist. Call whenever tasks or their statuses change.".into(),
            schema: json!({
                "type": "object",
                "properties": {
                    "tasks": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "title": {"type": "string"},
                                "status": {"type": "string", "enum": ["pending", "in_progress", "completed"]}
                            },
                            "required": ["title"]
                        }
                    }
                },
                "required": ["tasks"]
            }),
        },
    ]
}

pub fn provider_from_env() -> std::sync::Arc<dyn Provider> {
    use std::sync::Arc;
    let explicit = std::env::var("MORSE_PROVIDER")
        .ok()
        .map(|s| s.to_lowercase());
    if let Some(name) = explicit.as_deref() {
        match name {
            "mock" | "demo" => return Arc::new(crate::provider_mock::Mock::new()),
            "openai" => {
                if let Some(p) = crate::provider_openai::OpenAi::from_env() {
                    return Arc::new(p);
                }
            }
            "anthropic" => {
                if let Some(p) = crate::provider_anthropic::Anthropic::from_env() {
                    return Arc::new(p);
                }
            }
            other => tracing::warn!("unknown MORSE_PROVIDER '{other}', auto-detecting"),
        }
    } else {
        if std::env::var("MORSE_BASE_URL").is_ok() || std::env::var("OPENAI_API_KEY").is_ok() {
            if let Some(p) = crate::provider_openai::OpenAi::from_env() {
                return Arc::new(p);
            }
        }
        if std::env::var("ANTHROPIC_API_KEY").is_ok() || std::env::var("MORSE_API_KEY").is_ok() {
            if let Some(p) = crate::provider_anthropic::Anthropic::from_env() {
                return Arc::new(p);
            }
        }
    }
    tracing::warn!("no LLM configured: running in demo mode with the mock provider");
    Arc::new(crate::provider_mock::Mock::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_response_helpers() {
        let r = ChatResponse {
            blocks: vec![
                Block::Text {
                    text: "on it".into(),
                },
                Block::ToolUse {
                    id: "c1".into(),
                    name: "bash".into(),
                    input: json!({"command": "ls"}),
                },
            ],
            stop_reason: "tool_use".into(),
            usage: Usage::default(),
        };
        assert_eq!(r.text(), "on it");
        assert_eq!(r.tool_uses().len(), 1);
        assert_eq!(r.tool_uses()[0].1, "bash");
    }

    #[test]
    fn tool_specs_have_schemas() {
        let specs = tool_specs();
        assert_eq!(specs.len(), 8);
        for s in &specs {
            assert_eq!(s.schema["type"], "object");
        }
    }
}
