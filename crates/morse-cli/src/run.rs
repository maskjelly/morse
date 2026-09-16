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

    let mut started = false;
    let mut failed = false;
    let mut in_delta = false;
    let outcome = timeout(deadline, async {
        while let Some(env) = handle.events.recv().await {
            if json {
                println!("{}", serde_json::to_string(&env)?);
            } else {
                print_human(&env, &mut in_delta, &mut failed);
            }
            match &env.inner {
                ServerMsg::Hello { .. } => started = true,
                ServerMsg::Status {
                    status: StatusKind::Working | StatusKind::Interrupted,
                    ..
                } => started = true,
                ServerMsg::Status {
                    status: StatusKind::Idle,
                    ..
                } if started => return Ok(()),
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
    Ok(())
}

fn print_human(env: &Envelope, in_delta: &mut bool, failed: &mut bool) {
    match &env.inner {
        ServerMsg::Hello { .. } => {
            println!("{}", provider_banner(&env.inner));
        }
        ServerMsg::Status {
            status: StatusKind::Idle,
            ..
        } => flush_delta(in_delta),
        ServerMsg::Error { .. } => *failed = true,
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
