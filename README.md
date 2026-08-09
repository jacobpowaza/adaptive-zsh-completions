# Adaptive

Intelligent shell completion that learns commands instead of requiring giant completion scripts.

```console
$ git clone https://github.com/jacobpowaza/r
  jacobpowaza/relay.git          Fast local relay
  jacobpowaza/relay-plugins.git  Relay plugins
```

Adaptive discovers what a command accepts, resolves what the current argument means, and ranks useful values from local context. Its best recommendation appears as inline ghost text. Ambiguous filesystem paths appear in a small menu below the prompt.

> [!NOTE]
> v0.1 ships a complete Zsh frontend. The protocol and core are shell-independent; Bash and Fish frontends are planned.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/jacobpowaza/adaptive-zsh-completions/main/install.sh | sh
```

The installer verifies release checksums, installs `adaptive` in `~/.local/bin`, backs up `.zshrc`, and adds one marked, idempotent block. If a binary is unavailable for the machine, it falls back to a local Cargo build.

Restart Zsh, then check the installation:

```sh
adaptive doctor
```

To install with a different acceptance key, pass a Zsh `bindkey` sequence to the installer:

```sh
curl -fsSL https://raw.githubusercontent.com/jacobpowaza/adaptive-zsh-completions/main/install.sh | ADAPTIVE_ACCEPT_KEY='^F' sh
```

For a source checkout:

```sh
make install
```

To uninstall:

```sh
curl -fsSL https://raw.githubusercontent.com/jacobpowaza/adaptive-zsh-completions/main/uninstall.sh | sh
```

The uninstaller backs up `.zshrc`, removes only Adaptive's managed block and binary, and leaves local cache/history in place.

## What it completes

- Flags, subcommands, arguments, aliases, and enum values parsed from common `--help` formats
- Lazily discovered nested commands such as `docker compose up`
- Cobra-style native structured completions when safely detected
- Files and directories for `cd`, `cat`, `code`, `ls`, and `open`
- Local Git branches and remotes for `checkout`, `switch`, `push`, and `remote`
- Public GitHub, GitLab, Codeberg/Forgejo, Gitea, and Bitbucket repositories in HTTPS and SSH clone URL forms
- npm, pnpm, and Yarn scripts from the nearest `package.json`
- SSH aliases from `~/.ssh/config` and non-hashed `known_hosts` entries
- Docker container names for `exec` and `logs`, when Docker is responsive
- Sanitized local history as a fallback and ranking signal

For example:

```console
$ claude --dang
         erously-skip-permissions

$ npm run de
  dev     vite
  deploy  node deploy.js

$ git checkout fea
  feature/completion-engine
```

The demo image is intentionally not fabricated; a terminal recording will be added after the first tagged release.

## How it differs

`zsh-autosuggestions` predicts from history. Adaptive first discovers command structure and contextual providers, then uses sanitized history only as fallback/personalization.

Static completion repositories encode each CLI as a large shell function. Adaptive stores a generic command schema and keeps command-specific code limited to small adapters for dynamic values that cannot be inferred from help.

Adaptive's parser, ranking, cache, learning, query protocol, and ZLE frontend are implemented in this repository. It does not wrap, source, copy, or depend at runtime on zsh-autosuggestions, Oh My Zsh, or a static completion collection. The Rust crates in `Cargo.lock` provide ordinary low-level facilities such as TLS, serialization, and CLI argument parsing; they are not completion engines.

Claude, Codex, and other installed CLIs are not encoded as special cases. Adaptive discovers their current flags and subcommands from native completion metadata and safe `--help`/`-h`/`help`/man interfaces, then caches the resulting schema against the executable fingerprint.

## Zsh controls

- Tab accepts ghost text or the selected menu candidate. Set `ADAPTIVE_ACCEPT_KEY` before initialization to change it.
- The highest-ranked command, flag, branch, script, or repository recommendation stays inline in dark gray—even when alternatives exist.
- Ambiguous files and directories use the compact menu. Set `ADAPTIVE_GHOST_STYLE` to any ZLE highlight style before initialization to change the inline color.
- Right arrow cycles a visible candidate menu; Up/Down work too. Set `ADAPTIVE_MENU_NEXT_KEY` to change it.
- Press 1–9 to accept that numbered menu item immediately. Escape dismisses the menu.
- Enter runs the command by default. Set `ADAPTIVE_ENTER_ACCEPTS_MENU=1` to make Enter accept an open menu.
- Outside an Adaptive suggestion/menu, the widgets delegate to normal ZLE behavior. `bindkey -e` is supported.

## CLI

```text
adaptive init zsh
adaptive doctor
adaptive discover git
adaptive discover docker compose
adaptive inspect git
adaptive cache status
adaptive cache refresh git
adaptive cache prune
adaptive cache clear
adaptive history status
adaptive history clear
adaptive history disable
adaptive version
```

`adaptive query` is the stable frontend protocol. It emits JSON by default and a sanitized tab-separated stream for the Zsh integration.

## Architecture

```text
shell buffer
  parser -> resolver -> discovery/cache -> contextual providers
                                     \-> history fallback
                candidates -> normalizer/ranker -> shell protocol
                                                   |- ghost text
                                                   `- small menu
```

Discovery only invokes bounded informational interfaces: known native completion endpoints, `--help`, `help`, and lazily reached subcommand help. Schemas are cached against an executable path/size/mtime fingerprint. Official documentation retrieval is optional, allowlisted by command, size-limited, treated as untrusted data, and never evaluated.

See [architecture](docs/architecture.md), [provider development](docs/providers.md), and [configuration](docs/configuration.md).

## Privacy and security

Adaptive has no telemetry. Shell history never leaves the machine. A sanitizer rejects multiline pastes, token-shaped strings, credential keywords, large random values, private-key material patterns, and oversized commands before persistence. GitHub credentials may be read from `GH_TOKEN`, `GITHUB_TOKEN`, or an authenticated `gh`, but are kept only in process memory and never logged or cached.

Remote data is length-limited and rejected if it contains control characters, tabs, newlines, NUL, or terminal escapes. Suggestions are inserted as text and are never executed automatically. See [SECURITY.md](SECURITY.md) for the threat model and reporting process.

## Configuration

Adaptive works without a config file. It uses the platform config directory: `~/.config/adaptive/config.toml` on XDG systems and `~/Library/Application Support/adaptive/config.toml` on macOS.

```toml
[completion]
history = true
online_docs = false
fuzzy = true

[ui]
ghost_text = true
menu = true
max_candidates = 8
enter_accepts_menu = false

[providers.forge]
enabled = true

[providers.docker]
enabled = true

[privacy]
telemetry = false
```

## Development

Rust 1.85 or newer is required.

```sh
make test
make check
make build
./tests/install.sh
```

Provider contributions implement one narrow matching/resolution interface; ranking, cache, timeouts, sanitization, and display remain in the engine. Start with [docs/providers.md](docs/providers.md) and [CONTRIBUTING.md](CONTRIBUTING.md).

## Roadmap

- Bash and Fish frontends over the existing query protocol
- More native completion adapters and structured documentation parsers
- Workspace-aware npm/pnpm package glob expansion
- Background refresh for stale schemas and provider caches
- Terminal recording and broader ZLE compatibility matrix

Adaptive is MIT licensed.
