//! Shared adapter helpers — keep list/resolve policy in one place.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::model::{SessionSummary, Turn, TurnRole};

pub(crate) fn finalize_sessions(mut sessions: Vec<SessionSummary>) -> Vec<SessionSummary> {
    sessions.sort_by_key(|s| std::cmp::Reverse(s.updated_at));
    sessions.truncate(50);
    sessions
}

pub(crate) fn derive_last_user(turns: &[Turn]) -> Option<String> {
    turns
        .iter()
        .rev()
        .find(|t| t.role == TurnRole::User)
        .map(|t| t.text.clone())
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
        let Some(s) = sessions.first() else {
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
        .collect();
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
