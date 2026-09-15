use std::collections::VecDeque;
use std::path::PathBuf;
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
}

impl Session {
    pub fn new(id: String, workspace: PathBuf, provider: Arc<dyn Provider>) -> Arc<Session> {
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
            history: Mutex::new(Vec::new()),
            instr_tx,
            side_tx,
            current_cancel: Mutex::new(None),
        });
        tokio::spawn(agent::runner(session.clone(), instr_rx));
        tokio::spawn(side::runner(session.clone(), side_rx));
        session
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
        let _ = self.tx.send(env);
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
