use async_trait::async_trait;
use serde_json::json;

use crate::llm::{Block, ChatRequest, ChatResponse, Provider, Purpose, Role};

pub struct CommandAgent {
    program: String,
    args: Vec<String>,
    timeout_ms: u64,
}

impl CommandAgent {
    pub fn new(program: String, args: Vec<String>, timeout_ms: u64) -> anyhow::Result<Self> {
        anyhow::ensure!(
            !program.trim().is_empty(),
            "MORSE_AGENT_PROGRAM is required"
        );
        anyhow::ensure!(
            args.iter().any(|arg| arg.contains("{prompt}")),
            "MORSE_AGENT_ARGS must contain {{prompt}}"
        );
        anyhow::ensure!(timeout_ms > 0, "MORSE_AGENT_TIMEOUT_MS must be positive");
        Ok(Self {
            program,
            args,
            timeout_ms,
        })
    }

    pub fn from_env() -> anyhow::Result<Self> {
        Self::new(
            std::env::var("MORSE_AGENT_PROGRAM")?,
            serde_json::from_str(&std::env::var("MORSE_AGENT_ARGS")?)?,
            std::env::var("MORSE_AGENT_TIMEOUT_MS")
                .unwrap_or_else(|_| "1800000".into())
                .parse()?,
        )
    }
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[async_trait]
impl Provider for CommandAgent {
    fn name(&self) -> &'static str {
        "command"
    }
    fn model(&self) -> Option<String> {
        Some(self.program.clone())
    }
    fn local_side(&self) -> bool {
        true
    }

    async fn complete(&self, req: &ChatRequest) -> anyhow::Result<ChatResponse> {
        anyhow::ensure!(
            req.purpose == Purpose::Main,
            "command agents only run main instructions"
        );
        let last = req
            .messages
            .last()
            .ok_or_else(|| anyhow::anyhow!("missing instruction"))?;
        let blocks = if last.role == Role::User && last.text_content().is_some() {
            let prompt = last.text_content().unwrap();
            let command = std::iter::once(quote(&self.program))
                .chain(self.args.iter().map(|arg| {
                    quote(
                        &arg.replace("{session_id}", &req.session_id)
                            .replace("{prompt}", prompt),
                    )
                }))
                .collect::<Vec<_>>()
                .join(" ");
            vec![Block::ToolUse {
                id: format!("agent-{}", uuid::Uuid::new_v4()),
                name: "bash".into(),
                input: json!({"command": command, "timeout_ms": self.timeout_ms, "inherit_agent_env": true}),
            }]
        } else {
            vec![Block::Text {
                text: "Agent process finished; see its streamed output and exit status above."
                    .into(),
            }]
        };
        Ok(ChatResponse {
            blocks,
            stop_reason: "end_turn".into(),
            usage: Default::default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        tools::{exec_tool, ToolCtx},
        Session,
    };
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;

    #[tokio::test]
    async fn prompt_is_one_literal_argument_and_output_streams() {
        let agent =
            CommandAgent::new("printf".into(), vec!["%s".into(), "{prompt}".into()], 1000).unwrap();
        let prompt = "quotes ' ; $(touch injected) `id`\nnext line";
        let dir = std::env::temp_dir().join(format!("morse-command-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let req = ChatRequest {
            purpose: Purpose::Main,
            session_id: "s1".into(),
            system: String::new(),
            messages: vec![crate::llm::ChatMessage::user_text(prompt)],
            tools: vec![],
            max_tokens: 0,
        };
        let response = agent.complete(&req).await.unwrap();
        let (_, name, input) = response.tool_uses()[0];
        let cancel = CancellationToken::new();
        let mut events = Vec::new();
        let mut emit = |event| events.push(event);
        let mut ctx = ToolCtx {
            workspace: &dir,
            emit: &mut emit,
            cancel: &cancel,
        };
        let out = exec_tool(name, input, &mut ctx).await;
        assert!(out.ok, "{}", out.output);
        assert_eq!(out.output, format!("exit 0\n{prompt}"));
        assert!(!dir.join("injected").exists());
        assert!(!events.is_empty());
        let session = Session::new("s1".into(), dir.clone(), Arc::new(agent));
        assert!(session.provider.local_side());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn invalid_adapter_configuration_fails() {
        assert!(CommandAgent::new("".into(), vec!["{prompt}".into()], 1).is_err());
        assert!(CommandAgent::new("agent".into(), vec![], 1).is_err());
        assert!(CommandAgent::new("agent".into(), vec!["{prompt}".into()], 0).is_err());
    }
}
