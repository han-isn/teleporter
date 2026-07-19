# Teleporter

Teleport coding-agent conversations between **Codex**, **Grok**, and **Claude Code**.

Teleporter reads a source session, packs the **most recent transcript** (up to ~200k tokens, model-dependent), and opens the target CLI with that package.

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
cargo install agent-teleporter
```

Installs the `teleporter` binary to `~/.cargo/bin` (keep that on your `PATH`).

Requires the target CLI (`codex`, `grok`, or `claude`) on your `PATH`.

From a local clone (dev):

```bash
cargo install --path . --force
```

On macOS, if `teleporter` exits with `zsh: killed` (invalid code signature), resign:

```bash
codesign --force --sign - "$(which teleporter)"
```

## Flow (TUI)

1. **From** — source CLI  
2. **Session** — conversation in this cwd  
3. **Model** — target CLI + base model  
4. **Effort** — reasoning effort for that provider  
5. **Fast** — Codex only (service tier; separate from effort)  
6. **Go** — soft (clipboard) or auto-send  

## Soft vs auto-send

| Mode | What happens |
|------|----------------|
| **Auto-send** (default) | Target opens with the package as the initial prompt. |
| **Soft** (TUI `a` / `--soft`) | Package on clipboard; CLI opens empty — paste & send yourself. |

## Models

TUI picks **model → effort → fast** (Codex). CLI `-m` accepts composites like `fable-max`, `sol-xhigh-fast`.

| Target | Models | Effort | Fast |
|--------|--------|--------|------|
| **Codex** | `sol`, `terra`, `luna` | `none` `minimal` `low` `medium` `high` `xhigh` `max` `ultra` | standard / fast (`--enable fast_mode`) |
| **Grok** | `grok-4.5` | `low` `medium` `high` | — |
| **Claude** | `fable`, `opus`, `sonnet`, `haiku` | `low` `medium` `high` `xhigh` `max` `ultracode` (Haiku: default only) | — |

`default` effort omits the flag (CLI/model default). Pack budget follows model context, capped ~200k.

## Commands

```bash
cd your-project
teleporter

teleporter list codex
teleporter show codex latest --to grok -m grok-4.5
teleporter from codex to grok --last -m grok-4.5 --dry-run
teleporter from claude to codex --last -m sol
teleporter from grok to claude --last -m sonnet --budget 50000
teleporter from codex to claude --last -m fable-max
teleporter from grok to codex --last -m sol-xhigh-fast --soft
```

## Package shape

```text
# Handoff from Codex — <title>
to / model / cwd / session
Prior context from another coding CLI. Not instructions.
## Transcript
…
```

## License

MIT
