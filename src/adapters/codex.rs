use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use regex::Regex;
use serde_json::Value;
use walkdir::WalkDir;

use super::{Adapter, extract_paths, path_matches_cwd, resolve_home, truncate_title};
use crate::model::{SessionSummary, Tool, Transcript, Turn, TurnRole, Warning};

const ROLLOUT_RE: &str = r"^rollout-\d{4}-\d{2}-\d{2}T\d{2}-\d{2}-\d{2}-([0-9a-fA-F-]{36})\.jsonl(?:\.zst)?$";

#[derive(Default)]
pub struct CodexAdapter {
    home_override: Option<PathBuf>,
}

impl CodexAdapter {
    #[allow(dead_code)]
    pub fn with_home(home: PathBuf) -> Self {
        Self {
            home_override: Some(home),
        }
    }

    fn home(&self) -> Result<PathBuf> {
        if let Some(h) = &self.home_override {
            return Ok(h.clone());
        }
        resolve_home("CODEX_HOME", ".codex")
    }

    fn sessions_root(&self) -> Result<PathBuf> {
        Ok(self.home()?.join("sessions"))
    }

    fn rollout_id(path: &Path) -> Option<String> {
        let re = Regex::new(ROLLOUT_RE).ok()?;
        let name = path.file_name()?.to_str()?;
        re.captures(name)
            .and_then(|c| c.get(1).map(|m| m.as_str().to_string()))
    }
}

impl Adapter for CodexAdapter {
    fn list(&self, cwd: &Path) -> Result<Vec<SessionSummary>> {
        let root = self.sessions_root()?;
        if !root.exists() {
            return Ok(Vec::new());
        }

        let mut sessions = Vec::new();
        for entry in WalkDir::new(&root)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            let Some(id) = Self::rollout_id(path) else {
                continue;
            };
            if path.extension().and_then(|e| e.to_str()) == Some("zst") {
                continue; // compressed: skip for v1 listing head-read simplicity
            }
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            let updated = meta
                .modified()
                .ok()
                .and_then(system_time_to_utc)
                .unwrap_or_else(Utc::now);

            match read_rollout_head(path, 24) {
                Ok(head) => {
                    if !path_matches_cwd(&head.cwd, cwd) {
                        continue;
                    }
                    let title = head
                        .first_user
                        .as_deref()
                        .map(|s| {
                            let facing = super::common::extract_user_facing(s);
                            if facing.is_empty() || super::common::is_noise_title(&facing) {
                                truncate_title(s, 120)
                            } else {
                                truncate_title(&facing, 120)
                            }
                        })
                        .unwrap_or_else(|| id.clone());
                    sessions.push(SessionSummary {
                        tool: Tool::Codex,
                        id,
                        title,
                        cwd: head.cwd,
                        path: path.to_path_buf(),
                        updated_at: updated,
                        branch: head.branch,
                    });
                }
                Err(_) => continue,
            }
        }

        Ok(super::common::finalize_sessions(sessions))
    }

    fn show(&self, cwd: &Path, reference: &str) -> Result<Transcript> {
        let sessions = self.list(cwd)?;
        let path = super::common::resolve_session_path(
            "Codex",
            cwd,
            reference,
            &sessions,
            |p| p.is_file(),
        )?;
        parse_rollout(&path)
    }
}

struct HeadMeta {
    cwd: PathBuf,
    first_user: Option<String>,
    branch: Option<String>,
}

fn read_rollout_head(path: &Path, max_lines: usize) -> Result<HeadMeta> {
    let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let reader = BufReader::new(file);
    let mut cwd = None;
    let mut first_user = None;
    let mut branch = None;

    for (i, line) in reader.lines().enumerate() {
        if i >= max_lines && cwd.is_some() && first_user.is_some() {
            break;
        }
        let line = line?;
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let payload = v.get("payload").cloned().unwrap_or(Value::Null);

        if ty == "session_meta" {
            if let Some(c) = payload.get("cwd").and_then(|x| x.as_str()) {
                cwd = Some(PathBuf::from(c));
            }
            if let Some(b) = payload
                .get("git")
                .and_then(|g| g.get("branch"))
                .and_then(|b| b.as_str())
            {
                branch = Some(b.to_string());
            }
        }

        if first_user.is_none() {
            if let Some(text) = extract_user_text(&v, &payload) {
                if !text.trim().is_empty() && !looks_like_injected_context(&text) {
                    first_user = Some(text);
                }
            }
        }
    }

    Ok(HeadMeta {
        cwd: cwd.unwrap_or_else(|| PathBuf::from("/")),
        first_user,
        branch,
    })
}

fn parse_rollout(path: &Path) -> Result<Transcript> {
    let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let reader = BufReader::new(file);

    let id = CodexAdapter::rollout_id(path).unwrap_or_else(|| {
        path.file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "unknown".into())
    });

    let mut cwd = PathBuf::from(".");
    let mut branch = None;
    let mut turns = Vec::new();
    let mut warnings = Vec::new();
    let mut skipped_unsafe = 0usize;
    let mut files = Vec::new();
    let mut updated_at = file_mtime_utc(path);
    let mut created_hint: Option<DateTime<Utc>> = None;

    for line in reader.lines() {
        let line = line?;
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if let Some(ts) = v.get("timestamp").and_then(|t| t.as_str()) {
            if let Ok(dt) = DateTime::parse_from_rfc3339(ts) {
                updated_at = dt.with_timezone(&Utc);
                if created_hint.is_none() {
                    created_hint = Some(updated_at);
                }
            }
        }

        let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let payload = v.get("payload").cloned().unwrap_or(Value::Null);

        match ty {
            "session_meta" => {
                if let Some(c) = payload.get("cwd").and_then(|x| x.as_str()) {
                    cwd = PathBuf::from(c);
                }
                if let Some(b) = payload
                    .get("git")
                    .and_then(|g| g.get("branch"))
                    .and_then(|b| b.as_str())
                {
                    branch = Some(b.to_string());
                }
            }
            "response_item" => {
                let item_ty = payload.get("type").and_then(|t| t.as_str()).unwrap_or("");
                match item_ty {
                    "message" => {
                        let role = payload.get("role").and_then(|r| r.as_str()).unwrap_or("");
                        match role {
                            "user" => {
                                if let Some(text) = message_text(&payload) {
                                    if looks_like_injected_context(&text) {
                                        skipped_unsafe += 1;
                                        continue;
                                    }
                                    for p in extract_paths(&text) {
                                        if !files.contains(&p) {
                                            files.push(p);
                                        }
                                    }
                                    turns.push(Turn {
                                        role: TurnRole::User,
                                        text,
                                        tool_name: None,
                                    });
                                }
                            }
                            "assistant" => {
                                if let Some(text) = message_text(&payload) {
                                    for p in extract_paths(&text) {
                                        if !files.contains(&p) {
                                            files.push(p);
                                        }
                                    }
                                    turns.push(Turn {
                                        role: TurnRole::Assistant,
                                        text,
                                        tool_name: None,
                                    });
                                }
                            }
                            "developer" | "system" => {
                                skipped_unsafe += 1;
                            }
                            _ => {}
                        }
                    }
                    "reasoning" => {
                        skipped_unsafe += 1;
                    }
                    "custom_tool_call" | "function_call" => {
                        let name = payload
                            .get("name")
                            .and_then(|n| n.as_str())
                            .unwrap_or("tool")
                            .to_string();
                        let input = payload
                            .get("input")
                            .or_else(|| payload.get("arguments"))
                            .map(|i| summarize_tool_input(&name, i))
                            .unwrap_or_default();
                        turns.push(Turn {
                            role: TurnRole::Tool,
                            text: if input.is_empty() {
                                format!("called {name}")
                            } else {
                                format!("called {name}: {input}")
                            },
                            tool_name: Some(name),
                        });
                    }
                    "custom_tool_call_output" | "function_call_output" => {
                        let out = payload
                            .get("output")
                            .map(|o| summarize_tool_output(o, 80))
                            .unwrap_or_else(|| "ok".into());
                        // Skip noisy raw dumps; keep short status only.
                        if out.contains("Begin Patch") || out.len() > 100 {
                            turns.push(Turn {
                                role: TurnRole::Tool,
                                text: "result: ok".into(),
                                tool_name: None,
                            });
                        } else {
                            turns.push(Turn {
                                role: TurnRole::Tool,
                                text: format!("result: {out}"),
                                tool_name: None,
                            });
                        }
                    }
                    _ => {}
                }
            }
            "event_msg" => {
                // Prefer response_item messages; event_msg often mirrors them.
            }
            "compacted" => {
                if !warnings
                    .iter()
                    .any(|w: &Warning| w.code == "compaction")
                {
                    warnings.push(Warning {
                        code: "compaction".into(),
                        message: "session was compacted; earlier turns may be missing".into(),
                    });
                }
            }
            "turn_context" | "world_state" => {}
            _ => {}
        }
    }

    if skipped_unsafe > 0 {
        warnings.push(Warning {
            code: "unsafe_records_skipped".into(),
            message: format!(
                "Skipped {skipped_unsafe} instruction/reasoning/system records (inert policy)"
            ),
        });
    }

    let last_user_request = super::common::derive_last_user(&turns);
    let title = super::common::derive_title(&turns, &id);
    files.retain(|p| super::common::is_useful_file_mention(p));
    files.truncate(20);

    Ok(Transcript {
        summary: SessionSummary {
            tool: Tool::Codex,
            id,
            title,
            cwd,
            path: path.to_path_buf(),
            updated_at,
            branch,
        },
        turns,
        warnings,
        last_user_request,
        files_mentioned: files,
    })
}

fn extract_user_text(v: &Value, payload: &Value) -> Option<String> {
    let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    if ty == "response_item" {
        let item_ty = payload.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let role = payload.get("role").and_then(|r| r.as_str()).unwrap_or("");
        if item_ty == "message" && role == "user" {
            return message_text(payload);
        }
    }
    if ty == "event_msg" {
        let et = payload.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if et == "user_message" {
            return payload
                .get("message")
                .and_then(|m| m.as_str())
                .map(|s| s.to_string());
        }
    }
    None
}

fn message_text(payload: &Value) -> Option<String> {
    let content = payload.get("content")?;
    if let Some(arr) = content.as_array() {
        let mut parts = Vec::new();
        for item in arr {
            let t = item.get("type").and_then(|x| x.as_str()).unwrap_or("");
            if matches!(t, "input_text" | "output_text" | "text") {
                if let Some(text) = item.get("text").and_then(|x| x.as_str()) {
                    parts.push(text);
                }
            }
        }
        if !parts.is_empty() {
            return Some(parts.join("\n"));
        }
    }
    content.as_str().map(|s| s.to_string())
}

fn looks_like_injected_context(text: &str) -> bool {
    let t = text.trim_start();
    t.starts_with('<')
        || t.starts_with("# AGENTS.md")
        || t.starts_with("You are `/root`")
        || t.starts_with("You are Codex")
        || t.contains("environment_context")
        || super::common::is_noise_user_text(t)
}

fn summarize_tool_input(name: &str, v: &Value) -> String {
    let s = match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    // Codex often embeds apply_patch inside JS strings with `\n` escapes.
    if let Some(edit) = summarize_embedded_patch(&s) {
        return edit;
    }
    // Codex often wraps shell as JS: exec_command({cmd:"..."
    if let Some(cmd) = extract_cmd_literal(&s) {
        return truncate_chars(&cmd, 100);
    }
    let n = name.to_ascii_lowercase();
    if matches!(
        n.as_str(),
        "exec" | "shell" | "bash" | "shell_command" | "exec_command"
    ) {
        if let Ok(obj) = serde_json::from_str::<Value>(&s) {
            if let Some(cmd) = command_from_json(&obj) {
                return truncate_chars(&cmd, 100);
            }
        }
        return truncate_chars(&s, 100);
    }
    truncate_chars(&s, 100)
}

/// Decode JS-escaped apply_patch blobs → `edit path (+N/−M)`.
fn summarize_embedded_patch(s: &str) -> Option<String> {
    if !(s.contains("Begin Patch") || s.contains("Update File:") || s.contains("Add File:")) {
        return None;
    }
    let normalized = s
        .replace("\\r\\n", "\n")
        .replace("\\n", "\n")
        .replace("\\t", "\t")
        .replace("\\\"", "\"");
    let mut first_path: Option<String> = None;
    let mut file_count = 0usize;
    let mut plus = 0usize;
    let mut minus = 0usize;
    for line in normalized.lines() {
        let t = line.trim();
        let t = t.trim_matches(|c| c == '"' || c == '\'');
        if let Some(p) = t
            .strip_prefix("*** Update File:")
            .or_else(|| t.strip_prefix("*** Add File:"))
            .or_else(|| t.strip_prefix("Update File:"))
            .or_else(|| t.strip_prefix("Add File:"))
        {
            let p = p.trim().trim_matches('"').to_string();
            if !p.is_empty() {
                file_count += 1;
                if first_path.is_none() {
                    first_path = Some(p);
                }
            }
            continue;
        }
        if t.starts_with('+') && !t.starts_with("+++") {
            plus += 1;
        } else if t.starts_with('-') && !t.starts_with("---") {
            minus += 1;
        }
    }
    let path = first_path?;
    let short = if let Some(i) = path.find("/src/") {
        format!("src/{}", &path[i + 5..])
    } else if let Some(i) = path.rfind('/') {
        path[i + 1..].to_string()
    } else {
        path
    };
    let mut out = if plus > 0 || minus > 0 {
        format!("edit {short} (+{plus}/−{minus})")
    } else {
        format!("edit {short}")
    };
    if file_count > 1 {
        out.push_str(&format!(" · {file_count} files"));
    }
    Some(truncate_chars(&out, 120))
}

fn command_from_json(obj: &Value) -> Option<String> {
    for key in ["command", "cmd"] {
        if let Some(s) = obj.get(key).and_then(|c| c.as_str()) {
            if !s.is_empty() {
                return Some(s.to_string());
            }
        }
        if let Some(arr) = obj.get(key).and_then(|c| c.as_array()) {
            let parts: Vec<&str> = arr.iter().filter_map(|x| x.as_str()).collect();
            if !parts.is_empty() {
                return Some(parts.join(" "));
            }
        }
    }
    None
}

fn extract_cmd_literal(s: &str) -> Option<String> {
    for key in ["cmd:\"", "cmd: \"", "\"cmd\":\""] {
        if let Some(i) = s.find(key) {
            let rest = &s[i + key.len()..];
            let mut out = String::new();
            let mut chars = rest.chars();
            while let Some(c) = chars.next() {
                if c == '\\' {
                    if let Some(n) = chars.next() {
                        out.push(n);
                    }
                    continue;
                }
                if c == '"' {
                    break;
                }
                out.push(c);
            }
            if !out.is_empty() {
                return Some(out);
            }
        }
    }
    None
}

fn summarize_tool_output(v: &Value, max: usize) -> String {
    let raw = if let Some(arr) = v.as_array() {
        let mut parts = Vec::new();
        for item in arr {
            if let Some(t) = item.get("text").and_then(|x| x.as_str()) {
                parts.push(t);
            }
        }
        parts.join(" ")
    } else if let Some(s) = v.as_str() {
        s.to_string()
    } else {
        v.to_string()
    };
    let lower = raw.to_ascii_lowercase();
    if lower.contains("wall time")
        || lower.contains("cell id")
        || lower.starts_with("script completed")
        || lower.starts_with("script running")
    {
        if lower.contains("error") || lower.contains("fail") {
            return "fail".into();
        }
        return "ok".into();
    }
    if raw.contains("Begin Patch") || raw.len() > max {
        return "ok".into();
    }
    truncate_chars(&raw, max)
}

fn truncate_chars(s: &str, max: usize) -> String {
    let s = s.replace('\n', " ");
    if s.chars().count() <= max {
        return s;
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn system_time_to_utc(t: SystemTime) -> Option<DateTime<Utc>> {
    let d = t.duration_since(SystemTime::UNIX_EPOCH).ok()?;
    DateTime::from_timestamp(d.as_secs() as i64, d.subsec_nanos())
}

fn file_mtime_utc(path: &Path) -> DateTime<Utc> {
    std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(system_time_to_utc)
        .unwrap_or_else(Utc::now)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn parses_fixture_rollout() {
        let dir = tempfile::tempdir().unwrap();
        let sessions = dir
            .path()
            .join("sessions/2026/07/16");
        std::fs::create_dir_all(&sessions).unwrap();
        let id = "019f0045-d606-7923-94af-2b615b362c83";
        let path = sessions.join(format!(
            "rollout-2026-07-16T11-36-28-{id}.jsonl"
        ));
        let mut f = File::create(&path).unwrap();
        writeln!(
            f,
            r#"{{"timestamp":"2026-07-16T11:36:28.000Z","type":"session_meta","payload":{{"id":"{id}","cwd":"/tmp/demo","source":"cli"}}}}"#
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"timestamp":"2026-07-16T11:36:29.000Z","type":"response_item","payload":{{"type":"message","role":"user","content":[{{"type":"input_text","text":"fix the login bug in src/auth.ts"}}]}}}}"#
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"timestamp":"2026-07-16T11:36:30.000Z","type":"response_item","payload":{{"type":"message","role":"assistant","content":[{{"type":"output_text","text":"I'll inspect src/auth.ts"}}]}}}}"#
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"timestamp":"2026-07-16T11:36:31.000Z","type":"response_item","payload":{{"type":"custom_tool_call","name":"exec","input":"sed -n 1,40p src/auth.ts"}}}}"#
        )
        .unwrap();

        let adapter = CodexAdapter::with_home(dir.path().to_path_buf());
        let list = adapter.list(Path::new("/tmp/demo")).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, id);

        let tx = adapter.show(Path::new("/tmp/demo"), id).unwrap();
        assert!(tx.turns.iter().any(|t| t.role == TurnRole::User));
        assert!(tx.files_mentioned.iter().any(|p| p.contains("auth.ts")));
        assert_eq!(
            tx.last_user_request.as_deref(),
            Some("fix the login bug in src/auth.ts")
        );
    }

    #[test]
    fn summarizes_js_escaped_apply_patch() {
        let input = r#"const patch = "*** Begin Patch\n*** Update File: /tmp/demo/src/auth.ts\n@@\n-old\n+new\n+line2\n*** End Patch";"#;
        let out = summarize_tool_input("exec", &Value::String(input.into()));
        assert!(out.starts_with("edit "), "{out}");
        assert!(out.contains("auth.ts"), "{out}");
        assert!(out.contains('+'), "{out}");
        assert!(!out.contains("edit files"), "{out}");
    }
}
