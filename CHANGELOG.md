# Changelog

All notable changes follow [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and semantic versioning.

## [Unreleased]

### Changed

- Performance pass on the query pipeline: the warm carapace path touches exactly two filesystem entries (one binary-fingerprint stat, one cached-answer read); the completer listing and negative answers are consulted only on cache misses; provider subprocesses (git branches, npm scripts, ...) and carapace now run concurrently where both apply instead of sequentially; the usage profile is no longer parsed at all when history is disabled; and the HTTP client shrank from reqwest/tokio to `ureq` (binary 5.3 MB → 4.6 MB, same rustls TLS, identical forge/docs behavior). In-process, a fully warm query — carapace included — completes in about 60 µs; remaining end-to-end latency is process startup.

### Added

- Optional [carapace-bin](https://carapace-sh.github.io/carapace-bin/) integration: when carapace is on `PATH`, its exact command knowledge (subcommands, flags, enum values, positional and command-specific completions) is queried for any tool it has a completer for and merged with Adaptive's providers, learned history, priors, and help parsing. Answers are cached per position and working directory under a fingerprint of the carapace binary (path/size/mtime), the completer listing for 24 hours; a 400 ms hard timeout plus the frontend's existing off-thread queries keep completion instant, and missing, disabled, slow, or crashing carapace degrades to silence. `carapace.enabled = true` (default) in the config controls it, with `adaptive carapace status | enable | disable` toggles and a doctor line.
- Git workflows gain repo-aware slots alongside Carapace: `git worktree add <TAB>` suggests sibling directories, branches, and existing worktrees; `git branch -d <TAB>` offers only deletable local heads (the checked-out branch excluded); `git cherry-pick <TAB>` offers recent commits by hash with subjects in the display; plain `git branch <TAB>` completes local heads.
- The installer detects carapace and reports it; when absent it explains the optional install (`ADAPTIVE_INSTALL_CARAPACE=yes` tries Homebrew/apt/dnf/pacman/apk automatically, `ask` prompts only on interactive terminals) and always finishes successfully without it.

## [0.3.0] - 2026-08-23

### Added

- Bash frontend over the same query protocol: `eval "$(adaptive init bash)"` powers Tab for every command without its own completion function, falls back to native filename completion when the engine has nothing to say, reports single-match accepts through the context key, and predicts the first word on an empty line. Requires bash >= 4; live line recording remains Zsh-only because Bash has no dependable preexec hook.
- Real-world help fixtures (kubectl's grouped command sections, Poetry/Symfony Console listings) as parser regression tests, and a functional test harness (`tests/frontend.bash`) driving the Bash frontend end to end against shim CLIs built from captured help output.
- README previews: a recorded ghost-text demo and a `git clone` repository-browsing screenshot replace the static-only examples.

### Fixed

- Symfony Console tools (Poetry, Composer, phpstan, ...) now discover their commands: their bare `--help` describes a hidden `list` command instead of listing anything, so discovery probes `list` directly when that signature is detected.
- Invocation-shaped lines inside prose sections (Examples, Help) no longer become phantom subcommands — `poetry list` inside Poetry's help text used to surface as a bogus completion candidate.
- `git clone ` ghosts the generic `https://github.com/` prefix at the URL slot instead of a remembered repository URL, and the slot keeps answering with the prefix after clone's own flags (`git clone --depth 1 `); a specific repository is reached by typing its owner, which re-engages repository browsing.

### Changed

- The global prior table is documented for what it is now — ranking and prediction data plus common subcommands for undocumented positions — after kubectl's grouped `--help` sections were verified to parse directly with captured-output tests.

## [0.2.1] - 2026-08-23

### Added

- Shift-TAB (`ADAPTIVE_BROWSE_KEY`) as a dedicated browse gesture: from any state, including while an inline ghost suggestion is showing, it lists every candidate with the predicted pick pre-highlighted. Previously the candidate list was only reachable when no ghost was visible, so proactive predictions made Tab feel like autocorrect for anyone looking for alternatives.

### Fixed

- The installer's `ADAPTIVE_MENU_NEXT_KEY` customization now reaches the frontend, which was reading a different variable name and silently ignoring it.
- README and configuration docs no longer reference removed knobs (`menu`, `enter_accepts_menu`, `ADAPTIVE_ENTER_ACCEPTS_MENU`) and describe Tab/Shift-TAB behavior as shipped.

## [0.2.0] - 2026-08-23

### Added

- Persistent learned usage profile: next-token transitions per exact command path, command-to-command sequences, directory affinities, and explicit accept/reject feedback — all with half-life recency weighting so recent habits outweigh old ones without erasing them (`history.json` schema v2; old files load unchanged).
- Proactive prediction at untyped positions: a trailing space (`git `, `claude `, `brew `) and even an empty prompt immediately ghost the most likely continuation, driven by a dominance rule over the learned model (enough recent use plus enough probability share), not by hardcoded defaults.
- Full multi-token chain prediction: a typed prefix completes to an entire remembered line (`c` → `claude --dangerously-skip-permissions`), and prediction continues after every accepted token.
- `adaptive history import [file]` bootstraps the profile from an existing shell history file (default `$HISTFILE`, EXTENDED_HISTORY stamps handled); the Zsh frontend runs it once in the background on first start so day-one predictions reflect months of habits.
- `adaptive history feedback accept|reject --context KEY --value VALUE` records explicit accept/dismiss decisions; the engine echoes a `context_key` with each response so frontends never re-derive position internals.
- `adaptive query --prev LINE` and `adaptive history record --cwd DIR --prev LINE` feed sequence and directory learning.
- New `[completion]` knobs: `predictive` (untyped-position prediction), `chain_prediction` (whole-line completions), `half_life_days` (recency decay, default 21).

### Fixed

- Typing a brand-new flag on a position a dynamic provider covers (branches after `git checkout`, remotes after `git push`) now reads the command's own help/settings list instead of returning nothing; a leading `-` always consults documentation.
- The explicit Tab listing at provider-covered slots now includes the subcommand's documented options alongside its dynamic values.

### Changed

- Untyped positions now ghost the engine's top-ranked prediction immediately instead of staying silent until a letter is typed: arrow keys cycle alternatives, ESC dismisses (recorded as rejection feedback), TAB browses everything.
- Learned next-token predictions now merge with discovery results at every position instead of acting only as a last-resort fallback, and personal frequency/recency at a position is served from precomputed transitions rather than rescanning raw history lines.
- Rejection feedback demotes a candidate below untouched rivals while keeping it reachable in the Tab list.
- Global usage priors for bare command names are applied to learned candidates as a tie-breaker; directory affinity outranks them.
- Global usage priors (`data/priors.toml`, extendable per user via `~/.config/adaptive/priors.toml` or `ADAPTIVE_PRIORS`) keyed by full command path, with directory-context variants for Git work trees and Node projects.
- Well-known subcommands as completion candidates for tools whose `--help` cannot be parsed, such as `kubectl`.
- Rank candidates by position validity, global usage, personal frequency and recency, and prefix quality, instead of source confidence with per-command nudges.
- Rank subcommands above flags wherever a subcommand is valid, and keep flags in the Tab list after them.
- Predict the next token at an untyped position for any command with a known default (`brew ` to `install`, `docker compose ` to `up`), not just for a hardcoded few.
- Let personal history outrank global defaults for bare command names, so a heavy `gh` user gets `gh` before `git`.

## [0.1.2] - 2026-08-09

### Changed

- Show the highest-ranked command, flag, branch, script, or repository candidate as an inline recommendation even when alternatives exist.
- Show Tab alternatives as an unnumbered shell-style list while keeping the highest-ranked candidate inline.
- Rank blank `cd` suggestions from current-directory entries and learned local usage.
- Add an opt-in `adaptive query --list` protocol mode without changing ordinary query responses.
- Match the previous local autosuggestion appearance with a darker `fg=245` default.

## [0.1.1] - 2026-08-09

### Changed

- Render single suggestions inline in configurable dim-gray ZLE highlighting.
- Prefer one immediate `https://github.com/` ghost suggestion for an empty `git clone` argument while retaining GitLab, Codeberg, Gitea, and Bitbucket resolution for typed forge URLs.

## [0.1.0] - 2026-08-09

### Added

- Shell-independent Rust completion engine and JSON/Zsh protocols.
- Lazy generic help discovery, recursive subcommand schemas, native Cobra adapter, official documentation resolver, fingerprinted cache, pruning, and confidence-aware ranking.
- GitHub, GitLab, Codeberg/Forgejo, Gitea, and Bitbucket repository completion plus Git branch/remote, filesystem, npm/pnpm/Yarn, SSH, and Docker providers.
- Sanitized local history learning with enable, disable, status, and clear controls.
- Zsh ghost text, Tab acceptance, numbered compact menus, Right/arrow navigation, cancellation, configurable keys, and Emacs-keymap-compatible behavior.
- Idempotent checksum-verifying installer, safe uninstaller, CI, release builds, and contributor documentation.

[0.1.0]: https://github.com/jacobpowaza/adaptive-zsh-completions/releases/tag/v0.1.0
[0.1.1]: https://github.com/jacobpowaza/adaptive-zsh-completions/releases/tag/v0.1.1
[0.1.2]: https://github.com/jacobpowaza/adaptive-zsh-completions/releases/tag/v0.1.2
[0.2.0]: https://github.com/jacobpowaza/adaptive-zsh-completions/releases/tag/v0.2.0
[0.2.1]: https://github.com/jacobpowaza/adaptive-zsh-completions/releases/tag/v0.2.1
