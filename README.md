# Morse

**Give an agent a task. Watch it work. Ask another agent for updates.**

Morse runs commands and edits files on your computer or server. One terminal pane shows the work. A second pane answers questions about what is done and what is still running.

Built in Rust. Includes a demo that works without an API key.

## A real use case

You are running tests on a remote development server. You want to watch the output and check progress without stopping the tests.

```text
run cargo test
/ask what is running?
```

Morse runs the tests on that server. The side agent reports the active command and task status. You can close the client and reconnect later while the server keeps running.

Connect with `--workspace /path/to/project` to use an existing project on the server. Commands have a default two-minute timeout.

## What you see

Try this demo task, then ask the question during the pause:

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

1. **You send a task** from your terminal.
2. **The main agent does the work** using commands and file tools.
3. **Output appears as it happens**, including file changes and task updates.
4. **The side agent answers questions** from the current task status and recent activity. It does not change files or pause the work.

All commands run on the computer hosting the Morse server.

## Try it

Requires Rust, Cargo, and Bash on macOS or Linux.

**Terminal 1 — start the server:**

```sh
cargo build --workspace --locked
./target/debug/morse serve
```

**Terminal 2 — open the client:**

```sh
./target/debug/morse connect
```

Paste the demo task above. Files are created in `~/.morse/sessions/<id>/ws` on the server.

## Demo or real AI?

| Mode | What you can do |
|---|---|
| Demo: no key needed | Use `run`, `create file`, `write file`, `read file`, and `list files`. Side questions return a status summary. |
| AI: Anthropic key needed | Ask for work in plain language, such as “add a health endpoint and run the tests.” The model chooses tools and answers side questions. |

For AI mode, set `ANTHROPIC_API_KEY` or `MORSE_API_KEY` in the server environment and restart it. Use `MORSE_MODEL` to choose a model. Live model calls have not yet been tested with a paid API key.

Demo mode still runs real commands and writes real files. Separate demo steps with ` then `; use `&&` inside a shell command.

## Useful commands

| Do this | Command |
|---|---|
| Ask for an update | `/ask what is running?` |
| Stop current work | `/interrupt` |
| Disconnect | `/quit` |
| List sessions | `./target/debug/morse sessions` |
| Reconnect | `./target/debug/morse connect --session <id>` |
| Use an existing project | `./target/debug/morse connect --workspace /path/to/project` |
| Use plain text output | `./target/debug/morse connect --plain` |

## Know before using

- **Disconnecting is fine:** tasks continue while the server runs. Recent events replay when you reconnect.
- **Restarting clears history:** files remain, but sessions and task status are held in memory.
- **Use a trusted machine:** commands have the server user's permissions. There is no login or sandbox. Keep remote access behind an SSH tunnel.
- **This is a terminal app:** it does not create cloud machines or stream a graphical desktop.

## Learn more

- [How it works and more examples](docs/how-it-works.md)
- [Run it on a remote server](docs/operations.md)
- [Architecture and protocol](docs/architecture.md)
- [Development checks and completion notes](docs/completion.md)
