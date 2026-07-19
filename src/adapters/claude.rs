use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde_json::Value;
use walkdir::WalkDir;

use super::{Adapter, extract_paths, path_matches_cwd, resolve_home, truncate_title};
use crate::model::{SessionSummary, Tool, Transcript, Turn, TurnRole, Warning};

#[derive(Default)]
pub struct ClaudeAdapter {
    home_override: Option<PathBuf>,
}

impl ClaudeAdapter {
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
        resolve_home("CLAUDE_CONFIG_DIR", ".claude")
    }

    fn projects_root(&self) -> Result<PathBuf> {
        Ok(self.home()?.join("projects"))
    }

    fn project_dir_for_cwd(projects: &Path, cwd: &Path) -> PathBuf {
        // Claude encodes cwd as -Users-foo-bar
        let encoded = encode_claude_project(cwd);
        projects.join(encoded)
    }
}

fn encode_claude_project(cwd: &Path) -> String {
    let s = cwd.to_string_lossy();
    let mut out = String::new();
    for ch in s.chars() {
        if ch == '/' {
            out.push('-');
        } else {
            out.push(ch);
        }
    }
    out
}

impl Adapter for ClaudeAdapter {
    fn list(&self, cwd: &Path) -> Result<Vec<SessionSummary>> {
        let projects = self.projects_root()?;
        let project_dir = Self::project_dir_for_cwd(&projects, cwd);
        if !project_dir.exists() {
            return Ok(Vec::new());
        }

        let mut sessions = Vec::new();
        for entry in WalkDir::new(&project_dir)
            .follow_links(false)
            .max_depth(2)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            // Skip subagent side files under subagents/
            if path
                .components()
                .any(|c| c.as_os_str() == "subagents")
            {
                continue;
            }

            let id = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "unknown".into());

            let Ok(head) = read_claude_head(path) else {
                continue;
            };
            if let Some(ref scwd) = head.cwd {
                if !path_matches_cwd(scwd, cwd) {
                    continue;
                }
            }

            let updated = std::fs::metadata(path)
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| {
                    t.duration_since(std::time::SystemTime::UNIX_EPOCH)
                        .ok()
                        .and_then(|d| DateTime::from_timestamp(d.as_secs() as i64, d.subsec_nanos()))
                })
                .unwrap_or_else(Utc::now);

            let title = head
                .first_user
                .map(|s| truncate_title(&s, 120))
                .unwrap_or_else(|| id.clone());

            sessions.push(SessionSummary {
                tool: Tool::Claude,
                id,
                title,
                cwd: head.cwd.unwrap_or_else(|| cwd.to_path_buf()),
                path: path.to_path_buf(),
                updated_at: updated,
                branch: head.branch,
            });
        }

        Ok(super::common::finalize_sessions(sessions))
    }

    fn show(&self, cwd: &Path, reference: &str) -> Result<Transcript> {
        let sessions = self.list(cwd)?;
        let path = super::common::resolve_session_path(
            "Claude",
            cwd,
            reference,
            &sessions,
            |p| p.is_file(),
        )?;
        parse_claude_jsonl(&path)
    }
}

struct ClaudeHead {
    cwd: Option<PathBuf>,
    first_user: Option<String>,
    branch: Option<String>,
}

fn read_claude_head(path: &Path) -> Result<ClaudeHead> {
    let file = File::open(path)?;
    let mut cwd = None;
    let mut first_user = None;
    let mut branch = None;
    for (i, line) in BufReader::new(file).lines().enumerate() {
        if i > 40 {
            break;
        }
        let line = line?;
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if cwd.is_none() {
            if let Some(c) = v.get("cwd").and_then(|x| x.as_str()) {
                cwd = Some(PathBuf::from(c));
            }
        }
        if branch.is_none() {
            if let Some(b) = v.get("gitBranch").and_then(|x| x.as_str()) {
                if !b.is_empty() {
                    branch = Some(b.to_string());
                }
            }
        }
        if first_user.is_none() {
            let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
            if ty == "user" {
                if let Some(text) = claude_message_text(&v) {
                    if text != "Warmup" {
                        first_user = Some(text);
                    }
                }
            }
        }
    }
    Ok(ClaudeHead {
        cwd,
        first_user,
        branch,
    })
}

fn parse_claude_jsonl(path: &Path) -> Result<Transcript> {
    let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let id = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "unknown".into());

    let mut cwd = PathBuf::from(".");
    let mut branch = None;
    let mut turns = Vec::new();
    let mut files = Vec::new();
    let mut warnings = Vec::new();
    let mut skipped = 0usize;
    let mut updated_at = Utc::now();
    let mut session_id = id.clone();

    for line in BufReader::new(file).lines() {
        let line = line?;
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if let Some(ts) = v.get("timestamp").and_then(|t| t.as_str()) {
            if let Ok(dt) = DateTime::parse_from_rfc3339(ts) {
                updated_at = dt.with_timezone(&Utc);
            }
        }
        if let Some(c) = v.get("cwd").and_then(|x| x.as_str()) {
            cwd = PathBuf::from(c);
        }
        if let Some(sid) = v.get("sessionId").and_then(|x| x.as_str()) {
            session_id = sid.to_string();
        }
        if let Some(b) = v.get("gitBranch").and_then(|x| x.as_str()) {
            if !b.is_empty() {
                branch = Some(b.to_string());
            }
        }

        let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        match ty {
            "user" => {
                if let Some(text) = claude_message_text(&v) {
                    if text == "Warmup" {
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
                if let Some(blocks) = v.pointer("/message/content").and_then(|c| c.as_array()) {
                    for block in blocks {
                        let bty = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
                        match bty {
                            "text" => {
                                if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                                    for p in extract_paths(text) {
                                        if !files.contains(&p) {
                                            files.push(p);
                                        }
                                    }
                                    turns.push(Turn {
                                        role: TurnRole::Assistant,
                                        text: text.to_string(),
                                        tool_name: None,
                                    });
                                }
                            }
                            "tool_use" => {
                                let name = block
                                    .get("name")
                                    .and_then(|n| n.as_str())
                                    .unwrap_or("tool");
                                let input = block
                                    .get("input")
                                    .map(|i| truncate_title(&i.to_string(), 120))
                                    .unwrap_or_default();
                                turns.push(Turn {
                                    role: TurnRole::Tool,
                                    text: if input.is_empty() {
                                        format!("called {name}")
                                    } else {
                                        format!("called {name}: {input}")
                                    },
                                    tool_name: Some(name.to_string()),
                                });
                            }
                            "thinking" => {
                                skipped += 1;
                            }
                            _ => {}
                        }
                    }
                }
            }
            "system" | "summary" | "progress" | "file-history-snapshot" => {
                skipped += 1;
            }
            _ => {}
        }
    }

    if skipped > 0 {
        warnings.push(Warning {
            code: "records_skipped".into(),
            message: format!("Skipped {skipped} system/thinking/meta records"),
        });
    }

    let last_user_request = super::common::derive_last_user(&turns);
    let title = turns
        .iter()
        .find(|t| t.role == TurnRole::User)
        .map(|t| truncate_title(&t.text, 120))
        .unwrap_or_else(|| session_id.clone());

    files.truncate(20);

    Ok(Transcript {
        summary: SessionSummary {
            tool: Tool::Claude,
            id: session_id,
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

fn claude_message_text(v: &Value) -> Option<String> {
    let content = v.pointer("/message/content")?;
    if let Some(s) = content.as_str() {
        return Some(s.to_string());
    }
    if let Some(arr) = content.as_array() {
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
    fn parses_claude_session() {
        let home = tempfile::tempdir().unwrap();
        let cwd = Path::new("/tmp/claude-demo");
        let project = home
            .path()
            .join("projects")
            .join(encode_claude_project(cwd));
        std::fs::create_dir_all(&project).unwrap();
        let id = "30d2f5c6-0852-4740-a8ef-3d7ffcc3e0ed";
        let path = project.join(format!("{id}.jsonl"));
        let mut f = File::create(&path).unwrap();
        writeln!(
            f,
            r#"{{"type":"user","cwd":"/tmp/claude-demo","sessionId":"{id}","gitBranch":"main","message":{{"role":"user","content":"refactor src/app.ts"}},"timestamp":"2026-07-16T12:00:00Z"}}"#
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","cwd":"/tmp/claude-demo","sessionId":"{id}","message":{{"role":"assistant","content":[{{"type":"text","text":"Looking at src/app.ts"}}]}},"timestamp":"2026-07-16T12:00:01Z"}}"#
        )
        .unwrap();

        let adapter = ClaudeAdapter::with_home(home.path().to_path_buf());
        let list = adapter.list(cwd).unwrap();
        assert_eq!(list.len(), 1);
        let tx = adapter.show(cwd, id).unwrap();
        assert!(!tx.turns.is_empty());
    }
}
