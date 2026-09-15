# Morse

A Rust agent harness for a computer you run locally or on a server. Send a task, watch shell output and file changes arrive over WebSocket, and ask a separate side agent what's happening while the main agent works.

## See it in action

Send a task to the main agent, then ask the side agent for an update while it runs:

```text
run echo started && sleep 10 && echo finished then create file notes.txt: built with Morse
/ask what's done and what's running?
```

Illustrative terminal view during the pause (output shortened):

```text
┌─ stream ───────────────────────────────┬─ side agent (/ask …) ─────────────────┐
│ morse ▸ on it — here's the plan.       │ you ▸ what's done and what's running? │
│                                       │                                       │
│ ▸ run echo started && sleep 10 …       │ side ▸ progress: 0/2 tasks done        │
│ · create file notes.txt               │   [>] run echo started && sleep 10 …  │
│                                       │   [ ] create file notes.txt           │
│ $ echo started && sleep 10 …           │                                       │
│ │ started                             │ currently running: bash               │
│                                       │ echo started && sleep 10 …            │
└───────────────────────────────────────┴───────────────────────────────────────┘
  ● demo mode • session <id>
  > /ask what's next?
```

After the pause, `finished` appears, `notes.txt` is created, and both tasks are marked complete. The side question does not pause the main task.

## How it works

1. **Connect:** the client opens a session on the server, with its own workspace.
2. **Request:** the main agent selects tools to run commands and change files.
3. **Watch:** the server sends output, file diffs, and task updates as events.
4. **Ask alongside:** a separate worker answers side questions using current state and recent activity.
5. **Return later:** reconnect to the same running server to replay recent events.

Without a key, a deterministic demo parser selects tools. With an Anthropic key, a model selects tools and answers side questions. Execution always happens on the server computer.

## Use cases

| Use case | Main task | Side question |
|---|---|---|
| Remote build | `run cargo build` in an existing Rust workspace | `/ask what is running?` |
| Test run | `run cargo test` | `/ask did the command finish?` |
| File workflow | `create file notes.txt: first draft then read file notes.txt` | `/ask what's complete?` |
| Model-assisted coding¹ | “Add a health endpoint and run the tests.” | `/ask which files changed?` |

¹ Requires a configured model. The demo side agent gives a fixed status summary; the live side agent can address the specific question. Use `--workspace` to select an existing project on the server.

**Read the guide:** [How Morse works, use cases, and a full walkthrough](docs/how-it-works.md).

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
