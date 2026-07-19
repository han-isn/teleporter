//! Shared package pipeline for CLI + TUI.

use std::path::Path;

use anyhow::Result;

use crate::adapters;
use crate::catalog::{self, ModelInfo};
use crate::handoff::{self, HandoffOptions};
use crate::model::{Handoff, Tool};

pub fn resolve_opts(to: Tool, model: Option<&str>, budget: Option<u32>) -> Result<HandoffOptions> {
    let info = match model {
        Some(id) => catalog::find_model(to, id).ok_or_else(|| {
            let known: Vec<_> = catalog::models_for(to).iter().map(|m| m.id).collect();
            anyhow::anyhow!(
                "unknown model `{id}` for {} (try: {})",
                to.display_name(),
                known.join(", ")
            )
        })?,
        None => catalog::default_model(to),
    };
    Ok(opts_for_model(info, budget))
}

pub fn opts_for_model(info: ModelInfo, budget: Option<u32>) -> HandoffOptions {
    let mut opts = HandoffOptions::for_model(info);
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
