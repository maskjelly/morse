use morse_core::protocol::{ServerMsg, StreamKind, TaskStatus};

#[derive(Clone)]
pub struct Style {
    pub prefix: &'static str,
    pub prefix_color: Color,
    pub body_color: Color,
    pub dim: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Color {
    Cyan,
    Magenta,
    Yellow,
    Green,
    Red,
    Blue,
    DarkGray,
    White,
}

impl Color {
    pub fn ansi(self) -> &'static str {
        match self {
            Color::Cyan => "\x1b[36m",
            Color::Magenta => "\x1b[35m",
            Color::Yellow => "\x1b[33m",
            Color::Green => "\x1b[32m",
            Color::Red => "\x1b[31m",
            Color::Blue => "\x1b[34m",
            Color::DarkGray => "\x1b[90m",
            Color::White => "\x1b[97m",
        }
    }

    pub fn ratatui(self) -> ratatui::style::Color {
        use ratatui::style::Color as C;
        match self {
            Color::Cyan => C::Cyan,
            Color::Magenta => C::Magenta,
            Color::Yellow => C::Yellow,
            Color::Green => C::Green,
            Color::Red => C::Red,
            Color::Blue => C::Blue,
            Color::DarkGray => C::DarkGray,
            Color::White => C::White,
        }
    }
}

pub enum Render {
    Main(Style, String),
    Side(Style, String),
}

fn ok_mark(ok: bool) -> (&'static str, Color) {
    if ok {
        ("✓", Color::Green)
    } else {
        ("✗", Color::Red)
    }
}

pub fn render(msg: &ServerMsg) -> Vec<Render> {
    let mut out = Vec::new();
    match msg {
        ServerMsg::Hello { .. } => {}
        ServerMsg::Instruction { text } => out.push(Render::Main(
            Style {
                prefix: "you ▸",
                prefix_color: Color::Cyan,
                body_color: Color::White,
                dim: false,
            },
            text.clone(),
        )),
        ServerMsg::AgentText { text } => out.push(Render::Main(
            Style {
                prefix: "morse ▸",
                prefix_color: Color::Magenta,
                body_color: Color::White,
                dim: false,
            },
            text.clone(),
        )),
        ServerMsg::ToolCall { id, tool, input } => {
            let sum = morse_core::tools::summarize_input(tool, input);
            let (prefix, color) = match tool.as_str() {
                "bash" => ("  $", Color::Yellow),
                "plan" => ("  plan", Color::Blue),
                _ => ("  tool", Color::Yellow),
            };
            out.push(Render::Main(
                Style {
                    prefix,
                    prefix_color: color,
                    body_color: Color::White,
                    dim: false,
                },
                sum,
            ));
            let _ = id;
        }
        ServerMsg::Output { stream, chunk, .. } => {
            let color = match stream {
                StreamKind::Stdout => Color::DarkGray,
                StreamKind::Stderr => Color::Red,
            };
            out.push(Render::Main(
                Style {
                    prefix: "  │",
                    prefix_color: color,
                    body_color: color,
                    dim: true,
                },
                chunk.trim_end_matches('\n').to_string(),
            ));
        }
        ServerMsg::ToolResult {
            ok,
            exit_code,
            duration_ms,
            summary,
            ..
        } => {
            let (mark, color) = ok_mark(*ok);
            let exit = match exit_code {
                Some(c) => format!("exit {c}"),
                None => "no exit".to_string(),
            };
            let secs = *duration_ms as f64 / 1000.0;
            out.push(Render::Main(
                Style {
                    prefix: "  ",
                    prefix_color: color,
                    body_color: Color::White,
                    dim: false,
                },
                format!("{mark} {exit} ({secs:.1}s) {summary}"),
            ));
        }
        ServerMsg::FileEdit { path, diff, .. } => {
            out.push(Render::Main(
                Style {
                    prefix: "  ✎",
                    prefix_color: Color::Blue,
                    body_color: Color::White,
                    dim: false,
                },
                path.clone(),
            ));
            for line in diff.lines().skip(2) {
                let (color, body) = if line.starts_with('+') {
                    (Color::Green, line.to_string())
                } else if line.starts_with('-') {
                    (Color::Red, line.to_string())
                } else {
                    (Color::DarkGray, line.to_string())
                };
                out.push(Render::Main(
                    Style {
                        prefix: "  ",
                        prefix_color: color,
                        body_color: color,
                        dim: true,
                    },
                    body,
                ));
            }
        }
        ServerMsg::Plan { tasks } => {
            out.push(Render::Main(
                Style {
                    prefix: "plan",
                    prefix_color: Color::Blue,
                    body_color: Color::White,
                    dim: false,
                },
                format!("{} tasks", tasks.len()),
            ));
            for t in tasks {
                let (mark, color) = match t.status {
                    TaskStatus::Completed => ("✓", Color::Green),
                    TaskStatus::InProgress => ("▸", Color::Yellow),
                    TaskStatus::Pending => ("·", Color::DarkGray),
                };
                out.push(Render::Main(
                    Style {
                        prefix: "  ",
                        prefix_color: color,
                        body_color: Color::White,
                        dim: false,
                    },
                    format!("{mark} {}", t.title),
                ));
            }
        }
        ServerMsg::Side { question, answer } => {
            out.push(Render::Side(
                Style {
                    prefix: "you ▸",
                    prefix_color: Color::Cyan,
                    body_color: Color::White,
                    dim: false,
                },
                question.clone(),
            ));
            out.push(Render::Side(
                Style {
                    prefix: "side ▸",
                    prefix_color: Color::Magenta,
                    body_color: Color::White,
                    dim: false,
                },
                answer.clone(),
            ));
        }
        ServerMsg::Status { .. } => {}
        ServerMsg::Error { message } => out.push(Render::Main(
            Style {
                prefix: "error ▸",
                prefix_color: Color::Red,
                body_color: Color::Red,
                dim: false,
            },
            message.clone(),
        )),
        ServerMsg::Pong => {}
    }
    out
}
