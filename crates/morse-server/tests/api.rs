use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use morse_core::{new_session_id, ServerMsg, StatusKind};
use serde_json::{json, Value};

fn temp_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("morse-api-{}-{name}", new_session_id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

async fn start(app: morse_server::App) -> (String, tokio::task::JoinHandle<anyhow::Result<()>>) {
    let (addr, handle) = morse_server::serve("127.0.0.1:0".parse().unwrap(), app)
        .await
        .unwrap();
    (format!("http://{addr}"), handle)
}

async fn post_json(url: &str, body: Value) -> (reqwest::StatusCode, Value) {
    let resp = reqwest::Client::new()
        .post(url)
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = resp.status();
    let body = resp.json().await.unwrap_or(Value::Null);
    (status, body)
}

async fn wait_for_status(client: &reqwest::Client, base: &str, id: &str, want: StatusKind) {
    for _ in 0..100 {
        let body: Value = client
            .get(format!("{base}/api/sessions/{id}"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let status = serde_json::from_value::<StatusKind>(body["status"].clone()).unwrap();
        if status == want {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("session {id} never reached {want:?}");
}

#[tokio::test]
async fn rest_run_read_events_and_interrupt() {
    let root = temp_root("rest");
    let app = morse_server::App::with_options(
        Arc::new(morse_core::provider_mock::Mock::new()),
        root.clone(),
        None,
        64,
    );
    let (base, server) = start(app).await;
    let client = reqwest::Client::new();

    let (status, created) = post_json(&format!("{base}/api/sessions"), json!({})).await;
    assert_eq!(status, 200);
    let id = created["id"].as_str().unwrap().to_string();

    let (status, _) = post_json(
        &format!("{base}/api/sessions/{id}/instruction"),
        json!({"text": "run echo hello-api then create file api.txt: from rest"}),
    )
    .await;
    assert_eq!(status, 202);

    wait_for_status(&client, &base, &id, StatusKind::Idle).await;

    let body: Value = client
        .get(format!("{base}/api/sessions/{id}/events"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let events = body["events"].as_array().unwrap();
    assert!(events
        .windows(2)
        .all(|w| w[0]["seq"].as_u64() < w[1]["seq"].as_u64()));
    assert!(events
        .iter()
        .any(|e| e["type"] == "output" && e["chunk"].as_str().unwrap_or("").contains("hello-api")));
    let last_seq = body["last_seq"].as_u64().unwrap();
    let session: Value = client
        .get(format!("{base}/api/sessions/{id}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(session["tasks_completed"], 2);
    assert!(root.join("sessions").join(&id).join("meta.json").exists());

    // since=last_seq yields nothing new
    let tail: Value = client
        .get(format!("{base}/api/sessions/{id}/events?since={last_seq}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(tail["events"].as_array().unwrap().len(), 0);

    // interrupt while idle produces an informational event
    let (status, _) = post_json(&format!("{base}/api/sessions/{id}/interrupt"), json!({})).await;
    assert_eq!(status, 202);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let after: Value = client
        .get(format!("{base}/api/sessions/{id}/events?since={last_seq}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(after["events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["type"] == "agent_text"
            && e["text"].as_str().unwrap().contains("nothing to interrupt")));

    let (status, _) = post_json(
        &format!("{base}/api/sessions/nope/instruction"),
        json!({"text": "hi"}),
    )
    .await;
    assert_eq!(status, 404);

    let (status, _) = post_json(&format!("{base}/api/sessions/{id}/instruction"), json!({})).await;
    assert_eq!(status, 400);

    server.abort();
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn token_auth_protects_api_and_websocket() {
    let root = temp_root("auth");
    let app = morse_server::App::with_options(
        Arc::new(morse_core::provider_mock::Mock::new()),
        root.clone(),
        Some("s3cret".to_string()),
        64,
    );
    let (base, server) = start(app).await;
    let client = reqwest::Client::new();

    // No token: rejected.
    let resp = client
        .get(format!("{base}/api/sessions"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);

    // Wrong token: rejected.
    let resp = client
        .get(format!("{base}/api/sessions"))
        .bearer_auth("wrong")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);

    // Correct header token: allowed.
    let resp = client
        .get(format!("{base}/api/sessions"))
        .bearer_auth("s3cret")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Query token also works for simple clients.
    let resp = client
        .get(format!("{base}/api/sessions?token=s3cret"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // healthz stays open.
    let resp = client.get(format!("{base}/healthz")).send().await.unwrap();
    assert_eq!(resp.status(), 200);

    // WebSocket upgrade without a token is rejected before any session message.
    let ws_url = base.replace("http://", "ws://") + "/ws";
    let ws = tokio_tungstenite::connect_async(&ws_url).await;
    match ws {
        Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => {
            assert_eq!(resp.status(), 401);
        }
        other => panic!("expected 401 handshake, got {other:?}"),
    }

    server.abort();
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn sessions_survive_server_restart() {
    let root = temp_root("restart");
    let provider = || Arc::new(morse_core::provider_mock::Mock::new());
    let app1 = morse_server::App::with_options(provider(), root.clone(), None, 64);
    let (base, server) = start(app1).await;
    let client = reqwest::Client::new();

    let (_, created) = post_json(&format!("{base}/api/sessions"), json!({})).await;
    let id = created["id"].as_str().unwrap().to_string();
    post_json(
        &format!("{base}/api/sessions/{id}/instruction"),
        json!({"text": "run echo persisted then create file persisted.txt: yes"}),
    )
    .await;
    wait_for_status(&client, &base, &id, StatusKind::Idle).await;
    let before: Value = client
        .get(format!("{base}/api/sessions/{id}/events"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let last_seq = before["last_seq"].as_u64().unwrap();
    server.abort();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Fresh process-level state, same home directory.
    let app2 = morse_server::App::with_options(provider(), root.clone(), None, 64);
    let loaded = app2.load_sessions().await;
    assert_eq!(loaded, 1, "session dir was not restored");
    let (base2, server2) = start(app2).await;
    let session: Value = client
        .get(format!("{base2}/api/sessions/{id}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(session["status"], "idle");
    assert_eq!(session["tasks_completed"], 2);

    let replay: Value = client
        .get(format!("{base2}/api/sessions/{id}/events"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let events = replay["events"].as_array().unwrap();
    assert_eq!(replay["last_seq"].as_u64().unwrap(), last_seq);
    assert!(events.iter().any(|e| e["type"] == "output"));
    assert!(events.iter().any(|e| e["type"] == "tool_result"));

    // The restored session still accepts new work.
    post_json(
        &format!("{base2}/api/sessions/{id}/instruction"),
        json!({"text": "run echo again"}),
    )
    .await;
    wait_for_status(&client, &base2, &id, StatusKind::Idle).await;
    let after: Value = client
        .get(format!("{base2}/api/sessions/{id}/events?since={last_seq}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(after["events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["type"] == "output"));

    server2.abort();
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn session_cap_evicts_oldest_idle() {
    let root = temp_root("cap");
    let app = morse_server::App::with_options(
        Arc::new(morse_core::provider_mock::Mock::new()),
        root.clone(),
        None,
        2,
    );
    let (base, server) = start(app).await;
    let client = reqwest::Client::new();
    let mut ids = Vec::new();
    for _ in 0..3 {
        let (_, created) = post_json(&format!("{base}/api/sessions"), json!({})).await;
        ids.push(created["id"].as_str().unwrap().to_string());
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let list: Value = client
        .get(format!("{base}/api/sessions"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let listed: Vec<&str> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap())
        .collect();
    assert_eq!(listed.len(), 2);
    assert!(
        !listed.contains(&ids[0].as_str()),
        "oldest should be evicted"
    );
    // Eviction is in-memory only: files remain for a future restart.
    assert!(root
        .join("sessions")
        .join(&ids[0])
        .join("meta.json")
        .exists());
    server.abort();
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn websocket_hello_reports_provider() {
    let root = temp_root("hello");
    let app = morse_server::App::with_options(
        Arc::new(morse_core::provider_mock::Mock::new()),
        root.clone(),
        None,
        64,
    );
    let (base, server) = start(app).await;
    let ws_url = base.replace("http://", "ws://") + "/ws";
    let (mut ws, _) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();
    use futures_util::SinkExt;
    use futures_util::StreamExt;
    ws.send(tokio_tungstenite::tungstenite::Message::Text(
        json!({"type":"create"}).to_string(),
    ))
    .await
    .unwrap();
    let msg = ws.next().await.unwrap().unwrap();
    let text = msg.to_text().unwrap().to_string();
    let env: morse_core::Envelope = serde_json::from_str(&text).unwrap();
    assert!(matches!(env.inner, ServerMsg::Hello { demo: true, .. }));
    server.abort();
    let _ = std::fs::remove_dir_all(&root);
}
