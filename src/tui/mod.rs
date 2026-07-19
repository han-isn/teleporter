use std::io::{self, stdout};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Result, bail};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Frame;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};

use crate::adapters;
use crate::brand::{self, DANGER};
use crate::catalog::{self, ModelInfo};
use crate::launch;
use crate::model::{Handoff, SessionSummary, Tool};
use crate::teleport;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    From,
    Session,
    To,
    Model,
    Preview,
}

pub struct App {
    step: Step,
    from: Tool,
    to: Tool,
    model: ModelInfo,
    cwd: PathBuf,
    /// Full list from disk (unfiltered).
    all_sessions: Vec<SessionSummary>,
    /// Filtered view for the session list.
    sessions: Vec<SessionSummary>,
    session_state: ListState,
    tool_state: ListState,
    to_state: ListState,
    model_state: ListState,
    filter: String,
    auto_send: bool,
    handoff: Option<Handoff>,
    preview_scroll: u16,
    status: String,
    error: Option<String>,
}

impl App {
    pub fn new(cwd: PathBuf) -> Self {
        let mut session_state = ListState::default();
        session_state.select(Some(0));
        let mut tool_state = ListState::default();
        tool_state.select(Some(0));
        let mut to_state = ListState::default();
        to_state.select(Some(1));
        let to = Tool::Grok;
        let model = catalog::default_model(to);
        let mut model_state = ListState::default();
        model_state.select(Some(0));

        let mut app = Self {
            step: Step::From,
            from: Tool::Codex,
            to,
            model,
            cwd,
            all_sessions: Vec::new(),
            sessions: Vec::new(),
            session_state,
            tool_state,
            to_state,
            model_state,
            filter: String::new(),
            // Default on: soft mode opens an empty CLI and feels broken.
            auto_send: true,
            handoff: None,
            preview_scroll: 0,
            status: "select source · auto-send on".into(),
            error: None,
        };
        app.reload_sessions();
        app
    }

    fn sync_model_list(&mut self) {
        let list = catalog::models_for(self.to);
        self.model = list[0];
        self.model_state.select(Some(0));
    }

    fn selected_model(&self) -> ModelInfo {
        let list = catalog::models_for(self.to);
        self.model_state
            .selected()
            .and_then(|i| list.get(i).copied())
            .unwrap_or_else(|| catalog::default_model(self.to))
    }

    fn reload_sessions(&mut self) {
        let adapter = adapters::adapter_for(self.from);
        match adapter.list(&self.cwd) {
            Ok(list) => {
                self.all_sessions = list;
                self.error = None;
            }
            Err(e) => {
                self.all_sessions.clear();
                self.error = Some(format!("list failed: {e}"));
            }
        }
        self.apply_filter();
    }

    fn apply_filter(&mut self) {
        if self.filter.is_empty() {
            self.sessions = self.all_sessions.clone();
        } else {
            let f = self.filter.to_ascii_lowercase();
            self.sessions = self
                .all_sessions
                .iter()
                .filter(|s| s.title.to_ascii_lowercase().contains(&f) || s.id.contains(&f))
                .cloned()
                .collect();
        }
        if self.sessions.is_empty() {
            self.session_state.select(None);
        } else {
            let cur = self.session_state.selected().unwrap_or(0);
            self.session_state
                .select(Some(cur.min(self.sessions.len() - 1)));
        }
    }

    fn selected_session(&self) -> Option<&SessionSummary> {
        self.session_state
            .selected()
            .and_then(|i| self.sessions.get(i))
    }

    fn build_handoff(&mut self) -> Result<()> {
        let Some(sess) = self.selected_session().cloned() else {
            bail!("no session selected");
        };
        if self.from == self.to {
            bail!("source and target must differ");
        }
        self.model = self.selected_model();
        let opts = teleport::opts_for_model(self.model, None);
        let h = teleport::package(self.from, self.to, &self.cwd, &sess.id, opts)?;
        self.handoff = Some(h);
        self.preview_scroll = 0;
        Ok(())
    }
}

pub fn run(cwd: PathBuf) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(cwd);
    let result = event_loop(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    let result = result?;
    if let Some(handoff) = result {
        brand::print_splash();
        let plan = launch::plan_launch(&handoff, app.auto_send)?;
        launch::execute(&plan)?;
    }
    Ok(())
}

fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> Result<Option<Handoff>> {
    loop {
        terminal.draw(|f| ui(f, app))?;

        if !event::poll(Duration::from_millis(80))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Ok(None);
        }

        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                if app.step == Step::From {
                    return Ok(None);
                }
                app.step = match app.step {
                    Step::Session => Step::From,
                    Step::To => Step::Session,
                    Step::Model => Step::To,
                    Step::Preview => Step::Model,
                    Step::From => Step::From,
                };
                app.error = None;
            }
            KeyCode::Char('a')
                if matches!(app.step, Step::Preview | Step::Model | Step::To) =>
            {
                app.auto_send = !app.auto_send;
                app.status = if app.auto_send {
                    "auto-send on — target gets the package as first prompt".into()
                } else {
                    "soft on — CLI opens empty; paste ⌘V then send".into()
                };
            }
            KeyCode::Enter => match app.step {
                Step::From => {
                    if let Some(i) = app.tool_state.selected() {
                        app.from = Tool::all()[i];
                        app.to = Tool::all()
                            .into_iter()
                            .find(|t| *t != app.from)
                            .unwrap_or(Tool::Grok);
                        app.to_state
                            .select(Tool::all().iter().position(|t| *t == app.to));
                        app.sync_model_list();
                        app.reload_sessions();
                        app.step = Step::Session;
                        app.status = "select session".into();
                    }
                }
                Step::Session => {
                    if app.selected_session().is_some() {
                        app.step = Step::To;
                        app.status = "select target CLI".into();
                    }
                }
                Step::To => {
                    if let Some(i) = app.to_state.selected() {
                        let next = Tool::all()[i];
                        if next == app.from {
                            app.error = Some("source and target must differ".into());
                        } else {
                            app.to = next;
                            app.sync_model_list();
                            app.step = Step::Model;
                            app.status = format!(
                                "select model · budget ~{}k tokens",
                                app.model.handoff_budget_tokens() / 1000
                            );
                            app.error = None;
                        }
                    }
                }
                Step::Model => {
                    app.model = app.selected_model();
                    match app.build_handoff() {
                        Ok(()) => {
                            app.step = Step::Preview;
                            app.status = "j/k scroll · enter go · a auto-send · esc back".into();
                            app.error = None;
                        }
                        Err(e) => app.error = Some(e.to_string()),
                    }
                }
                Step::Preview => return Ok(app.handoff.clone()),
            },
            KeyCode::PageUp if app.step == Step::Preview => {
                app.preview_scroll = app.preview_scroll.saturating_sub(10);
            }
            KeyCode::PageDown if app.step == Step::Preview => {
                app.preview_scroll = app.preview_scroll.saturating_add(10);
            }
            KeyCode::Home if app.step == Step::Preview => {
                app.preview_scroll = 0;
            }
            KeyCode::Up | KeyCode::Char('k') => move_sel(app, -1),
            KeyCode::Down | KeyCode::Char('j') => move_sel(app, 1),
            KeyCode::Backspace if app.step == Step::Session => {
                app.filter.pop();
                app.apply_filter();
            }
            KeyCode::Char(c)
                if app.step == Step::Session && !key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                if c != 'j' && c != 'k' && c != 'q' && c != 'a' {
                    app.filter.push(c);
                    app.apply_filter();
                } else if c == 'j' {
                    move_sel(app, 1);
                } else if c == 'k' {
                    move_sel(app, -1);
                }
            }
            _ => {}
        }
    }
}

fn move_sel(app: &mut App, delta: i32) {
    match app.step {
        Step::From => {
            let len = Tool::all().len() as i32;
            let cur = app.tool_state.selected().unwrap_or(0) as i32;
            app.tool_state
                .select(Some((cur + delta).rem_euclid(len) as usize));
        }
        Step::Session => {
            let len = app.sessions.len() as i32;
            if len == 0 {
                return;
            }
            let cur = app.session_state.selected().unwrap_or(0) as i32;
            app.session_state
                .select(Some((cur + delta).rem_euclid(len) as usize));
        }
        Step::To => {
            let len = Tool::all().len() as i32;
            let cur = app.to_state.selected().unwrap_or(0) as i32;
            app.to_state
                .select(Some((cur + delta).rem_euclid(len) as usize));
        }
        Step::Model => {
            let len = catalog::models_for(app.to).len() as i32;
            let cur = app.model_state.selected().unwrap_or(0) as i32;
            let next = (cur + delta).rem_euclid(len) as usize;
            app.model_state.select(Some(next));
            app.model = catalog::models_for(app.to)[next];
            app.status = format!(
                "{} · pack up to ~{}k tokens",
                app.model.id,
                app.model.handoff_budget_tokens() / 1000
            );
        }
        Step::Preview => {
            let next = app.preview_scroll as i32 + delta;
            app.preview_scroll = next.max(0) as u16;
        }
    }
}

fn ui(f: &mut Frame, app: &App) {
    let area = f.area();
    let logo = brand::logo_lines_for(area.width);
    let header_h = (logo.len() as u16).saturating_add(2); // logo + route + bottom rule
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header_h),
            Constraint::Min(6),
            Constraint::Length(1),
        ])
        .split(area);

    // CLI-style wordmark stays for the whole session (Codex/Claude splash family).
    let mut header = logo;
    header.push(Line::from(vec![
        Span::styled(app.from.display_name(), brand::body()),
        Span::styled(" → ", brand::muted()),
        Span::styled(app.to.display_name(), brand::body()),
        Span::styled("  ", brand::muted()),
        Span::styled(app.model.id, brand::accent()),
        Span::styled(
            format!("  ~{}k", app.model.handoff_budget_tokens() / 1000),
            brand::muted(),
        ),
        Span::styled(
            if app.auto_send { "  auto" } else { "  soft" },
            brand::muted(),
        ),
    ]));

    f.render_widget(
        Paragraph::new(header).block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(brand::dim_border()),
        ),
        chunks[0],
    );

    match app.step {
        Step::From => render_tool_list(f, chunks[1], "from", &app.tool_state, None),
        Step::Session => render_sessions(f, chunks[1], app),
        Step::To => render_tool_list(f, chunks[1], "to", &app.to_state, Some(app.from)),
        Step::Model => render_models(f, chunks[1], app),
        Step::Preview => render_preview(f, chunks[1], app),
    }

    let footer = if let Some(err) = &app.error {
        Paragraph::new(Line::from(Span::styled(
            err.clone(),
            Style::default().fg(DANGER),
        )))
    } else {
        let keys = match app.step {
            Step::Preview => "j/k pgup/pgdn  enter  esc  a  q",
            _ => "j/k  enter  esc  a  q",
        };
        Paragraph::new(Line::from(vec![
            Span::styled(&app.status, brand::muted()),
            Span::styled(format!("   {keys}"), brand::dim_border()),
        ]))
    };
    f.render_widget(footer, chunks[2]);
}

fn panel(title: &str) -> Block<'_> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(brand::dim_border())
        .title(Span::styled(format!(" {title} "), brand::accent()))
}

fn render_tool_list(
    f: &mut Frame,
    area: Rect,
    title: &str,
    state: &ListState,
    exclude: Option<Tool>,
) {
    let items: Vec<ListItem> = Tool::all()
        .into_iter()
        .map(|t| {
            if exclude == Some(t) {
                ListItem::new(Line::from(Span::styled(
                    format!("{}  · source", t.display_name()),
                    brand::muted(),
                )))
            } else {
                ListItem::new(Line::from(Span::styled(
                    t.display_name(),
                    brand::body(),
                )))
            }
        })
        .collect();
    let list = List::new(items)
        .block(panel(title))
        .highlight_style(brand::select())
        .highlight_symbol("▸ ");
    let mut st = *state;
    f.render_stateful_widget(list, area, &mut st);
}

fn render_models(f: &mut Frame, area: Rect, app: &App) {
    let items: Vec<ListItem> = catalog::models_for(app.to)
        .iter()
        .map(|m| {
            ListItem::new(Line::from(vec![
                Span::styled(format!("{:<12}", m.label), brand::body()),
                Span::styled(
                    format!(
                        "{}   pack ~{}k",
                        m.id,
                        m.handoff_budget_tokens() / 1000
                    ),
                    brand::muted(),
                ),
            ]))
        })
        .collect();
    let title = format!("model · {}", app.to.display_name());
    let list = List::new(items)
        .block(panel(&title))
        .highlight_style(brand::select())
        .highlight_symbol("▸ ");
    let mut st = app.model_state;
    f.render_stateful_widget(list, area, &mut st);
}

fn render_sessions(f: &mut Frame, area: Rect, app: &App) {
    let items: Vec<ListItem> = if app.sessions.is_empty() {
        vec![ListItem::new(Span::styled(
            format!("no {} sessions here", app.from.display_name()),
            brand::muted(),
        ))]
    } else {
        app.sessions
            .iter()
            .map(|s| {
                let age = s.updated_at.format("%m-%d %H:%M");
                ListItem::new(Line::from(vec![
                    Span::styled(format!("{age}  "), brand::muted()),
                    Span::styled(truncate(&s.title, 64), brand::body()),
                ]))
            })
            .collect()
    };
    let title = if app.filter.is_empty() {
        format!("session · {}", app.sessions.len())
    } else {
        format!("session · /{}", app.filter)
    };
    let list = List::new(items)
        .block(panel(&title))
        .highlight_style(brand::select())
        .highlight_symbol("▸ ");
    let mut st = app.session_state;
    f.render_stateful_widget(list, area, &mut st);
}

fn render_preview(f: &mut Frame, area: Rect, app: &App) {
    let lines = app
        .handoff
        .as_ref()
        .map(|h| preview_lines(&h.markdown))
        .unwrap_or_else(|| vec![Line::from(Span::styled("(empty)", brand::muted()))]);

    let inner_h = area.height.saturating_sub(2) as usize;
    let max_scroll = lines.len().saturating_sub(inner_h.max(1));
    let scroll = (app.preview_scroll as usize).min(max_scroll) as u16;

    let title = if max_scroll == 0 {
        "package · enter to go".to_string()
    } else {
        format!("package · {scroll}/{} · j/k scroll", max_scroll)
    };

    let para = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0))
        .block(panel(&title));
    f.render_widget(para, area);
}

fn preview_lines(md: &str) -> Vec<Line<'static>> {
    md.lines().map(style_preview_line).collect()
}

fn style_preview_line(raw: &str) -> Line<'static> {
    let s = raw.to_string();
    if s.starts_with("# ") {
        return Line::from(Span::styled(s, brand::accent_bold()));
    }
    if s.starts_with("## ") {
        return Line::from(Span::styled(s, brand::accent()));
    }
    if s.starts_with("user") {
        return Line::from(Span::styled(s, brand::user()));
    }
    if s.starts_with("assistant") {
        return Line::from(Span::styled(s, brand::body()));
    }
    if s.starts_with("tool  ") {
        return Line::from(Span::styled(s, brand::tool()));
    }
    if s.starts_with("Rules:") || s.starts_with("warn:") || s.starts_with("model:") {
        return Line::from(Span::styled(s, brand::muted()));
    }
    if s.starts_with("cwd:") || s.starts_with("branch:") || s.starts_with("session:") || s.starts_with("turns:") {
        return Line::from(Span::styled(s, brand::muted()));
    }
    Line::from(Span::styled(s, brand::body()))
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}
