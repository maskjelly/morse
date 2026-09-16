# Benchmarks

Two levels: **micro benchmarks** (Criterion, per-function) and an **end-to-end benchmark** (real WebSocket protocol, mock provider). Both run offline with no API key.

## Micro benchmarks

```sh
cargo bench -p morse-core
# quick pass (~4s): add -- --warm-up-time 0.2 --measurement-time 0.5
```

| Benchmark | What it measures |
|---|---|
| `diff/unified_500_lines` | unified diff of a 500-line file with one changed line |
| `diff/cap_64k` | middle-truncation of a 64 KiB string |
| `state/apply_plan_call_result` | applying a plan + tool call + tool result to session state |
| `state/summarize_bash_input` | one-line tool summary used by side-agent context |
| `protocol/envelope_roundtrip` | serialize + deserialize one event envelope |
| `tools/bash_echo` | full bash tool round trip (spawn, pipe, drain, exit) |
| `tools/write_file_8k` | writing 8 KiB plus generating a diff |

Sample run (Apple Silicon, release, August 2026 — numbers are machine-dependent, rerun locally):

| Benchmark | Time |
|---|---|
| `diff/unified_500_lines` | 39.0 µs |
| `diff/cap_64k` | 9.6 µs |
| `state/apply_plan_call_result` | 543 ns |
| `state/summarize_bash_input` | 66 ns |
| `protocol/envelope_roundtrip` | 435 ns |
| `tools/bash_echo` | 1.70 ms |
| `tools/write_file_8k` | 74.6 µs |

`tools/bash_echo` is dominated by process spawn, not Morse; it is the floor for any command-based agent.

## End-to-end throughput

```sh
cargo run --release -p morse-server --example throughput
MORSE_BENCH_N=200 MORSE_BENCH_ECHO="echo x" cargo run --release -p morse-server --example throughput
```

It starts a real server on an ephemeral port, opens a WebSocket, and runs `MORSE_BENCH_N` instructions (default 25) through the full pipeline: instruction → mock provider → plan tool → bash → tool result → idle.

Sample run (Apple Silicon, release):

```text
morse end-to-end benchmark (mock provider)
  instructions        25
  events              451
  wall time           0.076 s
  instructions/sec    328.7
  events/sec          5930
  latency per run     3.0 ms (round trip, "echo bench")
```

Interpretation:

- **Latency per run** is the round trip for a trivial command including event fan-out. Real work is bounded by the shell command and the model provider, not the harness.
- **Events/sec** shows the streaming path (serialize → broadcast → WebSocket → parse) is not the bottleneck at these rates: ~6k events/s single-client.
- Compare a change by running the benchmark before and after; treat ±10% as noise on a laptop.

## What is not benchmarked (yet)

- Live provider latency/tokens (needs keys; varies by provider).
- Multi-client fan-out and long-session memory growth.
- Event-log replay time for a full 4,000-event log (`GET /api/sessions/{id}/events`).

These are reasonable next additions; see `docs/testing.md` for how tests are organized. Run both suites before/after protocol or persistence changes.
