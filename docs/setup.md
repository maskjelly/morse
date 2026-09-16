# Setup

Install Morse, connect a model, and run it safely. macOS and Linux are supported.

## 1. Build

```sh
git clone <your-fork-or-remote>
cd morse
cargo build --release --locked
```

Binaries:

- `target/release/morse` — server, client, and headless runner in one CLI.
- `target/release/morse-server` — standalone server binary (same behavior as `morse serve`).

Requires a stable Rust toolchain and `bash` on the server host.

## 2. Run the server

```sh
./target/release/morse serve --bind 127.0.0.1:7800
```

Without any model key the server starts in **demo mode**: commands and file edits are real, but tool selection comes from a deterministic parser (`run …`, `create file …`, `read file …`, `list files`, `glob …`, `grep …`, chained with ` then ` or `;`). Use it to verify the whole loop without spending API credits.

Session state and workspaces live under `MORSE_HOME` (default `~/.morse`).

## 3. Connect a model

Configure models in the **server** environment — clients never need keys.

### Anthropic

```sh
export MORSE_PROVIDER=anthropic
export MORSE_API_KEY=sk-ant-…            # or ANTHROPIC_API_KEY
export MORSE_MODEL=claude-sonnet-4-5     # default
```

### OpenAI

```sh
export MORSE_PROVIDER=openai
export MORSE_API_KEY=sk-…                # or OPENAI_API_KEY
export MORSE_MODEL=gpt-4o-mini           # default
```

### Any OpenAI-compatible endpoint

Works with OpenRouter, Groq, Together, DeepSeek, Fireworks, Azure-style gateways, and local servers:

```sh
# OpenRouter
export MORSE_PROVIDER=openai
export MORSE_BASE_URL=https://openrouter.ai/api/v1
export MORSE_API_KEY=sk-or-…
export MORSE_MODEL=anthropic/claude-sonnet-4.5

# Ollama, no key needed
export MORSE_PROVIDER=openai
export MORSE_BASE_URL=http://127.0.0.1:11434/v1
export MORSE_MODEL=qwen3-coder

# LM Studio / vLLM
export MORSE_PROVIDER=openai
export MORSE_BASE_URL=http://127.0.0.1:1234/v1
export MORSE_MODEL=your-model
```

Auto-detection: if `MORSE_PROVIDER` is unset, Morse uses OpenAI when `MORSE_BASE_URL` or `OPENAI_API_KEY` is present, then Anthropic when `ANTHROPIC_API_KEY` or `MORSE_API_KEY` is present, and otherwise demo mode. Set `MORSE_PROVIDER=mock` to force demo mode.

Morse retries transient provider failures (network errors, 429, 5xx) up to three times with backoff.

## 4. Open the client

```sh
# local server
./target/release/morse connect

# remote server
MORSE_URL=ws://127.0.0.1:7900/ws ./target/release/morse connect
```

Useful client flags:

```sh
morse connect --workspace /path/on/server     # use an existing project
morse connect --session <id>                  # reattach and replay
morse connect --plain                         # line output, no TUI
morse run "run cargo test"                    # one-shot, CI-friendly
morse run --json "run cargo test"             # JSONL events
morse sessions                                # list sessions
```

## 5. Secure a remote server

Morse executes arbitrary commands as the server user. Treat it like a shell.

**Option A — SSH tunnel (recommended):**

```sh
# on your machine, keep the server bound to loopback on the remote host
ssh -L 7800:127.0.0.1:7800 user@server
MORSE_URL=ws://127.0.0.1:7800/ws morse connect
```

**Option B — built-in token:**

```sh
# server
export MORSE_TOKEN=$(openssl rand -hex 24)
./target/release/morse serve --bind 0.0.0.0:7800

# client (URL is positional; MORSE_URL is the default source)
MORSE_URL=ws://server:7800/ws morse connect --token "$MORSE_TOKEN"
MORSE_URL=ws://server:7800/ws morse run --token "$MORSE_TOKEN" "run uname -a"
```

The token protects HTTP and WebSocket endpoints; `/healthz` stays open. It is a shared secret over plain `ws://` — pair it with TLS (a reverse proxy) or keep the tunnel.

Run the server as an account that only has access to the projects it needs.

## 6. Run as a service

### systemd (Linux)

```ini
# /etc/systemd/system/morse.service
[Unit]
Description=Morse agent server
After=network.target

[Service]
User=agent
Environment=MORSE_HOME=/home/agent/.morse
Environment=MORSE_PROVIDER=anthropic
Environment=MORSE_API_KEY=<key>
Environment=MORSE_TOKEN=<token>
ExecStart=/usr/local/bin/morse serve --bind 127.0.0.1:7800
Restart=on-failure

[Install]
WantedBy=multi-user.target
```

### launchd (macOS)

```sh
# ~/Library/LaunchAgents/dev.morse.server.plist — run at login
launchctl load -w ~/Library/LaunchAgents/dev.morse.server.plist
```

Keep both behind the tunnel or token; do not expose them directly to the internet.

## Environment reference

| Variable | Scope | Purpose |
|---|---|---|
| `MORSE_HOME` | server | State root, default `~/.morse` |
| `MORSE_BIND` | `morse-server` binary | Bind address (use `--bind` for `morse serve`) |
| `MORSE_PROVIDER` | server | `anthropic`, `openai`, or `mock` |
| `MORSE_API_KEY` | server | Credential (also `ANTHROPIC_API_KEY` / `OPENAI_API_KEY`) |
| `MORSE_BASE_URL` | server | Override API base URL |
| `MORSE_MODEL` | server | Model ID |
| `MORSE_TOKEN` | server + client | Shared bearer token |
| `MORSE_MAX_SESSIONS` | server | In-memory session cap, default 64 |
| `MORSE_MAX_TURNS` | server | Model turns per instruction, default 48 |
| `MORSE_URL` | client | Server WebSocket URL |
| `MORSE_RUN_TIMEOUT` | client | `morse run` timeout in seconds, default 600 |
| `RUST_LOG` | both | Log filter, e.g. `morse_core=debug` |

## Verify the install

```sh
curl --fail http://127.0.0.1:7800/healthz
./target/release/morse sessions
./target/release/morse run "run echo setup-works && uname -a"
```

## Troubleshooting

- **Connection refused:** server not running, wrong bind address, or tunnel down.
- **401 unauthorized:** server has `MORSE_TOKEN`; pass `--token` or set `MORSE_TOKEN`.
- **Unknown session:** the session was evicted or never persisted; `morse sessions` lists what exists. Create a new session on the old workspace with `--workspace`.
- **Demo banner despite a key:** keys are server-side; restart the server after exporting them.
- **Build fails on Xcode license (macOS):** finish Apple's license prompt, then rebuild.
- **Command timeout:** default 120 s per bash call; ask for `timeout_ms` in the tool call or split the command.
