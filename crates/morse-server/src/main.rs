use std::net::SocketAddr;
use std::str::FromStr;

use morse_core::provider_from_env;
use morse_server::serve;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let bind = std::env::var("MORSE_BIND").unwrap_or_else(|_| "127.0.0.1:7800".to_string());
    let bind = SocketAddr::from_str(&bind)?;
    let provider = provider_from_env();
    let demo = provider.is_mock();
    let app = morse_server::App::new(provider);
    let (addr, handle) = serve(bind, app).await?;
    tracing::info!(
        "morse server listening on ws://{addr}/ws ({} mode)",
        if demo { "demo" } else { "live" }
    );
    println!("morse server listening on ws://{addr}/ws");
    if demo {
        println!("demo mode: no LLM key set (use MORSE_API_KEY or ANTHROPIC_API_KEY)");
    }
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::select! {
        _ = ctrl_c => {}
        r = handle => {
            if let Err(e) = r {
                tracing::error!("server task failed: {e:#}");
            }
        }
    }
    Ok(())
}
