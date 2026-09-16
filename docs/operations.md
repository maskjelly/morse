# Operations

For running one trusted user's Morse server on macOS or Linux. See [Setup](setup.md) for first-time installation.

## Configuration

| Variable | Scope | Purpose |
|---|---|---|
| `MORSE_HOME` | server | State root; defaults to `~/.morse` |
| `MORSE_BIND` | `morse-server` binary | Bind address; `morse serve` uses `--bind` instead |
| `MORSE_PROVIDER` | server | `anthropic`, `openai`, or `mock` (default: auto-detect, else demo) |
| `MORSE_API_KEY` | server | Credential; also `ANTHROPIC_API_KEY` / `OPENAI_API_KEY` |
| `MORSE_BASE_URL` | server | API base URL override (OpenAI-compatible endpoints) |
| `MORSE_MODEL` | server | Model ID |
| `MORSE_TOKEN` | server + client | Require/send `Authorization: Bearer` |
| `MORSE_MAX_SESSIONS` | server | In-memory session cap, default 64 |
| `MORSE_MAX_TURNS` | server | Model turns per instruction, default 48 |
| `MORSE_URL` | client | Client URL, default `ws://127.0.0.1:7800/ws` |
| `MORSE_RUN_TIMEOUT` | client | `morse run` timeout in seconds, default 600 |
| `RUST_LOG` | both | Log filter |

## Build and run

```sh
cargo build --release --locked
./target/release/morse serve --bind 127.0.0.1:7800
```

Put credentials in the server environment, never in git or a client URL. Run the server as an account with access only to the work it needs. Remote access goes through an SSH tunnel or `MORSE_TOKEN` over TLS.

## State and recovery

```text
~/.morse/sessions/<id>/
  meta.json      # id, workspace, created_ts
  events.jsonl   # append-only event log (rotated at 8 MiB)
  history.json   # model conversation
  ws/            # default workspace
```

- **Restart:** all sessions reload at boot. Status, task list, token totals, replay, and model history are restored; new instructions continue the previous conversation.
- **Eviction:** at `MORSE_MAX_SESSIONS` (64) the oldest **idle** session is dropped from memory. Files stay on disk and reload at the next restart. Running sessions are never evicted; the cap yields to them.
- **Deleting a session is deleting a directory:** stop the server, remove `~/.morse/sessions/<id>`, restart. Workspaces under `ws/` go with it — copy them first if they matter.
- **Corrupt lines** in `events.jsonl` are skipped on load; the rest of the session survives.
- **Rollback an update:** rebuild the previous git revision and restart. There are no migrations.

## Verify

```sh
curl --fail http://127.0.0.1:7800/healthz
./target/release/morse sessions
./target/release/morse run "run echo early && sleep 3 && echo late"
```

During the pause, ask the side agent from another terminal:

```sh
printf '/ask what is running?\n' | ./target/release/morse connect --plain --session <id>
```

Then restart the server and confirm `morse sessions` still lists the session and `morse connect --session <id>` replays it.

## Observe

- `RUST_LOG=morse_core=debug,morse_server=debug` for request/tool detail.
- `GET /api/sessions` for a JSON snapshot of every session; `GET /api/sessions/{id}/events?since=N` for incremental polling.
- `morse run --json` emits one JSON object per event for log pipelines.

## Capacity notes

- One server handles many sessions; each session runs one instruction at a time.
- Events are appended synchronously (one small write per event), so a spinning command producing thousands of lines per second costs disk I/O. The 8 MiB rotation keeps files bounded.
- The broadcast buffer is 2,048 events; a client slower than the stream is disconnected and must reconnect to replay.

## Troubleshooting

- **401 / unauthorized:** server sets `MORSE_TOKEN`; pass `--token` or export it for the client.
- **Connection refused:** check the bind address and tunnel; `morse serve` ignores `MORSE_BIND`.
- **Unknown session:** it was never persisted (pre-0.2.0) or its directory was removed. Create a new session on the old workspace with `--workspace`.
- **Demo banner despite expecting a model:** keys are read by the server at startup; export them and restart.
- **Provider errors:** the client shows the provider message; check key, model name, base URL, and balance. Transient 429/5xx errors are retried automatically.
- **Command timeout:** bash defaults to 120 s; ask for `timeout_ms` or split the command.
- **Session listed but feels slow after restart:** the replay is capped at the newest 4,000 events; older activity is on disk in `events.jsonl`.
