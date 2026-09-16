use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use morse_core::llm::{Block, ChatMessage, ChatRequest, ChatResponse, Provider, Purpose};
use morse_core::provider_anthropic::Anthropic;
use morse_core::provider_openai::OpenAi;
use serde_json::{json, Value};

fn req() -> ChatRequest {
    ChatRequest {
        purpose: Purpose::Main,
        session_id: "t".into(),
        system: "be terse".into(),
        messages: vec![ChatMessage::user_text("do it")],
        tools: vec![],
        max_tokens: 256,
    }
}

async fn listen(app: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

fn sse(body: String) -> Response {
    ([("content-type", "text/event-stream")], body).into_response()
}

#[tokio::test]
async fn openai_streams_text_tool_calls_and_usage() {
    async fn handler() -> Response {
        sse([
            r#"data: {"choices":[{"delta":{"content":"hel"}}]}"#,
            r#"data: {"choices":[{"delta":{"content":"lo "}}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"bash","arguments":"{\"command\":"}}]}}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"ls\"}"}}]},"finish_reason":"tool_calls"}]}"#,
            r#"data: {"choices":[],"usage":{"prompt_tokens":7,"completion_tokens":3}}"#,
            "data: [DONE]",
        ].join("\n\n") + "\n\n")
    }
    let addr = listen(Router::new().route("/v1/chat/completions", post(handler))).await;
    let provider = OpenAi::new("test".into(), "fake".into(), format!("http://{addr}/v1"));
    let mut streamed = String::new();
    let resp = provider
        .complete_stream(&req(), &mut |t| streamed.push_str(&t))
        .await
        .unwrap();
    assert_eq!(streamed, "hello ");
    assert_eq!(resp.text(), "hello ");
    assert_eq!(resp.stop_reason, "tool_calls");
    assert_eq!(resp.usage.input_tokens, 7);
    assert_eq!(resp.usage.output_tokens, 3);
    match &resp.blocks[1] {
        Block::ToolUse { name, input, .. } => {
            assert_eq!(name, "bash");
            assert_eq!(input["command"], "ls");
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn openai_non_streaming_complete() {
    async fn handler() -> Response {
        axum::Json(json!({
            "choices": [{"message": {"content": "done"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 2}
        }))
        .into_response()
    }
    let addr = listen(Router::new().route("/chat/completions", post(handler))).await;
    let provider = OpenAi::new("test".into(), "fake".into(), format!("http://{addr}"));
    let resp = provider.complete(&req()).await.unwrap();
    assert_eq!(resp.text(), "done");
    assert_eq!(resp.usage.output_tokens, 2);
}

struct Flaky {
    calls: AtomicUsize,
}

async fn flaky(State(state): State<Arc<Flaky>>) -> Response {
    let n = state.calls.fetch_add(1, Ordering::SeqCst);
    if n == 0 {
        (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "boom").into_response()
    } else {
        axum::Json(json!({"choices": [{"message": {"content": "recovered"}}]})).into_response()
    }
}

#[tokio::test]
async fn provider_retries_transient_failures() {
    let state = Arc::new(Flaky {
        calls: AtomicUsize::new(0),
    });
    let addr = listen(
        Router::new()
            .route("/chat/completions", post(flaky))
            .with_state(state.clone()),
    )
    .await;
    let provider = OpenAi::new("test".into(), "fake".into(), format!("http://{addr}"));
    let resp = provider.complete(&req()).await.unwrap();
    assert_eq!(resp.text(), "recovered");
    assert_eq!(state.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn openai_reports_api_errors() {
    async fn handler() -> Response {
        (
            axum::http::StatusCode::BAD_REQUEST,
            axum::Json(json!({"error": {"message": "bad model"}})),
        )
            .into_response()
    }
    let addr = listen(Router::new().route("/chat/completions", post(handler))).await;
    let provider = OpenAi::new("test".into(), "fake".into(), format!("http://{addr}"));
    let err = provider.complete(&req()).await.unwrap_err().to_string();
    assert!(err.contains("400"), "{err}");
    assert!(err.contains("bad model"), "{err}");
}

#[tokio::test]
async fn anthropic_streams_text_and_tool_use() {
    async fn handler(body: String) -> Response {
        let body: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(body["stream"], true);
        sse([
            r#"data: {"type":"message_start","message":{"usage":{"input_tokens":11,"output_tokens":1}}}"#,
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"wor"}}"#,
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"king"}}"#,
            r#"data: {"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"tu1","name":"bash"}}"#,
            r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"command\":\"pwd\"}"}}"#,
            r#"data: {"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":4}}"#,
            "data: {\"type\":\"message_stop\"}",
        ].join("\n\n") + "\n\n")
    }
    let addr = listen(Router::new().route("/v1/messages", post(handler))).await;
    let provider = Anthropic::new(
        "test".into(),
        "fake".into(),
        format!("http://{addr}/v1/messages"),
    );
    let mut streamed = String::new();
    let resp = provider
        .complete_stream(&req(), &mut |t| streamed.push_str(&t))
        .await
        .unwrap();
    assert_eq!(streamed, "working");
    assert_eq!(resp.usage.input_tokens, 11);
    assert_eq!(resp.usage.output_tokens, 4);
    assert_eq!(resp.stop_reason, "tool_use");
    match &resp.blocks[1] {
        Block::ToolUse { id, input, .. } => {
            assert_eq!(id, "tu1");
            assert_eq!(input["command"], "pwd");
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn anthropic_non_streaming_complete() {
    async fn handler() -> Response {
        axum::Json(json!({
            "content": [{"type": "text", "text": "ok"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 3, "output_tokens": 1}
        }))
        .into_response()
    }
    let addr = listen(Router::new().route("/v1/messages", post(handler))).await;
    let provider = Anthropic::new(
        "test".into(),
        "fake".into(),
        format!("http://{addr}/v1/messages"),
    );
    let resp: ChatResponse = provider.complete(&req()).await.unwrap();
    assert_eq!(resp.text(), "ok");
    assert_eq!(resp.usage.input_tokens, 3);
}
