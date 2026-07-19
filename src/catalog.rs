//! Models + effort + (Codex-only) fast — separate axes.
//!
//! Web sources (2026-07):
//! - Codex `model_reasoning_effort`: none|minimal|low|medium|high|xhigh (+ max|ultra in protocol)
//!   Fast mode is separate (`--enable fast_mode` / `/fast`), not an effort level.
//! - Grok `grok-4.5` reasoning_effort: low|medium|high (docs.x.ai)
//! - Claude Code `--effort`: low|medium|high|xhigh|max (+ ultracode); Haiku has no effort

use crate::model::Tool;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelInfo {
    pub id: &'static str,
    pub cli_model: &'static str,
    pub label: &'static str,
    pub context_tokens: u32,
    pub effort: Option<&'static str>,
    pub enable: &'static [&'static str],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EffortChoice {
    pub id: &'static str,
    pub label: &'static str,
    /// Wire value; `None` = omit flag (CLI/model default).
    pub effort: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FastChoice {
    pub id: &'static str,
    pub label: &'static str,
    pub on: bool,
}

impl EffortChoice {
    pub const fn default_choice() -> Self {
        Self {
            id: "default",
            label: "default",
            effort: None,
        }
    }

    pub fn is_default(self) -> bool {
        self.id == "default"
    }
}

impl FastChoice {
    pub const OFF: Self = Self {
        id: "off",
        label: "standard",
        on: false,
    };
    pub const ON: Self = Self {
        id: "on",
        label: "fast",
        on: true,
    };

}

impl ModelInfo {
    pub fn handoff_budget_tokens(self) -> u32 {
        let reserve = 32_000u32;
        let cap = 200_000u32;
        self.context_tokens
            .saturating_sub(reserve)
            .min(cap)
            .max(8_000)
    }
}

pub fn models_for(tool: Tool) -> &'static [ModelInfo] {
    match tool {
        Tool::Codex => CODEX_MODELS,
        Tool::Grok => GROK_MODELS,
        Tool::Claude => CLAUDE_MODELS,
    }
}

pub fn default_model(tool: Tool) -> ModelInfo {
    models_for(tool)[0]
}

pub fn efforts_for(tool: Tool, model: ModelInfo) -> &'static [EffortChoice] {
    match tool {
        Tool::Codex => CODEX_EFFORTS,
        Tool::Grok => GROK_EFFORTS,
        Tool::Claude => {
            if model.id == "haiku" {
                CLAUDE_HAIKU_EFFORTS
            } else {
                CLAUDE_EFFORTS
            }
        }
    }
}

pub fn default_effort(tool: Tool, model: ModelInfo) -> EffortChoice {
    efforts_for(tool, model)[0]
}

/// Codex only — Fast mode is a service tier, not an effort level.
pub fn fast_options_for(tool: Tool) -> Option<&'static [FastChoice]> {
    match tool {
        Tool::Codex => Some(CODEX_FAST),
        Tool::Grok | Tool::Claude => None,
    }
}

pub fn default_fast(tool: Tool) -> FastChoice {
    fast_options_for(tool)
        .and_then(|o| o.first().copied())
        .unwrap_or(FastChoice::OFF)
}

pub fn apply_selection(base: ModelInfo, effort: EffortChoice, fast: FastChoice) -> ModelInfo {
    ModelInfo {
        effort: effort.effort,
        enable: if fast.on { &["fast_mode"] } else { &[] },
        ..base
    }
}

/// Handoff / `-m` key: `sol`, `sol-xhigh`, `sol-fast`, `sol-xhigh-fast`.
pub fn selection_key(base: ModelInfo, effort: EffortChoice, fast: FastChoice) -> String {
    let mut key = base.id.to_string();
    if !effort.is_default() {
        key.push('-');
        key.push_str(effort.id);
    }
    if fast.on {
        key.push_str("-fast");
    }
    key
}

pub fn find_effort(tool: Tool, model: ModelInfo, id: &str) -> Option<EffortChoice> {
    let id = id.trim();
    efforts_for(tool, model)
        .iter()
        .copied()
        .find(|e| e.id.eq_ignore_ascii_case(id) || e.label.eq_ignore_ascii_case(id))
}

/// Resolve `-m` key: base, `{base}-{effort}`, `{base}-fast`, `{base}-{effort}-fast`.
pub fn find_model(tool: Tool, id: &str) -> Option<ModelInfo> {
    let id = id.trim();
    if id.is_empty() {
        return None;
    }

    for m in models_for(tool) {
        if m.id.eq_ignore_ascii_case(id)
            || m.cli_model.eq_ignore_ascii_case(id)
            || m.label.eq_ignore_ascii_case(id)
        {
            return Some(*m);
        }
    }

    let lower = id.to_ascii_lowercase();
    let (body, fast_on) = if let Some(rest) = lower.strip_suffix("-fast") {
        (rest.to_string(), true)
    } else {
        (lower, false)
    };

    let mut bases: Vec<ModelInfo> = models_for(tool).to_vec();
    bases.sort_by_key(|m| std::cmp::Reverse(m.id.len()));

    for base in bases {
        let bid = base.id.to_ascii_lowercase();
        if body == bid {
            let effort = EffortChoice::default_choice();
            let fast = if fast_on {
                FastChoice::ON
            } else {
                FastChoice::OFF
            };
            return Some(apply_selection(base, effort, fast));
        }
        let prefix = format!("{bid}-");
        if let Some(rest) = body.strip_prefix(&prefix) {
            if let Some(effort) = find_effort(tool, base, rest) {
                let fast = if fast_on {
                    FastChoice::ON
                } else {
                    FastChoice::OFF
                };
                return Some(apply_selection(base, effort, fast));
            }
        }
    }

    None
}

// --- Codex ---
// Effort: https://developers.openai.com/codex + protocol ReasoningEffort
// Fast: separate --enable fast_mode / service_tier fast

const CODEX_MODELS: &[ModelInfo] = &[
    ModelInfo {
        id: "sol",
        cli_model: "gpt-5.6-sol",
        label: "Sol",
        context_tokens: 400_000,
        effort: None,
        enable: &[],
    },
    ModelInfo {
        id: "terra",
        cli_model: "gpt-5.6-terra",
        label: "Terra",
        context_tokens: 400_000,
        effort: None,
        enable: &[],
    },
    ModelInfo {
        id: "luna",
        cli_model: "gpt-5.6-luna",
        label: "Luna",
        context_tokens: 400_000,
        effort: None,
        enable: &[],
    },
];

const CODEX_EFFORTS: &[EffortChoice] = &[
    EffortChoice::default_choice(),
    EffortChoice {
        id: "none",
        label: "none",
        effort: Some("none"),
    },
    EffortChoice {
        id: "minimal",
        label: "minimal",
        effort: Some("minimal"),
    },
    EffortChoice {
        id: "low",
        label: "low",
        effort: Some("low"),
    },
    EffortChoice {
        id: "medium",
        label: "medium",
        effort: Some("medium"),
    },
    EffortChoice {
        id: "high",
        label: "high",
        effort: Some("high"),
    },
    EffortChoice {
        id: "xhigh",
        label: "xhigh",
        effort: Some("xhigh"),
    },
    EffortChoice {
        id: "max",
        label: "max",
        effort: Some("max"),
    },
    EffortChoice {
        id: "ultra",
        label: "ultra",
        effort: Some("ultra"),
    },
];

const CODEX_FAST: &[FastChoice] = &[FastChoice::OFF, FastChoice::ON];

// --- Grok ---
// grok-4.5: low | medium | high (docs.x.ai). Default high if omitted.

const GROK_MODELS: &[ModelInfo] = &[ModelInfo {
    id: "grok-4.5",
    cli_model: "grok-4.5",
    label: "Grok 4.5",
    context_tokens: 500_000,
    effort: None,
    enable: &[],
}];

const GROK_EFFORTS: &[EffortChoice] = &[
    EffortChoice::default_choice(),
    EffortChoice {
        id: "low",
        label: "low",
        effort: Some("low"),
    },
    EffortChoice {
        id: "medium",
        label: "medium",
        effort: Some("medium"),
    },
    EffortChoice {
        id: "high",
        label: "high",
        effort: Some("high"),
    },
];

// --- Claude ---
// Fable/Sonnet 5/Opus 4.8: low|medium|high|xhigh|max (+ ultracode). Haiku: no effort.

const CLAUDE_MODELS: &[ModelInfo] = &[
    ModelInfo {
        id: "fable",
        cli_model: "fable",
        label: "Fable 5",
        context_tokens: 1_000_000,
        effort: None,
        enable: &[],
    },
    ModelInfo {
        id: "opus",
        cli_model: "opus",
        label: "Opus 4.8",
        context_tokens: 1_000_000,
        effort: None,
        enable: &[],
    },
    ModelInfo {
        id: "sonnet",
        cli_model: "sonnet",
        label: "Sonnet 5",
        context_tokens: 1_000_000,
        effort: None,
        enable: &[],
    },
    ModelInfo {
        id: "haiku",
        cli_model: "haiku",
        label: "Haiku 4.5",
        context_tokens: 200_000,
        effort: None,
        enable: &[],
    },
];

const CLAUDE_HAIKU_EFFORTS: &[EffortChoice] = &[EffortChoice::default_choice()];

const CLAUDE_EFFORTS: &[EffortChoice] = &[
    EffortChoice::default_choice(),
    EffortChoice {
        id: "low",
        label: "low",
        effort: Some("low"),
    },
    EffortChoice {
        id: "medium",
        label: "medium",
        effort: Some("medium"),
    },
    EffortChoice {
        id: "high",
        label: "high",
        effort: Some("high"),
    },
    EffortChoice {
        id: "xhigh",
        label: "xhigh",
        effort: Some("xhigh"),
    },
    EffortChoice {
        id: "max",
        label: "max",
        effort: Some("max"),
    },
    EffortChoice {
        id: "ultracode",
        label: "ultracode",
        effort: Some("ultracode"),
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_composite_claude_effort() {
        let m = find_model(Tool::Claude, "fable-max").unwrap();
        assert_eq!(m.cli_model, "fable");
        assert_eq!(m.effort, Some("max"));
        assert!(m.enable.is_empty());
    }

    #[test]
    fn find_codex_effort_and_fast_separate() {
        let high = find_model(Tool::Codex, "sol-high").unwrap();
        assert_eq!(high.effort, Some("high"));
        assert!(high.enable.is_empty());

        let fast = find_model(Tool::Codex, "sol-fast").unwrap();
        assert!(fast.effort.is_none());
        assert_eq!(fast.enable, &["fast_mode"]);

        let both = find_model(Tool::Codex, "sol-xhigh-fast").unwrap();
        assert_eq!(both.effort, Some("xhigh"));
        assert_eq!(both.enable, &["fast_mode"]);
    }

    #[test]
    fn find_grok_efforts() {
        assert_eq!(
            find_model(Tool::Grok, "grok-4.5-medium")
                .unwrap()
                .effort,
            Some("medium")
        );
    }

    #[test]
    fn codex_has_full_effort_ladder() {
        let sol = default_model(Tool::Codex);
        let ids: Vec<_> = efforts_for(Tool::Codex, sol).iter().map(|e| e.id).collect();
        assert!(ids.contains(&"low"));
        assert!(ids.contains(&"medium"));
        assert!(ids.contains(&"xhigh"));
        assert!(ids.contains(&"max"));
    }

    #[test]
    fn selection_key_axes() {
        let sol = default_model(Tool::Codex);
        let xhigh = find_effort(Tool::Codex, sol, "xhigh").unwrap();
        assert_eq!(
            selection_key(sol, xhigh, FastChoice::ON),
            "sol-xhigh-fast"
        );
        assert_eq!(
            selection_key(sol, EffortChoice::default_choice(), FastChoice::ON),
            "sol-fast"
        );
    }
}
