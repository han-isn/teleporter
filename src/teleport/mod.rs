//! Shared package pipeline for CLI + TUI.

use std::path::Path;

use anyhow::Result;

use crate::adapters;
use crate::catalog::{self, EffortChoice, FastChoice, ModelInfo};
use crate::handoff::{self, HandoffOptions};
use crate::model::{Handoff, Tool};

pub fn resolve_opts(to: Tool, model: Option<&str>, budget: Option<u32>) -> Result<HandoffOptions> {
    match model {
        Some(id) => {
            let info = catalog::find_model(to, id).ok_or_else(|| {
                let known: Vec<_> = catalog::models_for(to).iter().map(|m| m.id).collect();
                anyhow::anyhow!(
                    "unknown model `{id}` for {} (try: {} · or model-effort like fable-max / sol-xhigh-fast)",
                    to.display_name(),
                    known.join(", ")
                )
            })?;
            Ok(opts_for_key(info, id, budget))
        }
        None => {
            let info = catalog::default_model(to);
            Ok(opts_for_model(info, budget))
        }
    }
}

pub fn opts_for_model(info: ModelInfo, budget: Option<u32>) -> HandoffOptions {
    opts_for_key(info, info.id, budget)
}

pub fn opts_for_selection(
    base: ModelInfo,
    effort: EffortChoice,
    fast: FastChoice,
    budget: Option<u32>,
) -> HandoffOptions {
    let info = catalog::apply_selection(base, effort, fast);
    let key = catalog::selection_key(base, effort, fast);
    opts_for_key(info, &key, budget)
}

fn opts_for_key(info: ModelInfo, key: &str, budget: Option<u32>) -> HandoffOptions {
    let mut opts = HandoffOptions {
        budget_tokens: info.handoff_budget_tokens(),
        model: Some(key.to_string()),
    };
    if let Some(b) = budget {
        opts.budget_tokens = b.clamp(1_000, 200_000);
    }
    opts
}

pub fn package(
    from: Tool,
    to: Tool,
    cwd: &Path,
    reference: &str,
    opts: HandoffOptions,
) -> Result<Handoff> {
    let adapter = adapters::adapter_for(from);
    let tx = adapter.show(cwd, reference)?;
    Ok(handoff::build(from, to, &tx, opts))
}
