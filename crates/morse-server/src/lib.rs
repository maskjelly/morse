use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpListener;

use morse_core::protocol::{now_ms, ClientMsg, Envelope, ServerMsg};
use morse_core::{new_session_id, Session, TaskStatus};

#[derive(Clone)]
pub struct App {
    pub provider: Arc<dyn morse_core::llm::Provider>,
    sessions: Arc<tokio::sync::Mutex<HashMap<String, Arc<Session>>>>,
    workspace_root: PathBuf,
}

impl App {
    pub fn new(provider: Arc<dyn morse_core::llm::Provider>) -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        let root = std::env::var("MORSE_HOME").unwrap_or_else(|_| format!("{home}/.morse"));
        Self {
            provider,
            sessions: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            workspace_root: PathBuf::from(root),
        }
    }

    async fn create_session(&self, workspace: Option<String>) -> Arc<Session> {
        let id = new_session_id();
        let ws = match workspace {
            Some(w) if !w.trim().is_empty() => {
                let p = PathBuf::from(&w);
                if p.is_absolute() {
                    p
                } else {
                    std::env::current_dir().unwrap_or_default().join(p)
                }
            }
            _ => self.workspace_root.join("sessions").join(&id).join("ws"),
        };
        let session = Session::new(id, ws, self.provider.clone());
        self.sessions
            .lock()
            .await
            .insert(session.id.clone(), session.clone());
        session
    }

    async fn get_session(&self, id: &str) -> Option<Arc<Session>> {
        self.sessions.lock().await.get(id).cloned()
    }

    async fn list(&self) -> Value {
        let sessions = self.sessions.lock().await;
        let arr: Vec<Value> = sessions
            .values()
            .map(|s| {
                let st = s.state_snapshot();
                json!({
                    "id": s.id,
                    "workspace": s.workspace.display().to_string(),
                    "status": serde_json::to_value(st.status).unwrap_or_default(),
                    "instruction": st.instruction,
                    "tasks": st.tasks.iter().map(|t| json!({
                        "title": t.title,
                        "status": serde_json::to_value(t.status).unwrap_or_default(),
                    })).collect::<Vec<_>>(),
                    "tasks_completed": st.tasks.iter().filter(|t| t.status == TaskStatus::Completed).count(),
                    "tasks_total": st.tasks.len(),
                    "created_ts": st.created_ts,
                })
            })
            .collect();
        Value::Array(arr)
    }
}

pub async fn serve(
    bind: SocketAddr,
    app: App,
) -> anyhow::Result<(SocketAddr, tokio::task::JoinHandle<anyhow::Result<()>>)> {
    let listener = TcpListener::bind(bind).await?;
    let addr = listener.local_addr()?;
    let router = Router::new()
        .route("/ws", get(ws_handler))
        .route("/api/sessions", get(list_sessions).post(create_session))
        .route("/healthz", get(|| async { "ok" }))
        .with_state(Arc::new(app));
    let handle = tokio::spawn(async move {
        axum::serve(listener, router)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))
    });
    Ok((addr, handle))
}

async fn list_sessions(State(app): State<Arc<App>>) -> Json<Value> {
    Json(app.list().await)
}

async fn create_session(State(app): State<Arc<App>>, body: axum::body::Bytes) -> Response {
    let body: Value = if body.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&body).unwrap_or(json!({}))
    };
    let session = app
        .create_session(body["workspace"].as_str().map(|s| s.to_string()))
        .await;
    let st = session.state_snapshot();
    Json(json!({
        "id": session.id,
        "workspace": session.workspace.display().to_string(),
        "provider": session.provider.name(),
        "model": session.provider.model(),
        "demo": session.provider.is_mock(),
        "created_ts": st.created_ts,
    }))
    .into_response()
}

async fn ws_handler(State(app): State<Arc<App>>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| handle_socket(app, socket))
}

fn wire(inner: ServerMsg) -> Message {
    envelope_wire(&Envelope {
        seq: 0,
        ts: now_ms(),
        inner,
    })
}

fn envelope_wire(env: &Envelope) -> Message {
    Message::Text(
        serde_json::to_string(env)
            .expect("serializable event")
            .into(),
    )
}

async fn handle_socket(app: Arc<App>, mut socket: WebSocket) {
    let first = tokio::time::timeout(std::time::Duration::from_secs(15), socket.recv()).await;
    let command = match first {
        Ok(Some(Ok(Message::Text(text)))) => serde_json::from_str::<ClientMsg>(&text),
        _ => {
            let _ = socket
                .send(wire(ServerMsg::Error {
                    message: "expected create/attach first".into(),
                }))
                .await;
            return;
        }
    };
    let session = match command {
        Ok(ClientMsg::Create { workspace }) => app.create_session(workspace).await,
        Ok(ClientMsg::Attach { session_id }) => match app.get_session(&session_id).await {
            Some(session) => session,
            None => {
                let _ = socket
                    .send(wire(ServerMsg::Error {
                        message: format!("unknown session: {session_id}"),
                    }))
                    .await;
                return;
            }
        },
        _ => {
            let _ = socket
                .send(wire(ServerMsg::Error {
                    message: "expected valid create/attach first".into(),
                }))
                .await;
            return;
        }
    };
    // Subscribe and snapshot atomically, then send hello and replay before live events.
    let (mut sub, snapshot) = session.subscribe();
    if socket
        .send(wire(ServerMsg::Hello {
            session_id: session.id.clone(),
            workspace: session.workspace.display().to_string(),
            provider: session.provider.name().to_string(),
            model: session.provider.model(),
            demo: session.provider.is_mock(),
        }))
        .await
        .is_err()
    {
        return;
    }
    for env in snapshot {
        if socket.send(envelope_wire(&env)).await.is_err() {
            return;
        }
    }
    let (mut sink, mut stream) = socket.split();
    loop {
        tokio::select! {
            event = sub.recv() => {
                let message = match event {
                    Ok(env) => envelope_wire(&env),
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        // Close so clients reconnect and replay instead of silently losing events.
                        let _ = sink.send(Message::Close(None)).await;
                        break;
                    },
                    Err(_) => break,
                };
                if sink.send(message).await.is_err() { break; }
            }
            message = stream.next() => {
                let text = match message {
                    Some(Ok(Message::Text(text))) => text,
                    Some(Ok(Message::Ping(bytes))) => {
                        if sink.send(Message::Pong(bytes)).await.is_err() { break; }
                        continue;
                    }
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    _ => continue,
                };
                let response = match serde_json::from_str::<ClientMsg>(&text) {
                    Ok(ClientMsg::Instruction { text }) => { session.submit_instruction(text); None }
                    Ok(ClientMsg::SideQuery { text }) => { session.ask_side(text); None }
                    Ok(ClientMsg::Interrupt) => { session.interrupt(); None }
                    Ok(ClientMsg::Ping) => Some(ServerMsg::Pong),
                    Ok(_) => Some(ServerMsg::Error { message: "already attached".into() }),
                    Err(e) => Some(ServerMsg::Error { message: format!("bad message: {e}") }),
                };
                if let Some(response) = response {
                    if sink.send(wire(response)).await.is_err() { break; }
                }
            }
        }
    }
}
