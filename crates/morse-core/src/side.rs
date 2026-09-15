use std::sync::Arc;

use crate::llm::{Block, ChatMessage, ChatRequest, Purpose};
use crate::session::Session;
use crate::state::brief;

pub const SIDE_PROMPT: &str =
    "You are the side agent of a Morse cloud computer session: you watch the main \
agent's live event stream and answer the user's side questions. Do not perform work yourself. \
Answer briefly and factually: what's done, what's in progress, what's next, any errors. \
One to six lines, no preamble.";

pub async fn runner(session: Arc<Session>, mut rx: tokio::sync::mpsc::UnboundedReceiver<String>) {
    let mut history: Vec<ChatMessage> = Vec::new();
    while let Some(question) = rx.recv().await {
        let answer = answer(session.as_ref(), &mut history, &question).await;
        session.emit(crate::protocol::ServerMsg::Side { question, answer });
    }
}

pub async fn answer(session: &Session, history: &mut Vec<ChatMessage>, question: &str) -> String {
    let state = session.state_snapshot();
    if session.provider.is_mock() {
        return state.heuristic_answer();
    }

    let tail: Vec<String> = session
        .log_tail(40)
        .iter()
        .filter_map(|e| brief(&e.inner))
        .collect();
    let system = format!(
        "{SIDE_PROMPT}\n\nSESSION STATE:\n{}\n\nRECENT EVENTS:\n{}",
        serde_json::to_string_pretty(&state.summary_json()).unwrap_or_default(),
        tail.join("\n"),
    );

    let mut messages = history.clone();
    messages.push(ChatMessage::user_text(format!(
        "{question}\n\nAnswer from the session state above."
    )));

    let req = ChatRequest {
        purpose: Purpose::Side,
        session_id: session.id.clone(),
        system,
        messages,
        tools: vec![],
        max_tokens: 1024,
    };
    match session.provider.complete(&req).await {
        Ok(resp) => {
            let text = resp.text();
            history.push(ChatMessage::user_text(question.to_string()));
            history.push(ChatMessage::assistant(
                resp.blocks
                    .into_iter()
                    .filter(|b| matches!(b, Block::Text { .. }))
                    .collect::<Vec<_>>(),
            ));
            while history.len() > 16 {
                history.remove(0);
            }
            if text.trim().is_empty() {
                state.heuristic_answer()
            } else {
                text.trim().to_string()
            }
        }
        Err(_) => format!(
            "(llm unavailable — heuristic answer)\n{}",
            state.heuristic_answer()
        ),
    }
}
