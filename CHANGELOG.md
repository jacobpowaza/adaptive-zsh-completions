# Changelog

All notable changes follow [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and semantic versioning.

## [0.5.0] - 2026-10-20

### Added

- A real Bash frontend, rebuilt around Readline instead of Tab-only completion. Adaptive now keeps the suggestion *inside* the line buffer (`READLINE_LINE` holds what you typed plus the suggestion, with the cursor on the boundary), because Readline erases anything painted from outside the buffer. The suggestion is visible, every Readline feature keeps working, Right/End accept it, Ctrl-Right accepts one word of it, Ctrl-X discards it, and Shift-Tab walks the candidate list backwards. Tab still completes normally and falls back to Bash's own rules (and to `bash-completion`) whenever the engine has nothing.
- A ble.sh frontend (`adaptive init blesh`): ble.sh replaces Readline with a line editor written in pure Bash that renders styled regions, so the inline suggestion can be *dimmed* — what the Zsh frontend does and what Readline cannot. It plugs into ble.sh's documented extension points (the auto-complete source list and its ordinary completion), and the key map matches the Zsh frontend: arrows cycle candidates, Tab accepts, Shift-Tab lists.
- A shared Bash runtime (`shell/adaptive-core.bash`): locating the binary and the data directories, the TSV query protocol, the per-word candidate cache that keeps typing at Readline's own speed, history capture, and the shell's own forgetting rules (`HISTCONTROL`, `HISTIGNORE`) — evaluated in the recording shell, not in the long-lived daemon.
- `adaptive init bash` and `adaptive init blesh` now write their integration to the data directory and print a `source` line for it, exactly like `adaptive init zsh`, so a Bash shell starts without launching the binary. The generated file also carries the binary's stat identity and regenerates itself after an upgrade.
- The installer detects the shell and wires up Zsh, Bash, or both (`ADAPTIVE_SHELL=auto|bash|zsh|both`, auto by default). The Bash block loads ble.sh when it is installed and falls back to the Readline frontend otherwise, and it is the same marked, idempotent, backed-up block the Zsh path has always used. `ADAPTIVE_BASH_ACCEPT_KEY` and `ADAPTIVE_BASH_BROWSE_KEY` configure the Bash keys in the same caret notation the Zsh settings use.
- `adaptive doctor` reports the Bash line editor in use (ble.sh or GNU Readline) next to the Bash integration status.

### Changed

- The uninstaller removes Adaptive's managed block from `.bashrc` as well as `.zshrc`, backing both up first.

## [0.4.0] - 2026-08-26

### Added

- Optional [carapace-bin](https://carapace-sh.github.io/carapace-bin/) integration: when carapace is on `PATH`, its exact command knowledge (subcommands, flags, enum values, positional and command-specific completions) is queried for any tool it has a completer for and merged with Adaptive's providers, learned history, priors, and help parsing. Answers are cached per position and working directory under a fingerprint of the carapace binary (path/size/mtime), the completer listing for 24 hours; a 400 ms hard timeout plus the frontend's existing off-thread queries keep completion instant, and missing, disabled, slow, or crashing carapace degrades to silence. `carapace.enabled = true` (default) in the config controls it, with `adaptive carapace status | enable | disable` toggles and a doctor line.
- Git workflows gain repo-aware slots alongside Carapace: `git worktree add <TAB>` suggests sibling directories, branches, and existing worktrees; `git branch -d <TAB>` offers only deletable local heads (the checked-out branch excluded); `git cherry-pick <TAB>` offers recent commits by hash with subjects in the display; plain `git branch <TAB>` completes local heads.
- The installer detects carapace and reports it; when absent it explains the optional install (`ADAPTIVE_INSTALL_CARAPACE=yes` tries Homebrew/apt/dnf/pacman/apk automatically, `ask` prompts only on interactive terminals) and always finishes successfully without it.
- A long-lived completion server (`adaptive serve`, managed via `adaptive daemon start | stop | status`): one process holds the usage profile in memory and answers every terminal's queries over a per-user Unix socket, so a keystroke costs a sub-millisecond round trip instead of a process launch plus a profile parse. The Zsh integration prefers the socket automatically, falls back to one-shot processes whenever the server is missing or dies mid-session (auto-spawning it on first use), routes record/feedback through it so learning is instant, clamps the data dir and socket to owner-only access, and exits by itself after 15 idle minutes. `adaptive doctor` reports whether the running server matches your installed version.
- Fuzzy sprawl presets (`tight` / `smart` / `loose`, default smart): how many unmatched characters may sit between typed letters when matching as an in-order subsequence — `gco` reaches `git checkout` under smart but can no longer wander into distant matches. Cycle live with Shift-Right / Shift-Left (`ADAPTIVE_CYCLE_FUZZY_KEY` / `ADAPTIVE_CYCLE_FUZZY_BACK_KEY`) or persist via `adaptive fuzzy <preset|cycle|back>`; the ladder with your active rung prints after every change.
- Whole-command prediction on an untouched prompt is now a persisted global toggle: `adaptive empty on|off|toggle` or Shift-Up (`ADAPTIVE_TOGGLE_EMPTY_KEY`), with `ADAPTIVE_EMPTY=on|off` overriding per shell.
- Session-wide suppression on Ctrl-X (`ADAPTIVE_SUPPRESS_KEY`): pauses all suggestions for this shell only, distinct from ESC dismiss which clears just the current line and records a rejection.
- The shell's own forgetting rules now gate learning: space-prefixed lines are never recorded (the universal HIST_IGNORE_SPACE gesture; Adaptive applies it unconditionally since it cannot see your setopt state) and `HISTORY_IGNORE` glob patterns — including the `(pat1|pat2)` multi-pattern form — drop whole lines at record and import time.
- Keystroke coalescing in the Zsh frontend: while a query is in flight, additional keystrokes mark a pending re-query instead of piling up work, and exactly one follow-up runs for the newest buffer when the result lands.
- An interactive ZLE test harness (`tests/zle_pty.zsh`, wired into `cargo test` as two cases): a real interactive zsh is spawned under a pseudo-terminal via zsh/zpty and driven with actual keystrokes — ghost accept, arrow cycling, Shift-TAB browsing with row selection, shell-escaped directory insertion, Ctrl-X suppression, ESC dismiss — while an in-shell dump widget reports genuine editor state back to the assertions. Both transports run the gauntlet: one-shot fork processes and the live daemon socket.

### Fixed

- The Zsh socket encoder escaped fields through command substitution — three forks per keystroke that silently erased most of the daemon's latency win. Escaping is now pure parameter expansion (measured 0.19 ms per send, fork-free), with a regression test driving hostile tab/backslash buffers through a real server round trip.
- A slow acknowledgement from the daemon (over one second) made the frontend re-send records and feedback through the CLI fallback, double-counting habits. The fallback now fires only when the daemon is genuinely unreachable; a timed-out ack is treated as delivered.
- Concurrent record/feedback requests in the daemon could race their load-modify-save cycle against each other (a lost update) — the whole cycle now runs under the write lock.
- Queries no longer hold the profile lock while discovery runs its subprocesses, so a slow `--help` probe can never delay a concurrent record or feedback write.
- Resetting UI state mid-query (Ctrl-X, ESC, history navigation) left the coalescing counters set, so the next keystroke inside the 50 ms window could mark a pending re-query that nothing would ever consume; resets now retire the in-flight request id as well.
- Truncated or failed result payloads clear both candidate views instead of leaving the previous query's suggestions on screen under the new buffer.
- The generated init script falls back to default ghost settings when the saved config is broken, instead of failing shell startup; `adaptive daemon stop` says "no daemon is running" instead of leaking a connect error; fractional-mtime filesystems no longer trip the init-cache staleness check.
- Daemon-served candidate rows stripped backslashes from shell-escaped values (`cd adaptive\ test` arrived as `cd adaptive test`, splitting into two words on accept); both transports now emit byte-identical payloads, covered by a live parity check.
- The daemon's wire escaping leaked through to the frontend after that parity fix: socket rows arrive frame-escaped (`spaced\\ dir`), but the frontend never unescaped them, so an accepted completion inserted doubled backslashes and split the path into two words — the fork path inserts one backslash. The frontend now decodes wire-escaped socket payloads (pure parameter expansion, no forks), making the two transports identical after framing; caught by the new interactive ZLE harness on its first daemon-socket run.
- A ghost answer crossing the wire after Shift-TAB could repaint over the open candidate list; browsing retires the in-flight request and ghost rendering refuses while a selector is up, and every counted socket read carries a two-second deadline so a daemon dying mid-payload can never freeze the prompt.
- A corrupt `history.json` silently disabled learning everywhere and wedged `history` commands behind a manual file deletion; all paths now degrade to a fresh enabled profile and the next save rewrites the file.
- `HISTORY_IGNORE` was evaluated against the daemon's environment rather than the recording shell's; the frontend now applies the shell's own forgetting rules locally before reporting a line.
- The Zsh structural test's assertions were vacuous (a failed `[[ ]]` did not fail the run, and editor state was never initialized), so it protected nothing despite passing; the harness now aborts on any failed assertion and drives real buffer/cursor state.


### Changed

- Performance pass on the query pipeline: the warm carapace path touches exactly two filesystem entries (one binary-fingerprint stat, one cached-answer read); the completer listing and negative answers are consulted only on cache misses; provider subprocesses (git branches, npm scripts, ...) and carapace now run concurrently where both apply instead of sequentially; the usage profile is no longer parsed at all when history is disabled; and the HTTP client shrank from reqwest/tokio to `ureq` (binary 5.3 MB → 4.6 MB, same rustls TLS, identical forge/docs behavior). In-process, a fully warm query — carapace included — completes in about 60 µs; remaining end-to-end latency is process startup.
- Shell startup no longer launches the binary: `adaptive init zsh` writes the integration to `<data dir>/init.zsh` once and prints a `source` line for it; the installer appends a direct-source block with an eval fallback. The generated script compares the installed binary's stat identity against its own baked copy (one builtin stat, no subprocess) and regenerates itself in the background after an upgrade — the current session keeps running what it loaded, the next shell picks up the fresh file.
- Engine queries take an explicit profile reference, letting the daemon share one parsed copy across every connection while CLI invocations keep loading their own.

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
[0.3.0]: https://github.com/jacobpowaza/adaptive-zsh-completions/releases/tag/v0.3.0
[0.4.0]: https://github.com/jacobpowaza/adaptive-zsh-completions/releases/tag/v0.4.0
