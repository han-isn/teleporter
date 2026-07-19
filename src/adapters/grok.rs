use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde_json::Value;
use walkdir::WalkDir;

use super::{Adapter, extract_paths, path_matches_cwd, resolve_home, truncate_title};
use crate::model::{SessionSummary, Tool, Transcript, Turn, TurnRole, Warning};

#[derive(Default)]
pub struct GrokAdapter {
    home_override: Option<PathBuf>,
}

impl GrokAdapter {
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
        resolve_home("GROK_HOME", ".grok")
    }

    fn sessions_root(&self) -> Result<PathBuf> {
        Ok(self.home()?.join("sessions"))
    }
}

impl Adapter for GrokAdapter {
    fn list(&self, cwd: &Path) -> Result<Vec<SessionSummary>> {
        let root = self.sessions_root()?;
        if !root.exists() {
            return Ok(Vec::new());
        }

        let mut sessions = Vec::new();
        for entry in WalkDir::new(&root)
            .follow_links(false)
            .max_depth(3)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if !entry.file_type().is_dir() {
                continue;
            }
            let dir = entry.path();
            let summary_path = dir.join("summary.json");
            if !summary_path.is_file() {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&summary_path) else {
                continue;
            };
            let Ok(v) = serde_json::from_str::<Value>(&text) else {
                continue;
            };

            let session_cwd = extract_grok_cwd(&v, dir);
            if !path_matches_cwd(&session_cwd, cwd) {
                // Also try URL-decoded parent folder name
                if !parent_encoded_matches(dir, cwd) {
                    continue;
                }
            }

            let id = v
                .pointer("/info/sessionId")
                .or_else(|| v.pointer("/info/session_id"))
                .or_else(|| v.get("sessionId"))
                .or_else(|| v.get("session_id"))
                .and_then(|x| x.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| {
                    dir.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "unknown".into())
                });

            let title = v
                .get("generated_title")
                .or_else(|| v.get("session_summary"))
                .or_else(|| v.pointer("/session_summary"))
                .and_then(|x| x.as_str())
                .map(|s| truncate_title(s, 120))
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| id.clone());

            let updated_at = parse_grok_time(&v).unwrap_or_else(Utc::now);

            sessions.push(SessionSummary {
                tool: Tool::Grok,
                id,
                title,
                cwd: if path_matches_cwd(&session_cwd, cwd) {
                    session_cwd
                } else {
                    cwd.to_path_buf()
                },
                path: dir.to_path_buf(),
                updated_at,
                branch: None,
            });
        }

        Ok(super::common::finalize_sessions(sessions))
    }

    fn show(&self, cwd: &Path, reference: &str) -> Result<Transcript> {
        let sessions = self.list(cwd)?;
        let dir = super::common::resolve_session_path("Grok", cwd, reference, &sessions, |p| {
            p.join("updates.jsonl").is_file() || p.join("summary.json").is_file()
        })?;
        parse_grok_session(&dir)
    }
}

fn parent_encoded_matches(session_dir: &Path, cwd: &Path) -> bool {
    let Some(parent) = session_dir.parent() else {
        return false;
    };
    let Some(name) = parent.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let decoded = urlencoding_decode(name);
    Path::new(&decoded) == cwd
        || std::fs::canonicalize(Path::new(&decoded))
            .ok()
            .and_then(|a| std::fs::canonicalize(cwd).ok().map(|b| a == b))
            .unwrap_or(false)
}

fn urlencoding_decode(s: &str) -> String {
    let mut out = String::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (from_hex(bytes[i + 1]), from_hex(bytes[i + 2])) {
                out.push((h << 4 | l) as char);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

fn from_hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn extract_grok_cwd(v: &Value, dir: &Path) -> PathBuf {
    if let Some(c) = v
        .pointer("/info/cwd")
        .or_else(|| v.get("cwd"))
        .and_then(|x| x.as_str())
    {
        return PathBuf::from(c);
    }
    if let Some(parent) = dir.parent() {
        if let Some(name) = parent.file_name().and_then(|n| n.to_str()) {
            return PathBuf::from(urlencoding_decode(name));
        }
    }
    PathBuf::from(".")
}

fn parse_grok_time(v: &Value) -> Option<DateTime<Utc>> {
    for key in [
        "/updated_at",
        "/updatedAt",
        "/info/updated_at",
        "/created_at",
        "/createdAt",
    ] {
        if let Some(s) = v.pointer(key).and_then(|x| x.as_str()) {
            if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
                return Some(dt.with_timezone(&Utc));
            }
        }
        if let Some(n) = v.pointer(key).and_then(|x| x.as_i64()) {
            // ms or secs
            let secs = if n > 10_000_000_000 { n / 1000 } else { n };
            return DateTime::from_timestamp(secs, 0);
        }
    }
    None
}

fn parse_grok_session(dir: &Path) -> Result<Transcript> {
    let summary_path = dir.join("summary.json");
    let summary_val = if summary_path.is_file() {
        let text = std::fs::read_to_string(&summary_path)?;
        serde_json::from_str::<Value>(&text).unwrap_or(Value::Null)
    } else {
        Value::Null
    };

    let id = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "unknown".into());
    let cwd = extract_grok_cwd(&summary_val, dir);
    let title = summary_val
        .get("generated_title")
        .or_else(|| summary_val.get("session_summary"))
        .and_then(|x| x.as_str())
        .map(|s| truncate_title(s, 120))
        .unwrap_or_else(|| id.clone());
    let updated_at = parse_grok_time(&summary_val).unwrap_or_else(Utc::now);

    let mut turns = Vec::new();
    let mut files = Vec::new();
    let mut warnings = Vec::new();

    // Prefer chat_history.jsonl if present; else updates.jsonl
    let chat_path = dir.join("chat_history.jsonl");
    let updates_path = dir.join("updates.jsonl");

    if chat_path.is_file() {
        parse_chat_history(&chat_path, &mut turns, &mut files)?;
    } else if updates_path.is_file() {
        parse_updates(&updates_path, &mut turns, &mut files, &mut warnings)?;
    } else {
        bail!("no transcript in {}", dir.display());
    }

    let last_user_request = super::common::derive_last_user(&turns);
    let title = super::common::derive_title(&turns, &title);
    files.retain(|p| super::common::is_useful_file_mention(p));
    files.truncate(20);

    Ok(Transcript {
        summary: SessionSummary {
            tool: Tool::Grok,
            id,
            title,
            cwd,
            path: dir.to_path_buf(),
            updated_at,
            branch: None,
        },
        turns,
        warnings,
        last_user_request,
        files_mentioned: files,
    })
}

fn parse_chat_history(path: &Path, turns: &mut Vec<Turn>, files: &mut Vec<String>) -> Result<()> {
    let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    for line in BufReader::new(file).lines() {
        let line = line?;
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        // Grok uses `type`; older fixtures / other exporters may use `role`.
        let kind = v
            .get("type")
            .or_else(|| v.get("role"))
            .or_else(|| v.pointer("/message/role"))
            .and_then(|r| r.as_str())
            .unwrap_or("");

        // Skip synthetic wrappers (skills dump, compaction meta, etc.).
        if kind == "user" {
            if let Some(reason) = v.get("synthetic_reason").and_then(|r| r.as_str()) {
                if reason != "none" && !reason.is_empty() {
                    // Keep only if a real <user_query> is embedded.
                    let raw = v
                        .get("content")
                        .and_then(content_to_text)
                        .unwrap_or_default();
                    if !raw.contains("<user_query>") {
                        continue;
                    }
                }
            }
        }

        let mut text = v
            .get("content")
            .and_then(content_to_text)
            .or_else(|| v.pointer("/message/content").and_then(content_to_text))
            .unwrap_or_default();

        // Prefer assistant text; if empty, skip (tool_calls-only turns handled below).
        let turn_role = match kind {
            "user" => TurnRole::User,
            "assistant" => TurnRole::Assistant,
            "tool" | "tool_result" => TurnRole::Tool,
            "system" | "developer" | "reasoning" => continue,
            _ => continue,
        };

        if kind == "tool_result" && !text.is_empty() && !text.starts_with("result:") {
            text = format!("result: {text}");
        }

        if turn_role == TurnRole::Assistant {
            if let Some(calls) = v.get("tool_calls").and_then(|c| c.as_array()) {
                if !text.trim().is_empty() {
                    push_turn(turns, files, TurnRole::Assistant, text, None);
                }
                for call in calls {
                    let name = call
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("tool");
                    let args = call
                        .get("arguments")
                        .and_then(|a| a.as_str())
                        .unwrap_or("");
                    let line = format!("called {name}: {args}");
                    push_turn(turns, files, TurnRole::Tool, line, Some(name.to_string()));
                }
                continue;
            }
        }

        if text.trim().is_empty() {
            continue;
        }
        push_turn(
            turns,
            files,
            turn_role,
            text,
            v.get("name")
                .and_then(|n| n.as_str())
                .map(|s| s.to_string()),
        );
    }
    Ok(())
}

fn push_turn(
    turns: &mut Vec<Turn>,
    files: &mut Vec<String>,
    role: TurnRole,
    text: String,
    tool_name: Option<String>,
) {
    for p in extract_paths(&text) {
        if !files.contains(&p) {
            files.push(p);
        }
    }
    turns.push(Turn {
        role,
        text,
        tool_name,
    });
}

fn parse_updates(
    path: &Path,
    turns: &mut Vec<Turn>,
    files: &mut Vec<String>,
    warnings: &mut Vec<Warning>,
) -> Result<()> {
    let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut user_buf = String::new();
    let mut assistant_buf = String::new();

    for line in BufReader::new(file).lines() {
        let line = line?;
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let update = v
            .pointer("/params/update")
            .cloned()
            .unwrap_or(Value::Null);
        let kind = update
            .get("sessionUpdate")
            .and_then(|k| k.as_str())
            .unwrap_or("");

        match kind {
            "user_message_chunk" => {
                if let Some(t) = update.pointer("/content/text").and_then(|x| x.as_str()) {
                    user_buf.push_str(t);
                }
            }
            "agent_message_chunk" | "assistant_message_chunk" => {
                if !user_buf.is_empty() {
                    flush_user(turns, files, &mut user_buf);
                }
                if let Some(t) = update.pointer("/content/text").and_then(|x| x.as_str()) {
                    assistant_buf.push_str(t);
                }
            }
            "tool_call" | "tool_call_update" => {
                if !user_buf.is_empty() {
                    flush_user(turns, files, &mut user_buf);
                }
                if !assistant_buf.is_empty() {
                    flush_assistant(turns, files, &mut assistant_buf);
                }
                let name = update
                    .get("title")
                    .or_else(|| update.get("toolName"))
                    .or_else(|| update.pointer("/toolCall/name"))
                    .and_then(|n| n.as_str())
                    .unwrap_or("tool");
                turns.push(Turn {
                    role: TurnRole::Tool,
                    text: format!("called {name}"),
                    tool_name: Some(name.to_string()),
                });
            }
            "session_info_update" => {}
            _ => {
                if kind.contains("thought") || kind.contains("reasoning") {
                    // skip
                }
            }
        }
    }
    if !user_buf.is_empty() {
        flush_user(turns, files, &mut user_buf);
    }
    if !assistant_buf.is_empty() {
        flush_assistant(turns, files, &mut assistant_buf);
    }
    if turns.is_empty() {
        warnings.push(Warning {
            code: "empty_updates".into(),
            message: "updates.jsonl produced no recoverable turns".into(),
        });
    }
    Ok(())
}

fn flush_user(turns: &mut Vec<Turn>, files: &mut Vec<String>, buf: &mut String) {
    let text = std::mem::take(buf);
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

fn flush_assistant(turns: &mut Vec<Turn>, files: &mut Vec<String>, buf: &mut String) {
    let text = std::mem::take(buf);
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

fn content_to_text(v: &Value) -> Option<String> {
    if let Some(s) = v.as_str() {
        return Some(s.to_string());
    }
    if let Some(arr) = v.as_array() {
        let mut parts = Vec::new();
        for item in arr {
            if let Some(t) = item.get("text").and_then(|x| x.as_str()) {
                parts.push(t);
            } else if let Some(s) = item.as_str() {
                parts.push(s);
            }
        }
        if !parts.is_empty() {
            return Some(parts.join("\n"));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn lists_and_shows_grok_session() {
        let home = tempfile::tempdir().unwrap();
        let cwd = "/tmp/grok-demo";
        let enc = cwd.replace('/', "%2F");
        let dir = home.path().join("sessions").join(&enc).join("sess-1");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("summary.json"),
            r#"{"generated_title":"Auth work","info":{"sessionId":"sess-1","cwd":"/tmp/grok-demo"},"updated_at":"2026-07-16T12:00:00Z"}"#,
        )
        .unwrap();
        let mut chat = File::create(dir.join("chat_history.jsonl")).unwrap();
        writeln!(
            chat,
            r#"{{"type":"user","content":[{{"type":"text","text":"<user_query>\nupdate src/main.rs\n</user_query>"}}]}}"#
        )
        .unwrap();
        writeln!(
            chat,
            r#"{{"type":"assistant","content":"updated src/main.rs","tool_calls":[{{"id":"1","name":"Bash","arguments":"{{\"command\":\"cargo test\"}}"}}]}}"#
        )
        .unwrap();
        writeln!(
            chat,
            r#"{{"type":"tool_result","tool_call_id":"1","content":"ok"}}"#
        )
        .unwrap();

        let adapter = GrokAdapter::with_home(home.path().to_path_buf());
        let list = adapter.list(Path::new(cwd)).unwrap();
        assert_eq!(list.len(), 1);
        let tx = adapter.show(Path::new(cwd), "sess-1").unwrap();
        assert!(tx.turns.len() >= 3);
        assert!(tx.turns.iter().any(|t| t.role == TurnRole::User));
        assert!(tx.turns.iter().any(|t| t.text.contains("cargo test")));
    }
}
