use std::collections::VecDeque;

use serde_json::{json, Value};

use crate::protocol::{ServerMsg, StatusKind, TaskStatus, TaskView};

#[derive(Debug, Clone)]
pub struct RecentTool {
    pub call_id: String,
    pub tool: String,
    pub summary: String,
    pub ok: Option<bool>,
    pub exit_code: Option<i32>,
    pub duration_ms: Option<u64>,
    pub ts: u64,
}

#[derive(Debug, Default, Clone)]
pub struct SessionState {
    pub created_ts: u64,
    pub status: StatusKind,
    pub status_detail: Option<String>,
    pub instruction: Option<String>,
    pub tasks: Vec<TaskView>,
    pub current_tool: Option<RecentTool>,
    pub recent_tools: VecDeque<RecentTool>,
    pub side_turns: VecDeque<(String, String)>,
}

impl SessionState {
    pub fn new() -> Self {
        Self {
            created_ts: crate::protocol::now_ms(),
            ..Default::default()
        }
    }

    pub fn apply(&mut self, msg: &ServerMsg) {
        match msg {
            ServerMsg::Instruction { text } => {
                self.instruction = Some(text.clone());
                self.status = StatusKind::Working;
                self.status_detail = Some(text.clone());
            }
            ServerMsg::Plan { tasks } => {
                self.tasks = tasks.clone();
            }
            ServerMsg::ToolCall { id, tool, input } => {
                let rt = RecentTool {
                    call_id: id.clone(),
                    tool: tool.clone(),
                    summary: crate::tools::summarize_input(tool, input),
                    ok: None,
                    exit_code: None,
                    duration_ms: None,
                    ts: crate::protocol::now_ms(),
                };
                self.current_tool = Some(rt.clone());
                self.recent_tools.push_back(rt);
                if self.recent_tools.len() > 60 {
                    self.recent_tools.pop_front();
                }
            }
            ServerMsg::ToolResult {
                call_id,
                ok,
                exit_code,
                duration_ms,
                ..
            } => {
                if let Some(rt) = self
                    .recent_tools
                    .iter_mut()
                    .rev()
                    .find(|rt| rt.call_id == *call_id)
                {
                    rt.ok = Some(*ok);
                    rt.exit_code = *exit_code;
                    rt.duration_ms = Some(*duration_ms);
                }
                if self
                    .current_tool
                    .as_ref()
                    .is_some_and(|rt| rt.call_id == *call_id)
                {
                    self.current_tool = None;
                }
            }
            ServerMsg::Side { question, answer } => {
                self.side_turns
                    .push_back((question.clone(), answer.clone()));
                if self.side_turns.len() > 20 {
                    self.side_turns.pop_front();
                }
            }
            ServerMsg::Status { status, detail } => {
                self.status = *status;
                self.status_detail = detail.clone();
            }
            _ => {}
        }
    }

    pub fn summary_json(&self) -> Value {
        let tasks: Vec<Value> = self
            .tasks
            .iter()
            .map(|t| {
                json!({
                    "title": t.title,
                    "status": serde_json::to_value(t.status).unwrap_or_default(),
                })
            })
            .collect();
        let done = self
            .tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Completed)
            .count();
        json!({
            "status": serde_json::to_value(self.status).unwrap_or_default(),
            "status_detail": self.status_detail,
            "instruction": self.instruction,
            "tasks": tasks,
            "tasks_completed": done,
            "tasks_total": self.tasks.len(),
            "current_tool": self.current_tool.as_ref().map(|rt| {
                json!({"tool": rt.tool, "summary": rt.summary})
            }),
            "recent_tools": self.recent_tools.iter().rev().take(10).map(|rt| {
                json!({
                    "tool": rt.tool,
                    "summary": rt.summary,
                    "ok": rt.ok,
                    "exit_code": rt.exit_code,
                    "duration_ms": rt.duration_ms,
                })
            }).collect::<Vec<_>>(),
        })
    }

    pub fn heuristic_answer(&self) -> String {
        let mut out = String::new();
        match self.status {
            StatusKind::Working => {
                out.push_str(&format!(
                    "working on: {}\n",
                    self.status_detail
                        .as_deref()
                        .unwrap_or("(active instruction)")
                ));
            }
            StatusKind::Interrupted => out.push_str("last run was interrupted\n"),
            StatusKind::Idle => out.push_str("idle — nothing running right now\n"),
        }
        if !self.tasks.is_empty() {
            let done = self
                .tasks
                .iter()
                .filter(|t| t.status == TaskStatus::Completed)
                .count();
            out.push_str(&format!(
                "progress: {}/{} tasks done\n",
                done,
                self.tasks.len()
            ));
            for t in &self.tasks {
                let mark = match t.status {
                    TaskStatus::Completed => "[x]",
                    TaskStatus::InProgress => "[>]",
                    TaskStatus::Pending => "[ ]",
                };
                out.push_str(&format!("  {mark} {}\n", t.title));
            }
        } else if self.instruction.is_some() {
            out.push_str(&format!(
                "instruction: {}\n",
                self.instruction.as_deref().unwrap_or_default()
            ));
        }
        if let Some(rt) = &self.current_tool {
            out.push_str(&format!("currently running: {} {}\n", rt.tool, rt.summary));
        }
        if let Some(last) = self.recent_tools.back() {
            let verdict = match (last.ok, last.exit_code) {
                (Some(true), _) => "ok".to_string(),
                (Some(false), Some(c)) => format!("exit {c}"),
                (Some(false), None) => "failed".to_string(),
                (None, _) => "running".to_string(),
            };
            out.push_str(&format!(
                "last tool: {} {} -> {verdict}\n",
                last.tool, last.summary
            ));
        }
        if out.is_empty() {
            out.push_str("no activity in this session yet\n");
        }
        out.trim_end().to_string()
    }
}

pub fn brief(msg: &ServerMsg) -> Option<String> {
    let clip = |s: &str, n: usize| -> String {
        let t = s.trim();
        if t.chars().count() > n {
            t.chars().take(n).collect::<String>() + "..."
        } else {
            t.to_string()
        }
    };
    Some(match msg {
        ServerMsg::Instruction { text } => format!("user: {}", clip(text, 80)),
        ServerMsg::AgentText { text } => format!("morse: {}", clip(text, 100)),
        ServerMsg::ToolCall { tool, input, .. } => {
            let sum = crate::tools::summarize_input(tool, input);
            format!("call {tool}: {}", clip(&sum, 80))
        }
        ServerMsg::Output { chunk, .. } => format!("out: {}", clip(chunk, 60)),
        ServerMsg::ToolResult {
            call_id,
            ok,
            exit_code,
            tool,
            ..
        } => format!("result {tool} ok={ok} exit={exit_code:?} id={call_id}"),
        ServerMsg::FileEdit { path, .. } => format!("edited {path}"),
        ServerMsg::Plan { tasks } => {
            format!(
                "plan: {}",
                tasks
                    .iter()
                    .map(|t| t.title.as_str())
                    .collect::<Vec<_>>()
                    .join(" | ")
            )
        }
        ServerMsg::Status { status, .. } => format!("status: {status:?}"),
        ServerMsg::Side { .. } => return None,
        ServerMsg::Error { message } => format!("error: {}", clip(message, 120)),
        ServerMsg::Hello { .. } | ServerMsg::Pong => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::now_ms;

    fn plan(tasks: &[(&str, TaskStatus)]) -> ServerMsg {
        ServerMsg::Plan {
            tasks: tasks
                .iter()
                .enumerate()
                .map(|(i, (title, status))| TaskView {
                    id: format!("t{}", i + 1),
                    title: title.to_string(),
                    status: *status,
                })
                .collect(),
        }
    }

    #[test]
    fn tracks_plan_and_tools() {
        let mut st = SessionState::new();
        st.apply(&ServerMsg::Instruction {
            text: "run echo hi".into(),
        });
        st.apply(&plan(&[("run echo hi", TaskStatus::InProgress)]));
        st.apply(&ServerMsg::ToolCall {
            id: "c1".into(),
            tool: "bash".into(),
            input: serde_json::json!({"command": "echo hi"}),
        });
        st.apply(&ServerMsg::ToolResult {
            call_id: "c1".into(),
            tool: "bash".into(),
            ok: true,
            exit_code: Some(0),
            duration_ms: 12,
            truncated: false,
            summary: "hi".into(),
        });

        assert_eq!(st.tasks.len(), 1);
        assert_eq!(st.tasks[0].status, TaskStatus::InProgress);
        assert!(st.current_tool.is_none());

        let answer = st.heuristic_answer();
        assert!(answer.contains("echo hi"), "answer: {answer}");
        assert!(answer.contains("[>]"));
    }

    #[test]
    fn heuristic_lists_done_and_pending() {
        let mut st = SessionState::new();
        st.apply(&ServerMsg::Instruction {
            text: "build it".into(),
        });
        st.apply(&plan(&[
            ("scaffold", TaskStatus::Completed),
            ("compile", TaskStatus::InProgress),
            ("ship", TaskStatus::Pending),
        ]));
        let a = st.heuristic_answer();
        assert!(a.contains("[x] scaffold"));
        assert!(a.contains("[>] compile"));
        assert!(a.contains("[ ] ship"));
        assert!(a.contains("1/3"));
        assert!(st.recent_tools.is_empty());
        assert!(now_ms() > 0);
    }
}
