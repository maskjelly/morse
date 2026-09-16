use criterion::{criterion_group, criterion_main, BatchSize, Criterion};

use morse_core::diff::{cap, unified_diff};
use morse_core::protocol::{Envelope, ServerMsg, TaskStatus, TaskView};
use morse_core::state::SessionState;
use morse_core::tools::{exec_tool, summarize_input, ToolCtx};
use tokio_util::sync::CancellationToken;

fn bench_diff(c: &mut Criterion) {
    let old: String = (0..500).map(|i| format!("line {i} unchanged\n")).collect();
    let new = old.replace("line 250 unchanged", "line 250 changed");
    c.bench_function("diff/unified_500_lines", |b| {
        b.iter(|| unified_diff("src/lib.rs", Some(&old), &new))
    });

    let big = "x".repeat(64 * 1024);
    c.bench_function("diff/cap_64k", |b| b.iter(|| cap(&big, 8192)));
}

fn bench_state(c: &mut Criterion) {
    let plan = ServerMsg::Plan {
        tasks: (0..20)
            .map(|i| TaskView {
                id: format!("t{i}"),
                title: format!("task {i}"),
                status: TaskStatus::Pending,
            })
            .collect(),
    };
    let call = ServerMsg::ToolCall {
        id: "c1".into(),
        tool: "bash".into(),
        input: serde_json::json!({"command": "cargo test"}),
    };
    let result = ServerMsg::ToolResult {
        call_id: "c1".into(),
        tool: "bash".into(),
        ok: true,
        exit_code: Some(0),
        duration_ms: 1234,
        truncated: false,
        summary: "all green".into(),
    };
    c.bench_function("state/apply_plan_call_result", |b| {
        b.iter(|| {
            let mut st = SessionState::new();
            st.apply(&plan);
            st.apply(&call);
            st.apply(&result);
            st
        })
    });
    c.bench_function("state/summarize_bash_input", |b| {
        let input = serde_json::json!({"command": "cargo test --workspace --locked"});
        b.iter(|| summarize_input("bash", &input))
    });
}

fn bench_protocol(c: &mut Criterion) {
    let env = Envelope {
        seq: 42,
        ts: 1_700_000_000_000,
        inner: ServerMsg::ToolCall {
            id: "c1".into(),
            tool: "bash".into(),
            input: serde_json::json!({"command": "cargo test"}),
        },
    };
    c.bench_function("protocol/envelope_roundtrip", |b| {
        b.iter(|| {
            let s = serde_json::to_string(&env).unwrap();
            let back: Envelope = serde_json::from_str(&s).unwrap();
            back.seq
        })
    });
}

fn bench_tools(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    c.bench_function("tools/bash_echo", |b| {
        b.to_async(&rt).iter_batched(
            || {
                let ws = std::env::temp_dir().join(format!("morse-bench-{}", std::process::id()));
                std::fs::create_dir_all(&ws).unwrap();
                ws
            },
            |ws| async move {
                let cancel = CancellationToken::new();
                let mut emit = |_: ServerMsg| {};
                let mut ctx = ToolCtx {
                    workspace: &ws,
                    emit: &mut emit,
                    cancel: &cancel,
                };
                exec_tool(
                    "bash",
                    &serde_json::json!({"command": "echo hello"}),
                    &mut ctx,
                )
                .await
            },
            BatchSize::SmallInput,
        )
    });
}

fn bench_write_file(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    c.bench_function("tools/write_file_8k", |b| {
        b.to_async(&rt).iter_batched(
            || {
                let ws = std::env::temp_dir().join(format!("morse-bench-w-{}", std::process::id()));
                std::fs::create_dir_all(&ws).unwrap();
                (ws, "x".repeat(8192))
            },
            |(ws, content)| async move {
                let cancel = CancellationToken::new();
                let mut emit = |_: ServerMsg| {};
                let mut ctx = ToolCtx {
                    workspace: &ws,
                    emit: &mut emit,
                    cancel: &cancel,
                };
                exec_tool(
                    "write_file",
                    &serde_json::json!({"path": "bench.txt", "content": content}),
                    &mut ctx,
                )
                .await
            },
            BatchSize::SmallInput,
        )
    });
}

criterion_group!(
    benches,
    bench_diff,
    bench_state,
    bench_protocol,
    bench_tools,
    bench_write_file
);
criterion_main!(benches);
