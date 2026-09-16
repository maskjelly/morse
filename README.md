# Morse

**Give an agent a task. Watch it work. Ask another agent for updates.**

Morse is a small agent harness in Rust that runs work on your computer or a server. One terminal pane shows the work. A second pane answers questions about what is done and what is still running — without stopping the work.

Built in Rust. Ships with a demo mode that needs no API key.

## Why Morse

Most coding agents run where you run them and stop when you close the laptop. Morse separates the **server** (a computer that runs commands and edits files) from the **client** (a terminal that watches and steers). That gives you three things:

- **Remote work that survives disconnects.** Close the client; the run continues on the server. Reconnect and replay the log.
- **A side agent while work continues.** `/ask` is answered from live session state by a second agent, in a separate queue, so long commands are never interrupted.
- **A small, auditable runtime.** Roughly 5k lines of Rust across three crates, tests included. No hidden services.

```text
run cargo test on /srv/my-project
/ask what is running?
```

Morse runs the tests on that server. The side agent reports the active command and task status. You can close the client and reconnect later while the server keeps running.

## Quickstart

Requires Rust, Cargo, and Bash on macOS or Linux.

```sh
cargo build --release --locked

# Terminal 1 — the server (demo mode needs no key)
./target/release/morse serve

# Terminal 2 — the client
./target/release/morse connect
```

Try this task, then ask the question during the pause:

```text
run echo started && sleep 10 && echo finished then create file notes.txt: built with Morse
/ask what's done and what's running?
```

Illustrative terminal view during the pause (output shortened):

```text
┌─ stream ───────────────────────────────┬─ side agent (/ask …) ─────────────────┐
│ morse ▸ on it — here's the plan.       │ you ▸ what's done and what's running? │
│ plan 2 tasks                           │                                       │
│   ▸ run echo started && sleep 10 …      │ side ▸ working on: run echo …         │
│   · create file notes.txt               │   progress: 0/2 tasks done            │
│ $ echo started && sleep 10 …            │   [>] run echo started && sleep 10 …  │
│ │ started                               │   [ ] create file notes.txt           │
│                                         │ currently running: bash               │
└─────────────────────────────────────────┴───────────────────────────────────────┘
  ● demo mode • 5d8498e4 • ↑0 ↓0
  > /ask what's next?
```

After the pause, `finished` appears, `notes.txt` is created, and both tasks are marked complete.

## Headless mode

For CI or scripts, `morse run` sends one instruction and streams the result, exiting non-zero when the run fails (provider error, failing last tool call, or timeout):

```sh
MORSE_URL=ws://server:7800/ws morse run "run cargo test" # human-readable
MORSE_URL=ws://server:7800/ws morse run --json "run cargo test" | jq .
```

## Providers

Morse speaks two model APIs and works with anything compatible. Pick one with env vars on the **server**:

| Provider | Setup |
|---|---|
| Demo (default, no key) | nothing — real commands, scripted tool selection |
| Anthropic | `MORSE_PROVIDER=anthropic MORSE_API_KEY=sk-ant-…` |
| OpenAI | `MORSE_PROVIDER=openai MORSE_API_KEY=sk-…` |
| Any OpenAI-compatible API | `MORSE_PROVIDER=openai MORSE_BASE_URL=…` + optional key |
| Local models (Ollama, LM Studio, vLLM) | `MORSE_PROVIDER=openai MORSE_BASE_URL=http://127.0.0.1:11434/v1 MORSE_MODEL=qwen3-coder` |

`MORSE_MODEL` selects the model. Live text streams token-by-token; tool calls stream as they happen. Add a project `AGENTS.md` to the workspace and Morse loads it into the system prompt automatically.

## Sessions that survive restarts

Sessions are persisted under `MORSE_HOME` (default `~/.morse`):

```text
~/.morse/sessions/<id>/
  meta.json      # id, workspace, created
  events.jsonl   # append-only event log (rotated at 8 MiB)
  history.json   # model conversation, so runs continue with context
  ws/            # default workspace for this session
```

Restart the server and the sessions come back — status, task list, token counts, and replay. `morse connect --session <id>` attaches to any of them.

## Commands

| Do this | Command |
|---|---|
| Start the server | `morse serve --bind 127.0.0.1:7800` |
| Open the client | `morse connect` |
| Ask for an update | `/ask what is running?` |
| Stop current work | `/interrupt` |
| Show help | `/help` |
| Disconnect | `/quit` |
| List sessions | `morse sessions` |
| Reconnect to a session | `morse connect --session <id>` |
| Use an existing project | `morse connect --workspace /path/on/server` |
| Plain text output | `morse connect --plain` |
| Run one instruction headlessly | `morse run "run cargo test"` |
| JSON event stream | `morse run --json "run cargo test"` |

## HTTP API

The server exposes the same sessions over REST, so scripts and dashboards can drive it:

```text
GET  /healthz                                  liveness
GET  /api/sessions                             list sessions
POST /api/sessions                             {"workspace": "/path"} create
GET  /api/sessions/{id}                        status snapshot
POST /api/sessions/{id}/instruction            {"text": "run ls"}
POST /api/sessions/{id}/interrupt              cancel current run
GET  /api/sessions/{id}/events?since=&limit=   event log
WS   /ws                                       streaming protocol
```

Set `MORSE_TOKEN` on the server to require `Authorization: Bearer <token>` (WebSocket clients pass `--token`).

## Know before using

- **Disconnecting is fine:** tasks continue while the server runs, and state survives restarts.
- **Commands are unsandboxed:** they run with the server user's permissions. There is no container. Use a dedicated account and keep remote access behind an SSH tunnel or the built-in token.
- **Single-user by design:** no tenant isolation, quotas, or audit log. Session count is capped (`MORSE_MAX_SESSIONS`, default 64); the oldest idle session is evicted from memory, and its files remain on disk.
- **This is a terminal app:** it does not create cloud machines or stream a graphical desktop.

## Learn more

| Doc | Contents |
|---|---|
| [Setup](docs/setup.md) | Install, providers, auth, remote access, services |
| [How it works](docs/how-it-works.md) | The two agents, the event flow, a full example |
| [Architecture](docs/architecture.md) | Protocol, persistence, concurrency, boundaries |
| [Operations](docs/operations.md) | Configuration, recovery, troubleshooting |
| [Testing](docs/testing.md) | Test layout, how to run everything, CI |
| [Benchmarks](docs/benchmarks.md) | Micro and end-to-end benchmarks with sample numbers |
| [Comparison](docs/comparison.md) | How Morse compares to other agent CLIs |
| [Changelog](CHANGELOG.md) | Release history |

MIT licensed.
