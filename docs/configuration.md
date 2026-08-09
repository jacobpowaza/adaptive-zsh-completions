# Configuration

Adaptive reads `$XDG_CONFIG_HOME/adaptive/config.toml`, or `~/.config/adaptive/config.toml` when XDG is unset. `ADAPTIVE_CONFIG` overrides the path for testing.

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

[providers.github]
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
export ADAPTIVE_ENTER_ACCEPTS_MENU=1
```

Cache defaults to the platform cache directory and history defaults to the platform local-data directory. `ADAPTIVE_CACHE_DIR` and `ADAPTIVE_DATA_DIR` are available for controlled test environments.
