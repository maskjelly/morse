use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpListener;

use morse_core::protocol::{now_ms, ClientMsg, Envelope, ServerMsg, StatusKind};
use morse_core::{load_session_files, new_session_id, Session, TaskStatus};

pub const DEFAULT_MAX_SESSIONS: usize = 64;

#[derive(Clone)]
pub struct App {
    pub provider: Arc<dyn morse_core::llm::Provider>,
    sessions: Arc<tokio::sync::Mutex<HashMap<String, Arc<Session>>>>,
    workspace_root: PathBuf,
    token: Option<String>,
    max_sessions: usize,
}

impl App {
    pub fn new(provider: Arc<dyn morse_core::llm::Provider>) -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        let root = std::env::var("MORSE_HOME").unwrap_or_else(|_| format!("{home}/.morse"));
        Self::with_home(provider, PathBuf::from(root))
    }

    pub fn with_home(provider: Arc<dyn morse_core::llm::Provider>, root: PathBuf) -> Self {
        let token = std::env::var("MORSE_TOKEN").ok().filter(|t| !t.is_empty());
        let max_sessions = std::env::var("MORSE_MAX_SESSIONS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_MAX_SESSIONS);
        Self::with_options(provider, root, token, max_sessions)
    }

    pub fn with_options(
        provider: Arc<dyn morse_core::llm::Provider>,
        root: PathBuf,
        token: Option<String>,
        max_sessions: usize,
    ) -> Self {
        Self {
            provider,
            sessions: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            workspace_root: root,
            token,
            max_sessions,
        }
    }

    pub fn auth_enabled(&self) -> bool {
        self.token.is_some()
    }

    fn sessions_root(&self) -> PathBuf {
        self.workspace_root.join("sessions")
    }

    /// Reload sessions persisted under `MORSE_HOME/sessions/*`. Called once at boot.
    pub async fn load_sessions(&self) -> usize {
        let root = self.sessions_root();
        let Ok(entries) = std::fs::read_dir(&root) else {
            return 0;
        };
        let mut loaded = 0usize;
        for entry in entries.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let Some(meta) = read_meta(&dir) else {
                continue;
            };
            let id = dir
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let workspace = meta["workspace"]
                .as_str()
                .map(PathBuf::from)
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or_else(|| dir.join("ws"));
            let (events, history) = load_session_files(&dir);
            let session = Session::restore(
                id.clone(),
                workspace,
                self.provider.clone(),
                events,
                history,
            );
            if let Some(ts) = meta["created_ts"].as_u64() {
                session.state.lock().unwrap().created_ts = ts;
            }
            let _ = session.enable_persistence(&dir);
            self.sessions.lock().await.insert(id, session);
            loaded += 1;
        }
        if loaded > 0 {
            tracing::info!("restored {loaded} session(s) from {}", root.display());
        }
        loaded
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
            _ => self.sessions_root().join(&id).join("ws"),
        };
        let dir = self.sessions_root().join(&id);
        let session = Session::new(id.clone(), ws.clone(), self.provider.clone());
        let _ = std::fs::create_dir_all(&dir);
        let _ = session.enable_persistence(&dir);
        let created_ts = session.state_snapshot().created_ts;
        let _ = std::fs::write(
            dir.join("meta.json"),
            serde_json::to_vec(&json!({
                "id": id,
                "workspace": ws.display().to_string(),
                "created_ts": created_ts,
            }))
            .unwrap_or_default(),
        );
        let mut sessions = self.sessions.lock().await;
        self.evict_if_needed(&mut sessions);
        sessions.insert(id, session.clone());
        session
    }

    /// Drop the oldest idle session from memory when at capacity.
    /// Persisted files remain, so a release+restart can bring it back.
    fn evict_if_needed(&self, sessions: &mut HashMap<String, Arc<Session>>) {
        while sessions.len() >= self.max_sessions {
            let victim = sessions
                .values()
                .filter(|s| s.state_snapshot().status == StatusKind::Idle)
                .min_by_key(|s| s.state_snapshot().created_ts)
                .map(|s| s.id.clone());
            match victim {
                Some(id) => {
                    tracing::info!(
                        "evicting idle session {id} (capacity {})",
                        self.max_sessions
                    );
                    sessions.remove(&id);
                }
                None => break,
            }
        }
    }

    async fn get_session(&self, id: &str) -> Option<Arc<Session>> {
        self.sessions.lock().await.get(id).cloned()
    }

    async fn list(&self) -> Value {
        let sessions = self.sessions.lock().await;
        let mut arr: Vec<Value> = sessions.values().map(session_json).collect();
        arr.sort_by_key(|v| v["created_ts"].as_u64().unwrap_or(0));
        Value::Array(arr)
    }
}

fn read_meta(dir: &Path) -> Option<Value> {
    let bytes = std::fs::read(dir.join("meta.json")).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn session_json(s: &Arc<Session>) -> Value {
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
        "input_tokens": st.input_tokens,
        "output_tokens": st.output_tokens,
        "created_ts": st.created_ts,
    })
}

pub async fn serve(
    bind: SocketAddr,
    app: App,
) -> anyhow::Result<(SocketAddr, tokio::task::JoinHandle<anyhow::Result<()>>)> {
    let listener = TcpListener::bind(bind).await?;
    let addr = listener.local_addr()?;
    let state = Arc::new(app);
    let protected = Router::new()
        .route("/ws", get(ws_handler))
        .route("/api/sessions", get(list_sessions).post(create_session))
        .route("/api/sessions/{id}", get(get_session))
        .route("/api/sessions/{id}/instruction", post(post_instruction))
        .route("/api/sessions/{id}/interrupt", post(post_interrupt))
        .route("/api/sessions/{id}/events", get(get_events))
        .with_state(state.clone())
        .layer(middleware::from_fn_with_state(state.clone(), auth));
    let router = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .merge(protected);
    let handle = tokio::spawn(async move {
        axum::serve(listener, router)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))
    });
    Ok((addr, handle))
}

async fn auth(
    State(app): State<Arc<App>>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let Some(token) = app.token.as_deref() else {
        return next.run(request).await;
    };
    let header_ok = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|v| v == format!("Bearer {token}"))
        .unwrap_or(false);
    let query_ok = request
        .uri()
        .query()
        .map(|q| q.split('&').any(|kv| kv == format!("token={token}")))
        .unwrap_or(false);
    if header_ok || query_ok {
        next.run(request).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            "unauthorized: send an Authorization: Bearer <token> header",
        )
            .into_response()
    }
}

async fn list_sessions(State(app): State<Arc<App>>) -> Json<Value> {
    Json(app.list().await)
}

async fn get_session(State(app): State<Arc<App>>, UrlPath(id): UrlPath<String>) -> Response {
    match app.get_session(&id).await {
        Some(session) => Json(session_json(&session)).into_response(),
        None => not_found(&id),
    }
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

async fn post_instruction(
    State(app): State<Arc<App>>,
    UrlPath(id): UrlPath<String>,
    body: axum::body::Bytes,
) -> Response {
    let Some(session) = app.get_session(&id).await else {
        return not_found(&id);
    };
    let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let text = body["text"]
        .as_str()
        .map(|s| s.to_string())
        .filter(|s| !s.trim().is_empty());
    let Some(text) = text else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "body must be {\"text\": \"...\"}"})),
        )
            .into_response();
    };
    session.submit_instruction(text);
    (StatusCode::ACCEPTED, Json(json!({"accepted": true}))).into_response()
}

async fn post_interrupt(State(app): State<Arc<App>>, UrlPath(id): UrlPath<String>) -> Response {
    let Some(session) = app.get_session(&id).await else {
        return not_found(&id);
    };
    session.interrupt();
    (StatusCode::ACCEPTED, Json(json!({"accepted": true}))).into_response()
}

#[derive(serde::Deserialize)]
struct EventsQuery {
    since: Option<u64>,
    limit: Option<usize>,
}

async fn get_events(
    State(app): State<Arc<App>>,
    UrlPath(id): UrlPath<String>,
    Query(params): Query<EventsQuery>,
) -> Response {
    let Some(session) = app.get_session(&id).await else {
        return not_found(&id);
    };
    let limit = params.limit.unwrap_or(1000).min(4000);
    let events = session.log_after(params.since.unwrap_or(0), limit);
    Json(json!({
        "session_id": id,
        "last_seq": session.seq_now(),
        "events": events,
    }))
    .into_response()
}

fn not_found(id: &str) -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({"error": format!("unknown session: {id}")})),
    )
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
    let replay_seq = snapshot.last().map(|e| e.seq).unwrap_or(0);
    if socket
        .send(wire(ServerMsg::Hello {
            session_id: session.id.clone(),
            workspace: session.workspace.display().to_string(),
            provider: session.provider.name().to_string(),
            model: session.provider.model(),
            demo: session.provider.is_mock(),
            replay_seq,
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
                    }
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
