//! Shared adapter helpers.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::model::{SessionSummary, Turn, TurnRole};

pub(crate) fn finalize_sessions(mut sessions: Vec<SessionSummary>) -> Vec<SessionSummary> {
    sessions.sort_by(|a, b| {
        is_noise_title(&a.title)
            .cmp(&is_noise_title(&b.title))
            .then_with(|| b.updated_at.cmp(&a.updated_at))
    });
    sessions.truncate(50);
    sessions
}

pub(crate) fn is_noise_title(title: &str) -> bool {
    let t = title.trim();
    t.is_empty()
        || t.starts_with('<')
        || t.starts_with("# AGENTS.md")
        || t.starts_with("# Files mentioned")
        || t.starts_with("You are Codex")
        || t.starts_with("You are Grok")
        || t.contains("environment_context")
        || t.contains("Files mentioned by the user")
        || t.eq_ignore_ascii_case("warmup")
}

pub(crate) fn is_noise_user_text(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() || t.eq_ignore_ascii_case("warmup") {
        return true;
    }
    if t.contains("<user_query>") || t.contains("My request for Codex:") {
        return false;
    }
    t.starts_with('<')
        || t.starts_with("# AGENTS.md")
        || t.starts_with("You are Codex")
        || t.starts_with("You are Grok")
        || t.contains("environment_context")
}

/// Unwrap obvious Codex/Grok wrappers; otherwise collapse whitespace.
pub(crate) fn extract_user_facing(text: &str) -> String {
    let s = text.trim();
    if s.is_empty() {
        return String::new();
    }
    if let Some(q) = between(s, "<user_query>", "</user_query>") {
        return collapse(q);
    }
    for marker in ["## My request for Codex:", "My request for Codex:"] {
        if let Some(i) = s.find(marker) {
            let rest = s[i + marker.len()..].trim();
            if !rest.is_empty() {
                return collapse(&strip_image_tags(rest));
            }
        }
    }
    collapse(&strip_image_tags(s))
}

fn strip_image_tags(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(start) = rest.find("<image") {
        out.push_str(&rest[..start]);
        if let Some(end) = rest[start..].find("</image>") {
            rest = &rest[start + end + "</image>".len()..];
        } else {
            break;
        }
    }
    out.push_str(rest);
    out
}

pub(crate) fn derive_title(turns: &[Turn], fallback: &str) -> String {
    for t in turns.iter().filter(|t| t.role == TurnRole::User) {
        let facing = extract_user_facing(&t.text);
        if facing.is_empty() || is_noise_title(&facing) || is_noise_user_text(&facing) {
            continue;
        }
        return super::truncate_title(&facing, 120);
    }
    fallback.to_string()
}

pub(crate) fn derive_last_user(turns: &[Turn]) -> Option<String> {
    turns
        .iter()
        .rev()
        .filter(|t| t.role == TurnRole::User)
        .map(|t| extract_user_facing(&t.text))
        .find(|t| !t.is_empty() && !is_noise_user_text(t))
}

/// Keep real code paths; drop screenshots / empty junk.
pub(crate) fn is_useful_file_mention(path: &str) -> bool {
    let p = path.trim();
    if p.is_empty() {
        return false;
    }
    let lower = p.to_ascii_lowercase();
    if lower.ends_with(".png")
        || lower.ends_with(".jpg")
        || lower.ends_with(".jpeg")
        || lower.ends_with(".gif")
        || lower.ends_with(".webp")
        || lower.contains("fireshot")
        || lower.contains("/var/folders/")
    {
        return false;
    }
    p.contains('.') || p.contains("src/")
}

fn between<'a>(s: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let i = s.find(start)?;
    let rest = &s[i + start.len()..];
    let j = rest.find(end)?;
    Some(rest[..j].trim())
}

fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_codex_request_marker() {
        let raw = r#"# Files mentioned ## My request for Codex: hey bro. oyun liveda"#;
        let facing = extract_user_facing(raw);
        assert!(facing.starts_with("hey bro"), "{facing}");
    }
}

/// Resolve `latest` / path / id / fuzzy title → session path.
pub(crate) fn resolve_session_path(
    tool_label: &str,
    cwd: &Path,
    reference: &str,
    sessions: &[SessionSummary],
    path_ok: impl Fn(&Path) -> bool,
) -> Result<PathBuf> {
    let reference = reference.trim();
    if reference == "latest" || reference.is_empty() {
        let s = sessions
            .iter()
            .find(|s| !is_noise_title(&s.title))
            .or_else(|| sessions.first());
        let Some(s) = s else {
            bail!("no {tool_label} sessions for {}", cwd.display());
        };
        return Ok(s.path.clone());
    }

    let as_path = PathBuf::from(reference);
    if path_ok(&as_path) {
        return Ok(as_path);
    }

    if let Some(s) = sessions.iter().find(|s| s.id == reference) {
        return Ok(s.path.clone());
    }

    let lower = reference.to_ascii_lowercase();
    let matches: Vec<_> = sessions
        .iter()
        .filter(|s| s.title.to_ascii_lowercase().contains(&lower))
        .filter(|s| !is_noise_title(&s.title))
        .collect();
    let matches = if matches.is_empty() {
        sessions
            .iter()
            .filter(|s| s.title.to_ascii_lowercase().contains(&lower))
            .collect::<Vec<_>>()
    } else {
        matches
    };
    match matches.as_slice() {
        [one] => Ok(one.path.clone()),
        [] => bail!("{tool_label} session not found: {reference}"),
        many => {
            let ids: Vec<_> = many
                .iter()
                .map(|s| format!("{}  {}", s.id, super::truncate_title(&s.title, 60)))
                .collect();
            bail!(
                "ambiguous {tool_label} session reference `{reference}`:\n{}",
                ids.join("\n")
            );
        }
    }
}
