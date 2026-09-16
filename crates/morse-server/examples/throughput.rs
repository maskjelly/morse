//! End-to-end throughput benchmark over the real WebSocket protocol.
//!
//! Runs the mock provider so it needs no API key and is reproducible:
//!   cargo run --release -p morse-server --example throughput
//!
//! Env: MORSE_BENCH_N (instructions, default 25), MORSE_BENCH_ECHO (longer command).

use std::sync::Arc;
use std::time::Instant;

use futures_util::{SinkExt, StreamExt};
use morse_core::{Envelope, ServerMsg, StatusKind};
use serde_json::json;
use tokio_tungstenite::tungstenite::Message;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let n: usize = std::env::var("MORSE_BENCH_N")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(25);
    let command = std::env::var("MORSE_BENCH_ECHO").unwrap_or_else(|_| "echo bench".to_string());

    let root = std::env::temp_dir().join(format!("morse-bench-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let app = morse_server::App::with_options(
        Arc::new(morse_core::provider_mock::Mock::new()),
        root.clone(),
        None,
        64,
    );
    let (addr, server) = morse_server::serve("127.0.0.1:0".parse()?, app).await?;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws")).await?;
    ws.send(Message::Text(
        json!({"type":"create", "workspace": root.join("ws")}).to_string(),
    ))
    .await?;

    let mut events = 0usize;
    while let Some(Ok(msg)) = ws.next().await {
        if let Message::Text(text) = msg {
            let env: Envelope = serde_json::from_str(&text)?;
            events += 1;
            if matches!(env.inner, ServerMsg::Hello { .. }) {
                break;
            }
        }
    }

    let started = Instant::now();
    for i in 0..n {
        let instruction = format!("run {command} #{i}");
        ws.send(Message::Text(
            json!({"type":"instruction", "text": instruction}).to_string(),
        ))
        .await?;
        let mut waiting = false;
        while let Some(Ok(msg)) = ws.next().await {
            let Message::Text(text) = msg else { continue };
            let env: Envelope = serde_json::from_str(&text)?;
            events += 1;
            match env.inner {
                ServerMsg::Status {
                    status: StatusKind::Working | StatusKind::Interrupted,
                    ..
                } => waiting = true,
                ServerMsg::Status {
                    status: StatusKind::Idle,
                    ..
                } if waiting => break,
                ServerMsg::Error { message } => anyhow::bail!("server error: {message}"),
                _ => {}
            }
        }
    }
    let elapsed = started.elapsed();

    server.abort();
    let _ = std::fs::remove_dir_all(&root);

    let per_run = elapsed.as_secs_f64() / n as f64;
    println!("morse end-to-end benchmark (mock provider)");
    println!("  instructions        {n}");
    println!("  events              {events}");
    println!("  wall time           {:.3} s", elapsed.as_secs_f64());
    println!(
        "  instructions/sec    {:.1}",
        n as f64 / elapsed.as_secs_f64()
    );
    println!(
        "  events/sec          {:.0}",
        events as f64 / elapsed.as_secs_f64()
    );
    println!(
        "  latency per run     {:.1} ms (round trip, {command:?})",
        per_run * 1000.0
    );
    Ok(())
}
