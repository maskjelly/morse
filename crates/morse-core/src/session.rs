use std::collections::VecDeque;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;

use crate::agent;
use crate::llm::{ChatMessage, Provider};
use crate::protocol::{now_ms, Envelope, ServerMsg};
use crate::side;
use crate::state::SessionState;

const LOG_CAP: usize = 4000;
const EVENTS_FILE: &str = "events.jsonl";
const HISTORY_FILE: &str = "history.json";
const ROTATE_BYTES: u64 = 8 * 1024 * 1024;

pub struct Session {
    pub id: String,
    pub workspace: PathBuf,
    pub provider: Arc<dyn Provider>,
    tx: broadcast::Sender<Envelope>,
    log: Mutex<Vec<Envelope>>,
    emit_lock: Mutex<()>,
    seq: AtomicU64,
    pub state: Mutex<SessionState>,
    history: Mutex<Vec<ChatMessage>>,
    instr_tx: mpsc::UnboundedSender<String>,
    side_tx: mpsc::UnboundedSender<String>,
    current_cancel: Mutex<Option<CancellationToken>>,
    session_dir: Mutex<Option<PathBuf>>,
    event_file: Mutex<Option<std::fs::File>>,
}

impl Session {
    pub fn new(id: String, workspace: PathBuf, provider: Arc<dyn Provider>) -> Arc<Session> {
        Self::build(id, workspace, provider, Vec::new(), Vec::new())
    }

    /// Rebuild a session from a persisted event log and history.
    pub fn restore(
        id: String,
        workspace: PathBuf,
        provider: Arc<dyn Provider>,
        events: Vec<Envelope>,
        history: Vec<ChatMessage>,
    ) -> Arc<Session> {
        Self::build(id, workspace, provider, events, history)
    }

    fn build(
        id: String,
        workspace: PathBuf,
        provider: Arc<dyn Provider>,
        events: Vec<Envelope>,
        history: Vec<ChatMessage>,
    ) -> Arc<Session> {
        std::fs::create_dir_all(&workspace).ok();
        let (tx, _) = broadcast::channel(2048);
        let (instr_tx, instr_rx) = mpsc::unbounded_channel();
        let (side_tx, side_rx) = mpsc::unbounded_channel();
        let session = Arc::new(Session {
            id,
            workspace,
            provider,
            tx,
            log: Mutex::new(Vec::new()),
            emit_lock: Mutex::new(()),
            seq: AtomicU64::new(0),
            state: Mutex::new(SessionState::new()),
            history: Mutex::new(history),
            instr_tx,
            side_tx,
            current_cancel: Mutex::new(None),
            session_dir: Mutex::new(None),
            event_file: Mutex::new(None),
        });
        if !events.is_empty() {
            let mut log = session.log.lock().unwrap();
            for env in &events {
                session.seq.fetch_add(1, Ordering::SeqCst);
                session.state.lock().unwrap().apply(&env.inner);
                log.push(env.clone());
            }
            if log.len() > LOG_CAP {
                let cut = log.len() - LOG_CAP;
                log.drain(0..cut);
            }
            session
                .seq
                .store(log.last().map(|e| e.seq).unwrap_or(0), Ordering::SeqCst);
        }
        let weak = Arc::downgrade(&session);
        tokio::spawn(agent::runner(weak.clone(), instr_rx));
        tokio::spawn(side::runner(weak, side_rx));
        session
    }

    /// Persist the event log and model history into `dir` (created if needed).
    pub fn enable_persistence(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join(EVENTS_FILE);
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        *self.event_file.lock().unwrap() = Some(file);
        *self.session_dir.lock().unwrap() = Some(dir.to_path_buf());
        // A restored session keeps its existing log; a fresh one rewrites from scratch.
        let existing = self.log.lock().unwrap().len();
        if existing > 0 {
            self.rewrite_events(dir);
        }
        Ok(())
    }

    fn rewrite_events(&self, dir: &Path) {
        let log = self.log.lock().unwrap().clone();
        let mut tmp = dir.join("events.jsonl.tmp");
        if std::fs::write(&tmp, serialize_events(&log)).is_err() {
            return;
        }
        let final_path = dir.join(EVENTS_FILE);
        if std::fs::rename(&tmp, &final_path).is_err() {
            let _ = std::fs::remove_file(&tmp);
            return;
        }
        tmp = final_path;
        if let Ok(file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&tmp)
        {
            *self.event_file.lock().unwrap() = Some(file);
        }
    }

    pub fn emit(&self, msg: ServerMsg) {
        let _g = self.emit_lock.lock().unwrap();
        let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        let env = Envelope {
            seq,
            ts: now_ms(),
            inner: msg,
        };
        self.state.lock().unwrap().apply(&env.inner);
        {
            let mut log = self.log.lock().unwrap();
            log.push(env.clone());
            if log.len() > LOG_CAP {
                let excess = log.len() - LOG_CAP;
                log.drain(0..excess);
            }
        }
        self.append_event(&env);
        let _ = self.tx.send(env);
    }

    fn append_event(&self, env: &Envelope) {
        let mut rotate = false;
        {
            let mut guard = self.event_file.lock().unwrap();
            let Some(file) = guard.as_mut() else {
                return;
            };
            let line = match serde_json::to_string(env) {
                Ok(line) => line,
                Err(_) => return,
            };
            if writeln!(file, "{line}").is_err() {
                *guard = None;
                return;
            }
            if env.seq.is_multiple_of(512)
                && file
                    .metadata()
                    .map(|m| m.len() >= ROTATE_BYTES)
                    .unwrap_or(false)
            {
                *guard = None;
                rotate = true;
            }
        }
        if rotate {
            if let Some(dir) = self.session_dir.lock().unwrap().clone() {
                self.rewrite_events(&dir);
            }
        }
    }

    fn persist_history(&self) {
        let Some(dir) = self.session_dir.lock().unwrap().clone() else {
            return;
        };
        let history = self.history.lock().unwrap().clone();
        if let Ok(json) = serde_json::to_vec(&history) {
            let _ = std::fs::write(dir.join(HISTORY_FILE), json);
        }
    }

    pub fn subscribe(&self) -> (broadcast::Receiver<Envelope>, Vec<Envelope>) {
        let _g = self.emit_lock.lock().unwrap();
        let rx = self.tx.subscribe();
        let log = self.log.lock().unwrap().clone();
        (rx, log)
    }

    pub fn seq_now(&self) -> u64 {
        self.seq.load(Ordering::SeqCst)
    }

    pub fn log_tail(&self, n: usize) -> Vec<Envelope> {
        let log = self.log.lock().unwrap();
        log.iter()
            .rev()
            .take(n)
            .cloned()
            .collect::<VecDeque<Envelope>>()
            .into_iter()
            .rev()
            .collect()
    }

    /// Events with `seq > since`, oldest first, capped at `limit`.
    pub fn log_after(&self, since: u64, limit: usize) -> Vec<Envelope> {
        let log = self.log.lock().unwrap();
        log.iter()
            .filter(|e| e.seq > since)
            .take(limit)
            .cloned()
            .collect()
    }

    pub fn state_snapshot(&self) -> SessionState {
        self.state.lock().unwrap().clone()
    }

    pub fn push_history(&self, msg: ChatMessage) {
        self.history.lock().unwrap().push(msg);
    }

    pub fn history_snapshot(&self) -> Vec<ChatMessage> {
        self.history.lock().unwrap().clone()
    }

    pub fn cap_history(&self) {
        let mut h = self.history.lock().unwrap();
        agent::cap_history_rule(&mut h);
    }

    pub fn set_cancel_token(&self, token: Option<CancellationToken>) {
        *self.current_cancel.lock().unwrap() = token;
    }

    pub fn begin_run(&self) -> CancellationToken {
        let token = CancellationToken::new();
        self.set_cancel_token(Some(token.clone()));
        token
    }

    pub fn end_run(&self) {
        self.set_cancel_token(None);
        self.persist_history();
    }

    pub fn submit_instruction(&self, text: String) {
        let _ = self.instr_tx.send(text);
    }

    pub fn ask_side(&self, question: String) {
        let _ = self.side_tx.send(question);
    }

    pub fn interrupt(&self) {
        if let Some(token) = self.current_cancel.lock().unwrap().as_ref() {
            token.cancel();
        } else {
            self.emit(ServerMsg::AgentText {
                text: "nothing to interrupt — idle".to_string(),
            });
        }
    }
}

fn serialize_events(events: &[Envelope]) -> String {
    let mut out = String::new();
    for env in events {
        if let Ok(line) = serde_json::to_string(env) {
            out.push_str(&line);
            out.push('\n');
        }
    }
    out
}

/// Read a persisted session: last `LOG_CAP` events and the model history.
pub fn load_session_files(dir: &Path) -> (Vec<Envelope>, Vec<ChatMessage>) {
    let events_path = dir.join(EVENTS_FILE);
    let mut events: VecDeque<Envelope> = VecDeque::new();
    if let Ok(file) = std::fs::File::open(&events_path) {
        use std::io::BufRead;
        for line in std::io::BufReader::new(file).lines().map_while(Result::ok) {
            if let Ok(env) = serde_json::from_str::<Envelope>(&line) {
                events.push_back(env);
                if events.len() > LOG_CAP {
                    events.pop_front();
                }
            }
        }
    }
    let history = std::fs::read(dir.join(HISTORY_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Vec<ChatMessage>>(&bytes).ok())
        .unwrap_or_default();
    (events.into_iter().collect(), history)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider_mock::Mock;

    fn tmp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("morse-session-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn persistence_roundtrip_restores_state_and_log() {
        let dir = tmp_dir("persist");
        let ws = dir.join("ws");
        let session = Session::new("abc12345".into(), ws.clone(), Arc::new(Mock::new()));
        session.enable_persistence(&dir).unwrap();
        session.emit(ServerMsg::Instruction {
            text: "run echo hi".into(),
        });
        session.emit(ServerMsg::Plan {
            tasks: vec![crate::protocol::TaskView {
                id: "t1".into(),
                title: "run echo hi".into(),
                status: crate::protocol::TaskStatus::Completed,
            }],
        });
        session.emit(ServerMsg::AgentDelta {
            text: "done".into(),
        });
        session.push_history(ChatMessage::user_text("run echo hi"));
        session.persist_history();

        let (events, history) = load_session_files(&dir);
        assert_eq!(events.len(), 3);
        assert_eq!(history.len(), 1);

        let restored = Session::restore(
            "abc12345".into(),
            ws,
            Arc::new(Mock::new()),
            events,
            history,
        );
        let state = restored.state_snapshot();
        assert_eq!(state.tasks.len(), 1);
        assert_eq!(state.agent_text, "done");
        assert_eq!(restored.log_tail(10).len(), 3);
        assert_eq!(restored.seq_now(), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn restored_session_appends_to_existing_file() {
        let dir = tmp_dir("append");
        let ws = dir.join("ws");
        let session = Session::new("s2".into(), ws.clone(), Arc::new(Mock::new()));
        session.enable_persistence(&dir).unwrap();
        session.emit(ServerMsg::Pong);
        let (events, history) = load_session_files(&dir);
        let restored = Session::restore("s2".into(), ws, Arc::new(Mock::new()), events, history);
        restored.enable_persistence(&dir).unwrap();
        restored.emit(ServerMsg::Pong);
        let (all, _) = load_session_files(&dir);
        assert_eq!(all.len(), 2);
        assert_eq!(all[1].seq, 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
