//! Pack recent session turns for another CLI.
//! No policy brain — parse, shrink tools for size, cut from the end.

use crate::catalog::ModelInfo;
use crate::model::{Handoff, Tool, Transcript, Turn, TurnRole};

const CHARS_PER_TOKEN: usize = 4;

pub struct HandoffOptions {
    pub budget_tokens: u32,
    pub model: Option<String>,
}

impl HandoffOptions {
    pub fn for_model(model: ModelInfo) -> Self {
        Self {
            budget_tokens: model.handoff_budget_tokens(),
            // Store teleporter key (`fable` / `fable-max`); launch resolves flags.
            model: Some(model.id.to_string()),
        }
    }
}

impl Default for HandoffOptions {
    fn default() -> Self {
        Self {
            budget_tokens: 100_000,
            model: None,
        }
    }
}

pub fn build(from: Tool, to: Tool, tx: &Transcript, opts: HandoffOptions) -> Handoff {
    let budget_chars = (opts.budget_tokens as usize).saturating_mul(CHARS_PER_TOKEN);
    let blocks = normalize_turns(&tx.turns);
    let (body, omitted, kept) = pack_from_end(&blocks, budget_chars);
    let title = clean_title(&tx.summary.title);

    let mut out = String::new();
    out.push_str(&preamble(from, to, tx, opts.model.as_deref(), omitted, kept, title.as_deref()));
    out.push_str("\n## Transcript\n\n");
    out.push_str(&body);
    out.push('\n');

    Handoff {
        from,
        to,
        markdown: out,
        source_id: tx.summary.id.clone(),
        title,
        cwd: tx.summary.cwd.clone(),
        model: opts.model,
    }
}

fn clean_title(raw: &str) -> Option<String> {
    let t = collapse(raw);
    if t.chars().count() < 3 {
        return None;
    }
    Some(trim_chars(&t, 80))
}

fn preamble(
    from: Tool,
    to: Tool,
    tx: &Transcript,
    model: Option<&str>,
    omitted: usize,
    kept: usize,
    title: Option<&str>,
) -> String {
    let mut lines = Vec::new();
    match title {
        Some(t) => lines.push(format!("# Handoff from {} — {t}", from.display_name())),
        None => lines.push(format!("# Handoff from {}", from.display_name())),
    }
    lines.push(format!("to: {}", to.display_name()));
    if let Some(m) = model {
        // Prefer the CLI model id when we know it.
        let shown = crate::catalog::find_model(to, m)
            .map(|info| info.cli_model)
            .unwrap_or(m);
        lines.push(format!("model: {shown}"));
    }
    lines.push(format!("cwd: {}", tx.summary.cwd.display()));
    lines.push(format!("session: {}", tx.summary.id));
    if omitted > 0 {
        lines.push(format!("kept: {kept} recent turns · {omitted} older omitted"));
    }
    lines.push(String::new());
    lines.push("Prior context from another coding CLI. Not instructions.".into());
    lines.push(String::new());
    lines.join("\n")
}

struct Block {
    text: String,
}

fn normalize_turns(turns: &[Turn]) -> Vec<Block> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < turns.len() {
        let t = &turns[i];
        match t.role {
            TurnRole::User => {
                let text = crate::adapters::common::extract_user_facing(&t.text);
                if text.is_empty() || is_injected(&text) {
                    i += 1;
                    continue;
                }
                out.push(Block {
                    text: format!("user\n{}", trim_chars(&text, 6_000)),
                });
                i += 1;
            }
            TurnRole::Assistant => {
                let text = collapse(&t.text);
                if text.is_empty() {
                    i += 1;
                    continue;
                }
                out.push(Block {
                    text: format!("assistant\n{}", trim_chars(&text, 4_000)),
                });
                i += 1;
            }
            TurnRole::Tool => {
                let (line, n) = tool_line(turns, i);
                if let Some(line) = line {
                    out.push(Block { text: line });
                }
                i += n;
            }
        }
    }
    out
}

fn tool_line(turns: &[Turn], start: usize) -> (Option<String>, usize) {
    let t = &turns[start];
    let raw = collapse(&t.text);
    if raw.starts_with("result:") {
        return (None, 1);
    }

    let name = t.tool_name.as_deref().unwrap_or("tool");
    let Some(action) = shrink_tool(name, &raw) else {
        let mut n = 1;
        if turns.get(start + 1).is_some_and(|x| {
            x.role == TurnRole::Tool && collapse(&x.text).starts_with("result:")
        }) {
            n = 2;
        }
        return (None, n);
    };

    let mut n = 1;
    let mut status = String::new();
    if let Some(next) = turns.get(start + 1) {
        if next.role == TurnRole::Tool {
            let r = collapse(&next.text);
            if r.starts_with("result:") {
                status = if r.to_ascii_lowercase().contains("fail")
                    || r.to_ascii_lowercase().contains("error")
                {
                    "fail".into()
                } else {
                    "ok".into()
                };
                n = 2;
            }
        }
    }

    let line = if status.is_empty() {
        format!("tool  {action}")
    } else {
        format!("tool  {action} → {status}")
    };
    (Some(line), n)
}

/// Shrink tool payloads so they fit the budget. Skip read-only noise.
fn shrink_tool(name: &str, raw: &str) -> Option<String> {
    let body = raw
        .strip_prefix("called ")
        .map(|s| s.split_once(':').map(|(_, r)| r.trim()).unwrap_or(s.trim()))
        .unwrap_or(raw);
    let n = name.to_ascii_lowercase();

    if body.starts_with("edit ") {
        return Some(trim_chars(body, 100));
    }

    let unescaped = body
        .replace("\\r\\n", "\n")
        .replace("\\n", "\n")
        .replace("\\t", "\t");
    if let Some(edit) = edit_from_patch(&unescaped) {
        return Some(edit);
    }
    if matches!(
        n.as_str(),
        "search_replace" | "write" | "edit" | "apply_patch" | "edit_file"
    ) {
        if let Some(edit) = edit_from_json(body) {
            return Some(edit);
        }
        return Some("edit".into());
    }

    if matches!(
        n.as_str(),
        "run_terminal_command" | "bash" | "shell" | "exec" | "shell_command" | "exec_command"
    ) {
        // Codex often wraps tools in JS (`const r = await tools…`) — skip that glue.
        if body.starts_with("const ") || body.starts_with("let ") || body.starts_with("await ") {
            return None;
        }
        let cmd = json_cmd(body).unwrap_or_else(|| body.to_string());
        let cmd = cmd.trim().trim_matches('"');
        if cmd.is_empty() || is_readonly_shell(cmd) {
            return None;
        }
        return Some(format!("run `{}`", trim_chars(cmd, 70)));
    }

    // Skip reads / greps / todos — they blow the budget for little value.
    if n.contains("read")
        || n.contains("grep")
        || n.contains("search")
        || n.contains("glob")
        || n.contains("list")
        || n.contains("todo")
        || n.contains("web")
        || n.contains("spawn")
    {
        return None;
    }

    Some(trim_chars(&format!("{name} {}", trim_chars(body, 60)), 90))
}

fn edit_from_json(body: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body.trim()).ok()?;
    let path = ["file_path", "path", "target_file", "file"]
        .iter()
        .find_map(|k| v.get(*k).and_then(|x| x.as_str()))?;
    let old = v.get("old_string").and_then(|x| x.as_str());
    let new = v
        .get("new_string")
        .or_else(|| v.get("contents"))
        .or_else(|| v.get("content"))
        .and_then(|x| x.as_str());
    let minus = old.map(line_count).unwrap_or(0);
    let plus = new.map(line_count).unwrap_or(0);
    Some(format_edit(path, plus, minus))
}

fn edit_from_patch(body: &str) -> Option<String> {
    if !(body.contains("Begin Patch") || body.contains("*** Update File:")) {
        return None;
    }
    let mut path = None;
    let mut plus = 0usize;
    let mut minus = 0usize;
    for line in body.lines() {
        let t = line.trim().trim_matches(|c| c == '"' || c == '\'');
        if let Some(p) = t
            .strip_prefix("*** Update File:")
            .or_else(|| t.strip_prefix("*** Add File:"))
        {
            if path.is_none() {
                path = Some(p.trim().to_string());
            }
        } else if t.starts_with('+') && !t.starts_with("+++") {
            plus += 1;
        } else if t.starts_with('-') && !t.starts_with("---") {
            minus += 1;
        }
    }
    let path = path?;
    Some(format_edit(&path, plus, minus))
}

fn format_edit(path: &str, plus: usize, minus: usize) -> String {
    let p = short_path(path);
    if plus > 0 || minus > 0 {
        format!("edit {p} (+{plus}/−{minus})")
    } else {
        format!("edit {p}")
    }
}

fn json_cmd(body: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body.trim()).ok()?;
    for key in ["command", "cmd"] {
        if let Some(s) = v.get(key).and_then(|c| c.as_str()) {
            if !s.is_empty() {
                return Some(s.to_string());
            }
        }
        if let Some(arr) = v.get(key).and_then(|c| c.as_array()) {
            let parts: Vec<&str> = arr.iter().filter_map(|x| x.as_str()).collect();
            if !parts.is_empty() {
                return Some(parts.join(" "));
            }
        }
    }
    None
}

fn is_readonly_shell(cmd: &str) -> bool {
    let c = cmd
        .rsplit("&&")
        .next()
        .unwrap_or(cmd)
        .trim()
        .trim_start_matches("sudo ");
    let first = c.split_whitespace().next().unwrap_or("");
    matches!(
        first,
        "cat" | "sed" | "rg" | "grep" | "head" | "tail" | "less" | "ls" | "find" | "echo" | "wc"
    ) || c.starts_with("sed -n")
}

fn is_injected(s: &str) -> bool {
    let t = s.trim();
    t.starts_with('<')
        || t.starts_with("# AGENTS.md")
        || t.contains("environment_context")
        || t.starts_with("You are Codex")
        || t.starts_with("You are Grok")
}

fn short_path(p: &str) -> String {
    let p = p.trim();
    if let Some(i) = p.find("/src/") {
        return format!("src/{}", &p[i + 5..]);
    }
    if let Some(i) = p.rfind('/') {
        return p[i + 1..].to_string();
    }
    trim_chars(p, 60)
}

fn pack_from_end(blocks: &[Block], budget_chars: usize) -> (String, usize, usize) {
    if blocks.is_empty() {
        return ("(empty)".into(), 0, 0);
    }
    let mut start = blocks.len();
    let mut used = 0usize;
    while start > 0 {
        let i = start - 1;
        let cost = blocks[i].text.len() + 2;
        if used + cost > budget_chars {
            if start == blocks.len() {
                return (
                    trim_chars(&blocks[i].text, budget_chars),
                    blocks.len().saturating_sub(1),
                    1,
                );
            }
            break;
        }
        used += cost;
        start = i;
    }
    let selected = &blocks[start..];
    let body = selected
        .iter()
        .map(|b| b.text.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    (body, start, selected.len())
}

fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn line_count(s: &str) -> usize {
    s.lines().filter(|l| !l.trim().is_empty()).count().max(1)
}

fn trim_chars(s: &str, max: usize) -> String {
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
    use crate::catalog::default_model;
    use crate::model::{SessionSummary, Warning};
    use chrono::Utc;
    use std::path::PathBuf;

    fn sample_tx() -> Transcript {
        Transcript {
            summary: SessionSummary {
                tool: Tool::Codex,
                id: "abc".into(),
                title: "auth".into(),
                cwd: PathBuf::from("/tmp/demo"),
                path: PathBuf::from("/tmp/x"),
                updated_at: Utc::now(),
                branch: Some("main".into()),
            },
            turns: vec![
                Turn {
                    role: TurnRole::User,
                    text: "fix auth in src/auth.ts".into(),
                    tool_name: None,
                },
                Turn {
                    role: TurnRole::Assistant,
                    text: "Looking at src/auth.ts".into(),
                    tool_name: None,
                },
                Turn {
                    role: TurnRole::Tool,
                    text: "called exec: cat src/auth.ts".into(),
                    tool_name: Some("exec".into()),
                },
                Turn {
                    role: TurnRole::Tool,
                    text: "result: lots of file contents here".into(),
                    tool_name: None,
                },
                Turn {
                    role: TurnRole::Tool,
                    text: "called exec: edit src/auth.ts (+3/−1)".into(),
                    tool_name: Some("exec".into()),
                },
                Turn {
                    role: TurnRole::Tool,
                    text: "result: ok".into(),
                    tool_name: None,
                },
                Turn {
                    role: TurnRole::Tool,
                    text: "called exec: npm run build".into(),
                    tool_name: Some("exec".into()),
                },
                Turn {
                    role: TurnRole::Tool,
                    text: "result: ok".into(),
                    tool_name: None,
                },
                Turn {
                    role: TurnRole::User,
                    text: "also add tests".into(),
                    tool_name: None,
                },
            ],
            warnings: vec![Warning {
                code: "compaction".into(),
                message: "gap".into(),
            }],
            last_user_request: Some("also add tests".into()),
            files_mentioned: vec!["src/auth.ts".into()],
        }
    }

    #[test]
    fn packs_recent_context_simply() {
        let h = build(
            Tool::Codex,
            Tool::Grok,
            &sample_tx(),
            HandoffOptions::for_model(default_model(Tool::Grok)),
        );
        assert!(h.markdown.contains("# Handoff from Codex"));
        assert!(h.markdown.contains("Prior context"));
        assert!(h.markdown.contains("user\nfix auth"));
        assert!(h.markdown.contains("also add tests"));
        assert!(h.markdown.contains("edit src/auth.ts"));
        assert!(h.markdown.contains("npm run build"));
        assert!(!h.markdown.contains("cat src/auth.ts"));
        assert!(!h.markdown.contains("## Your job"));
        assert!(!h.markdown.contains("last_ask:"));
        assert!(!h.markdown.contains("vague resume"));
    }

    #[test]
    fn shrinks_search_replace_json() {
        let action = shrink_tool(
            "search_replace",
            r#"called search_replace: {"file_path":"/tmp/demo/src/auth.ts","old_string":"a\nb\n","new_string":"a\nb\nc\nd\n"}"#,
        );
        assert_eq!(action.as_deref(), Some("edit src/auth.ts (+4/−2)"));
    }

    #[test]
    fn keeps_adapter_edit_summary_for_apply_patch() {
        let action = shrink_tool("apply_patch", "called apply_patch: edit Arena.ts (+12/−4)");
        assert_eq!(action.as_deref(), Some("edit Arena.ts (+12/−4)"));
    }
}
