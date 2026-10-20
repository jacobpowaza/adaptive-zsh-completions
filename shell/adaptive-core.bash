#!/usr/bin/env bash
# Adaptive shared runtime -- shell-independent logic for the Bash frontend.
#
# This file holds everything that is NOT about a particular line editor:
#   * locating the adaptive binary and its data directories
#   * the query protocol (running `adaptive query`, parsing its TSV answer)
#   * candidate caching and prefix filtering
#   * history capture and the shell's forgetting rules
#
# The Zsh frontend deliberately keeps its own copy of the transport: it drives
# the engine through ZLE's `zle -F` asynchronous socket pump, which cannot be
# expressed here. The parsing rules below are the same contract, and the
# frontend tests pin both sides to the same protocol.
#
# Requires bash >= 4 (associative arrays, mapfile, printf %q).

# ---------------------------------------------------------------------------
# locations
# ---------------------------------------------------------------------------

: "${ADAPTIVE_BIN_PATH:=}"
_adaptive_find_bin() {
  if [[ -n ${ADAPTIVE_BIN_PATH:-} && -x ${ADAPTIVE_BIN_PATH} ]]; then
    return 0
  fi
  if command -v adaptive >/dev/null 2>&1; then
    ADAPTIVE_BIN_PATH=$(command -v adaptive)
    return 0
  fi
  return 1
}

# Where the daemon socket lives. Mirrors the Zsh frontend's precedence:
# explicit ADAPTIVE_SOCK, then ADAPTIVE_DATA_DIR, then the baked default.
_adaptive_init_paths() {
  if [[ -n ${ADAPTIVE_SOCK:-} ]]; then
    :
  elif [[ -n ${ADAPTIVE_DATA_DIR:-} ]]; then
    ADAPTIVE_SOCK="$ADAPTIVE_DATA_DIR/adaptive.sock"
  else
    ADAPTIVE_SOCK="${ADAPTIVE_DEFAULT_SOCK:-${XDG_DATA_HOME:-$HOME/.local/share}/adaptive/adaptive.sock}"
  fi
  export ADAPTIVE_SOCK
}

# Let one-shot query processes start the daemon when nobody is home yet, so
# the first Tab press after a reboot does not pay the cold-start cost.
export ADAPTIVE_AUTO_DAEMON=${ADAPTIVE_AUTO_DAEMON:-1}

# ---------------------------------------------------------------------------
# diagnostics
# ---------------------------------------------------------------------------

_adaptive_log() {
  [[ -n ${ADAPTIVE_DEBUG_LOG:-} ]] || return 0
  printf '%s %s\n' "${EPOCHREALTIME:-0}" "$*" >>"$ADAPTIVE_DEBUG_LOG" 2>/dev/null || true
}

# ---------------------------------------------------------------------------
# the query protocol
# ---------------------------------------------------------------------------

# `_adaptive_query <buffer> <cursor> <list:0|1>` prints the engine's TSV
# answer: a header line (request-id, prefix-len, count, context-key) followed
# by `count` rows of `value<TAB>display<TAB>description<TAB>source`.
#
# Returns non-zero when the engine is unreachable, so callers can fall back
# to stock readline completion rather than failing the prompt.
_adaptive_query() {
  local buffer=$1 cursor=$2 list=$3
  _adaptive_find_bin || return 1
  local -a args=()
  if [[ $list == 1 ]]; then args+=(--list); fi
  "${ADAPTIVE_BIN_PATH}" query "${args[@]}" --format zsh \
    --buffer "$buffer" --cursor "$cursor" \
    --cwd "$PWD" --prev "${_ADAPTIVE_PREV_CMD:-}" 2>/dev/null
}

# Parse a TSV answer into the globals ADAPTIVE_Q_CONTEXT / ADAPTIVE_Q_COUNT /
# ADAPTIVE_Q_VALUES / ADAPTIVE_Q_DISPLAYS.
_adaptive_parse() {
  ADAPTIVE_Q_CONTEXT=
  ADAPTIVE_Q_COUNT=0
  ADAPTIVE_Q_VALUES=()
  ADAPTIVE_Q_DISPLAYS=()
  local header=0 line value display
  local -a fields=()
  while IFS= read -r line; do
    [[ -n $line ]] || continue
    mapfile -td $'\t' fields <<<"$line"
    if (( ! header )); then
      header=1
      ADAPTIVE_Q_CONTEXT="${fields[3]:-}"
      ADAPTIVE_Q_COUNT="${fields[2]:-0}"
      continue
    fi
    value="${fields[0]:-}"
    display="${fields[1]:-$value}"
    [[ -n $value ]] || continue
    ADAPTIVE_Q_VALUES+=("$value")
    ADAPTIVE_Q_DISPLAYS+=("$display")
  done
  return 0
}

# ---------------------------------------------------------------------------
# candidate cache
# ---------------------------------------------------------------------------
# The engine returns the full ranked list for the current word when asked at a
# word boundary, so the frontend asks once per word and filters locally as the
# user types. This keeps typing at readline's own speed: no process is forked
# for the keystrokes inside a word.
#
_ADAPTIVE_CACHE_KEY=
declare -a _ADAPTIVE_CACHE_VALUES=() _ADAPTIVE_CACHE_DISPLAYS=()

# The cache is keyed by everything that changes the answer: the query position,
# the directory, the previous command, and the data dirs. Newline-separated
# rather than NUL-separated, because command substitution strips NUL bytes and
# would emit a warning on every keystroke.
_adaptive_cache_key() {
  printf '%s\n%s\n%s\n%s\n%s' "$1" "$PWD" "${_ADAPTIVE_PREV_CMD:-}" \
    "${ADAPTIVE_DATA_DIR:-}" "${ADAPTIVE_SOCK:-}"
}

# Refresh the cache for the word before the cursor.
#
# The engine is asked once, at the word boundary -- i.e. with the line as the
# user has typed it plus a trailing space, which is the position where "what
# can go here?" is unambiguous. Its answer already contains every candidate
# for that position, including ones the user has since typed a prefix of, so
# the frontend filters locally for the rest of the word and forks nothing.
#
# Returns 1 when the engine produced nothing, leaving the cache empty.
_adaptive_cache_load() {
  local typed=${READLINE_LINE:0:$READLINE_POINT} prefix key
  # The query position is the last word boundary, so the cache key stays
  # stable while the user types the rest of the word. Otherwise every
  # keystroke would miss the cache and fork a process.
  prefix=$typed
  while [[ $prefix == *[![:space:]] ]]; do prefix=${prefix%?}; done
  key=$(_adaptive_cache_key "$prefix")
  if [[ $_ADAPTIVE_CACHE_KEY == "$key" ]]; then
    return 0
  fi
  _ADAPTIVE_CACHE_KEY=
  _ADAPTIVE_CACHE_VALUES=()
  _ADAPTIVE_CACHE_DISPLAYS=()
  local cursor=${#prefix}
  [[ $cursor -gt 0 ]] || return 1
  local out
  out=$(_adaptive_query "$prefix" "$cursor" 1) || return 1
  [[ -n $out ]] || return 1
  _adaptive_parse <<<"$out"
  (( ${#ADAPTIVE_Q_VALUES[@]} )) || return 1
  # De-duplicate while preserving rank order; the engine deliberately emits
  # the same value from several providers.
  local -A seen=()
  local v
  for v in "${ADAPTIVE_Q_VALUES[@]}"; do
    [[ -z ${seen[$v]:-} ]] || continue
    seen[$v]=1
    _ADAPTIVE_CACHE_VALUES+=("$v")
  done
  _ADAPTIVE_CACHE_KEY=$key
  _adaptive_log "cache_load prefix=[$prefix] n=${#_ADAPTIVE_CACHE_VALUES[@]}"
  return 0
}

# The best cached candidate that starts with `$1`, assigned to
# $_ADAPTIVE_BEST. Assigning rather than printing keeps this off the fork
# path: it runs on every keystroke, and command substitution would spawn a
# subshell each time.
_adaptive_cache_best() {
  local prefix=$1 v
  _ADAPTIVE_BEST=
  for v in ${_ADAPTIVE_CACHE_VALUES[@]+"${_ADAPTIVE_CACHE_VALUES[@]}"}; do
    [[ $v == "$prefix"* ]] || continue
    [[ $v != "$prefix" ]] || continue
    _ADAPTIVE_BEST=$v
    return 0
  done
  return 1
}

# Run a helper process in the background without letting it touch the prompt.
#
# An interactive shell has job-control monitor mode on, so every `cmd &` makes
# Bash announce the new job -- writing "[1] 12345" into the command line the
# user is editing. Readline then treats that text as typed input, which
# silently corrupts the line and, worse, can end up in the command that runs.
#
# Redirecting the enclosing group's stderr suppresses the announcement (turning
# monitor mode off with `set +m` does not), and the job is disowned so it is
# not reported at the next prompt either.
_adaptive_bg() {
  { "$@" >/dev/null 2>&1 & } 2>/dev/null
  disown 2>/dev/null || true
  return 0
}

# ---------------------------------------------------------------------------
# history
# ---------------------------------------------------------------------------
# Bash has no preexec hook, so the line that was just accepted is recovered
# from Bash's own history after the fact. That is accurate for anything typed
# at the prompt and keeps the engine's forgetting rules identical.

# The shell's own forgetting rules, evaluated HERE rather than in the daemon:
# HISTORY_IGNORE belongs to this shell's environment, and a long-lived server
# may have been spawned under a different one. Mirrors the Zsh frontend.
#  - HISTCONTROL=ignorespace / ignoreboth: leading space means "not history"
#  - HISTIGNORE patterns, including the (pat1|pat2) multi-pattern form
_adaptive_locally_ignored() {
  local line=$1 body alt
  case ${HISTCONTROL:-} in
    *ignorespace*|*ignoreboth*) [[ $line == ' '* || $line == $'\t'* ]] && return 0 ;;
  esac
  [[ -n ${HISTIGNORE:-} ]] || return 1
  body=${HISTIGNORE#\(}
  body=${body%\)}
  # bash honours the first match only, like HIST_IGNORE in zsh
  local first=${body%%|*}
  if [[ $first != "$body" ]]; then
    first=${first#\(}; first=${first%\)}
  fi
  [[ -n $first ]] || return 1
  # shellcheck disable=SC2053  # intentional glob match
  [[ $line == $first ]] && return 0
  return 1
}

# Record one accepted line into the usage profile. Fire-and-forget: the CLI
# owns its own persistence locking, so a slow disk never blocks the prompt.
_adaptive_history_record() {
  local line=${1%$'\n'}
  [[ -n $line ]] || return 0
  _adaptive_log "history_record [$line]"
  if _adaptive_locally_ignored "$line"; then
    _adaptive_log "history_record ignored_by_shell"
    _ADAPTIVE_PREV_CMD=$line
    return 0
  fi
  if _adaptive_find_bin; then
    _adaptive_bg "${ADAPTIVE_BIN_PATH}" history record \
      --cwd "$PWD" --prev "${_ADAPTIVE_PREV_CMD:-}" -- "$line"
  fi
  _ADAPTIVE_PREV_CMD=$line
  return 0
}

# One-time bootstrap: fold the existing shell history into the usage profile so
# day-one predictions reflect months of habits. The CLI skips repeats via its
# own marker file.
_adaptive_history_import() {
  [[ -n ${HISTFILE:-} && -r ${HISTFILE:-} ]] || return 0
  _adaptive_find_bin || return 0
  _adaptive_bg "${ADAPTIVE_BIN_PATH}" history import
}

# Learning feedback for an explicit accept or a dismissal.
_adaptive_feedback() {
  local verdict=$1 value=$2 key=$3
  [[ -n $value && -n $key ]] || return 0
  _adaptive_find_bin || return 0
  _adaptive_bg "${ADAPTIVE_BIN_PATH}" history feedback "$verdict" \
    --context "$key" --value "$value"
}