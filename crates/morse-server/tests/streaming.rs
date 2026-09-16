use futures_util::{SinkExt, StreamExt};
use morse_core::{Envelope, ServerMsg, StatusKind};
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

async fn send(ws: &mut Socket, value: Value) {
    ws.send(Message::Text(value.to_string())).await.unwrap();
}
async fn next(ws: &mut Socket) -> Envelope {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let message = ws.next().await.expect("socket closed").unwrap();
            if let Message::Text(text) = message {
                return serde_json::from_str(&text).unwrap();
            }
        }
    })
    .await
    .expect("event timeout")
}
async fn until(ws: &mut Socket, predicate: impl Fn(&ServerMsg) -> bool) -> Vec<Envelope> {
    let mut events = Vec::new();
    loop {
        let env = next(ws).await;
        assert!(!matches!(env.inner, ServerMsg::Error { .. }), "{:?}", env);
        let done = predicate(&env.inner);
        events.push(env);
        if done {
            return events;
        }
        assert!(events.len() < 200, "agent did not terminate");
    }
}

#[tokio::test]
async fn streaming_side_agent_disconnect_replay_and_repeat() {
    let root = std::env::temp_dir().join(format!("morse-e2e-{}", morse_core::new_session_id()));
    let app = morse_server::App::with_options(
        Arc::new(morse_core::provider_mock::Mock::new()),
        root.clone(),
        None,
        64,
    );
    let (addr, server) = morse_server::serve("127.0.0.1:0".parse().unwrap(), app)
        .await
        .unwrap();
    let url = format!("ws://{addr}/ws");
    let (mut ws, _) = connect_async(&url).await.unwrap();
    send(&mut ws, json!({"type":"create", "workspace":root})).await;
    let hello = next(&mut ws).await;
    let id = match hello.inner {
        ServerMsg::Hello { session_id, .. } => session_id,
        other => panic!("{other:?}"),
    };
    let instruction = "run echo early && sleep 1 && echo late then create file result.txt: saved";
    send(&mut ws, json!({"type":"instruction", "text":instruction})).await;
    let events = until(
        &mut ws,
        |m| matches!(m, ServerMsg::Output { chunk, .. } if chunk.contains("early")),
    )
    .await;
    let first_output = events.last().unwrap();
    let output_seq = first_output.seq;
    assert!(
        matches!(&first_output.inner, ServerMsg::Output { call_id, .. } if !call_id.is_empty())
    );
    send(
        &mut ws,
        json!({"type":"side_query", "text":"what is running?"}),
    )
    .await;
    let side = until(&mut ws, |m| matches!(m, ServerMsg::Side { .. })).await;
    assert!(
        matches!(&side.last().unwrap().inner, ServerMsg::Side { answer, .. } if answer.contains("currently running: bash"))
    );
    // Disconnect while the task runs; execution must continue on the server.
    ws.close(None).await.unwrap();
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let (mut ws, _) = connect_async(&url).await.unwrap();
    send(&mut ws, json!({"type":"attach", "session_id":id})).await;
    assert!(matches!(next(&mut ws).await.inner, ServerMsg::Hello { .. }));
    let replay = until(&mut ws, |m| {
        matches!(
            m,
            ServerMsg::Status {
                status: StatusKind::Idle,
                ..
            }
        )
    })
    .await;
    assert!(replay.windows(2).all(|pair| pair[0].seq < pair[1].seq));
    assert!(replay.iter().any(|e| e.seq == output_seq));
    assert!(replay
        .iter()
        .any(|e| matches!(&e.inner, ServerMsg::Output { chunk, .. } if chunk.contains("late"))));
    assert_eq!(
        std::fs::read_to_string(root.join("result.txt")).unwrap(),
        "saved"
    );
    // Identical instructions must start a fresh run.
    send(&mut ws, json!({"type":"instruction", "text":instruction})).await;
    let second = until(&mut ws, |m| {
        matches!(
            m,
            ServerMsg::Status {
                status: StatusKind::Idle,
                ..
            }
        )
    })
    .await;
    assert!(second
        .iter()
        .any(|e| matches!(&e.inner, ServerMsg::Output { chunk, .. } if chunk.contains("early"))));
    let sessions: Value = reqwest::get(format!("http://{addr}/api/sessions"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(sessions[0]["tasks_completed"], 2);
    ws.close(None).await.unwrap();
    server.abort();
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn interrupt_kills_command_and_accepts_next_instruction() {
    let root: PathBuf =
        std::env::temp_dir().join(format!("morse-interrupt-{}", morse_core::new_session_id()));
    let app = morse_server::App::with_options(
        Arc::new(morse_core::provider_mock::Mock::new()),
        root.clone(),
        None,
        64,
    );
    let (addr, server) = morse_server::serve("127.0.0.1:0".parse().unwrap(), app)
        .await
        .unwrap();
    let (mut ws, _) = connect_async(format!("ws://{addr}/ws")).await.unwrap();
    send(&mut ws, json!({"type":"create", "workspace":root})).await;
    next(&mut ws).await;
    send(
        &mut ws,
        json!({"type":"instruction", "text":"run echo started && sleep 2 && touch escaped"}),
    )
    .await;
    until(&mut ws, |m| matches!(m, ServerMsg::Output { .. })).await;
    send(&mut ws, json!({"type":"interrupt"})).await;
    until(&mut ws, |m| {
        matches!(
            m,
            ServerMsg::Status {
                status: StatusKind::Interrupted,
                ..
            }
        )
    })
    .await;
    send(
        &mut ws,
        json!({"type":"instruction", "text":"run echo recovered"}),
    )
    .await;
    until(&mut ws, |m| {
        matches!(
            m,
            ServerMsg::Status {
                status: StatusKind::Idle,
                ..
            }
        )
    })
    .await;
    tokio::time::sleep(Duration::from_millis(2100)).await;
    assert!(!root.join("escaped").exists(), "cancelled child survived");
    server.abort();
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn rejects_invalid_handshake() {
    let root =
        std::env::temp_dir().join(format!("morse-handshake-{}", morse_core::new_session_id()));
    let app = morse_server::App::with_options(
        Arc::new(morse_core::provider_mock::Mock::new()),
        root,
        None,
        64,
    );
    let (addr, server) = morse_server::serve("127.0.0.1:0".parse().unwrap(), app)
        .await
        .unwrap();
    for command in [
        json!({"type":"ping"}),
        json!({"type":"attach","session_id":"missing"}),
    ] {
        let (mut ws, _) = connect_async(format!("ws://{addr}/ws")).await.unwrap();
        send(&mut ws, command).await;
        assert!(matches!(next(&mut ws).await.inner, ServerMsg::Error { .. }));
    }
    server.abort();
}
