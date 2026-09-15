use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use crate::diff::unified_diff;
use crate::protocol::{ServerMsg, StreamKind, TaskStatus, TaskView};

const ACC_CAP: usize = 256 * 1024;
const READ_CAP: usize = 256 * 1024;
const BASH_DEFAULT_TIMEOUT_MS: u64 = 120_000;

pub struct ToolCtx<'a> {
    pub workspace: &'a Path,
    pub emit: &'a mut (dyn FnMut(ServerMsg) + Send + 'a),
    pub cancel: &'a CancellationToken,
}

#[derive(Debug, Clone, Default)]
pub struct ToolOutcome {
    pub output: String,
    pub ok: bool,
    pub truncated: bool,
    pub exit_code: Option<i32>,
}

impl ToolOutcome {
    fn err(msg: impl Into<String>) -> Self {
        Self {
            output: msg.into(),
            ok: false,
            ..Default::default()
        }
    }
}

pub fn summarize_input(tool: &str, input: &Value) -> String {
    match tool {
        "bash" => input
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .chars()
            .take(120)
            .collect(),
        "read_file" => input
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        "write_file" | "edit_file" => input
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        "list_files" => input
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or(".")
            .to_string(),
        "plan" => {
            let n = input
                .get("tasks")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            format!("{n} tasks")
        }
        _ => String::new(),
    }
}

fn safe_path(workspace: &Path, p: &str) -> Result<PathBuf> {
    let rel = Path::new(p.trim());
    let joined = if rel.is_absolute() {
        rel.to_path_buf()
    } else {
        workspace.join(rel)
    };
    for c in joined.components() {
        if matches!(c, Component::ParentDir) {
            bail!("path escapes the workspace: {p}");
        }
    }
    if !joined.starts_with(workspace) {
        bail!("path outside workspace: {p}");
    }
    Ok(joined)
}

fn str_field<'a>(input: &'a Value, key: &str) -> Result<&'a str> {
    input
        .get(key)
        .and_then(|v| v.as_str())
        .with_context(|| format!("missing field '{key}'"))
}

pub async fn exec_tool(name: &str, input: &Value, ctx: &mut ToolCtx<'_>) -> ToolOutcome {
    let out = match name {
        "bash" => bash(input, ctx).await,
        "read_file" => read_file(input, ctx.workspace).await,
        "write_file" => write_file(input, ctx).await,
        "edit_file" => edit_file(input, ctx).await,
        "list_files" => list_files(input, ctx.workspace).await,
        "plan" => plan(input, ctx),
        other => Ok(ToolOutcome {
            output: format!("unknown tool: {other}"),
            ok: false,
            ..Default::default()
        }),
    };
    out.unwrap_or_else(|e| ToolOutcome::err(format!("{e:#}")))
}

fn plan(input: &Value, ctx: &mut ToolCtx<'_>) -> Result<ToolOutcome> {
    let arr = input
        .get("tasks")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow!("plan requires a 'tasks' array"))?;
    let mut tasks = Vec::new();
    for (i, item) in arr.iter().enumerate() {
        let title = item
            .get("title")
            .and_then(|v| v.as_str())
            .with_context(|| format!("task {i} missing 'title'"))?;
        let status = item
            .get("status")
            .and_then(|v| v.as_str())
            .map(|s| match s {
                "completed" => TaskStatus::Completed,
                "in_progress" => TaskStatus::InProgress,
                _ => TaskStatus::Pending,
            })
            .unwrap_or(TaskStatus::Pending);
        tasks.push(TaskView {
            id: format!("t{}", i + 1),
            title: title.to_string(),
            status,
        });
    }
    let n = tasks.len();
    (ctx.emit)(ServerMsg::Plan { tasks });
    Ok(ToolOutcome {
        output: format!("plan updated ({n} tasks)"),
        ok: true,
        ..Default::default()
    })
}

async fn read_file(input: &Value, workspace: &Path) -> Result<ToolOutcome> {
    let p = str_field(input, "path")?;
    let full = safe_path(workspace, p)?;
    let bytes = std::fs::read(&full).with_context(|| format!("reading {}", full.display()))?;
    if bytes.len() > READ_CAP {
        let text = String::from_utf8_lossy(&bytes[..READ_CAP]).to_string();
        Ok(ToolOutcome {
            output: text,
            ok: true,
            truncated: true,
            ..Default::default()
        })
    } else {
        Ok(ToolOutcome {
            output: String::from_utf8_lossy(&bytes).to_string(),
            ok: true,
            ..Default::default()
        })
    }
}

async fn write_file(input: &Value, ctx: &mut ToolCtx<'_>) -> Result<ToolOutcome> {
    let p = str_field(input, "path")?;
    let content = str_field(input, "content")?;
    let full = safe_path(ctx.workspace, p)?;
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating dir {}", parent.display()))?;
    }
    let old = std::fs::read_to_string(&full).ok();
    std::fs::write(&full, content).with_context(|| format!("writing {}", full.display()))?;
    let diff = unified_diff(p, old.as_deref(), content);
    (ctx.emit)(ServerMsg::FileEdit {
        call_id: String::new(),
        path: p.to_string(),
        diff,
    });
    Ok(ToolOutcome {
        output: format!("wrote {} bytes to {p}", content.len()),
        ok: true,
        ..Default::default()
    })
}

async fn edit_file(input: &Value, ctx: &mut ToolCtx<'_>) -> Result<ToolOutcome> {
    let p = str_field(input, "path")?;
    let old_str = str_field(input, "old_string")?;
    let new_str = str_field(input, "new_string")?;
    if old_str == new_str {
        bail!("old_string and new_string are identical");
    }
    let full = safe_path(ctx.workspace, p)?;
    let old_content =
        std::fs::read_to_string(&full).with_context(|| format!("reading {}", full.display()))?;
    let count = old_content.matches(old_str).count();
    if count == 0 {
        bail!("old_string not found in {p}");
    }
    if count > 1 {
        bail!("old_string occurs {count} times in {p}; provide more context");
    }
    let new_content = old_content.replacen(old_str, new_str, 1);
    std::fs::write(&full, &new_content).with_context(|| format!("writing {}", full.display()))?;
    let diff = unified_diff(p, Some(&old_content), &new_content);
    (ctx.emit)(ServerMsg::FileEdit {
        call_id: String::new(),
        path: p.to_string(),
        diff,
    });
    Ok(ToolOutcome {
        output: format!("edited {p}"),
        ok: true,
        ..Default::default()
    })
}

async fn list_files(input: &Value, workspace: &Path) -> Result<ToolOutcome> {
    let rel = input
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or(".")
        .to_string();
    let max = input.get("max").and_then(|v| v.as_u64()).unwrap_or(200) as usize;
    let root = safe_path(workspace, &rel)?;
    let mut out = String::new();
    let mut stack = vec![root.clone()];
    let mut shown = 0usize;
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        let mut items: Vec<_> = entries.flatten().collect();
        items.sort_by_key(|e| e.file_name());
        for entry in items {
            if shown >= max {
                out.push_str("... (truncated)\n");
                return Ok(ToolOutcome {
                    output: out,
                    ok: true,
                    truncated: true,
                    ..Default::default()
                });
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || name == "target" || name == "node_modules" {
                continue;
            }
            let path = entry.path();
            let rel_path = path
                .strip_prefix(workspace)
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| path.display().to_string());
            if path.is_dir() {
                out.push_str(&format!("{rel_path}/\n"));
                stack.push(path);
            } else {
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                out.push_str(&format!("{rel_path} ({size} bytes)\n"));
            }
            shown += 1;
        }
    }
    Ok(ToolOutcome {
        output: if out.is_empty() {
            "(empty)".to_string()
        } else {
            out
        },
        ok: true,
        ..Default::default()
    })
}

async fn bash(input: &Value, ctx: &mut ToolCtx<'_>) -> Result<ToolOutcome> {
    let cmd = str_field(input, "command")?;
    if cmd.trim().is_empty() {
        bail!("empty command");
    }
    let timeout_ms = input
        .get("timeout_ms")
        .and_then(|v| v.as_u64())
        .unwrap_or(BASH_DEFAULT_TIMEOUT_MS);
    let mut command = Command::new("bash");
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command
        .arg("-c")
        .arg(cmd)
        .current_dir(ctx.workspace)
        .env("MORSE_SESSION", "1")
        .env_remove("MORSE_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("spawning bash for: {cmd}"))?;

    #[cfg(unix)]
    let _process_group = ProcessGroup(child.id().expect("spawned child") as i32);
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::channel::<(StreamKind, String)>(64);
    let h_out = tokio::spawn(read_pipe(stdout, StreamKind::Stdout, tx.clone()));
    let h_err = tokio::spawn(read_pipe(stderr, StreamKind::Stderr, tx));

    let mut acc = String::new();
    let mut truncated = false;
    let mut interrupted = false;
    let mut timed_out = false;
    let mut final_status: Option<std::process::ExitStatus> = None;

    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
    let mut pipes_open = true;
    loop {
        tokio::select! {
            biased;
            _ = ctx.cancel.cancelled() => {
                interrupted = true;
                let _ = child.start_kill();
                break;
            }
            _ = tokio::time::sleep_until(deadline) => {
                timed_out = true;
                let _ = child.start_kill();
                break;
            }
            chunk = rx.recv(), if pipes_open => match chunk {
                Some((kind, chunk)) => {
                    (ctx.emit)(ServerMsg::Output {
                        call_id: String::new(), stream: kind, chunk: chunk.clone(),
                    });
                    let remaining = ACC_CAP.saturating_sub(acc.len());
                    if chunk.len() > remaining { truncated = true; }
                    let mut end = chunk.len().min(remaining);
                    while !chunk.is_char_boundary(end) { end -= 1; }
                    acc.push_str(&chunk[..end]);
                }
                None => pipes_open = false,
            },
            status = child.wait(), if final_status.is_none() => {
                final_status = Some(status.context("bash wait failed")?);
            }
        }
        if final_status.is_some() && !pipes_open {
            break;
        }
    }
    // Readers must never outlive an interrupted/timed-out command indefinitely.
    h_out.abort();
    h_err.abort();
    let _ = h_out.await;
    let _ = h_err.await;
    if final_status.is_none() {
        final_status = child.wait().await.ok();
    }

    let exit_code = final_status.and_then(|s| s.code());
    let mut output = String::new();
    match (exit_code, interrupted, timed_out) {
        (Some(0), false, false) => output.push_str("exit 0\n"),
        (Some(c), false, false) => output.push_str(&format!("exit {c}\n")),
        (None, _, _) => output.push_str("no exit code (killed)\n"),
        (_, _, _) => {}
    }
    if interrupted {
        output.push_str("note: interrupted by user\n");
    }
    if timed_out {
        output.push_str(&format!("note: timed out after {timeout_ms}ms\n"));
    }
    output.push_str(&acc);
    Ok(ToolOutcome {
        output: crate::diff::cap(&output, ACC_CAP),
        ok: exit_code == Some(0) && !interrupted && !timed_out,
        truncated,
        exit_code,
    })
}

// Owned process group: dropping a cancelled tool also stops shell descendants.
#[cfg(unix)]
struct ProcessGroup(i32);
#[cfg(unix)]
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        // SAFETY: kill takes an integer process group ID; it dereferences no memory.
        unsafe {
            libc::kill(-self.0, libc::SIGKILL);
        }
    }
}

async fn read_pipe<R: AsyncRead + Unpin>(
    mut pipe: R,
    kind: StreamKind,
    tx: tokio::sync::mpsc::Sender<(StreamKind, String)>,
) {
    let mut buf = [0u8; 8192];
    loop {
        match pipe.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let chunk = String::from_utf8_lossy(&buf[..n]).to_string();
                if tx.send((kind, chunk)).await.is_err() {
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::now_ms;

    fn tmp_ws(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("morse-test-{}-{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn bash_streams_output() {
        let ws = tmp_ws("bash");
        let cancel = CancellationToken::new();
        let mut events = Vec::new();
        let mut emit = |m: ServerMsg| events.push(m);
        let mut ctx = ToolCtx {
            workspace: &ws,
            emit: &mut emit,
            cancel: &cancel,
        };
        let out = exec_tool(
            "bash",
            &serde_json::json!({"command": "echo hello; echo oops 1>&2"}),
            &mut ctx,
        )
        .await;
        assert!(out.ok, "output: {}", out.output);
        assert_eq!(out.exit_code, Some(0));
        assert!(out.output.contains("hello"));
        assert!(events
            .iter()
            .any(|m| matches!(m, ServerMsg::Output { chunk, .. } if chunk.contains("hello"))));
        assert!(events.iter().any(|m| matches!(
            m,
            ServerMsg::Output {
                stream: StreamKind::Stderr,
                ..
            }
        )));
        let _ = std::fs::remove_dir_all(&ws);
        assert!(now_ms() > 0);
    }

    #[tokio::test]
    async fn write_and_edit_files() {
        let ws = tmp_ws("files");
        let cancel = CancellationToken::new();
        let mut events = Vec::new();
        let mut emit = |m: ServerMsg| events.push(m);
        let mut ctx = ToolCtx {
            workspace: &ws,
            emit: &mut emit,
            cancel: &cancel,
        };

        let out = exec_tool(
            "write_file",
            &serde_json::json!({"path": "notes.txt", "content": "alpha\nbeta\n"}),
            &mut ctx,
        )
        .await;
        assert!(out.ok, "{:?}", out.output);
        assert!(std::fs::read_to_string(ws.join("notes.txt"))
            .unwrap()
            .contains("beta"));

        let out = exec_tool(
            "edit_file",
            &serde_json::json!({"path": "notes.txt", "old_string": "beta", "new_string": "gamma"}),
            &mut ctx,
        )
        .await;
        assert!(out.ok, "{:?}", out.output);
        let new = std::fs::read_to_string(ws.join("notes.txt")).unwrap();
        assert!(new.contains("gamma"));

        let edit_events: Vec<&ServerMsg> = events
            .iter()
            .filter(|m| matches!(m, ServerMsg::FileEdit { .. }))
            .collect();
        assert_eq!(edit_events.len(), 2);
        let last = edit_events[1];
        match last {
            ServerMsg::FileEdit { diff, .. } => {
                assert!(diff.contains("-beta"), "{diff}");
                assert!(diff.contains("+gamma"), "{diff}");
            }
            _ => panic!(),
        }
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn safe_path_rejects_escape() {
        let ws = Path::new("/tmp/ws");
        assert!(safe_path(ws, "../etc").is_err());
        assert!(safe_path(ws, "/etc/passwd").is_err());
        assert_eq!(
            safe_path(ws, "sub/file.txt").unwrap(),
            PathBuf::from("/tmp/ws/sub/file.txt")
        );
    }

    #[tokio::test]
    async fn missing_field_is_tool_error() {
        let ws = tmp_ws("err");
        let cancel = CancellationToken::new();
        let mut emit = |_: ServerMsg| {};
        let mut ctx = ToolCtx {
            workspace: &ws,
            emit: &mut emit,
            cancel: &cancel,
        };
        let out = exec_tool("bash", &serde_json::json!({}), &mut ctx).await;
        assert!(!out.ok);
        assert!(out.output.contains("missing field"));
        let _ = std::fs::remove_dir_all(&ws);
    }
}
