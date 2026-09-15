use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::llm::{Block, ChatMessage, ChatRequest, ChatResponse, Provider, Role};

#[derive(Default)]
struct MockState {
    instruction: String,
    segments: Vec<String>,
    next: usize,
}

impl MockState {
    fn new(instruction: String) -> Self {
        let segments = parse_segments(&instruction);
        Self {
            instruction,
            segments,
            next: 0,
        }
    }
}

fn parse_segments(instruction: &str) -> Vec<String> {
    let lowered = instruction.to_lowercase();
    let mut segments = Vec::new();
    if lowered.contains(" then ") {
        for part in instruction.split(" then ") {
            for sub in part.split(';') {
                let s = sub.trim();
                if !s.is_empty() {
                    segments.push(s.to_string());
                }
            }
        }
    } else {
        for sub in instruction.split(';') {
            let s = sub.trim();
            if !s.is_empty() {
                segments.push(s.to_string());
            }
        }
    }
    segments
}

fn exec_call(seg: &str, _idx: usize) -> (String, Value) {
    let lower = seg.to_lowercase();
    if lower.starts_with("run ") {
        return ("bash".into(), json!({"command": seg[4..].trim()}));
    }
    if let Some(rest) = seg.strip_prefix("create file ") {
        return file_call(rest);
    }
    if let Some(rest) = seg.strip_prefix("write file ") {
        return file_call(rest);
    }
    if let Some(rest) = seg.strip_prefix("read file ") {
        return ("read_file".into(), json!({"path": rest.trim()}));
    }
    if lower.starts_with("list files") {
        return ("list_files".into(), json!({}));
    }
    (
        "bash".into(),
        json!({"command": format!("printf '%s\\n' 'morse demo: {}'", seg.replace('\'', "'\"'\"'"))}),
    )
}

fn file_call(rest: &str) -> (String, Value) {
    let (path, content) = match rest.split_once(':') {
        Some((p, c)) => (p.trim().to_string(), c.trim_start().to_string()),
        None => (rest.trim().to_string(), "# created by morse\n".to_string()),
    };
    (
        "write_file".into(),
        json!({"path": path, "content": content}),
    )
}

fn plan_input(segments: &[String], active: Option<usize>, done: Option<usize>) -> Value {
    let tasks: Vec<Value> = segments
        .iter()
        .enumerate()
        .map(|(i, seg)| {
            let status = if let Some(d) = done {
                if i < d {
                    "completed"
                } else if Some(i) == active {
                    "in_progress"
                } else {
                    "pending"
                }
            } else if Some(i) == active {
                "in_progress"
            } else {
                "pending"
            };
            json!({"title": seg, "status": status})
        })
        .collect();
    json!({"tasks": tasks})
}

fn last_user_text(messages: &[ChatMessage]) -> Option<String> {
    messages.iter().rev().find_map(|m| match m.role {
        Role::User => m.text_content().map(|t| t.to_string()),
        Role::Assistant => None,
    })
}

pub struct Mock {
    states: Mutex<HashMap<String, MockState>>,
}

impl Mock {
    pub fn new() -> Self {
        Self {
            states: Mutex::new(HashMap::new()),
        }
    }
}

impl Default for Mock {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for Mock {
    fn name(&self) -> &'static str {
        "mock"
    }

    fn model(&self) -> Option<String> {
        None
    }

    fn is_mock(&self) -> bool {
        true
    }

    async fn complete(&self, req: &ChatRequest) -> anyhow::Result<ChatResponse> {
        if req.purpose == crate::llm::Purpose::Side {
            return Ok(ChatResponse {
                blocks: vec![Block::Text {
                    text: "side queries are answered locally in demo mode".into(),
                }],
                stop_reason: "end_turn".into(),
            });
        }
        let instruction = last_user_text(&req.messages).unwrap_or_default();
        let mut states = self.states.lock().unwrap();
        let entry = states
            .entry(req.session_id.clone())
            .or_insert_with(|| MockState::new(instruction.clone()));
        if entry.instruction != instruction
            || req
                .messages
                .last()
                .is_some_and(|m| m.role == Role::User && m.text_content().is_some())
        {
            *entry = MockState::new(instruction.clone());
        }
        if entry.segments.is_empty() {
            return Ok(ChatResponse {
                blocks: vec![Block::Text {
                    text: "nothing to do — give me an instruction like: run <cmd> then create file <path>: <content>".into(),
                }],
                stop_reason: "end_turn".into(),
            });
        }
        if req.messages.last().is_some_and(|m| {
            m.blocks
                .iter()
                .any(|b| matches!(b, Block::ToolResult { is_error: true, .. }))
        }) {
            return Ok(ChatResponse {
                blocks: vec![Block::Text { text: "stopped: a tool failed; see its result above. Remaining tasks are unfinished.".into() }],
                stop_reason: "end_turn".into(),
            });
        }
        let n = entry.segments.len();
        let blocks = if entry.next == 0 {
            entry.next = 1;
            vec![
                Block::Text {
                    text: "on it — here's the plan.".into(),
                },
                Block::ToolUse {
                    id: "plan-init".into(),
                    name: "plan".into(),
                    input: plan_input(&entry.segments, None, None),
                },
            ]
        } else if entry.next <= n {
            let i = entry.next - 1;
            entry.next += 1;
            let (tool, input) = exec_call(&entry.segments[i], i);
            let call_id = format!("m{i}");
            vec![
                Block::ToolUse {
                    id: "plan-upd".into(),
                    name: "plan".into(),
                    input: plan_input(&entry.segments, Some(i), Some(i)),
                },
                Block::ToolUse {
                    id: call_id,
                    name: tool,
                    input,
                },
            ]
        } else if entry.next == n + 1 {
            entry.next += 1;
            let summary: Vec<&str> = entry.segments.iter().map(|s| s.as_str()).collect();
            vec![
                Block::ToolUse {
                    id: "plan-final".into(),
                    name: "plan".into(),
                    input: plan_input(&entry.segments, None, Some(n)),
                },
                Block::Text {
                    text: format!("done — {} tasks completed: {}", n, summary.join("; ")),
                },
            ]
        } else {
            vec![Block::Text {
                text: "demo run finished".into(),
            }]
        };
        let stop_reason = if blocks.iter().any(|b| matches!(b, Block::ToolUse { .. })) {
            "tool_use"
        } else {
            "end_turn"
        };
        Ok(ChatResponse {
            blocks,
            stop_reason: stop_reason.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{ChatMessage, Purpose};

    fn req_for(session: &str, instruction: &str) -> ChatRequest {
        ChatRequest {
            purpose: Purpose::Main,
            session_id: session.into(),
            system: String::new(),
            messages: vec![ChatMessage::user_text(instruction)],
            tools: vec![],
            max_tokens: 128,
        }
    }

    #[test]
    fn parse_segments_chains() {
        let s = parse_segments("run echo hi then create file a.txt: hello; run ls");
        assert_eq!(s.len(), 3);
        assert_eq!(s[0], "run echo hi");
        assert_eq!(s[2], "run ls");
        assert!(parse_segments("  ").is_empty());
    }

    #[test]
    fn exec_call_variants() {
        let (t, v) = exec_call("run uname -a", 0);
        assert_eq!(t, "bash");
        assert_eq!(v["command"], "uname -a");
        let (t, v) = exec_call("create file notes.txt: hi there", 1);
        assert_eq!(t, "write_file");
        assert_eq!(v["path"], "notes.txt");
        assert_eq!(v["content"], "hi there");
        let (_, v) = exec_call("write file empty.md", 2);
        assert_eq!(v["content"], "# created by morse\n");
        let (t, _) = exec_call("list files", 3);
        assert_eq!(t, "list_files");
        let (t, v) = exec_call("do a backflip", 4);
        assert_eq!(t, "bash");
        assert!(v["command"].as_str().unwrap().contains("do a backflip"));
    }

    #[tokio::test]
    async fn mock_flow_plan_exec_done() {
        let m = Mock::new();
        let r0 = m.complete(&req_for("s1", "run echo hi")).await.unwrap();
        assert!(matches!(r0.blocks[0], Block::Text { .. }));
        let uses0 = r0.tool_uses();
        assert_eq!(uses0[0].1, "plan");

        let mut msgs = vec![
            ChatMessage::user_text("run echo hi"),
            ChatMessage::assistant(r0.blocks.clone()),
        ];
        let r1 = m
            .complete(&ChatRequest {
                purpose: Purpose::Main,
                session_id: "s1".into(),
                system: String::new(),
                messages: msgs.clone(),
                tools: vec![],
                max_tokens: 128,
            })
            .await
            .unwrap();
        let uses = r1.tool_uses();
        assert_eq!(uses.len(), 2);
        assert_eq!(uses[0].1, "plan");
        assert_eq!(uses[1].1, "bash");
        assert_eq!(uses[1].2["command"], "echo hi");

        msgs.push(ChatMessage::assistant(r1.blocks.clone()));
        msgs.push(ChatMessage::user_results(vec![Block::ToolResult {
            tool_use_id: uses[0].0.clone(),
            content: "ok".into(),
            is_error: false,
        }]));
        let r2 = m
            .complete(&ChatRequest {
                purpose: Purpose::Main,
                session_id: "s1".into(),
                system: String::new(),
                messages: msgs,
                tools: vec![],
                max_tokens: 128,
            })
            .await
            .unwrap();
        assert!(r2.text().contains("done"));
        assert_eq!(r2.tool_uses()[0].1, "plan");
    }
}
