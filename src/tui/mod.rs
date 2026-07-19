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
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};

use crate::adapters;
use crate::brand::{self, DANGER};
use crate::catalog::{self, EffortChoice, FastChoice, ModelInfo};
use crate::launch;
use crate::model::{Handoff, SessionSummary, Tool};
use crate::teleport;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    From,
    Session,
    Dest,
    Effort,
    Fast,
    Preview,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DestFocus {
    Target,
    Model,
}

pub struct App {
    step: Step,
    from: Tool,
    to: Tool,
    model: ModelInfo,
    effort: EffortChoice,
    fast: FastChoice,
    cwd: PathBuf,
    all_sessions: Vec<SessionSummary>,
    sessions: Vec<SessionSummary>,
    session_state: ListState,
    tool_state: ListState,
    to_state: ListState,
    model_state: ListState,
    effort_state: ListState,
    fast_state: ListState,
    dest_focus: DestFocus,
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
        let from = Tool::Codex;
        let to = first_target(from);
        let mut to_state = ListState::default();
        to_state.select(Some(0));
        let model = catalog::default_model(to);
        let effort = catalog::default_effort(to, model);
        let fast = catalog::default_fast(to);
        let mut model_state = ListState::default();
        model_state.select(Some(0));
        let mut effort_state = ListState::default();
        effort_state.select(Some(0));
        let mut fast_state = ListState::default();
        fast_state.select(Some(0));

        let mut app = Self {
            step: Step::From,
            from,
            to,
            model,
            effort,
            fast,
            cwd,
            all_sessions: Vec::new(),
            sessions: Vec::new(),
            session_state,
            tool_state,
            to_state,
            model_state,
            effort_state,
            fast_state,
            dest_focus: DestFocus::Target,
            filter: String::new(),
            auto_send: true,
            handoff: None,
            preview_scroll: 0,
            status: "where is the conversation?".into(),
            error: None,
        };
        app.reload_sessions();
        app
    }

    fn targets(&self) -> Vec<Tool> {
        target_tools(self.from)
    }

    fn has_fast_step(&self) -> bool {
        catalog::fast_options_for(self.to).is_some()
    }

    fn sync_target_from_state(&mut self) {
        let targets = self.targets();
        if targets.is_empty() {
            return;
        }
        let i = self
            .to_state
            .selected()
            .unwrap_or(0)
            .min(targets.len() - 1);
        self.to_state.select(Some(i));
        let next = targets[i];
        if next != self.to {
            self.to = next;
            self.reset_model_selection();
        }
    }

    fn reset_model_selection(&mut self) {
        self.model = catalog::default_model(self.to);
        self.model_state.select(Some(0));
        self.reset_effort_selection();
        self.reset_fast_selection();
    }

    fn reset_effort_selection(&mut self) {
        self.effort = catalog::default_effort(self.to, self.model);
        self.effort_state.select(Some(0));
    }

    fn reset_fast_selection(&mut self) {
        self.fast = catalog::default_fast(self.to);
        self.fast_state.select(Some(0));
    }

    fn selected_base_model(&self) -> ModelInfo {
        let list = catalog::models_for(self.to);
        self.model_state
            .selected()
            .and_then(|i| list.get(i).copied())
            .unwrap_or_else(|| catalog::default_model(self.to))
    }

    fn selected_effort(&self) -> EffortChoice {
        let list = catalog::efforts_for(self.to, self.model);
        self.effort_state
            .selected()
            .and_then(|i| list.get(i).copied())
            .unwrap_or_else(|| catalog::default_effort(self.to, self.model))
    }

    fn selected_fast(&self) -> FastChoice {
        let Some(list) = catalog::fast_options_for(self.to) else {
            return FastChoice::OFF;
        };
        self.fast_state
            .selected()
            .and_then(|i| list.get(i).copied())
            .unwrap_or(FastChoice::OFF)
    }

    fn resolved_model(&self) -> ModelInfo {
        catalog::apply_selection(self.model, self.effort, self.fast)
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
        self.model = self.selected_base_model();
        self.effort = self.selected_effort();
        self.fast = if self.has_fast_step() {
            self.selected_fast()
        } else {
            FastChoice::OFF
        };
        let opts = teleport::opts_for_selection(self.model, self.effort, self.fast, None);
        let h = teleport::package(self.from, self.to, &self.cwd, &sess.id, opts)?;
        self.handoff = Some(h);
        self.preview_scroll = 0;
        Ok(())
    }

    fn enter_dest(&mut self) {
        let targets = target_tools(self.from);
        self.to = first_target(self.from);
        let idx = targets.iter().position(|t| *t == self.to).unwrap_or(0);
        self.to_state.select(Some(idx));
        self.reset_model_selection();
        self.dest_focus = DestFocus::Target;
        self.step = Step::Dest;
        self.status = format!("pick target + model · {}", self.to.display_name());
        self.error = None;
    }

    fn enter_effort(&mut self) {
        self.sync_target_from_state();
        self.model = self.selected_base_model();
        self.reset_effort_selection();
        self.step = Step::Effort;
        self.status = format!("{} · pick effort", self.model.label);
        self.error = None;
    }

    fn enter_fast_or_preview(&mut self) -> Result<()> {
        self.effort = self.selected_effort();
        if self.has_fast_step() {
            self.reset_fast_selection();
            self.step = Step::Fast;
            self.status = format!("{} · fast mode (separate from effort)", self.model.label);
            self.error = None;
            Ok(())
        } else {
            self.go_preview()
        }
    }

    fn go_preview(&mut self) -> Result<()> {
        self.build_handoff()?;
        self.step = Step::Preview;
        self.status = "j/k scroll · enter go · a soft/auto · esc back".into();
        self.error = None;
        Ok(())
    }

    fn refresh_dest_status(&mut self) {
        self.model = self.selected_base_model();
        self.status = format!(
            "{} · {} · pack ~{}k",
            self.to.display_name(),
            self.model.label,
            self.model.handoff_budget_tokens() / 1000
        );
    }

    fn step_back(&mut self) {
        self.step = match self.step {
            Step::Session => Step::From,
            Step::Dest => Step::Session,
            Step::Effort => Step::Dest,
            Step::Fast => Step::Effort,
            Step::Preview => {
                if self.has_fast_step() {
                    Step::Fast
                } else {
                    Step::Effort
                }
            }
            Step::From => Step::From,
        };
        self.status = match self.step {
            Step::From => "where is the conversation?".into(),
            Step::Session => "select session".into(),
            Step::Dest => {
                self.refresh_dest_status();
                self.status.clone()
            }
            Step::Effort => format!("{} · pick effort", self.model.label),
            Step::Fast => format!("{} · fast mode", self.model.label),
            Step::Preview => self.status.clone(),
        };
        self.error = None;
    }
}

fn target_tools(from: Tool) -> Vec<Tool> {
    Tool::all().into_iter().filter(|t| *t != from).collect()
}

fn first_target(from: Tool) -> Tool {
    target_tools(from)
        .into_iter()
        .next()
        .unwrap_or(Tool::Grok)
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
                app.step_back();
            }
            KeyCode::Char('a')
                if matches!(
                    app.step,
                    Step::Preview | Step::Effort | Step::Fast | Step::Dest
                ) =>
            {
                app.auto_send = !app.auto_send;
                app.status = if app.auto_send {
                    "auto-send on — target gets the package as first prompt".into()
                } else {
                    "soft on — CLI opens empty; paste ⌘V then send".into()
                };
            }
            KeyCode::Left | KeyCode::Char('h') if app.step == Step::Dest => {
                app.dest_focus = DestFocus::Target;
            }
            KeyCode::Right | KeyCode::Char('l') if app.step == Step::Dest => {
                app.dest_focus = DestFocus::Model;
            }
            KeyCode::Enter => match app.step {
                Step::From => {
                    if let Some(i) = app.tool_state.selected() {
                        app.from = Tool::all()[i];
                        app.reload_sessions();
                        app.step = Step::Session;
                        app.filter.clear();
                        app.apply_filter();
                        app.status = "select session".into();
                        app.error = None;
                    }
                }
                Step::Session => {
                    if app.selected_session().is_some() {
                        app.enter_dest();
                    }
                }
                Step::Dest => app.enter_effort(),
                Step::Effort => {
                    if let Err(e) = app.enter_fast_or_preview() {
                        app.error = Some(e.to_string());
                    }
                }
                Step::Fast => {
                    if let Err(e) = app.go_preview() {
                        app.error = Some(e.to_string());
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
                if c != 'j' && c != 'k' && c != 'q' && c != 'a' && c != 'h' && c != 'l' {
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
        Step::Dest => match app.dest_focus {
            DestFocus::Target => {
                let len = app.targets().len() as i32;
                if len == 0 {
                    return;
                }
                let cur = app.to_state.selected().unwrap_or(0) as i32;
                app.to_state
                    .select(Some((cur + delta).rem_euclid(len) as usize));
                app.sync_target_from_state();
                app.refresh_dest_status();
            }
            DestFocus::Model => {
                let len = catalog::models_for(app.to).len() as i32;
                if len == 0 {
                    return;
                }
                let cur = app.model_state.selected().unwrap_or(0) as i32;
                app.model_state
                    .select(Some((cur + delta).rem_euclid(len) as usize));
                app.model = app.selected_base_model();
                app.refresh_dest_status();
            }
        },
        Step::Effort => {
            let len = catalog::efforts_for(app.to, app.model).len() as i32;
            if len == 0 {
                return;
            }
            let cur = app.effort_state.selected().unwrap_or(0) as i32;
            app.effort_state
                .select(Some((cur + delta).rem_euclid(len) as usize));
            app.effort = app.selected_effort();
            app.status = format!(
                "{} · {}",
                app.model.label,
                catalog::selection_key(app.model, app.effort, FastChoice::OFF)
            );
        }
        Step::Fast => {
            let Some(list) = catalog::fast_options_for(app.to) else {
                return;
            };
            let len = list.len() as i32;
            let cur = app.fast_state.selected().unwrap_or(0) as i32;
            app.fast_state
                .select(Some((cur + delta).rem_euclid(len) as usize));
            app.fast = app.selected_fast();
            app.status = format!(
                "{} · {}",
                app.model.label,
                catalog::selection_key(app.model, app.effort, app.fast)
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
    let header_h = (logo.len() as u16).saturating_add(4);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header_h),
            Constraint::Min(6),
            Constraint::Length(1),
        ])
        .split(area);

    let mut header = logo;
    header.push(brand::tagline());
    header.push(step_line(app));
    header.push(route_line(app));

    f.render_widget(
        Paragraph::new(header).block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(brand::dim_border()),
        ),
        chunks[0],
    );

    match app.step {
        Step::From => render_from(f, chunks[1], app),
        Step::Session => render_sessions(f, chunks[1], app),
        Step::Dest => render_dest(f, chunks[1], app),
        Step::Effort => render_effort(f, chunks[1], app),
        Step::Fast => render_fast(f, chunks[1], app),
        Step::Preview => render_preview(f, chunks[1], app),
    }

    let footer = if let Some(err) = &app.error {
        Paragraph::new(Line::from(Span::styled(
            err.clone(),
            Style::default().fg(DANGER),
        )))
    } else {
        let keys = match app.step {
            Step::Preview => "j/k · pgup/pgdn · enter go · esc · a soft/auto · q",
            Step::Session => "type to filter · j/k · enter · esc · q",
            Step::Dest => "h/l panes · j/k · enter · a soft · esc · q",
            Step::Effort => "j/k · enter · a soft · esc · q",
            Step::Fast => "j/k · enter package · a soft · esc · q",
            Step::From => "j/k · enter · esc · q",
        };
        Paragraph::new(Line::from(vec![
            Span::styled(&app.status, brand::muted()),
            Span::styled(format!("   {keys}"), brand::dim_border()),
        ]))
    };
    f.render_widget(footer, chunks[2]);
}

fn step_line(app: &App) -> Line<'static> {
    let mut steps = vec![
        (Step::From, "from"),
        (Step::Session, "session"),
        (Step::Dest, "model"),
        (Step::Effort, "effort"),
    ];
    if app.has_fast_step() || matches!(app.step, Step::Fast) {
        steps.push((Step::Fast, "fast"));
    }
    steps.push((Step::Preview, "go"));

    let mut spans = Vec::new();
    for (i, (step, label)) in steps.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("  ·  ", brand::dim_border()));
        }
        if *step == app.step {
            spans.push(Span::styled(format!("● {label}"), brand::accent_bold()));
        } else {
            spans.push(Span::styled(format!("○ {label}"), brand::muted()));
        }
    }
    Line::from(spans).alignment(Alignment::Center)
}

fn route_line(app: &App) -> Line<'static> {
    let mut spans = vec![
        Span::styled(app.from.display_name().to_string(), brand::body()),
        Span::styled(" → ", brand::accent()),
        Span::styled(app.to.display_name().to_string(), brand::body()),
    ];
    if matches!(
        app.step,
        Step::Dest | Step::Effort | Step::Fast | Step::Preview
    ) {
        spans.push(Span::styled("  ·  ", brand::muted()));
        spans.push(Span::styled(app.model.label.to_string(), brand::accent()));
    }
    if matches!(app.step, Step::Effort | Step::Fast | Step::Preview) {
        if app.effort.is_default() {
            spans.push(Span::styled(" · default", brand::muted()));
        } else {
            spans.push(Span::styled(
                format!(" · {}", app.effort.label),
                brand::accent(),
            ));
        }
    }
    if matches!(app.step, Step::Fast | Step::Preview) && app.has_fast_step() {
        spans.push(Span::styled(
            if app.fast.on {
                " · fast"
            } else {
                " · standard"
            },
            if app.fast.on {
                brand::accent()
            } else {
                brand::muted()
            },
        ));
    }
    if matches!(
        app.step,
        Step::Dest | Step::Effort | Step::Fast | Step::Preview
    ) {
        let m = if matches!(app.step, Step::Effort | Step::Fast | Step::Preview) {
            app.resolved_model()
        } else {
            app.model
        };
        spans.push(Span::styled(
            format!("  ·  ~{}k", m.handoff_budget_tokens() / 1000),
            brand::muted(),
        ));
    }
    spans.push(Span::styled(
        if app.auto_send {
            "  ·  auto"
        } else {
            "  ·  soft"
        },
        brand::muted(),
    ));
    Line::from(spans).alignment(Alignment::Center)
}

fn panel(title: &str, focused: bool) -> Block<'_> {
    let border = if focused {
        brand::accent()
    } else {
        brand::dim_border()
    };
    let title_style = if focused {
        brand::accent_bold()
    } else {
        brand::muted()
    };
    Block::default()
        .borders(Borders::ALL)
        .border_style(border)
        .title(Span::styled(format!(" {title} "), title_style))
}

fn render_from(f: &mut Frame, area: Rect, app: &App) {
    let items: Vec<ListItem> = Tool::all()
        .into_iter()
        .map(|t| {
            ListItem::new(Line::from(vec![
                Span::styled(format!("{:<12}", t.display_name()), brand::body()),
                Span::styled(t.binary_name().to_string(), brand::muted()),
            ]))
        })
        .collect();
    let list = List::new(items)
        .block(panel("where is the conversation?", true))
        .highlight_style(brand::select())
        .highlight_symbol("▸ ");
    let mut st = app.tool_state;
    f.render_stateful_widget(list, area, &mut st);
}

fn render_dest(f: &mut Frame, area: Rect, app: &App) {
    let wide = area.width >= 72;
    let (target_area, model_area) = if wide {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(28), Constraint::Min(30)])
            .split(area);
        (cols[0], cols[1])
    } else {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(6), Constraint::Min(4)])
            .split(area);
        (rows[0], rows[1])
    };

    render_target_pane(f, target_area, app);
    render_model_pane(f, model_area, app);
}

fn render_target_pane(f: &mut Frame, area: Rect, app: &App) {
    let focused = app.dest_focus == DestFocus::Target;
    let items: Vec<ListItem> = app
        .targets()
        .into_iter()
        .map(|t| {
            ListItem::new(Line::from(vec![
                Span::styled(format!("{:<14}", t.display_name()), brand::body()),
                Span::styled(t.binary_name().to_string(), brand::muted()),
            ]))
        })
        .collect();
    let title = if focused { "target ▸" } else { "target" };
    let list = List::new(items)
        .block(panel(title, focused))
        .highlight_style(brand::select())
        .highlight_symbol("▸ ");
    let mut st = app.to_state;
    f.render_stateful_widget(list, area, &mut st);
}

fn render_model_pane(f: &mut Frame, area: Rect, app: &App) {
    let focused = app.dest_focus == DestFocus::Model;
    let items: Vec<ListItem> = catalog::models_for(app.to)
        .iter()
        .map(|m| {
            ListItem::new(Line::from(vec![
                Span::styled(format!("{:<14}", m.label), brand::body()),
                Span::styled(m.cli_model.to_string(), brand::muted()),
            ]))
        })
        .collect();
    let title = if focused { "model ▸" } else { "model" };
    let list = List::new(items)
        .block(panel(title, focused))
        .highlight_style(brand::select())
        .highlight_symbol("▸ ");
    let mut st = app.model_state;
    f.render_stateful_widget(list, area, &mut st);
}

fn render_effort(f: &mut Frame, area: Rect, app: &App) {
    let wire_hint = match app.to {
        Tool::Codex => "codex -c model_reasoning_effort=…",
        Tool::Grok => "grok --effort …",
        Tool::Claude => "claude --effort …",
    };
    let items: Vec<ListItem> = catalog::efforts_for(app.to, app.model)
        .iter()
        .map(|e| {
            let hint = e.effort.unwrap_or("omit (CLI default)");
            ListItem::new(Line::from(vec![
                Span::styled(format!("{:<12}", e.label), brand::body()),
                Span::styled(hint.to_string(), brand::muted()),
            ]))
        })
        .collect();
    let title = format!("effort · {} · {wire_hint}", app.model.label);
    let list = List::new(items)
        .block(panel(&title, true))
        .highlight_style(brand::select())
        .highlight_symbol("▸ ");
    let mut st = app.effort_state;
    f.render_stateful_widget(list, area, &mut st);
}

fn render_fast(f: &mut Frame, area: Rect, app: &App) {
    let items: Vec<ListItem> = catalog::fast_options_for(app.to)
        .unwrap_or(&[])
        .iter()
        .map(|fc| {
            let hint = if fc.on {
                "--enable fast_mode  (service tier; not effort)"
            } else {
                "standard speed"
            };
            ListItem::new(Line::from(vec![
                Span::styled(format!("{:<12}", fc.label), brand::body()),
                Span::styled(hint.to_string(), brand::muted()),
            ]))
        })
        .collect();
    let title = format!("fast · {} · separate from effort", app.model.label);
    let list = List::new(items)
        .block(panel(&title, true))
        .highlight_style(brand::select())
        .highlight_symbol("▸ ");
    let mut st = app.fast_state;
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
        .block(panel(&title, true))
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
        .block(panel(&title, true));
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
    if s.starts_with("Prior context")
        || s.starts_with("warn:")
        || s.starts_with("model:")
        || s.starts_with("to:")
        || s.starts_with("cwd:")
        || s.starts_with("session:")
        || s.starts_with("kept:")
    {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_tools_excludes_source() {
        let t = target_tools(Tool::Codex);
        assert!(!t.contains(&Tool::Codex));
        assert_eq!(t.len(), 2);
    }
}
