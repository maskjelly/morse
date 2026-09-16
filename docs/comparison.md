# Comparison with other agent CLIs

Morse is a small, self-hosted agent harness for running work on another computer and watching it from a terminal. This page is an honest map of where it stands in August 2026. Competitor facts come from their public docs and 2026 comparisons; verify them against the vendors before making decisions.

## Feature matrix

| | Morse | Claude Code | Codex CLI | OpenCode | Aider | Goose |
|---|---|---|---|---|---|---|
| License | MIT | Proprietary | Apache-2.0 | MIT | Apache-2.0 | Apache-2.0 |
| Language | Rust | TS | Rust | TS | Python | Rust |
| Model providers | Anthropic, any OpenAI-compatible, demo | Claude only | OpenAI only | 75+ | 50+ | 15–25+ |
| Live model streaming | Yes (SSE) | Yes | Yes | Yes | Yes | Yes |
| Plan / task checklist | Yes (`plan` tool) | Yes | Yes | Yes | Partial | Yes |
| Session resume | Yes, across **server restarts** | Yes | Yes | Yes | Yes | Yes |
| Headless / scriptable | `morse run`, REST + WS | `claude -p` | `codex exec --json` | Yes | `aider -m` | `goose run` |
| Project memory | `AGENTS.md` | `CLAUDE.md` | `AGENTS.md` | `AGENTS.md` | conventions | `.goosehints` |
| Remote execution | First-class (server/client split) | Via IDE/cloud | Cloud tasks | Local | Local | Local |
| Second agent for status | **Yes (`/ask` side agent)** | Subagents (not for status) | Subagents | Subagents | No | Subagents |
| No-key demo mode | Yes | No | No | No | No | No |
| Built-in auth | Bearer token | Service auth | ChatGPT auth | BYOK | BYOK | BYOK |
| MCP tools | No | Yes | Yes | Yes | No | Yes (70+) |
| OS sandbox | No | Permissions | Yes | No | No | No |
| Git auto-commit | No | No | No | No | Yes | No |
| Cost visibility | Token counts | Usage/quota | Usage | Model-dependent | Model-dependent | Model-dependent |
| Lines of code | ~3k Rust | Large | Large | Large | Large | Large |

## Where Morse is on par

- **Multi-provider**: Anthropic plus any OpenAI-compatible endpoint, including local Ollama/LM Studio, matching the provider-agnostic tools.
- **Streaming**: token-by-token model text (SSE) and live tool output; tool-call arguments are assembled from stream fragments.
- **Plan visibility**: the `plan` tool produces a live checklist that doubles as side-agent context, like the todo/plan tools elsewhere.
- **Resume**: sessions, task state, token counts, and conversation history are persisted under `MORSE_HOME` and reloaded after a restart, so you can reattach with `--session`.
- **Headless**: `morse run` (human or JSONL), a REST API for instruction/status/events, and a WebSocket protocol. No approval prompts, same as other headless modes.
- **Project instructions**: `AGENTS.md` (and `.morse/AGENTS.md`) is loaded into the system prompt.
- **Cost visibility**: per-response input/output token counts are streamed and shown in the client status bar.

## Where Morse is deliberately different

- **Server/client split.** Morse is a harness you deploy, not an app you install where you work. Client disconnects and server restarts do not lose work.
- **Side agent.** A second model call answers status questions from live state in a separate queue. Other tools have subagents, but they run tasks; none exposes a persistent "what is happening right now" pane that never blocks the main run.
- **Demo mode.** Real commands/files with a deterministic parser, so the full product can be tried and tested in CI without keys.
- **Small surface.** ~3k lines of Rust in three crates. Everything is readable in an afternoon; there are no plugins, hooks, or hidden daemons.

## Honest gaps

| Gap | Impact | Workaround today |
|---|---|---|
| No MCP client | Cannot plug into the MCP tool ecosystem | Use `bash` to call CLIs/HTTP APIs |
| No OS sandbox | Commands have full server-user access | Dedicated user, SSH tunnel/token, container/VM around the server |
| No subagents | Cannot parallelize model work | Run multiple sessions (each is independent) |
| No git automation | No auto-commit/undo | `bash` runs `git` yourself; file diffs stream in the client |
| Single-user | No tenants, quotas, or audit log | One server per user; `MORSE_TOKEN` as the only gate |
| No LSP | No semantic code intelligence | `grep`/`glob` tools and `bash` |

## Choosing

- **Use Claude Code / Codex CLI** for the deepest first-party experience, OS sandboxing, MCP, and hosted models. They are the polished mainstream choice.
- **Use Aider** for git-native, commit-per-change workflows on a local repo.
- **Use OpenCode / Goose** for local work across many providers with big ecosystems.
- **Use Morse** when work should run on another machine and you want to watch and question it from anywhere: a build box, a lab machine, a home server — with a runtime small enough to audit and modify.

## Roadmap candidates (in rough priority order)

1. Optional MCP client for tool extensibility.
2. Multi-client fan-out benchmark and per-session quotas.
3. Lightweight sandbox guidance (bubblewrap/sandbox-exec presets).
4. Side-agent actions (e.g. "answer with the last diff") beyond read-only status.
5. Git-aware helpers (`/diff`, checkpoint before run).

Priorities are driven by the server-centric use case, not by feature-list parity for its own sake.
