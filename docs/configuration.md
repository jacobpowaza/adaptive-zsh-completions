# Configuration

Adaptive uses `$XDG_CONFIG_HOME/adaptive/config.toml` (normally `~/.config/adaptive/config.toml`) on Linux and `~/Library/Application Support/adaptive/config.toml` on macOS. `ADAPTIVE_CONFIG` overrides the path for testing.

```toml
[completion]
history = true
online_docs = false
fuzzy = true
# Predict the next token proactively at untyped positions (after a trailing
# space, and on an empty prompt) instead of waiting for a typed prefix.
predictive = true
# Complete to entire remembered command lines (`c` ->
# `claude --dangerously-skip-permissions`), not just single tokens.
chain_prediction = true
# Half-life in days for recency weighting: recent repetition outweighs old
# habits without erasing them from memory.
half_life_days = 21.0

[ui]
ghost_text = true
max_candidates = 8

[providers.forge]
enabled = true

[providers.docker]
enabled = true

# Optional external completion engine (carapace-bin). Detected on PATH at
# query time; Adaptive works identically without it.
[carapace]
enabled = true

[privacy]
telemetry = false
```

Online docs are opt-in because local help/cache should cover the hot path. The resolver only knows explicit official mappings and never evaluates retrieved code. Setting telemetry to true is rejected because v0.1 contains no telemetry implementation.

## Carapace integration (optional)

When [carapace-bin](https://carapace-sh.github.io/carapace-bin/) is installed, Adaptive consults it as an additional completion source: exact subcommands, flags, enum values, positional arguments, and command-specific completions for the hundreds of CLIs it ships completers for — without Adaptive maintaining any per-command list.

- **Detection is automatic.** If `carapace` is on `PATH`, its answers are merged into queries; if not, nothing changes. It is never a hard dependency.
- **Personalization wins.** Carapace provides correctness and context; your learned habits (frequency + recency + accept feedback) deliberately outweigh it in ranking. A branch you always check out ranks above Carapace's generic suggestions, while Carapace's answers suppress redundant `--help` parsing (`kubectl get <TAB>` comes straight from its schema).
- **Caching keeps it instant.** Answers are cached per position and working directory with a ten-minute TTL inside a key derived from a fingerprint of the carapace binary itself (path, size, mtime), so upgrading carapace invalidates everything it ever answered. The completer listing is cached for 24 hours; unknown commands never pay for a subprocess at all. The Zsh frontend already runs every query off the ZLE thread, and each carapace call has a hard 400 ms timeout, so a hung or broken carapace can only ever cost one missed answer — never a stalled shell.

Manage it without reinstalling:

```sh
adaptive carapace status    # detected path/version + enabled state
adaptive carapace disable   # stop consulting carapace
adaptive carapace enable    # use it again when present
```

The installer detects carapace too: found → it says so; missing → it prints optional install instructions (`ADAPTIVE_INSTALL_CARAPACE=yes` attempts the install automatically via Homebrew/apt/dnf/pacman/apk; `ask` prompts on interactive terminals). Adaptive installs and works fine either way.

Zsh-specific environment settings must be assigned before the managed initialization block:

```zsh
export ADAPTIVE_ACCEPT_KEY='^[f'
export ADAPTIVE_GHOST_STYLE='fg=245'
export ADAPTIVE_MENU_NEXT_KEY='^[[C'
export ADAPTIVE_BROWSE_KEY='^[[Z'
```

Adaptive renders the highest-ranked recommendation inline. Tab accepts that suggestion when one is showing and otherwise lists related commands, flags, or filesystem entries; recognized forge repository lists can appear automatically. Shift-TAB always opens the list -- even while an inline suggestion is visible -- so browsing candidates never requires dismissing the ghost first.

The installer also accepts `ADAPTIVE_ACCEPT_KEY`, `ADAPTIVE_MENU_NEXT_KEY`, `ADAPTIVE_BROWSE_KEY`, and `ADAPTIVE_GHOST_STYLE` and writes their quoted values into its managed block.

## Usage priors

Ranking blends what people generally run with what you run. The global half lives in a table shipped with the binary and can be extended or reordered in `~/.config/adaptive/priors.toml` (`ADAPTIVE_PRIORS` overrides the path):

```toml
# Bare command names, most used first.
commands = ["git", "kubectl", "brew"]

[subcommands]
# Keys are full command paths; lists are ordered most likely first.
mytool = ["deploy", "status", "rollback"]
"mytool deploy" = ["staging", "production"]

# Optional context-specific orderings for the same key.
[in_git_repo]
git = ["status", "add", "commit", "push"]

[in_node_project]
npm = ["run", "install", "test"]
```

A user list replaces the built-in list for that key. Entries are used three ways: they order candidates, they predict the next token before anything is typed, and they supply common subcommands for positions a tool's own `--help` leaves undocumented. They never override personal history — a few real uses of `brew update` outrank the global default `brew install`.

Cache defaults to the platform cache directory and history defaults to the platform local-data directory. `ADAPTIVE_CACHE_DIR` and `ADAPTIVE_DATA_DIR` are available for controlled test environments.

## Usage memory

Every executed line is recorded locally (secrets and pastes are rejected by the sanitizer before anything is written) into a structured usage profile: command frequencies, next-token transitions per exact command path, command sequences, per-directory affinities, and your accept/dismiss feedback on suggestions. Ranking blends all of it with recency decay, so what you have been doing lately leads without erasing long-term habits.

To start with months of context instead of an empty profile:

```sh
adaptive history import            # uses $HISTFILE
adaptive history import ~/path/to/histfile --force   # re-import explicitly
```

The import runs once (a marker file guards repeats) and the Zsh frontend fires it in the background on first start. `adaptive history status` shows how much has been learned; `adaptive history clear` forgets everything.

Accepting a suggestion (Tab) strengthens that exact continuation at that exact position; dismissing one (Alt+Esc, or your `ADAPTIVE_DISMISS_KEY`) demotes it below untouched rivals while keeping it reachable in the Tab list. Both are plain files under your local data directory and never leave the machine.
