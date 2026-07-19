# Teleporter

Teleport coding-agent conversations between **Codex**, **Grok**, and **Claude Code**.

Teleporter reads a source session, packs the **most recent transcript** (up to ~200k tokens, model-dependent), and opens the target CLI with that package. The **target model** writes the short handoff and continues — Teleporter does not invent a summary brief.

```
████████╗███████╗██╗     ███████╗██████╗  ██████╗ ██████╗ ████████╗███████╗██████╗
╚══██╔══╝██╔════╝██║     ██╔════╝██╔══██╗██╔═══██╗██╔══██╗╚══██╔══╝██╔════╝██╔══██╗
   ██║   █████╗  ██║     █████╗  ██████╔╝██║   ██║██████╔╝   ██║   █████╗  ██████╔╝
   ██║   ██╔══╝  ██║     ██╔══╝  ██╔═══╝ ██║   ██║██╔══██╗   ██║   ██╔══╝  ██╔══██╗
   ██║   ███████╗███████╗███████╗██║     ╚██████╔╝██║  ██║   ██║   ███████╗██║  ██║
   ╚═╝   ╚══════╝╚══════╝╚══════╝╚═╝      ╚═════╝ ╚═╝  ╚═╝   ╚═╝   ╚══════╝╚═╝  ╚═╝
```

## Install

```bash
cargo install --path .
```

Requires the target CLI (`codex`, `grok`, or `claude`) on your `PATH`.

## Flow

1. **From** — source CLI  
2. **Session** — conversation in this cwd  
3. **To** — target CLI  
4. **Model** — target model (sets `-m` / `--model` + pack budget)  
5. **Teleport** — soft (clipboard) or auto-send  

## Soft vs auto-send

| Mode | What happens |
|------|----------------|
| **Auto-send** (default in TUI / `--send`) | Target opens with the package as the **initial prompt** (large packs use a temp file pointer). |
| **Soft** (TUI `a` to toggle) | Package on clipboard; CLI opens empty — paste & send yourself. |

Package headline looks like: `Continue from Codex — <session title>`.

## Commands

```bash
cd your-project
teleporter

teleporter list codex
teleporter show codex latest --to grok -m grok-4.5
teleporter from codex to grok --last -m grok-build --dry-run
teleporter from claude to codex --last -m gpt-5.4
teleporter from grok to claude --last -m sonnet --budget 50000
teleporter from codex to grok --last --soft   # clipboard only
```

## Package shape

```text
# Continue from Codex — <title>
target / model / cwd / last_ask / files
## Rules   (untrusted inert history)
## Transcript (recent turns, older omitted)
## Continue (summarize, verify, continue)
```

No API key. No target-side skill required.

## License

MIT
