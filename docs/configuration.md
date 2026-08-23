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

[privacy]
telemetry = false
```

Online docs are opt-in because local help/cache should cover the hot path. The resolver only knows explicit official mappings and never evaluates retrieved code. Setting telemetry to true is rejected because v0.1 contains no telemetry implementation.

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

A user list replaces the built-in list for that key. Entries are used three ways: they order candidates, they predict the next token before anything is typed, and they supply candidates for tools whose `--help` cannot be parsed. They never override personal history — a few real uses of `brew update` outrank the global default `brew install`.

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
