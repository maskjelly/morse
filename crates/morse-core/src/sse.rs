use anyhow::{Context, Result};
use futures_util::StreamExt;

/// Feed Server-Sent Events from a streaming HTTP response to `on_data`,
/// one `data:` payload at a time. Lines are split across chunks safely.
pub async fn for_each_data(resp: reqwest::Response, mut on_data: impl FnMut(&str)) -> Result<()> {
    let mut stream = resp.bytes_stream();
    let mut buf = String::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("reading provider stream")?;
        buf.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(pos) = buf.find('\n') {
            let line = buf[..pos].trim_end_matches('\r').to_string();
            buf.drain(..=pos);
            if let Some(data) = line.strip_prefix("data:") {
                let data = data.trim();
                if !data.is_empty() && data != "[DONE]" {
                    on_data(data);
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;
    use axum::routing::get;
    use std::net::SocketAddr;

    async fn sse(
        axum::extract::State(chunks): axum::extract::State<Vec<&'static str>>,
    ) -> impl IntoResponse {
        let mut body = String::new();
        for c in chunks {
            body.push_str(c);
        }
        ([("content-type", "text/event-stream")], body)
    }

    async fn start(chunks: Vec<&'static str>) -> SocketAddr {
        let app = axum::Router::new()
            .route("/sse", get(sse))
            .with_state(chunks);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        addr
    }

    #[tokio::test]
    async fn parses_split_sse_data_lines() {
        let addr = start(vec![
            "data: {\"a\":1",
            "}\n\ndata: {\"b\":2}\n",
            "\ndata: [DONE]\n\n",
        ])
        .await;
        let resp = reqwest::get(format!("http://{addr}/sse")).await.unwrap();
        let mut seen = Vec::new();
        for_each_data(resp, |d| seen.push(d.to_string()))
            .await
            .unwrap();
        assert_eq!(seen, vec!["{\"a\":1}", "{\"b\":2}"]);
    }
}
