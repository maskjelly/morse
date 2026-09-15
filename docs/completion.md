# Completion notes

Completed the interrupted T3 implementation on 2026-09-15.

## Changes

- Rewrote the broken WebSocket handler with hello-first, ordered replay and live delivery.
- Fixed compiler errors in the CLI and existing tests.
- Drained stdout/stderr during execution instead of waiting for process exit.
- Stopped demo completion loops and made repeated instructions execute again.
- Stopped demo runs on tool failure instead of marking failed work complete.
- Added client replay deduplication and fixed plain-client quit handling.
- Made interruption cancel pending model requests and Unix shell process groups.
- Preserved tool-result history when interrupting a batch of model tool calls.
- Added call IDs to output/file edits and fixed TUI backspace indexing.
- Removed inaccurate automatic task-status changes; plans now control task status.
- Added setup, architecture, protocol and operations docs, plus macOS/Linux CI.

## Validation

- `cargo build --workspace --locked`: passed locally on macOS.
- `cargo fmt --all -- --check`: passed.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: passed.
- `cargo test --workspace --locked`: 20 tests passed (17 unit, 3 WebSocket integration).
- Integration coverage: early output, concurrent side answers, work surviving disconnect, ordered replay, identical instructions rerunning, file creation, session REST listing, cancellation of shell descendants, recovery after interrupt, and invalid handshakes.
- Actual plain CLI smoke: early shell output and a side answer arrived during a three-second command; file creation and `/quit` succeeded.

The Anthropic path has not been exercised against a paid API. The runtime remains a single-user, in-memory session harness; see the README for its host-access and persistence boundaries.
