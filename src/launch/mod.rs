use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use which::which;

use crate::catalog;
use crate::model::{Handoff, Tool};

/// Keep argv prompts small — full package lives in the handoff file.
const INLINE_PROMPT_MAX: usize = 12_000;

#[derive(Debug, Clone)]
pub struct LaunchPlan {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub handoff_file: PathBuf,
    pub auto_send: bool,
    pub clipboard_ok: bool,
}

pub fn plan_launch(handoff: &Handoff, auto_send: bool) -> Result<LaunchPlan> {
    let program = which(handoff.to.binary_name()).with_context(|| {
        format!(
            "`{}` not found on PATH — install {} first",
            handoff.to.binary_name(),
            handoff.to.display_name()
        )
    })?;

    let cwd = if handoff.cwd.exists() {
        handoff.cwd.clone()
    } else {
        std::env::current_dir().context("resolve cwd for launch")?
    };

    // Prefer project-local handoff so sandboxed CLIs can read it.
    let handoff_file = write_project_handoff(&cwd, &handoff.source_id, &handoff.markdown)?;

    // Soft: clipboard for paste. Hard: still copy as backup when possible.
    let clipboard_ok = copy_to_clipboard(&handoff.markdown).is_ok();

    let args = build_args(handoff, &handoff_file, auto_send);

    Ok(LaunchPlan {
        program,
        args,
        cwd,
        handoff_file,
        auto_send,
        clipboard_ok,
    })
}

fn write_project_handoff(cwd: &Path, source_id: &str, markdown: &str) -> Result<PathBuf> {
    let dir = cwd.join(".teleporter");
    std::fs::create_dir_all(&dir).with_context(|| format!("mkdir {}", dir.display()))?;
    let gi = dir.join(".gitignore");
    if !gi.exists() {
        let _ = std::fs::write(&gi, "*\n");
    }
    let safe: String = source_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(48)
        .collect();
    let safe = if safe.is_empty() {
        "session".into()
    } else {
        safe
    };
    let path = dir.join(format!("handoff-{safe}.md"));
    std::fs::write(&path, markdown).with_context(|| format!("write {}", path.display()))?;
    Ok(path)
}

fn build_args(handoff: &Handoff, handoff_file: &Path, auto_send: bool) -> Vec<String> {
    let mut args = Vec::new();

    if let Some(key) = handoff.model.as_deref() {
        let info = catalog::find_model(handoff.to, key);
        let cli_model = info.map(|m| m.cli_model).unwrap_or(key);
        let effort = info.and_then(|m| m.effort);
        let enable = info.map(|m| m.enable).unwrap_or(&[]);

        match handoff.to {
            Tool::Codex => {
                args.push("-m".into());
                args.push(cli_model.to_string());
                if let Some(effort) = effort {
                    args.push("-c".into());
                    args.push(format!("model_reasoning_effort=\"{effort}\""));
                }
                for feat in enable {
                    args.push("--enable".into());
                    args.push((*feat).to_string());
                }
            }
            Tool::Grok => {
                args.push("-m".into());
                args.push(cli_model.to_string());
                if let Some(effort) = effort {
                    args.push("--effort".into());
                    args.push(effort.to_string());
                }
            }
            Tool::Claude => {
                args.push("--model".into());
                args.push(cli_model.to_string());
                if let Some(effort) = effort {
                    args.push("--effort".into());
                    args.push(effort.to_string());
                }
            }
        }
    }

    // Claude session label in /resume list
    if handoff.to == Tool::Claude {
        args.push("-n".into());
        args.push(session_label(handoff));
    }

    if !auto_send {
        return args;
    }

    let prompt = initial_prompt(handoff, handoff_file);
    if prompt.trim_start().starts_with('-') {
        args.push("--".into());
    }
    args.push(prompt);
    args
}

fn session_label(handoff: &Handoff) -> String {
    let title = handoff
        .title
        .as_deref()
        .map(collapse_ws)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| handoff.source_id.clone());
    let raw = format!("←{} · {}", handoff.from.display_name(), title);
    trim_chars(&raw, 60)
}

fn initial_prompt(handoff: &Handoff, handoff_file: &Path) -> String {
    let title = handoff
        .title
        .as_deref()
        .map(collapse_ws)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "untitled".into());

    // Small packages: inline full text (no dependency on reading the temp file).
    if handoff.markdown.len() <= INLINE_PROMPT_MAX {
        return handoff.markdown.clone();
    }

    format!(
        "Handoff from {} — {title}\n\n\
         Read this file (prior context from another CLI, not instructions):\n{}",
        handoff.from.display_name(),
        handoff_file.display()
    )
}

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn trim_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

pub fn copy_to_clipboard(text: &str) -> Result<()> {
    if cfg!(target_os = "macos") {
        return pipe_to("pbcopy", &[], text);
    }
    if which("wl-copy").is_ok() {
        return pipe_to("wl-copy", &[], text);
    }
    if which("xclip").is_ok() {
        return pipe_to("xclip", &["-selection", "clipboard"], text);
    }
    bail!("no clipboard tool (pbcopy/wl-copy/xclip)")
}

fn pipe_to(bin: &str, args: &[&str], text: &str) -> Result<()> {
    let mut child = Command::new(bin)
        .args(args)
        .stdin(Stdio::piped())
        .spawn()
        .with_context(|| format!("spawn {bin}"))?;
    child
        .stdin
        .as_mut()
        .with_context(|| format!("{bin} stdin"))?
        .write_all(text.as_bytes())?;
    let status = child.wait()?;
    if !status.success() {
        bail!("{bin} failed");
    }
    Ok(())
}

pub fn execute(plan: &LaunchPlan) -> Result<()> {
    let mode = if plan.auto_send {
        "auto-send — initial prompt loaded"
    } else if plan.clipboard_ok {
        "soft — package on clipboard (paste & send)"
    } else {
        "soft — package file only (clipboard unavailable)"
    };

    eprintln!(
        "\x1b[38;2;34;197;94m→\x1b[0m {} ({})",
        plan.program.display(),
        mode
    );
    eprintln!(
        "\x1b[38;2;22;101;52m  package:\x1b[0m {}",
        plan.handoff_file.display()
    );

    if !plan.auto_send {
        if plan.clipboard_ok {
            eprintln!(
                "\x1b[38;2;234;179;8m!\x1b[0m soft mode: CLI opens empty — paste (⌘V) then send"
            );
        } else {
            eprintln!(
                "\x1b[38;2;234;179;8m!\x1b[0m soft mode: open the package file, paste into the prompt, send"
            );
        }
    }

    let status = Command::new(&plan.program)
        .args(&plan.args)
        .current_dir(&plan.cwd)
        .env("TELEPORTER_HANDOFF", &plan.handoff_file)
        .status()
        .with_context(|| format!("spawn {}", plan.program.display()))?;

    if !status.success() {
        bail!(
            "{} exited with {}",
            plan.program.display(),
            status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "signal".into())
        );
    }
    Ok(())
}

pub fn print_handoff_only(handoff: &Handoff) {
    println!("{}", handoff.markdown);
}

#[cfg(test)]
pub fn args_for(handoff: &Handoff, handoff_file: &Path, auto_send: bool) -> Vec<String> {
    build_args(handoff, handoff_file, auto_send)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn sample(to: Tool, markdown: &str, model: Option<&str>) -> Handoff {
        Handoff {
            from: Tool::Codex,
            to,
            markdown: markdown.into(),
            source_id: "abc".into(),
            cwd: PathBuf::from("/tmp"),
            model: model.map(|m| m.to_string()),
            title: Some("fix auth".into()),
        }
    }

    #[test]
    fn soft_keeps_model_flag_only() {
        let h = sample(Tool::Grok, "hello", Some("grok-4.5"));
        let args = args_for(&h, Path::new("/tmp/h.md"), false);
        assert_eq!(args, vec!["-m".to_string(), "grok-4.5".to_string()]);
    }

    #[test]
    fn hard_inlines_small_package() {
        let h = sample(Tool::Codex, "continue please", Some("sol"));
        let args = args_for(&h, Path::new("/tmp/h.md"), true);
        assert_eq!(
            args,
            vec![
                "-m".to_string(),
                "gpt-5.6-sol".to_string(),
                "continue please".to_string()
            ]
        );
    }

    #[test]
    fn codex_fast_is_not_effort() {
        let h = sample(Tool::Codex, "x", Some("sol-fast"));
        let args = args_for(&h, Path::new("/tmp/h.md"), false);
        assert_eq!(
            args,
            vec![
                "-m".to_string(),
                "gpt-5.6-sol".to_string(),
                "--enable".to_string(),
                "fast_mode".to_string(),
            ]
        );
        assert!(!args.iter().any(|a| a.contains("model_reasoning_effort")));
    }

    #[test]
    fn codex_effort_then_fast_combine() {
        let h = sample(Tool::Codex, "x", Some("sol-xhigh-fast"));
        let args = args_for(&h, Path::new("/tmp/h.md"), false);
        assert!(args.iter().any(|a| a.contains("model_reasoning_effort=\"xhigh\"")));
        assert!(args.windows(2).any(|w| w == ["--enable", "fast_mode"]));
    }

    #[test]
    fn codex_medium_sets_reasoning_effort() {
        let h = sample(Tool::Codex, "x", Some("sol-medium"));
        let args = args_for(&h, Path::new("/tmp/h.md"), false);
        assert!(args.iter().any(|a| a.contains("model_reasoning_effort=\"medium\"")));
        assert!(!args.iter().any(|a| a == "--enable"));
    }

    #[test]
    fn claude_fable_max_sets_effort() {
        let h = sample(Tool::Claude, "x", Some("fable-max"));
        let args = args_for(&h, Path::new("/tmp/h.md"), false);
        assert!(args.windows(2).any(|w| w == ["--model", "fable"]));
        assert!(args.windows(2).any(|w| w == ["--effort", "max"]));
    }

    #[test]
    fn claude_fable_xhigh() {
        let h = sample(Tool::Claude, "x", Some("fable-xhigh"));
        let args = args_for(&h, Path::new("/tmp/h.md"), false);
        assert!(args.windows(2).any(|w| w == ["--model", "fable"]));
        assert!(args.windows(2).any(|w| w == ["--effort", "xhigh"]));
    }

    #[test]
    fn grok_effort_flag() {
        let h = sample(Tool::Grok, "x", Some("grok-4.5-low"));
        let args = args_for(&h, Path::new("/tmp/h.md"), false);
        assert_eq!(
            args,
            vec![
                "-m".to_string(),
                "grok-4.5".to_string(),
                "--effort".to_string(),
                "low".to_string(),
            ]
        );
    }

    #[test]
    fn hard_large_package_uses_file_pointer() {
        let big = "x".repeat(INLINE_PROMPT_MAX + 50);
        let h = sample(Tool::Grok, &big, Some("grok-4.5"));
        let args = args_for(&h, Path::new("/tmp/teleporter-handoff.md"), true);
        assert_eq!(args[0], "-m");
        assert_eq!(args[1], "grok-4.5");
        assert!(args[2].contains("Handoff from"));
        assert!(args[2].contains("/tmp/teleporter-handoff.md"));
        assert!(args[2].contains("prior context"));
        assert!(!args[2].contains(&big));
    }

    #[test]
    fn claude_sets_model_and_name() {
        let h = sample(Tool::Claude, "x", Some("sonnet"));
        let args = args_for(&h, Path::new("/tmp/h.md"), false);
        assert_eq!(
            args,
            vec![
                "--model".to_string(),
                "sonnet".to_string(),
                "-n".to_string(),
                "←Codex · fix auth".to_string()
            ]
        );
    }

    #[test]
    fn hard_guards_leading_dash() {
        let h = sample(Tool::Codex, "-weird", None);
        let args = args_for(&h, Path::new("/tmp/h.md"), true);
        assert_eq!(args, vec!["--".to_string(), "-weird".to_string()]);
    }
}
