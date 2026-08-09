# Architecture

Adaptive is a shell-independent Rust library with a thin CLI protocol. Shells send a buffer, byte cursor, and working directory; the engine returns normalized candidates plus the current-token replacement length.

## Query path

1. `parser` tokenizes incomplete quoted input without evaluating shell syntax.
2. `discovery` resolves only executable metadata and bounded informational help operations.
3. `help_parser` extracts uncertain schema items with an explicit confidence score.
4. `native` may prefer an adapter's machine-oriented protocol when its signature is detected.
5. `providers` resolve dynamic argument values from narrow safe sources.
6. `history` contributes local sanitized fallback patterns and usage signals.
7. `ranking` combines prefix/fuzzy quality, source confidence, recency, and frequency.
8. `protocol` returns JSON or sanitized Zsh records.

The Zsh frontend starts a query through process substitution after edits. A new edit closes the old descriptor and terminates the stale process. The engine has no daemon in v0.1: a release binary plus cache-first discovery keeps warm queries fast while avoiding daemon lifecycle complexity.

## Discovery and cache

Root help is discovered on first encounter. A nested path is discovered only when the parsed root schema confirms that the user reached a subcommand. The safety layer rejects punctuation-bearing discovery path components, closes stdin, removes pagers, caps output at 2 MiB, and kills timed-out children.

Schemas are keyed by command/path and contain a fingerprint derived from executable location, size, and modification time. Provider results carry short TTLs; official docs carry a seven-day TTL. Cache writes use a temporary file plus rename. Pruning enforces age and total-size bounds.

## Frontend boundary

`adaptive query --buffer ... --cursor ... --cwd ...` is the portable boundary for future Bash/Fish integrations. Frontends own keybindings and rendering; they do not need to understand discovery, providers, cache, or ranking.

