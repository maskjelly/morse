use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use morse_core::protocol::{now_ms, ClientMsg, Envelope, ServerMsg};

#[derive(Clone, Debug)]
pub struct ClientConfig {
    pub url: String,
    pub session: Option<String>,
    pub workspace: Option<String>,
    pub token: Option<String>,
}

pub struct ClientHandle {
    pub events: mpsc::Receiver<Envelope>,
    pub cmds: mpsc::UnboundedSender<ClientMsg>,
}

pub fn spawn(cfg: ClientConfig) -> ClientHandle {
    let (evt_tx, evt_rx) = mpsc::channel::<Envelope>(512);
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<ClientMsg>();
    tokio::spawn(run(cfg, evt_tx, cmd_rx));
    ClientHandle {
        events: evt_rx,
        cmds: cmd_tx,
    }
}

async fn run(
    cfg: ClientConfig,
    evt_tx: mpsc::Sender<Envelope>,
    mut cmd_rx: mpsc::UnboundedReceiver<ClientMsg>,
) {
    let mut session = cfg.session.clone();
    let mut backoff = Duration::from_secs(1);
    let mut last_seq = 0;
    loop {
        match stream_once(&cfg, &mut session, &evt_tx, &mut cmd_rx, &mut last_seq).await {
            Flow::Done => break,
            Flow::Retry(reason) => {
                let _ = evt_tx
                    .send(Envelope {
                        seq: 0,
                        ts: now_ms(),
                        inner: ServerMsg::Error { message: reason },
                    })
                    .await;
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(8));
            }
        }
    }
}

enum Flow {
    Done,
    Retry(String),
}

async fn stream_once(
    cfg: &ClientConfig,
    session: &mut Option<String>,
    evt_tx: &mpsc::Sender<Envelope>,
    cmd_rx: &mut mpsc::UnboundedReceiver<ClientMsg>,
    last_seq: &mut u64,
) -> Flow {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut request = match cfg.url.as_str().into_client_request() {
        Ok(r) => r,
        Err(e) => return Flow::Retry(format!("bad url: {e}")),
    };
    if let Some(token) = &cfg.token {
        match format!("Bearer {token}").parse() {
            Ok(value) => {
                request.headers_mut().insert("authorization", value);
            }
            Err(_) => return Flow::Retry("invalid token".into()),
        }
    }
    let (ws, _) = match tokio_tungstenite::connect_async(request).await {
        Ok(v) => v,
        // A rejected handshake (e.g. bad token) will never succeed on retry.
        Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => {
            let status = resp.status();
            let _ = evt_tx
                .send(Envelope {
                    seq: 0,
                    ts: now_ms(),
                    inner: ServerMsg::Error {
                        message: if status == 401 {
                            "unauthorized: server requires MORSE_TOKEN".to_string()
                        } else {
                            format!("server rejected the connection: {status}")
                        },
                    },
                })
                .await;
            return Flow::Done;
        }
        Err(e) => return Flow::Retry(format!("connect failed: {e}")),
    };
    let (mut sink, mut stream) = ws.split();
    let first = match session {
        Some(id) => ClientMsg::Attach {
            session_id: id.clone(),
        },
        None => ClientMsg::Create {
            workspace: cfg.workspace.clone(),
        },
    };
    if sink
        .send(Message::Text(serde_json::to_string(&first).unwrap()))
        .await
        .is_err()
    {
        return Flow::Retry("send failed".into());
    }

    let hello = match tokio::time::timeout(Duration::from_secs(15), stream.next()).await {
        Ok(Some(Ok(Message::Text(t)))) => serde_json::from_str::<Envelope>(&t),
        _ => return Flow::Retry("no hello from server".into()),
    };
    let hello = match hello {
        Ok(env) if matches!(env.inner, ServerMsg::Hello { .. }) => env,
        Ok(env) if matches!(env.inner, ServerMsg::Error { .. }) => {
            let _ = evt_tx.send(env).await;
            return Flow::Done;
        }
        _ => return Flow::Retry("bad hello".into()),
    };
    if let ServerMsg::Hello { session_id, .. } = &hello.inner {
        *session = Some(session_id.clone());
    }
    if evt_tx.send(hello).await.is_err() {
        return Flow::Done;
    }

    loop {
        tokio::select! {
            cmd = cmd_rx.recv() => {
                match cmd {
                    None => return Flow::Done,
                    Some(msg) => {
                        if sink.send(Message::Text(serde_json::to_string(&msg).unwrap())).await.is_err() {
                            return Flow::Retry("send failed".into());
                        }
                    }
                }
            }
            msg = stream.next() => {
                match msg {
                    None | Some(Err(_)) => return Flow::Retry("connection closed".into()),
                    Some(Ok(Message::Text(t))) => {
                        let env: Envelope = match serde_json::from_str(&t) {
                            Ok(e) => e,
                            Err(e) => {
                                let _ = evt_tx.send(Envelope {
                                    seq: 0,
                                    ts: now_ms(),
                                    inner: ServerMsg::Error { message: format!("bad envelope: {e}") },
                                }).await;
                                continue;
                            }
                        };
                        if env.seq > 0 {
                            if env.seq <= *last_seq { continue; }
                            *last_seq = env.seq;
                        }
                        if evt_tx.send(env).await.is_err() {
                            return Flow::Done;
                        }
                    }
                    Some(Ok(_)) => {}
                }
            }
        }
    }
}

pub async fn list_sessions(origin: &str, token: Option<&str>) -> anyhow::Result<serde_json::Value> {
    let http = http_origin(origin);
    let url = format!("{http}/api/sessions");
    let mut request = reqwest::Client::new().get(&url);
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    let resp = request.send().await?;
    anyhow::ensure!(
        resp.status().is_success(),
        "server returned {}",
        resp.status()
    );
    let body: serde_json::Value = resp.json().await?;
    Ok(body)
}

pub fn http_origin(origin: &str) -> String {
    origin
        .replace("ws://", "http://")
        .replace("wss://", "https://")
        .trim_end_matches("/ws")
        .trim_end_matches('/')
        .to_string()
}

pub fn provider_banner(env: &ServerMsg) -> String {
    match env {
        ServerMsg::Hello {
            session_id,
            workspace,
            provider,
            model,
            demo,
        } => {
            let llm = match (provider.as_str(), model) {
                ("mock", _) => "demo mode (no LLM)".to_string(),
                (p, Some(m)) => format!("{p}/{m}"),
                (p, None) => p.to_string(),
            };
            format!("session {session_id} • {llm} • workspace {workspace} • demo={demo}")
        }
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn rejected_handshake_reports_once_and_stops_retrying() {
        let root = std::env::temp_dir().join(format!("morse-client-{}", now_ms()));
        let app = morse_server::App::with_options(
            Arc::new(morse_core::provider_mock::Mock::new()),
            root.clone(),
            Some("right-token".into()),
            8,
        );
        let (addr, server) = morse_server::serve("127.0.0.1:0".parse().unwrap(), app)
            .await
            .unwrap();
        let mut handle = spawn(ClientConfig {
            url: format!("ws://{addr}/ws"),
            session: None,
            workspace: None,
            token: Some("wrong-token".into()),
        });
        let env = tokio::time::timeout(std::time::Duration::from_secs(5), handle.events.recv())
            .await
            .expect("event timeout")
            .expect("channel closed too early");
        assert!(
            matches!(&env.inner, ServerMsg::Error { message } if message.contains("unauthorized"))
        );
        // Channel closes instead of retrying forever.
        let end =
            tokio::time::timeout(std::time::Duration::from_secs(5), handle.events.recv()).await;
        assert!(matches!(end, Ok(None)));
        server.abort();
        let _ = std::fs::remove_dir_all(&root);
    }
}
