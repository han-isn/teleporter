use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tool {
    Codex,
    Grok,
    Claude,
}

impl Tool {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Grok => "grok",
            Self::Claude => "claude",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Grok => "Grok",
            Self::Claude => "Claude Code",
        }
    }

    pub fn binary_name(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Grok => "grok",
            Self::Claude => "claude",
        }
    }

    pub fn all() -> [Self; 3] {
        [Self::Codex, Self::Grok, Self::Claude]
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "codex" => Some(Self::Codex),
            "grok" => Some(Self::Grok),
            "claude" | "claude-code" => Some(Self::Claude),
            _ => None,
        }
    }
}

impl std::fmt::Display for Tool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Tool {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s).ok_or_else(|| format!("unknown tool `{s}` (expected: codex, grok, claude)"))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub tool: Tool,
    pub id: String,
    pub title: String,
    pub cwd: PathBuf,
    pub path: PathBuf,
    pub updated_at: DateTime<Utc>,
    pub branch: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnRole {
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Turn {
    pub role: TurnRole,
    pub text: String,
    /// For tool turns: tool name if known.
    pub tool_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Warning {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transcript {
    pub summary: SessionSummary,
    pub turns: Vec<Turn>,
    pub warnings: Vec<Warning>,
    pub last_user_request: Option<String>,
    pub files_mentioned: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Handoff {
    pub from: Tool,
    pub to: Tool,
    pub markdown: String,
    pub source_id: String,
    /// Short source session title for labels / continue line.
    pub title: Option<String>,
    pub cwd: PathBuf,
    /// Target model id for launch (`-m` / `--model`).
    pub model: Option<String>,
}
