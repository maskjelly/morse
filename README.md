# Morse

A Rust agent harness for a computer you run locally or on a server. Send a task, watch shell output and file changes arrive over WebSocket, and ask a separate side agent what's happening while the main agent works.

## Start in two terminals

Requires Rust/Cargo and Bash on macOS or Linux.

```sh
cargo build --workspace --locked
./target/debug/morse serve
```

In another terminal:

```sh
./target/debug/morse connect
```

Try these in the client:

```text
run echo hello && sleep 3 && echo finished then create file notes.txt: built with Morse
/ask what's done and what's running?
```

The left pane shows work; the right pane shows side-agent answers. No API key is needed for this deterministic demo. Demo mode understands `run <shell command>`, `create file <path>: <content>`, `write file`, `read file`, and `list files`. Separate steps with lowercase ` then ` or `;`; use `&&` inside shell commands. Unsupported requests print a demo message.

**Demo commands execute real shell commands and write real files.** The default workspace is a new directory under `~/.morse/sessions/<id>/ws`.

## Use a real model

Set `ANTHROPIC_API_KEY` (or `MORSE_API_KEY`) in the **server's environment**, then restart the server. `MORSE_MODEL` selects the Anthropic model; the current code defaults to `claude-sonnet-4-5`. The client needs no model credentials.

The main agent can run Bash, read/write/edit files, list files, and update a task plan. The side agent receives recent activity and session state, with no tools. Shell output streams live; model prose arrives once each model response completes.

Live API execution has not been validated with a paid model key. Automated tests use the demo provider.

## Commands

| Action | Command |
|---|---|
| Plain terminal client | `morse connect --plain` |
| List server sessions | `morse sessions` |
| Reattach and replay recent events | `morse connect --session <id>` |
| Choose a server-side workspace | `morse connect --workspace /absolute/path` |
| Ask the side agent | `/ask <question>` |
| Cancel current work | `/interrupt` |
| Disconnect | `/quit` or `/q` |

In the TUI, Tab selects the pane to scroll, PageUp/PageDown scroll it, and Ctrl+C exits. Disconnecting leaves server work running. Reconnecting replays the retained events.

## Run on a remote computer

Run `morse serve` on your server, then forward its loopback port:

```sh
ssh -N -L 7800:127.0.0.1:7800 user@your-server
```

On your laptop, run `morse connect`. Keep the server process alive using your normal service manager or terminal multiplexer.

**Boundary:** this is a single-user host agent, with no authentication or OS sandbox. Bash has the server user's permissions. Keep it bound to loopback and use SSH; do not expose its port to the public internet. Workspace path checks are convenience checks, not an isolation boundary.

## What persists

- Tasks survive client disconnection while the server keeps running.
- Workspace files stay on disk.
- Sessions, model history, task state, and the last 4,000 events live in memory and are lost on server restart.
- This version provides a terminal interface and event stream; it does not provision VMs or stream a graphical desktop.

## Develop

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --workspace --locked
```

[Architecture and protocol](docs/architecture.md) · [Operations](docs/operations.md) · [Completion notes](docs/completion.md)
