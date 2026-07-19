mod claude;
mod codex;
pub(crate) mod common;
mod grok;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::model::{SessionSummary, Tool, Transcript};

pub use claude::ClaudeAdapter;
pub use codex::CodexAdapter;
pub use grok::GrokAdapter;

pub trait Adapter: Send + Sync {
    /// List sessions for `cwd`, newest first.
    fn list(&self, cwd: &Path) -> Result<Vec<SessionSummary>>;

    /// Load a full transcript by id or path.
    fn show(&self, cwd: &Path, reference: &str) -> Result<Transcript>;
}

pub fn adapter_for(tool: Tool) -> Box<dyn Adapter> {
    match tool {
        Tool::Codex => Box::new(CodexAdapter::default()),
        Tool::Grok => Box::new(GrokAdapter::default()),
        Tool::Claude => Box::new(ClaudeAdapter::default()),
    }
}

pub fn resolve_home(env_key: &str, default_leaf: &str) -> Result<PathBuf> {
    if let Ok(v) = std::env::var(env_key) {
        return Ok(PathBuf::from(v));
    }
    let home = dirs::home_dir().context("could not resolve home directory")?;
    Ok(home.join(default_leaf))
}

pub(crate) fn path_matches_cwd(session_cwd: &Path, cwd: &Path) -> bool {
    let Ok(a) = dunce_canonicalize(session_cwd) else {
        return session_cwd == cwd;
    };
    let Ok(b) = dunce_canonicalize(cwd) else {
        return session_cwd == cwd;
    };
    a == b
}

fn dunce_canonicalize(path: &Path) -> std::io::Result<PathBuf> {
    // Prefer real paths when possible; fall back to absolute.
    std::fs::canonicalize(path).or_else(|_| {
        if path.is_absolute() {
            Ok(path.to_path_buf())
        } else {
            std::env::current_dir().map(|c| c.join(path))
        }
    })
}

pub(crate) fn truncate_title(s: &str, max: usize) -> String {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() <= max {
        return s;
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

pub(crate) fn extract_paths(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    // Markdown links like [File.ts](/abs/path/File.ts:12 — strip junk.
    for token in text.split_whitespace() {
        let mut t = token.trim_matches(|c: char| {
            matches!(
                c,
                ',' | '.' | ';' | ')' | '(' | '"' | '\'' | '`' | '[' | ']'
            )
        });
        if let Some(idx) = t.find("](") {
            t = &t[idx + 2..];
        }
        // Drop trailing :line
        let t = t.split(':').next().unwrap_or(t);
        if t.contains('/')
            && (t.starts_with('/')
                || t.starts_with("./")
                || t.starts_with("../")
                || t.contains("src/")
                || t.contains("crates/")
                || t.ends_with(".rs")
                || t.ends_with(".ts")
                || t.ends_with(".tsx")
                || t.ends_with(".js")
                || t.ends_with(".py")
                || t.ends_with(".md")
                || t.ends_with(".toml")
                || t.ends_with(".json"))
        {
            let clean = t.to_string();
            if !found.iter().any(|x| x == &clean) {
                found.push(clean);
            }
        }
        if found.len() >= 16 {
            break;
        }
    }
    found
}
