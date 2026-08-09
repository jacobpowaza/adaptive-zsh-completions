# Changelog

All notable changes follow [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and semantic versioning.

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
