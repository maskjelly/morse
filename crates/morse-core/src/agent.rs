use std::sync::Arc;
use std::time::Instant;

use crate::llm::{tool_specs, Block, ChatMessage, ChatRequest, ChatResponse, Purpose, Role};
use crate::protocol::{ServerMsg, StatusKind};
use crate::session::Session;
use crate::tools::{exec_tool, ToolCtx};

const MAX_TURNS: usize = 48;

pub fn system_prompt(workspace: &str) -> String {
    format!(
        "You are Morse, an agent operating inside a cloud computer session. Workspace directory: {workspace}.\n\
         Get the user's request done using tools. Run shell commands with bash, read and write files.\n\
         Keep a visible checklist: call the plan tool whenever tasks or their statuses change.\n\
         Keep prose terse. When the request is complete, finish with a short summary."
    )
}

pub async fn runner(session: Arc<Session>, mut rx: tokio::sync::mpsc::UnboundedReceiver<String>) {
    while let Some(text) = rx.recv().await {
        run_instruction(&session, text).await;
    }
}

pub async fn run_instruction(session: &Arc<Session>, text: String) {
    let token = session.begin_run();
    session.emit(ServerMsg::Instruction { text: text.clone() });
    session.emit(ServerMsg::Status {
        status: StatusKind::Working,
        detail: Some(text.clone()),
    });

    session.push_history(ChatMessage::user_text(text));

    let mut interrupted = false;
    let workspace = session.workspace.display().to_string();
    let mut turn = 0usize;

    'outer: while turn < MAX_TURNS {
        turn += 1;
        if token.is_cancelled() {
            interrupted = true;
            break;
        }
        let req = ChatRequest {
            purpose: Purpose::Main,
            session_id: session.id.clone(),
            system: system_prompt(&workspace),
            messages: session.history_snapshot(),
            tools: tool_specs(),
            max_tokens: 4096,
        };
        let response = tokio::select! {
            biased;
            _ = token.cancelled() => { interrupted = true; break; }
            response = session.provider.complete(&req) => response,
        };
        let resp: ChatResponse = match response {
            Ok(r) => r,
            Err(e) => {
                session.emit(ServerMsg::Error {
                    message: format!("provider error: {e:#}"),
                });
                break;
            }
        };

        let text_blocks: Vec<&Block> = resp
            .blocks
            .iter()
            .filter(|b| matches!(b, Block::Text { .. }))
            .collect();
        for b in &text_blocks {
            if let Block::Text { text } = b {
                if !text.trim().is_empty() {
                    session.emit(ServerMsg::AgentText {
                        text: text.trim().to_string(),
                    });
                }
            }
        }

        let tool_uses: Vec<Block> = resp
            .blocks
            .iter()
            .filter(|b| matches!(b, Block::ToolUse { .. }))
            .cloned()
            .collect();

        if tool_uses.is_empty() {
            session.push_history(ChatMessage::assistant(resp.blocks.clone()));
            break;
        }

        session.push_history(ChatMessage::assistant(resp.blocks.clone()));

        let mut results: Vec<Block> = Vec::new();
        for (tool_index, tu) in tool_uses.iter().enumerate() {
            let (call_id, name, input) = match tu {
                Block::ToolUse { id, name, input } => (id.clone(), name.clone(), input.clone()),
                _ => continue,
            };
            session.emit(ServerMsg::ToolCall {
                id: call_id.clone(),
                tool: name.clone(),
                input: input.clone(),
            });

            let ws = session.workspace.clone();
            let session2 = session.clone();
            let event_call_id = call_id.clone();
            let mut emit = move |mut m: ServerMsg| {
                match &mut m {
                    ServerMsg::Output { call_id, .. } | ServerMsg::FileEdit { call_id, .. } => {
                        *call_id = event_call_id.clone()
                    }
                    _ => {}
                }
                session2.emit(m);
            };
            let mut ctx = ToolCtx {
                workspace: &ws,
                emit: &mut emit,
                cancel: &token,
            };

            let started = Instant::now();
            let outcome = tokio::select! {
                biased;
                _ = token.cancelled() => None,
                out = exec_tool(&name, &input, &mut ctx) => Some(out),
            };
            let duration_ms = started.elapsed().as_millis() as u64;

            let outcome = match outcome {
                Some(o) => o,
                None => {
                    interrupted = true;
                    let out = crate::tools::ToolOutcome {
                        output: "interrupted by user".to_string(),
                        ok: false,
                        ..Default::default()
                    };
                    session.emit(ServerMsg::ToolResult {
                        call_id: call_id.clone(),
                        tool: name.clone(),
                        ok: false,
                        exit_code: None,
                        duration_ms,
                        truncated: false,
                        summary: "interrupted".to_string(),
                    });
                    results.push(Block::ToolResult {
                        tool_use_id: call_id,
                        content: out.output,
                        is_error: true,
                    });
                    for pending in &tool_uses[tool_index + 1..] {
                        if let Block::ToolUse { id, .. } = pending {
                            results.push(Block::ToolResult {
                                tool_use_id: id.clone(),
                                content: "not executed: interrupted".into(),
                                is_error: true,
                            });
                        }
                    }
                    session.push_history(ChatMessage::user_results(results));
                    break 'outer;
                }
            };

            let summary: String = outcome
                .output
                .lines()
                .filter(|l| !l.starts_with("exit "))
                .collect::<Vec<_>>()
                .join(" | ")
                .chars()
                .take(200)
                .collect();
            session.emit(ServerMsg::ToolResult {
                call_id: call_id.clone(),
                tool: name.clone(),
                ok: outcome.ok,
                exit_code: outcome.exit_code,
                duration_ms,
                truncated: outcome.truncated,
                summary,
            });
            let content = if outcome.ok {
                outcome.output
            } else {
                format!(
                    "tool error (exit {:?}):\n{}",
                    outcome.exit_code, outcome.output
                )
            };
            results.push(Block::ToolResult {
                tool_use_id: call_id,
                content: crate::diff::cap(&content, 32 * 1024),
                is_error: !outcome.ok,
            });
        }
        session.push_history(ChatMessage::user_results(results));
    }

    if turn >= MAX_TURNS {
        session.emit(ServerMsg::Error {
            message: format!("instruction hit the {MAX_TURNS}-turn limit"),
        });
    }
    if interrupted {
        session.emit(ServerMsg::AgentText {
            text: "interrupted by user".to_string(),
        });
    }
    session.emit(ServerMsg::Status {
        status: if interrupted {
            StatusKind::Interrupted
        } else {
            StatusKind::Idle
        },
        detail: None,
    });
    session.end_run();
    session.cap_history();
}

pub fn cap_history_rule(history: &mut Vec<ChatMessage>) {
    const MAX: usize = 300;
    if history.len() <= MAX {
        return;
    }
    let cut = history.len() - MAX + 1;
    let cut = history[cut..]
        .iter()
        .position(|m| m.role == Role::User && matches!(m.blocks.first(), Some(Block::Text { .. })))
        .map(|i| i + cut)
        .unwrap_or(history.len());
    history.drain(0..cut);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_cap_keeps_user_boundary() {
        let mut h: Vec<ChatMessage> = (0..10)
            .map(|i| {
                if i % 2 == 0 {
                    ChatMessage::user_text(format!("u{i}"))
                } else {
                    ChatMessage::assistant(vec![])
                }
            })
            .collect();
        for i in 0..300 {
            h.push(ChatMessage::assistant(vec![Block::ToolUse {
                id: format!("c{i}"),
                name: "bash".into(),
                input: serde_json::json!({}),
            }]));
        }
        cap_history_rule(&mut h);
        assert!(h.len() <= 300);
        assert!(
            h.is_empty()
                || matches!(
                    h[0],
                    ChatMessage {
                        role: Role::User,
                        ..
                    }
                )
        );
    }
}
