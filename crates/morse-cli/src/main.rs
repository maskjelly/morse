use clap::{Parser, Subcommand};

use morse_server::App as ServerApp;

mod client;
mod plain;
mod render;
mod run;
mod tui;

#[derive(Parser)]
#[command(
    name = "morse",
    version,
    about = "Morse: a cloud computer harness — a streaming agent you can steer, plus a side agent for status questions"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Start the server (the cloud computer)
    Serve {
        /// Address to bind, e.g. 127.0.0.1:7800
        #[arg(long, default_value = "127.0.0.1:7800")]
        bind: String,
    },
    /// Connect to a server and stream a session
    Connect {
        /// Server websocket url
        url: Option<String>,
        /// Attach to an existing session (replays its event log)
        #[arg(long)]
        session: Option<String>,
        /// Server-side workspace path for a new session
        #[arg(long)]
        workspace: Option<String>,
        /// Plain line-oriented mode instead of the TUI
        #[arg(long)]
        plain: bool,
        /// Bearer token for a server started with MORSE_TOKEN
        #[arg(long, env = "MORSE_TOKEN", hide_env_values = true)]
        token: Option<String>,
    },
    /// Run one instruction headlessly and stream the result (CI-friendly)
    Run {
        /// Instruction text
        text: String,
        /// Server websocket url
        url: Option<String>,
        /// Attach to an existing session instead of creating one
        #[arg(long)]
        session: Option<String>,
        /// Server-side workspace path for a new session
        #[arg(long)]
        workspace: Option<String>,
        /// Emit one JSON object per event instead of formatted text
        #[arg(long)]
        json: bool,
        /// Bearer token for a server started with MORSE_TOKEN
        #[arg(long, env = "MORSE_TOKEN", hide_env_values = true)]
        token: Option<String>,
    },
    /// List sessions on a server
    Sessions {
        /// Server websocket url
        url: Option<String>,
        /// Bearer token for a server started with MORSE_TOKEN
        #[arg(long, env = "MORSE_TOKEN", hide_env_values = true)]
        token: Option<String>,
    },
}

fn default_url() -> String {
    std::env::var("MORSE_URL").unwrap_or_else(|_| "ws://127.0.0.1:7800/ws".to_string())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Serve { bind } => serve(&bind).await,
        Cmd::Connect {
            url,
            session,
            workspace,
            plain,
            token,
        } => {
            connect(
                url.unwrap_or_else(default_url),
                session,
                workspace,
                plain,
                token,
            )
            .await
        }
        Cmd::Run {
            text,
            url,
            session,
            workspace,
            json,
            token,
        } => {
            run::run_once(
                text,
                url.unwrap_or_else(default_url),
                session,
                workspace,
                json,
                token,
            )
            .await
        }
        Cmd::Sessions { url, token } => sessions(url.unwrap_or_else(default_url), token).await,
    }
}

async fn serve(bind: &str) -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let addr: std::net::SocketAddr = bind.parse()?;
    let provider = morse_core::provider_from_env();
    let demo = provider.is_mock();
    let app = ServerApp::new(provider);
    let (bound, handle) = morse_server::serve(addr, app).await?;
    println!("morse server listening on ws://{bound}/ws");
    if demo {
        println!("demo mode: no LLM key set (export MORSE_API_KEY or ANTHROPIC_API_KEY for a real agent)");
    }
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::select! {
        _ = ctrl_c => {}
        r = handle => {
            if let Err(e) = r {
                anyhow::bail!("server task failed: {e:#}");
            }
        }
    }
    Ok(())
}

async fn connect(
    url: String,
    session: Option<String>,
    workspace: Option<String>,
    is_plain: bool,
    token: Option<String>,
) -> anyhow::Result<()> {
    let cfg = client::ClientConfig {
        url,
        session,
        workspace,
        token,
    };
    if is_plain {
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| "warn".into()),
            )
            .init();
        plain::run_plain(cfg).await
    } else {
        tui::run_tui(cfg).await
    }
}

async fn sessions(url: String, token: Option<String>) -> anyhow::Result<()> {
    let list = client::list_sessions(&url, token.as_deref()).await?;
    let arr = list.as_array().cloned().unwrap_or_default();
    if arr.is_empty() {
        println!("no sessions");
        return Ok(());
    }
    println!("{:<10} {:<8} {:<7} WORKSPACE", "SESSION", "STATUS", "TASKS");
    for s in arr {
        let id = s["id"].as_str().unwrap_or_default();
        let status = s["status"].as_str().unwrap_or_default();
        let done = s["tasks_completed"].as_u64().unwrap_or(0);
        let total = s["tasks_total"].as_u64().unwrap_or(0);
        let ws = s["workspace"].as_str().unwrap_or_default();
        println!("{id:<10} {status:<8} {done}/{total:<6} {ws}");
    }
    Ok(())
}
