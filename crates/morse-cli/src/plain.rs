use morse_core::protocol::{ClientMsg, Envelope, ServerMsg, StatusKind};

use crate::client::{provider_banner, ClientConfig};
use crate::render::{render, Render, Style};

fn ts(env: &Envelope) -> String {
    chrono::DateTime::<chrono::Local>::from(
        std::time::UNIX_EPOCH + std::time::Duration::from_millis(env.ts),
    )
    .format("%H:%M:%S")
    .to_string()
}

pub async fn run_plain(cfg: ClientConfig) -> anyhow::Result<()> {
    let mut handle = crate::client::spawn(cfg);
    let stdin_tx = handle.cmds;

    tokio::task::spawn_blocking(move || {
        use std::io::BufRead;
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            let Ok(line) = line else { break };
            let line = line.trim().to_string();
            if line.is_empty() {
                continue;
            }
            if line == "/quit" || line == "/q" {
                break;
            }
            let msg = if let Some(q) = line.strip_prefix("/ask ") {
                ClientMsg::SideQuery {
                    text: q.to_string(),
                }
            } else if line == "/interrupt" {
                ClientMsg::Interrupt
            } else {
                ClientMsg::Instruction { text: line }
            };
            if stdin_tx.send(msg).is_err() {
                break;
            }
        }
    });

    while let Some(env) = handle.events.recv().await {
        let stamp = ts(&env);
        match &env.inner {
            ServerMsg::Hello { .. } => {
                println!("[{stamp}] connected: {}", provider_banner(&env.inner));
                println!(
                    "[{stamp}] instructions (demo verbs): run <cmd>, create file <path>: <content>, read file <path>, list files — chain with 'then'. side: /ask <q>. also: /interrupt, /quit"
                );
            }
            ServerMsg::Status { status, .. } if *status == StatusKind::Idle => {
                println!("[{stamp}] ── idle");
            }
            _ => {}
        }
        let is_side = matches!(env.inner, ServerMsg::Side { .. });
        for r in render(&env.inner) {
            match r {
                Render::Main(style, body) => print_lines(&stamp, &style, &body, false),
                Render::Side(style, body) => print_lines(&stamp, &style, &body, true),
            }
        }
        if is_side {
            println!();
        }
    }
    Ok(())
}

fn print_lines(stamp: &str, style: &Style, body: &str, side: bool) {
    let tag = if side { "side│" } else { "main│" };
    let dim = if style.dim { "\x1b[2m" } else { "" };
    let reset = "\x1b[0m";
    for (i, line) in body.lines().filter(|l| !l.is_empty()).enumerate() {
        if i == 0 {
            println!(
                "{stamp} {tag} {}{}{reset}{} {}{}{}{reset}",
                style.prefix_color.ansi(),
                dim,
                style.prefix,
                reset,
                style.prefix_color.ansi(),
                line,
            );
        } else {
            println!(
                "{stamp} {tag}       {}{}{}{reset}",
                style.prefix_color.ansi(),
                dim,
                line
            );
        }
    }
}
