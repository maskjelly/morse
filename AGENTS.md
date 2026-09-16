# AGENTS.md — working on Morse

Morse is a Rust workspace: an agent harness with a server, a terminal client, and a shared core. Keep changes small and simple; this codebase values readability over cleverness.

## Commands

```sh
cargo build --workspace --locked          # build
cargo fmt --all -- --check                # format check (run cargo fmt to fix)
cargo clippy --workspace --all-targets --locked -- -D warnings   # lints (must be clean)
cargo test --workspace --locked           # full suite, offline, no API keys
cargo bench --workspace --no-run          # benchmarks must compile
cargo doc --workspace --no-deps           # docs must build
```

Run the full check before committing:

```sh
cargo fmt --all -- --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked
```

## Layout

- `crates/morse-core` — protocol, session runtime, tools, providers, persistence, side agent. All tests are offline; provider HTTP paths use fake SSE servers in `tests/providers.rs`.
- `crates/morse-server` — Axum server, token auth, REST API, WebSocket transport, session registry.
- `crates/morse-cli` — `morse` binary: `serve`, `connect`, `run`, `sessions`; TUI in `tui.rs`, rendering in `render.rs`.

## Conventions

- No comments unless they explain non-obvious behavior. Prefer small functions over abstractions.
- New protocol messages go in `morse-core/src/protocol.rs` and must handle `snake_case` tags. Adding one requires updating `state::brief`, `render.rs`, and the TUI/plain renderers.
- Session events must be safe to replay: clients dedupe by `seq`, and `hello.replay_seq` marks the replay boundary.
- Keep the demo (`Mock`) provider working for any new tool so tests and no-key users keep a complete loop.
- Never call real model APIs in tests; add a fake handler instead.
- After behavior changes, update the relevant doc in `docs/` and add a line to `CHANGELOG.md`.

## Testing notes

- Integration helpers in `crates/morse-server/tests/api.rs` start a real server on port 0 with `App::with_options` (explicit home dir, token, cap) — prefer this over env vars, which are process-global.
- Use `Mock::new()` and temporary directories; keep timeouts short and bounded.
- The TUI needs a real terminal to test manually; plain mode covers the same rendering paths in CI.
