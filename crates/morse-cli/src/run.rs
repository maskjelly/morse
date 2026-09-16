use anyhow::Context;
use tokio::time::{timeout, Duration};

use morse_core::protocol::{ClientMsg, Envelope, ServerMsg, StatusKind};

use crate::client::{provider_banner, ClientConfig};
use crate::render::{render, Render, Style};

/// Headless one-shot mode: submit an instruction, stream the outcome, exit
/// non-zero on connect failure, run error, or timeout.
pub async fn run_once(
    text: String,
    url: String,
    session: Option<String>,
    workspace: Option<String>,
    json: bool,
    token: Option<String>,
) -> anyhow::Result<()> {
    let cfg = ClientConfig {
        url: url.clone(),
        session,
        workspace,
        token,
    };
    let mut handle = crate::client::spawn(cfg);
    handle
        .cmds
        .send(ClientMsg::Instruction { text: text.clone() })
        .ok()
        .context("client stopped before the instruction was sent")?;

    let seconds: u64 = std::env::var("MORSE_RUN_TIMEOUT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(600);
    let deadline = Duration::from_secs(seconds);

    // Replay makes it ambiguous whether an `idle` status belongs to our run, so
    // only events after the replay boundary (`hello.seq`) and after our own
    // instruction event count. That makes re-attaching to a finished session safe.
    let mut baseline = 0u64;
    let mut ours = false;
    let mut working = false;
    let mut failed = false;
    let mut last_tool_ok: Option<bool> = None;
    let mut in_delta = false;
    let outcome = timeout(deadline, async {
        while let Some(env) = handle.events.recv().await {
            if json {
                println!("{}", serde_json::to_string(&env)?);
            } else {
                print_human(&env, &mut in_delta);
            }
            match &env.inner {
                ServerMsg::Hello { replay_seq, .. } => baseline = *replay_seq,
                ServerMsg::Error { .. } => failed = true,
                ServerMsg::ToolResult { ok, .. } if ours => last_tool_ok = Some(*ok),
                ServerMsg::Instruction { text: t } if env.seq > baseline && t == &text => {
                    ours = true;
                }
                ServerMsg::Status {
                    status: StatusKind::Working | StatusKind::Interrupted,
                    ..
                } if ours => working = true,
                ServerMsg::Status {
                    status: StatusKind::Idle,
                    ..
                } if working => return Ok(()),
                _ => {}
            }
        }
        anyhow::bail!("connection closed before the run finished")
    })
    .await;

    if !json {
        flush_delta(&mut in_delta);
    }
    match outcome {
        Ok(result) => result?,
        Err(_) => anyhow::bail!("run timed out after {seconds}s (set MORSE_RUN_TIMEOUT to change)"),
    }
    if failed {
        anyhow::bail!("run reported an error (see output above)");
    }
    if last_tool_ok == Some(false) {
        anyhow::bail!("the last tool call in the run failed (see output above)");
    }
    Ok(())
}

fn print_human(env: &Envelope, in_delta: &mut bool) {
    match &env.inner {
        ServerMsg::Hello { .. } => {
            println!("{}", provider_banner(&env.inner));
        }
        ServerMsg::Status {
            status: StatusKind::Idle,
            ..
        } => flush_delta(in_delta),
        _ => {}
    }
    for item in render(&env.inner) {
        match item {
            Render::Main(style, body) => {
                flush_delta(in_delta);
                print_style(&style, &body);
            }
            Render::MainAppend(_style, body) => {
                if !*in_delta {
                    print!("morse ▸ ");
                    *in_delta = true;
                }
                print!("{body}");
                let _ = std::io::Write::flush(&mut std::io::stdout());
            }
            Render::Side(style, body) => {
                flush_delta(in_delta);
                print_style(&style, &body);
            }
        }
    }
}

fn flush_delta(in_delta: &mut bool) {
    if *in_delta {
        println!();
        *in_delta = false;
    }
}

fn print_style(style: &Style, body: &str) {
    for line in body.lines().filter(|l| !l.is_empty()) {
        println!("{} {line}", style.prefix);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::sync::Arc;

    #[tokio::test]
    async fn run_once_attaches_and_runs_after_replay() {
        let root = std::env::temp_dir().join(format!("morse-run-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let app = morse_server::App::with_options(
            Arc::new(morse_core::provider_mock::Mock::new()),
            root.clone(),
            None,
            8,
        );
        let (addr, server) = morse_server::serve("127.0.0.1:0".parse().unwrap(), app)
            .await
            .unwrap();
        let url = format!("ws://{addr}/ws");
        let http = format!("http://{addr}");

        // First run creates the session and leaves replayed Working/Idle events.
        run_once("run echo first".into(), url.clone(), None, None, true, None)
            .await
            .unwrap();
        let sessions: Value = reqwest::get(format!("{http}/api/sessions"))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let id = sessions[0]["id"].as_str().unwrap().to_string();

        // Second run must not exit on the replayed idle; it should execute.
        run_once(
            "run echo second".into(),
            url,
            Some(id.clone()),
            None,
            true,
            None,
        )
        .await
        .unwrap();

        let session: Value = reqwest::get(format!("{http}/api/sessions/{id}"))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(session["instruction"], "run echo second");
        let events: Value = reqwest::get(format!("{http}/api/sessions/{id}/events"))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(
            events["events"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["type"] == "output"
                    && e["chunk"].as_str().unwrap_or("").contains("second")),
            "follow-up instruction did not run: {events}"
        );
        server.abort();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn run_once_fails_when_last_tool_fails() {
        let root = std::env::temp_dir().join(format!("morse-run-fail-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let app = morse_server::App::with_options(
            Arc::new(morse_core::provider_mock::Mock::new()),
            root.clone(),
            None,
            8,
        );
        let (addr, server) = morse_server::serve("127.0.0.1:0".parse().unwrap(), app)
            .await
            .unwrap();
        let result = run_once(
            "run false".into(),
            format!("ws://{addr}/ws"),
            None,
            None,
            true,
            None,
        )
        .await;
        assert!(result.is_err(), "failing tool should exit non-zero");
        server.abort();
        let _ = std::fs::remove_dir_all(&root);
    }
}
