# Testing

Morse is tested at three levels: unit tests in `morse-core`, protocol integration tests that run a real server on an ephemeral port, and CLI-level tests. Provider HTTP paths are tested against local fake SSE servers, so the suite runs offline with no API keys.

## Run everything

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo bench --workspace --no-run     # benchmarks compile
cargo doc --workspace --no-deps      # docs build
```

In one line (what CI runs):

```sh
cargo fmt --all -- --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked
```

## Test layout

| Suite | Location | Covers |
|---|---|---|
| Core unit | `crates/morse-core/src/**` (`#[cfg(test)]`) | protocol round-trips, diff/cap, state tracking, history capping, tools (bash/Files/glob/grep/safe paths), mock provider flow, OpenAI + Anthropic stream parsing, SSE line splitting, persistence round-trip |
| Provider integration | `crates/morse-core/tests/providers.rs` | real HTTP against fake OpenAI/Anthropic servers: streaming text, streamed tool-call assembly, usage, retry on 5xx, API error reporting |
| Server integration | `crates/morse-server/tests/streaming.rs` | live WebSocket: early output, side answers during a running command, disconnect/replay, identical-instruction rerun, interrupt kills process groups, invalid handshakes |
| Server API/auth/persistence | `crates/morse-server/tests/api.rs` | REST run + event log, token auth (HTTP + WS), restart persistence and resume, idle-session eviction, WS hello |
| CLI | `crates/morse-cli/src/client.rs` (`#[cfg(test)]`) | rejected handshake stops retrying instead of looping |

Current count: **42 tests** (27 core unit, 6 provider integration, 8 server integration, 1 CLI).

## What each important test proves

- **Streaming** — `openai_streams_text_tool_calls_and_usage` and `anthropic_streams_text_and_tool_use` feed split SSE frames into the providers and assert the assembled blocks, tool calls, stop reason, and token usage.
- **Retry** — `provider_retries_transient_failures` returns 500 once, then 200, and asserts the request was retried exactly once.
- **Crash-free reconnect** — `streaming_side_agent_disconnect_replay_and_repeat` closes the socket mid-command, reattaches, and asserts ordered replay (monotonic `seq`) with no lost output.
- **Cancellation** — `interrupt_kills_command_and_accepts_next_instruction` interrupts `sleep` and asserts the descendant never ran (no `escaped` file), then runs another instruction.
- **Durability** — `sessions_survive_server_restart` runs work, rebuilds the app from the same home directory, and asserts status, task counts, replay, and new instructions all work.
- **Auth** — `token_auth_protects_api_and_websocket` asserts 401 without/with a wrong token, 200 with a header or query token, and a rejected WS upgrade.
- **Workspace safety** — `safe_path_rejects_escape` plus tool tests confirm traversal and absolute paths outside the workspace are rejected.

## Manual smoke test

Demo mode exercises the whole loop without a key:

```sh
MORSE_HOME=$(mktemp -d) ./target/debug/morse serve --bind 127.0.0.1:7800 &

MORSE_URL=ws://127.0.0.1:7800/ws ./target/debug/morse run \
  "run echo early && sleep 1 && echo late then create file notes.txt: ok"

MORSE_URL=ws://127.0.0.1:7800/ws ./target/debug/morse sessions
head -c 200 "$MORSE_HOME/sessions/"*/notes.txt
```

During a long run, ask the side agent from a second client:

```sh
printf '/ask what is running?\n' | morse connect --plain --session <id>
```

## Adding tests

- Core behavior: add `#[cfg(test)]` modules next to the code (see `tools.rs`, `session.rs`).
- Wire changes: extend `crates/morse-server/tests/api.rs`; the helpers there start a real server on port 0.
- Provider changes: add a fake handler in `crates/morse-core/tests/providers.rs`; never call the real APIs in tests.
- Keep tests deterministic: use the mock provider, temporary directories, and bounded timeouts.

## CI

`.github/workflows/ci.yml` runs on Linux and macOS: format check, clippy with `-D warnings`, the full test suite, a workspace build, documentation build, and a benchmark compile. No secrets are required.
