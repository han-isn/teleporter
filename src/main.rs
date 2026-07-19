mod adapters;
mod brand;
mod catalog;
mod handoff;
mod launch;
mod model;
mod teleport;
mod tui;

use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};

use crate::model::Tool;

#[derive(Parser, Debug)]
#[command(
    name = "teleporter",
    about = "Teleport coding-agent conversations between Codex, Grok, and Claude Code",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Interactive TUI (default when no subcommand)
    Tui {
        #[arg(long)]
        cwd: Option<PathBuf>,
    },
    /// List sessions for a tool
    List {
        tool: Tool,
        #[arg(long)]
        cwd: Option<PathBuf>,
    },
    /// Print a packaged transcript (no launch)
    Show {
        tool: Tool,
        /// Session id, path, title fragment, or `latest`
        #[arg(default_value = "latest")]
        reference: String,
        #[arg(long)]
        cwd: Option<PathBuf>,
        #[arg(long, default_value = "grok")]
        to: Tool,
        #[arg(long)]
        model: Option<String>,
        /// Override transcript token budget
        #[arg(long)]
        budget: Option<u32>,
    },
    /// Teleport: `teleporter from <source> to <target>`
    From {
        source: Tool,
        /// Must be the word `to`
        to_word: String,
        target: Tool,
        #[arg(long, default_value = "latest")]
        id: String,
        /// Alias for `--id latest`
        #[arg(long)]
        last: bool,
        #[arg(long)]
        cwd: Option<PathBuf>,
        /// Target model id (e.g. sol, terra, fable, opus, claude-sonnet-5)
        #[arg(long, short = 'm')]
        model: Option<String>,
        /// Override transcript token budget (default: from model context, capped ~200k)
        #[arg(long)]
        budget: Option<u32>,
        /// Submit package as the initial prompt (default: on)
        #[arg(long, default_value_t = true)]
        send: bool,
        /// Clipboard only — open target CLI without an initial prompt
        #[arg(long)]
        soft: bool,
        /// Print package only
        #[arg(long)]
        dry_run: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        None => {
            let cwd = resolve_cwd(None)?;
            tui::run(cwd)
        }
        Some(Commands::Tui { cwd }) => {
            let cwd = resolve_cwd(cwd)?;
            tui::run(cwd)
        }
        Some(Commands::List { tool, cwd }) => {
            let cwd = resolve_cwd(cwd)?;
            let adapter = adapters::adapter_for(tool);
            let sessions = adapter.list(&cwd)?;
            if sessions.is_empty() {
                eprintln!("no {} sessions for {}", tool.display_name(), cwd.display());
                return Ok(());
            }
            for s in sessions {
                println!(
                    "{}\t{}\t{}",
                    s.id,
                    s.updated_at.format("%Y-%m-%dT%H:%M:%SZ"),
                    s.title.replace('\t', " ")
                );
            }
            Ok(())
        }
        Some(Commands::Show {
            tool,
            reference,
            cwd,
            to,
            model,
            budget,
        }) => {
            let cwd = resolve_cwd(cwd)?;
            let opts = teleport::resolve_opts(to, model.as_deref(), budget)?;
            let h = teleport::package(tool, to, &cwd, &reference, opts)?;
            launch::print_handoff_only(&h);
            Ok(())
        }
        Some(Commands::From {
            source,
            to_word,
            target,
            id,
            last,
            cwd,
            model,
            budget,
            send,
            soft,
            dry_run,
        }) => {
            if to_word != "to" {
                bail!("usage: teleporter from <source> to <target> [-m model] [--soft] [--send]");
            }
            if source == target {
                bail!("source and target must differ");
            }
            let cwd = resolve_cwd(cwd)?;
            let reference = if last {
                "latest".to_string()
            } else {
                id
            };
            brand::print_splash();
            let opts = teleport::resolve_opts(target, model.as_deref(), budget)?;
            let h = teleport::package(source, target, &cwd, &reference, opts)?;
            if dry_run {
                launch::print_handoff_only(&h);
                return Ok(());
            }
            eprintln!(
                "package → {} ({}) · ~{} chars\n",
                target.display_name(),
                h.model.as_deref().unwrap_or("default"),
                h.markdown.len()
            );
            let auto_send = if soft { false } else { send };
            let plan = launch::plan_launch(&h, auto_send)?;
            launch::execute(&plan)?;
            Ok(())
        }
    }
}

fn resolve_cwd(cwd: Option<PathBuf>) -> Result<PathBuf> {
    match cwd {
        Some(p) => Ok(std::fs::canonicalize(&p).unwrap_or(p)),
        None => Ok(std::env::current_dir()?),
    }
}
