# Changelog

All notable changes to Morse. Version 0.2.0 is the first release intended as a complete, self-hosted product.

## 0.2.0 — 2026-09-16

### Added

- **Multi-provider support.** `MORSE_PROVIDER=anthropic|openai|mock` plus auto-detection. The OpenAI-compatible provider works with OpenAI, OpenRouter, Groq, DeepSeek, Ollama, LM Studio, vLLM, and any `/chat/completions` endpoint (`MORSE_BASE_URL`, `MORSE_MODEL`).
- **Streaming model text.** Both providers stream over SSE; tool-call fragments are reassembled across deltas. Clients show prose as it is produced (`agent_delta` events).
- **Token usage.** Providers parse usage; sessions accumulate input/output tokens, stream `usage` events, expose totals over REST, and the TUI status bar shows them.
- **Transient-failure retries.** Network errors and 408/409/425/429/5xx are retried up to three times with backoff.
- **Session persistence and resume.** `~/.morse/sessions/<id>/` stores `meta.json`, `events.jsonl` (rotated at 8 MiB), and `history.json`. Sessions, task state, token totals, replay, and model context survive server restarts.
- **Bearer-token auth.** `MORSE_TOKEN` protects `/api/*` and `/ws`; clients pass `--token` or `MORSE_TOKEN`. `/healthz` stays open.
- **REST API for automation.** Create sessions, submit instructions, interrupt, fetch status snapshots, and poll the event log with `since`/`limit`.
- **Headless mode.** `morse run [--json] "instruction"` streams a single run and exits non-zero on error or timeout (`MORSE_RUN_TIMEOUT`).
- **`glob` and `grep` tools** for file discovery and regex content search, with hidden/build-directory skipping and size caps.
- **Project instructions.** `AGENTS.md` (or `.morse/AGENTS.md`) in the workspace is loaded into the system prompt.
- **Session cap and eviction.** `MORSE_MAX_SESSIONS` (default 64) evicts the oldest idle session from memory only.
- **`MORSE_MAX_TURNS`** to tune the per-instruction turn limit (default 48).
- **Benchmarks.** Criterion micro benchmarks for diff, state, protocol, and tools, plus an end-to-end WebSocket throughput example.
- **Docs.** Setup, testing, benchmarks, comparison, architecture, operations, and this changelog.

### Changed

- Demo-mode completion no longer prints repeated agent notices; plan rendering is no longer duplicated by tool call/result lines.
- `morse sessions` now sorts by creation time and shows task progress.
- Client connect failures with a rejected handshake (e.g. bad token) fail fast instead of retrying forever.

### Fixed

- Runner tasks held strong references to their session, so sessions could never be dropped; they now hold `Weak<Session>` (enables eviction and clean shutdown).
- Interrupted or timed-out commands kill the whole Unix process group, including descendants.

## 0.1.0 — 2026-09-15

Initial streaming harness: WebSocket protocol, session workers, bash/file tools, plan tool, side agent, demo provider, Anthropic provider, ratatui client, ordered replay, and the first integration tests.
