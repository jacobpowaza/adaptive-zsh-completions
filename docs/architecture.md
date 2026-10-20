# Architecture

Adaptive is a shell-independent Rust library with a thin CLI protocol. Shells send a buffer, byte cursor, and working directory; the engine returns normalized candidates plus the current-token replacement length.

## Query path

1. `parser` tokenizes incomplete quoted input without evaluating shell syntax.
2. `providers` resolve dynamic argument values from narrow safe sources (PATH commands, Git branches, npm scripts, SSH hosts, Docker, forge URLs, the filesystem).
3. `carapace` (optional) consults an installed `carapace-bin` for exact command knowledge — subcommands, flags, enum values, positional arguments — for the hundreds of CLIs it ships completers for. Answers are cached against a fingerprint of the carapace binary and capped by a hard subprocess timeout; where Carapace spoke, help parsing is skipped. It is never required: when absent or disabled every other layer runs unchanged.
4. `history` contributes candidates from the learned usage profile: next-token predictions for the exact typed command path, bare-prompt command predictions from directory affinity plus previous-command sequences plus global frequency, and full-line chain completions. These merge with provider results at every position rather than acting only as a fallback.
5. `discovery` resolves only executable metadata and bounded informational help operations when no contextual provider spoke.
6. `help_parser` extracts uncertain schema items with an explicit confidence score.
7. `native` may prefer an adapter's machine-oriented protocol when its signature is detected.
8. `priors` supplies global usage defaults for the exact position being completed, and offers common subcommands for positions a tool's own help left undocumented.
9. `ranking` combines position validity (source confidence and candidate kind), global priors, personal frequency and recency from the profile's precomputed indexes, explicit accept/reject feedback, and prefix/fuzzy quality. Flags are demoted unless the current token begins with `-`, so a position that accepts a subcommand answers with subcommands and lists flags after them. Personal frequency and recency deliberately outweigh source trust, so learned habits rank above anything Carapace or help text offers.
10. The engine echoes a `context_key` identifying this exact completion position; frontends return it with accept/reject feedback so learning lands back on the same context.
11. The response is serialized to JSON or sanitized Zsh records.

### Proactive prediction

A trailing space or an empty prompt is a first-class completion event: the current token is empty but the position is fully known. At such positions the engine predicts the next token outright instead of waiting for a typed prefix. The Zsh frontend ghosts exactly one candidate there, so the engine collapses to one candidate only when it is justified: a personal habit repeated at least three times at this position, the head of the global prior list for a fresh install, or a learned continuation holding most of the recent probability mass at this position (the dominance rule). Otherwise several alternatives stay available through Tab.

Chain prediction extends this across tokens: remembered lines complete from a single typed letter (`c` → `claude --dangerously-skip-permissions`) and prediction continues after every accepted token, because accepting inserts the token plus a space and immediately re-queries the new position. A fresh edit buffer also starts predicting — an empty prompt ghosts the command the user most likely runs in this directory, next, after the previous line.

### Usage memory

The profile (`history.json`, schema v2) is written at record time by folding each executed line into small indexes so queries are O(1) lookups:

- `patterns` — sanitized full lines (also the source of chain completions);
- `transitions` — exact typed path (`git commit -m`) → next token;
- `sequences` — previous command head → next command head;
- `dirs` — normalized working directory → commands run there;
- `accepted`/`rejected` — per-context feedback from the frontend.

Every counter stores raw count and last-used time; read-time scoring applies a half-life decay (default 21 days, `[completion] half_life_days`), so recent repetition outweighs old habits without deleting them. Retention caps keep the file small enough to load instantly on every query process. Recording is secret-safe: the same sanitizer rejects tokens, pastes, and assignments that look like credentials whether they arrive live or via import. `adaptive history import` backfills the indexes from `$HISTFILE` once (marker-guarded, `--force` to repeat).

The Zsh frontend records every executed line with its working directory and the previously executed line, reports accept/dismiss decisions through the echoed context key, and bootstraps the import in the background on first start.

## Ranking model

Ranking asks "given everything typed so far, what is the user most likely to type next?" rather than "which strings match what was typed?". Every candidate carries a kind (subcommand, value, flag) and a prior, and is scored as a sum of independent terms: validity at this position, global/common usage, personal frequency, personal recency, explicit accept/reject feedback, and prefix match quality. Flags are demoted unless the current token begins with `-`, so a position that accepts a subcommand answers with subcommands and lists flags after them. Rejection demotes below untouched rivals without removing a candidate from the Tab list; acceptance folds into frequency/recency like another real execution.

Priors are ordinary data (`data/priors.toml`, overridable per user), keyed by full command path (`git`, `docker compose`, `gh pr`) and optionally by directory context: `git ` predicts `clone` outside a repository and `status` inside one. Positions the table does not know about — `git commit `, a path argument, a branch name — get no prior at all rather than a guess. Personal history outweighs any global default once a habit exists, and a head-of-list prior is what lets an untyped `brew ` ghost `install` on a fresh install.

## Discovery and cache

Root help is discovered on first encounter through a bounded probe chain: `--help`, `-h`, `help`, `list` (for Symfony Console tools such as Poetry, whose bare `--help` describes a hidden `list` command), and finally the man page at reduced confidence. Parser regressions against captured real-world output (kubectl's grouped sections, Symfony listings) live in `src/help_parser.rs` tests. A nested path is discovered only when the parsed root schema confirms that the user reached a subcommand. The safety layer rejects punctuation-bearing discovery path components, closes stdin, removes pagers, caps output at 2 MiB, and kills timed-out children.

Schemas are stored in an engine-versioned namespace, keyed by command/path, and contain a fingerprint derived from executable location, size, and modification time. Provider results carry short TTLs; official docs carry a seven-day TTL. Cache writes use a temporary file plus rename. Pruning enforces age and total-size bounds.

## Frontend boundary

`adaptive query --buffer ... --cursor ... --cwd ...` is the portable boundary shared by the Zsh and Bash frontends. Frontends may add `--list` for an explicit alternatives listing; without it, the existing prefix-focused response is unchanged. Frontends own keybindings and rendering; they do not need to understand discovery, providers, cache, or ranking.

The Bash side is split in two: `shell/adaptive-core.bash` holds everything that is not about a particular line editor (binary and directory lookup, the TSV query protocol, the per-word candidate cache, history capture, the shell's forgetting rules), and `shell/adaptive.bash` / `shell/adaptive.blesh` are the line-editor adapters. The generated `init.bash` and `init.blesh` files source the core and then the adapter, so the split is invisible to the user.

The two Bash adapters differ only in how a suggestion is rendered. ble.sh paints styled regions, so the suggestion can be dimmed; GNU Readline paints exactly the line buffer, so the suggestion has to live *inside* `READLINE_LINE` (the cursor sits on the boundary between the typed text and the suggestion) and cannot be styled. Both adapters speak the same protocol and cache candidates per word, so typing a word never forks a process.
