//! Known target models and context budgets.

use crate::model::Tool;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelInfo {
    pub id: &'static str,
    pub label: &'static str,
    /// Approx context window in tokens.
    pub context_tokens: u32,
}

impl ModelInfo {
    /// How many tokens we pack into the teleported transcript body.
    /// Leaves headroom for system prompt + model reply.
    pub fn handoff_budget_tokens(self) -> u32 {
        let reserve = 32_000u32;
        let cap = 200_000u32;
        self.context_tokens.saturating_sub(reserve).min(cap).max(8_000)
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

pub fn find_model(tool: Tool, id: &str) -> Option<ModelInfo> {
    let id = id.trim();
    models_for(tool)
        .iter()
        .copied()
        .find(|m| m.id.eq_ignore_ascii_case(id) || m.label.eq_ignore_ascii_case(id))
}

const CODEX_MODELS: &[ModelInfo] = &[
    ModelInfo {
        id: "gpt-5.4",
        label: "GPT-5.4",
        context_tokens: 200_000,
    },
    ModelInfo {
        id: "gpt-5.3",
        label: "GPT-5.3",
        context_tokens: 200_000,
    },
    ModelInfo {
        id: "gpt-5.2",
        label: "GPT-5.2",
        context_tokens: 200_000,
    },
    ModelInfo {
        id: "o3",
        label: "o3",
        context_tokens: 200_000,
    },
    ModelInfo {
        id: "o4-mini",
        label: "o4-mini",
        context_tokens: 200_000,
    },
];

const GROK_MODELS: &[ModelInfo] = &[
    ModelInfo {
        id: "grok-build",
        label: "Grok Build",
        context_tokens: 200_000,
    },
    ModelInfo {
        id: "grok-4.5",
        label: "Grok 4.5",
        context_tokens: 256_000,
    },
    ModelInfo {
        id: "grok-4",
        label: "Grok 4",
        context_tokens: 256_000,
    },
];

const CLAUDE_MODELS: &[ModelInfo] = &[
    ModelInfo {
        id: "opus",
        label: "Opus",
        context_tokens: 200_000,
    },
    ModelInfo {
        id: "sonnet",
        label: "Sonnet",
        context_tokens: 200_000,
    },
    ModelInfo {
        id: "haiku",
        label: "Haiku",
        context_tokens: 200_000,
    },
];
