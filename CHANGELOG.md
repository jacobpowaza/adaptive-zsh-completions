# Changelog

All notable changes follow [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and semantic versioning.

## [Unreleased]

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
