# Operations

For running one trusted user's Morse server on macOS or Linux.

## Configuration

| Variable | Purpose |
|---|---|
| `MORSE_HOME` | Workspace root; defaults to `~/.morse` |
| `MORSE_API_KEY` | Anthropic credential, preferred over `ANTHROPIC_API_KEY` |
| `ANTHROPIC_API_KEY` | Anthropic credential fallback |
| `MORSE_MODEL` | Model ID, default `claude-sonnet-4-5` |
| `MORSE_URL` | Client URL, default `ws://127.0.0.1:7800/ws` |
| `MORSE_BIND` | Bind address for the standalone `morse-server` binary |
| `RUST_LOG` | Log filter |

For `morse serve`, use `--bind`; that command does not read `MORSE_BIND`.

## Build and run

```sh
cargo build --release --locked
./target/release/morse serve --bind 127.0.0.1:7800
```

Use an SSH tunnel for remote access, as shown in the README. Credentials belong in the server environment, never in git or a client URL. Run the server as an account with access only to the work it needs.

## Verify

```sh
curl --fail http://127.0.0.1:7800/healthz
./target/release/morse sessions
./target/release/morse connect --plain
```

Enter `run echo early && sleep 3 && echo late`. Confirm `early` appears while work is active. During the pause, enter `/ask what is running?`. Confirm a side answer arrives. Save the session ID, disconnect, then use `morse connect --session <id>` to inspect replay.

## Troubleshooting

- **Build fails on the Xcode license:** complete Apple's Xcode setup/license prompt, then rebuild.
- **Connection refused:** start the server; check the bind address and SSH tunnel.
- **Unknown session:** verify the ID and whether the server restarted. Files remain, but old session state cannot be restored. Create a new session with `--workspace` pointing at the old directory.
- **Demo banner despite expecting a model:** check the server environment, then restart. The client environment does not configure the provider.
- **Provider error:** inspect the reported response; verify the key, model availability, and account balance.
- **Command timeout:** ask for a shorter command or specify `timeout_ms` in a direct Bash tool request through the provider.

## Stop and recover

Use `/interrupt` before stopping if work is active. Ctrl+C stops the server. Restarting clears session history and task state; retain workspace files separately if needed. To roll back an update, rebuild the previous git revision and restart it. There are no database migrations.
