use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style as RStyle};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;
use tokio::sync::mpsc;

use morse_core::protocol::{ClientMsg, Envelope, ServerMsg, StatusKind};

use crate::client::{provider_banner, ClientConfig};
use crate::render::{render, Color, Render, Style};

#[derive(Clone, Copy, PartialEq, Eq)]
enum PaneFocus {
    Main,
    Side,
}

struct Item {
    prefix: String,
    prefix_color: Color,
    body: String,
    body_color: Color,
    streaming: bool,
}

impl Item {
    fn from(style: &Style, body: String) -> Self {
        Self {
            prefix: style.prefix.to_string(),
            prefix_color: style.prefix_color,
            body,
            body_color: style.body_color,
            streaming: false,
        }
    }

    fn streaming(style: &Style, body: String) -> Self {
        Self {
            streaming: true,
            ..Self::from(style, body)
        }
    }
}

struct Pane {
    items: Vec<Item>,
    wrapped: Vec<Line<'static>>,
    wrap_width: usize,
    scroll: usize,
    follow: bool,
    last_len: usize,
}

const MAX_LINES: usize = 8000;

impl Pane {
    fn new() -> Self {
        Self {
            items: Vec::new(),
            wrapped: Vec::new(),
            wrap_width: 0,
            scroll: 0,
            follow: true,
            last_len: 0,
        }
    }

    fn push(&mut self, item: Item) {
        if self.wrap_width > 0 {
            self.last_len = wrap_item(&item, self.wrap_width).len();
            let mut lines = wrap_item(&item, self.wrap_width);
            self.wrapped.append(&mut lines);
            if self.wrapped.len() > MAX_LINES {
                let cut = self.wrapped.len() - MAX_LINES;
                self.wrapped.drain(0..cut);
            }
        }
        self.items.push(item);
        if self.items.len() > MAX_LINES {
            let cut = self.items.len() - MAX_LINES;
            self.items.drain(0..cut);
        }
        if self.follow {
            self.scroll = 0;
        }
    }

    /// Append a streaming fragment to the previous agent item, or start one.
    fn append(&mut self, item: Item) {
        if let Some(last) = self.items.last_mut() {
            if last.streaming && last.prefix == item.prefix {
                last.body.push_str(&item.body);
                if self.wrap_width > 0 {
                    let lines = wrap_item(last, self.wrap_width);
                    self.wrapped
                        .truncate(self.wrapped.len().saturating_sub(self.last_len));
                    self.last_len = lines.len();
                    self.wrapped.extend(lines);
                    if self.wrapped.len() > MAX_LINES {
                        let cut = self.wrapped.len() - MAX_LINES;
                        self.wrapped.drain(0..cut);
                    }
                }
                if self.follow {
                    self.scroll = 0;
                }
                return;
            }
        }
        self.push(Item::streaming(
            &Style {
                prefix: "",
                prefix_color: item.prefix_color,
                body_color: item.body_color,
                dim: false,
            },
            String::new(),
        ));
        // Replace the placeholder with the real item so styling is exact.
        self.items.pop();
        self.push(item);
    }

    fn set_width(&mut self, w: usize) {
        if self.wrap_width == w {
            return;
        }
        self.wrap_width = w;
        self.wrapped = self.items.iter().flat_map(|i| wrap_item(i, w)).collect();
        self.last_len = self
            .items
            .last()
            .map(|i| wrap_item(i, w).len())
            .unwrap_or(0);
    }

    fn page_up(&mut self) {
        self.follow = false;
        self.scroll += 1;
    }

    fn page_down(&mut self) {
        self.scroll = self.scroll.saturating_sub(1);
        if self.scroll == 0 {
            self.follow = true;
        }
    }
}

fn wrap_item(item: &Item, width: usize) -> Vec<Line<'static>> {
    let prefix = &item.prefix;
    let first_avail = width.saturating_sub(prefix.chars().count() + 1).max(8);
    let cont_avail = width.saturating_sub(2).max(4);
    let style = RStyle::default().fg(item.body_color.ratatui());
    let pstyle = RStyle::default().fg(item.prefix_color.ratatui());
    let mut lines: Vec<Line<'static>> = Vec::new();

    for (li, raw) in item.body.lines().enumerate() {
        let avail = if li == 0 { first_avail } else { cont_avail };
        if raw.chars().count() <= avail {
            let spans = if li == 0 {
                vec![
                    Span::styled(prefix.clone(), pstyle),
                    Span::raw(" "),
                    Span::styled(raw.to_string(), style),
                ]
            } else {
                vec![Span::styled(format!("  {raw}"), style)]
            };
            lines.push(Line::from(spans));
            continue;
        }
        let mut cur: Vec<Span<'static>> = Vec::new();
        let mut cur_len = 0usize;
        if li == 0 {
            cur.push(Span::styled(prefix.clone(), pstyle));
            cur.push(Span::raw(" "));
            cur_len = prefix.chars().count() + 1;
        }
        for word in raw.split(' ') {
            let wlen = word.chars().count();
            if wlen >= width {
                for ch in word.chars() {
                    if cur_len >= width {
                        lines.push(Line::from(std::mem::take(&mut cur)));
                        cur = vec![Span::raw("  ")];
                        cur_len = 2;
                    }
                    cur.push(Span::styled(ch.to_string(), style));
                    cur_len += 1;
                }
                continue;
            }
            if cur_len + wlen + 1 > width && cur_len > 0 {
                lines.push(Line::from(std::mem::take(&mut cur)));
                cur = vec![Span::raw("  ")];
                cur_len = 2;
            }
            cur.push(Span::styled(word.to_string(), style));
            cur_len += wlen;
            cur.push(Span::raw(" "));
            cur_len += 1;
        }
        if !cur.is_empty() {
            lines.push(Line::from(cur));
        }
    }
    if lines.is_empty() {
        lines.push(Line::from(vec![
            Span::styled(prefix.clone(), pstyle),
            Span::raw(" "),
        ]));
    }
    lines
}

pub struct App {
    cfg: ClientConfig,
    cmds: mpsc::UnboundedSender<ClientMsg>,
    events: mpsc::Receiver<Envelope>,
    main: Pane,
    side: Pane,
    input: String,
    cursor: usize,
    focus: PaneFocus,
    quit: bool,
    banner: String,
    status: StatusKind,
    session: Option<String>,
    demo: bool,
    provider: String,
    input_tokens: u64,
    output_tokens: u64,
}

impl App {
    fn new(cfg: ClientConfig) -> Self {
        let handle = crate::client::spawn(cfg.clone());
        Self {
            cfg,
            cmds: handle.cmds,
            events: handle.events,
            main: Pane::new(),
            side: Pane::new(),
            input: String::new(),
            cursor: 0,
            focus: PaneFocus::Main,
            quit: false,
            banner: String::new(),
            status: StatusKind::Idle,
            session: None,
            demo: false,
            provider: String::new(),
            input_tokens: 0,
            output_tokens: 0,
        }
    }

    fn send(&self, msg: ClientMsg) {
        let _ = self.cmds.send(msg);
    }

    fn submit(&mut self) {
        let text = self.input.trim().to_string();
        self.input.clear();
        self.cursor = 0;
        if text.is_empty() {
            return;
        }
        if text == "/help" || text == "/?" {
            self.main.push(Item::from(
                &Style {
                    prefix: "help ▸",
                    prefix_color: Color::Blue,
                    body_color: Color::White,
                    dim: false,
                },
                "commands:\n/ask <question> — ask the side agent about progress\n/interrupt — cancel the current run\n/quit — disconnect (work keeps running on the server)\nkeys: tab switches panes, pgup/pgdn scroll, ctrl-c quits".to_string(),
            ));
        } else if let Some(q) = text.strip_prefix("/ask ") {
            self.send(ClientMsg::SideQuery {
                text: q.to_string(),
            });
        } else if text == "/interrupt" {
            self.send(ClientMsg::Interrupt);
        } else if text == "/quit" || text == "/q" {
            self.quit = true;
        } else {
            self.send(ClientMsg::Instruction { text });
        }
    }

    fn pump_events(&mut self) {
        while let Ok(env) = self.events.try_recv() {
            match &env.inner {
                ServerMsg::Hello {
                    session_id,
                    provider,
                    demo,
                    ..
                } => {
                    self.banner = provider_banner(&env.inner);
                    self.session = Some(session_id.clone());
                    self.provider = provider.clone();
                    self.demo = *demo;
                }
                ServerMsg::Status { status, .. } => {
                    self.status = *status;
                }
                ServerMsg::Usage {
                    input_tokens,
                    output_tokens,
                } => {
                    self.input_tokens += input_tokens;
                    self.output_tokens += output_tokens;
                }
                _ => {}
            }
            for r in render(&env.inner) {
                match r {
                    Render::Main(style, body) => self.main.push(Item::from(&style, body)),
                    Render::MainAppend(style, body) => {
                        self.main.append(Item::streaming(&style, body))
                    }
                    Render::Side(style, body) => self.side.push(Item::from(&style, body)),
                }
            }
        }
    }

    fn handle_key(&mut self, key: crossterm::event::KeyEvent) {
        use crossterm::event::{KeyCode, KeyModifiers};
        if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c')) {
            self.quit = true;
            return;
        }
        match key.code {
            KeyCode::Enter => self.submit(),
            KeyCode::Backspace => {
                if self.cursor > 0 && self.cursor <= self.input.chars().count() {
                    let byte_idx = byte_index(&self.input, self.cursor - 1);
                    self.input.remove(byte_idx);
                    self.cursor -= 1;
                }
            }
            KeyCode::Delete => {
                if self.cursor < self.input.chars().count() {
                    let byte_idx = byte_index(&self.input, self.cursor);
                    self.input.remove(byte_idx);
                }
            }
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => {
                if self.cursor < self.input.chars().count() {
                    self.cursor += 1;
                }
            }
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.input.chars().count(),
            KeyCode::Esc => {
                self.input.clear();
                self.cursor = 0;
            }
            KeyCode::Tab => {
                self.focus = match self.focus {
                    PaneFocus::Main => PaneFocus::Side,
                    PaneFocus::Side => PaneFocus::Main,
                };
            }
            KeyCode::PageUp => match self.focus {
                PaneFocus::Main => self.main.page_up(),
                PaneFocus::Side => self.side.page_up(),
            },
            KeyCode::PageDown => match self.focus {
                PaneFocus::Main => self.main.page_down(),
                PaneFocus::Side => self.side.page_down(),
            },
            KeyCode::Char(c)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                let byte_idx = byte_index(&self.input, self.cursor);
                self.input.insert(byte_idx, c);
                self.cursor += 1;
            }
            _ => {}
        }
    }
}

fn byte_index(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(b, _)| b)
        .unwrap_or(s.len())
}

pub async fn run_tui(cfg: ClientConfig) -> anyhow::Result<()> {
    let mut terminal = ratatui::init();
    let result = drive(&mut terminal, cfg).await;
    ratatui::restore();
    result
}

async fn drive(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    cfg: ClientConfig,
) -> anyhow::Result<()> {
    let mut app = App::new(cfg);
    loop {
        app.pump_events();
        terminal.draw(|f| draw(f, &mut app))?;
        if crossterm::event::poll(std::time::Duration::from_millis(50))? {
            if let crossterm::event::Event::Key(key) = crossterm::event::read()? {
                app.handle_key(key);
            }
        }
        if app.quit {
            break;
        }
    }
    Ok(())
}

fn draw(f: &mut Frame, app: &mut App) {
    let root = f.area();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),
            Constraint::Length(1),
            Constraint::Length(3),
        ])
        .split(root);

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(62), Constraint::Percentage(38)])
        .split(rows[0]);

    let focus = app.focus;
    draw_pane(
        f,
        &mut app.main,
        cols[0],
        "stream",
        focus == PaneFocus::Main,
    );
    draw_pane(
        f,
        &mut app.side,
        cols[1],
        "side agent (/ask …)",
        focus == PaneFocus::Side,
    );

    let status_line = build_status(app);
    f.render_widget(Paragraph::new(status_line), rows[1]);

    let input_block = Block::default().borders(Borders::TOP).title(
        " input — enter send • /ask side agent • /interrupt • tab panes • pgup/pgdn scroll ",
    );
    let input_area = input_block.inner(rows[2]);
    f.render_widget(input_block, rows[2]);
    let max = input_area.width.saturating_sub(3) as usize;
    let total_chars = app.input.chars().count();
    let visible: String = if total_chars > max {
        app.input.chars().skip(total_chars - max).collect()
    } else {
        app.input.clone()
    };
    f.render_widget(Paragraph::new(format!("> {visible}")), input_area);
    let cursor_x = input_area.x + 2 + total_chars.min(max) as u16;
    f.set_cursor_position((cursor_x, input_area.y));
}

fn build_status(app: &App) -> Line<'static> {
    let (dot, dot_color) = match app.status {
        StatusKind::Working => ("●", Color::Green),
        StatusKind::Interrupted => ("●", Color::Yellow),
        StatusKind::Idle => ("○", Color::DarkGray),
    };
    let mode = if app.demo {
        "demo mode".to_string()
    } else {
        app.provider.clone()
    };
    let tokens = if app.input_tokens + app.output_tokens > 0 {
        format!(" • ↑{} ↓{}", app.input_tokens, app.output_tokens)
    } else {
        String::new()
    };
    Line::from(vec![
        Span::styled(dot, RStyle::default().fg(dot_color.ratatui())),
        Span::raw(format!(
            " {mode} • {}{tokens} • ",
            app.session.as_deref().unwrap_or("connecting…")
        )),
        Span::styled(
            app.cfg.url.clone(),
            RStyle::default().add_modifier(Modifier::DIM),
        ),
        Span::raw("   "),
        Span::styled(
            "enter send · tab pane · pgup/pgdn scroll",
            RStyle::default().add_modifier(Modifier::DIM),
        ),
    ])
}

fn draw_pane(f: &mut Frame, pane: &mut Pane, area: Rect, title: &str, active: bool) {
    let border_style = if active {
        RStyle::default().fg(Color::White.ratatui())
    } else {
        RStyle::default().fg(Color::DarkGray.ratatui())
    };
    let scroll_hint = if pane.follow {
        String::new()
    } else {
        format!("  [scroll {}]", pane.scroll)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(Span::styled(
            format!(" {title}{scroll_hint} "),
            border_style,
        ));
    let inner = block.inner(area);
    f.render_widget(block, area);

    pane.set_width(inner.width.saturating_sub(1).max(10) as usize);
    let total = pane.wrapped.len();
    let h = inner.height as usize;
    let start = if pane.follow {
        total.saturating_sub(h)
    } else {
        total.saturating_sub(h + pane.scroll * h)
    };
    let slice: Vec<Line> = pane.wrapped[start.min(total)..].to_vec();
    f.render_widget(Paragraph::new(slice), inner);
}
