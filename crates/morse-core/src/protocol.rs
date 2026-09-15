use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMsg {
    Create { workspace: Option<String> },
    Attach { session_id: String },
    Instruction { text: String },
    SideQuery { text: String },
    Interrupt,
    Ping,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskView {
    pub id: String,
    pub title: String,
    pub status: TaskStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum StatusKind {
    #[default]
    Idle,
    Working,
    Interrupted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamKind {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMsg {
    Hello {
        session_id: String,
        workspace: String,
        provider: String,
        model: Option<String>,
        demo: bool,
    },
    Instruction {
        text: String,
    },
    Plan {
        tasks: Vec<TaskView>,
    },
    AgentText {
        text: String,
    },
    ToolCall {
        id: String,
        tool: String,
        input: Value,
    },
    Output {
        call_id: String,
        stream: StreamKind,
        chunk: String,
    },
    ToolResult {
        call_id: String,
        tool: String,
        ok: bool,
        exit_code: Option<i32>,
        duration_ms: u64,
        truncated: bool,
        summary: String,
    },
    FileEdit {
        call_id: String,
        path: String,
        diff: String,
    },
    Side {
        question: String,
        answer: String,
    },
    Status {
        status: StatusKind,
        detail: Option<String>,
    },
    Error {
        message: String,
    },
    Pong,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub seq: u64,
    pub ts: u64,
    #[serde(flatten)]
    pub inner: ServerMsg,
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn new_session_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..8].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_flatten_roundtrip() {
        let env = Envelope {
            seq: 7,
            ts: 12345,
            inner: ServerMsg::Pong,
        };
        let s = serde_json::to_string(&env).unwrap();
        assert_eq!(s, r#"{"seq":7,"ts":12345,"type":"pong"}"#);
        let back: Envelope = serde_json::from_str(&s).unwrap();
        assert_eq!(back.seq, 7);
        assert!(matches!(back.inner, ServerMsg::Pong));
    }

    #[test]
    fn client_msg_roundtrip() {
        let msg = ClientMsg::SideQuery {
            text: "what's done?".into(),
        };
        let s = serde_json::to_string(&msg).unwrap();
        assert!(s.contains(r#""type":"side_query""#));
        let back: ClientMsg = serde_json::from_str(&s).unwrap();
        assert!(matches!(back, ClientMsg::SideQuery { text } if text == "what's done?"));
    }

    #[test]
    fn tool_call_roundtrip() {
        let msg = ServerMsg::ToolCall {
            id: "c1".into(),
            tool: "bash".into(),
            input: serde_json::json!({"command": "echo hi"}),
        };
        let s = serde_json::to_string(&msg).unwrap();
        let back: ServerMsg = serde_json::from_str(&s).unwrap();
        assert!(matches!(back, ServerMsg::ToolCall { tool, .. } if tool == "bash"));
    }
}
