# Run Morse on a VPS or microVM

For a single owner running agents on Linux. The guest needs Bash, outbound access to the provider, and enough RAM for your builds. A microVM is a normal Linux host here; create it with your infrastructure provider, then use these same steps inside the guest.

## Fastest: Docker Compose

Install Docker with Compose on your server. From a checkout:

```sh
bash deploy/init-env.sh
# Optional: edit .env to configure your provider or agent CLI.
docker compose up --build -d
docker compose ps
docker compose logs -f morse
```

The port is published only to the host's loopback. State and projects use named volumes. The default demo runs real tools without a model key. Set a provider in `.env`, then `docker compose up -d` to recreate the service with that environment.

The image includes Bash, Git and curl. Add your project's compiler/runtime or agent CLI in a derived image. Local model servers must be reachable from inside the container; `127.0.0.1` there is the container itself. Use a private Compose service hostname for the model endpoint.

## Native Linux service

Build as your normal user, then install the binary and service:

```sh
cargo build --release --locked --bin morse
sudo bash deploy/install-systemd.sh target/release/morse
sudoedit /etc/morse.env
sudo systemctl restart morse
sudo journalctl -u morse -f
```

The installer creates a dedicated `morse` account, a random bearer token, private state, and a loopback-only systemd service. It preserves an existing environment file. Provider keys belong in `/etc/morse.env` (root-owned, mode 600). Install external agents and authenticate them as the `morse` account; its home is `/home/morse`, not your SSH user's home. The service's PATH is minimal: use an absolute agent executable path.

## Connect from your laptop

```sh
ssh -N -L 7800:127.0.0.1:7800 user@your-vps
# In another terminal, set MORSE_TOKEN to the server token via a private channel.
export MORSE_TOKEN=your-server-token
morse connect ws://127.0.0.1:7800/ws
morse run --json "run uname -a" ws://127.0.0.1:7800/ws
morse sessions ws://127.0.0.1:7800/ws
```

A remote workspace path belongs to the server. For Compose use `/home/morse/projects/my-app`; for the native service use `/home/morse/projects/my-app`. `morse connect --workspace <path>` attaches a new session to it. Save the session ID and use `--session <id>` to reconnect.

## Use an existing agent

Model providers run Morse's own tool loop. The `command` provider runs your installed, noninteractive agent instead, with streamed stdout/stderr, cancellation, timeouts, exit status and replay:

```dotenv
MORSE_PROVIDER=command
MORSE_AGENT_PROGRAM=/home/morse/bin/my-agent
MORSE_AGENT_ARGS=["--prompt", "{prompt}", "--session", "{session_id}"]
MORSE_AGENT_TIMEOUT_MS=1800000
```

Arguments are a JSON array, not shell syntax. `{prompt}` becomes one literal argument, including quotes and newlines. `{session_id}` is stable for that Morse session. Your wrapper chooses the agent's own resume behavior and output format. The child runs in the session workspace and inherits its agent credentials; Morse's access token is removed. `/ask` answers from observed process state without starting a second CLI process.

Any agent that accepts a prompt and exits can be adapted. If yours expects stdin, install this executable wrapper as `/home/morse/bin/my-agent` and configure `MORSE_AGENT_ARGS=["{prompt}"]`:

```sh
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$1" | /absolute/path/to/your-agent --noninteractive
```

Use the flags supported by your installed agent. Morse records the process output; it does not translate arbitrary agent-specific events into its native file diff or token accounting protocol. Each invocation is fresh unless your wrapper explicitly resumes it. An interactive TUI needs its own headless mode first.

## Verify, upgrade, recover

```sh
curl -fsS http://127.0.0.1:7800/healthz
# Use the same token as the client:
curl -fsS -H "Authorization: Bearer $MORSE_TOKEN" http://127.0.0.1:7800/api/sessions
morse run "run printf deployment-ok" ws://127.0.0.1:7800/ws
```

Disconnect during a long command, reconnect with `--session`, and verify its output. Before an upgrade, wait for active commands to finish and stop the service. Back up all of `MORSE_HOME` and your project directories; restore them together with the matching configuration. Native upgrades: keep the previous binary, install the new one, restart and check health; restore the previous binary if startup fails. Compose upgrades: retain the previous image ID and volumes; rebuild and verify, or point Compose at the previous image. Never use `docker compose down -v` for an upgrade: it deletes state volumes.

Sessions and replay survive a server restart; running processes do not resume. Inspect the workspace before resubmitting work whose outcome is uncertain. Queued instructions are memory-only. Keep the service private and use one owner per host/guest. Native commands have the account's permissions; containers share a kernel. For hostile workloads use separate disposable microVMs with restricted credentials and network policy.

## Verify the container setup

```sh
docker build -t morse:local .
python3 deploy/container-smoke.py morse:local
```

This offline smoke uses the actual Compose configuration with an isolated project, fresh volumes and a free loopback port. It verifies the nonroot account, external-agent argument quoting, authentication, and persistence after restart, then removes only its test containers and volumes. It preserves any existing `.env`. CI runs the same check on Linux.
