use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// Terminal palette — phosphor green on near-black.
pub const GREEN: Color = Color::Rgb(34, 197, 94);
pub const GREEN_DIM: Color = Color::Rgb(22, 101, 52);
pub const GREEN_BRIGHT: Color = Color::Rgb(74, 222, 128);
pub const FG: Color = Color::Rgb(226, 232, 240);
pub const MUTED: Color = Color::Rgb(148, 163, 184);
pub const SELECT_BG: Color = Color::Rgb(14, 40, 24);
pub const DANGER: Color = Color::Rgb(220, 90, 90);
pub const USER: Color = Color::Rgb(134, 239, 172);
pub const TOOL: Color = Color::Rgb(74, 120, 90);

pub fn logo_style() -> Style {
    Style::default().fg(GREEN_BRIGHT)
}

pub fn accent() -> Style {
    Style::default().fg(GREEN)
}

pub fn accent_bold() -> Style {
    Style::default().fg(GREEN_BRIGHT).add_modifier(Modifier::BOLD)
}

pub fn muted() -> Style {
    Style::default().fg(MUTED)
}

pub fn body() -> Style {
    Style::default().fg(FG)
}

pub fn dim_border() -> Style {
    Style::default().fg(GREEN_DIM)
}

pub fn select() -> Style {
    Style::default()
        .fg(GREEN_BRIGHT)
        .bg(SELECT_BG)
        .add_modifier(Modifier::BOLD)
}

pub fn user() -> Style {
    Style::default().fg(USER)
}

pub fn tool() -> Style {
    Style::default().fg(TOOL)
}

/// Full TELEPORTER wordmark — same family as Codex / Claude CLI splash art.
pub fn pixel_logo_full() -> &'static [&'static str] {
    &[
        "████████╗███████╗██╗     ███████╗██████╗  ██████╗ ██████╗ ████████╗███████╗██████╗",
        "╚══██╔══╝██╔════╝██║     ██╔════╝██╔══██╗██╔═══██╗██╔══██╗╚══██╔══╝██╔════╝██╔══██╗",
        "   ██║   █████╗  ██║     █████╗  ██████╔╝██║   ██║██████╔╝   ██║   █████╗  ██████╔╝",
        "   ██║   ██╔══╝  ██║     ██╔══╝  ██╔═══╝ ██║   ██║██╔══██╗   ██║   ██╔══╝  ██╔══██╗",
        "   ██║   ███████╗███████╗███████╗██║     ╚██████╔╝██║  ██║   ██║   ███████╗██║  ██║",
        "   ╚═╝   ╚══════╝╚══════╝╚══════╝╚═╝      ╚═════╝ ╚═╝  ╚═╝   ╚═╝   ╚══════╝╚═╝  ╚═╝",
    ]
}

/// Narrow-terminal fallback (README compact mark).
pub fn pixel_logo_compact() -> &'static [&'static str] {
    &[
        "▄▄▄▄▄ ▄▄▄ ▄   ▄▄▄ ▄▄▄▄ ▄▄▄ ▄▄▄ ▄▄▄▄▄ ▄▄▄ ▄▄▄",
        "  █   █▄  █   █▄  █▄▄█ █▄█ █▄█   █   █▄  █▄█",
        "  █   █▄▄ █▄▄ █▄▄ █    █ █ █ █   █   █▄▄ █ █",
    ]
}

pub fn pixel_logo_for(width: u16) -> &'static [&'static str] {
    // Full mark is ~82 cols; keep a little margin.
    if width >= 86 {
        pixel_logo_full()
    } else {
        pixel_logo_compact()
    }
}

pub fn logo_lines_for(width: u16) -> Vec<Line<'static>> {
    pixel_logo_for(width)
        .iter()
        .map(|l| Line::from(Span::styled(*l, logo_style())))
        .collect()
}

pub fn print_splash() {
    eprintln!();
    let width = crossterm::terminal::size()
        .map(|(w, _)| w)
        .unwrap_or(80);
    for line in pixel_logo_for(width) {
        eprintln!("\x1b[38;2;74;222;128m{line}\x1b[0m");
    }
    eprintln!("\x1b[38;2;148;163;184m  conversation handoff · codex ↔ grok ↔ claude\x1b[0m");
    eprintln!();
}
