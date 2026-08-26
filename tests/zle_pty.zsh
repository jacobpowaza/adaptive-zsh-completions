#!/usr/bin/env zsh
# Interactive ZLE harness: drives the real frontend through real keystrokes
# on a pseudo-terminal and asserts on genuine editor state.
#
# The structural tests instantiate the frontend's functions directly; this
# harness complements them with what they cannot see: widget flows as a key
# board actually delivers them (self-insert churn, TAB accept, Shift-TAB
# browse, arrow cycling, ESC dismiss, Ctrl-X suppression) against a live zsh
# whose prompt, event loop, and zle -F watchers are fully running.
#
# Mechanism: zsh/zpty spawns `zsh -d -i` with a generated .zshrc that sources
# `adaptive init zsh` and binds a state-dump widget (^K) appending one line of
# editor state (buffer, POSTDISPLAY, selector/index/suppression flags, word
# count under shell splitting) to a file. The driver sends keys, waits for
# monotonically numbered dump lines, and compares fields. Assertions never
# screen-scrape rendered terminal bytes; the dump IS the state.
#
# Inputs (environment):
#   ADAPTIVE_PTY_BIN     adaptive binary under test
#   ADAPTIVE_PTY_BASE    scratch directory (created if missing)
#   ADAPTIVE_PTY_DAEMON  1 => run against a live daemon socket; 0 => fork only
#
# Exits nonzero if any scenario failed. Skips cleanly (exit 0) when zsh/zpty
# is unavailable.

setopt err_exit

(( $+commands[zsh] )) || { print -u2 "skip: zsh not found"; exit 0 }
if ! zmodload zsh/zpty 2>/dev/null; then
  print -u2 "skip: zsh/zpty module unavailable"
  exit 0
fi
zmodload zsh/datetime 2>/dev/null || true

[[ -n "$ADAPTIVE_PTY_BIN" && -x "$ADAPTIVE_PTY_BIN" ]] || {
  print -u2 "ADAPTIVE_PTY_BIN must point at the adaptive binary"
  exit 1
}
BASE=${ADAPTIVE_PTY_BASE:-$(mktemp -d)}
DAEMON_MODE=${ADAPTIVE_PTY_DAEMON:-0}

BIN=$ADAPTIVE_PTY_BIN
RC_DIR=$BASE/rc
RUN_DIR=$BASE/run
OUT_DIR=$BASE/out
SHIM_BIN=$BASE/bin
DUMP_FILE=$OUT_DIR/state.dump
RAW_LOG=$OUT_DIR/raw.log
for d in "$RC_DIR" "$RUN_DIR" "$OUT_DIR" "$SHIM_BIN"; do
  mkdir -p "$d"
done
: > "$DUMP_FILE"
: > "$RAW_LOG"

# ---- hermetic environment (overrides anything leaked from the caller) -------
export ADAPTIVE_DATA_DIR=$BASE/data
export ADAPTIVE_CACHE_DIR=$BASE/cache
export ADAPTIVE_CONFIG=$BASE/config.toml
# Hermetic like every other test: the optional carapace source stays off so
# candidate sets never depend on what the host has installed.
printf '[carapace]\nenabled = false\n' > "$ADAPTIVE_CONFIG"
export HISTFILE=$BASE/hist-empty
: > "$HISTFILE"
unset ADAPTIVE_SOCK ADAPTIVE_DEFAULT_SOCK
# Frontend trace for post-mortem (only written when this var is set).
export ADAPTIVE_DEBUG_LOG=$OUT_DIR/frontend.log
export TERM=xterm-256color COLUMNS=100 LINES=30
if [[ "$DAEMON_MODE" == 1 ]]; then
  unset ADAPTIVE_NO_DAEMON
else
  export ADAPTIVE_NO_DAEMON=1
fi
# A tiny world: only the shimmed tool answers discovery probes, so candidate
# order is decided by seeded habits, never by whatever the host has installed.
export PATH="$SHIM_BIN:$PATH"

cat > "$SHIM_BIN/demo-tool" <<'SHIM'
#!/bin/sh
cat <<'EOF'
Usage: demo-tool [COMMAND]

Commands:
  alpha   First choice
  apricot Second choice
EOF
SHIM
chmod +x "$SHIM_BIN/demo-tool"
mkdir "$RUN_DIR/spaced dir"

seed() {
  "$BIN" history record --cwd "$RUN_DIR" --prev "" -- "$@" >/dev/null || exit 1
}
# Frequency decides ranking: alpha dominates every demo-tool slot, the schema
# contributes the same two verbs, so candidates collapse to a stable pair.
seed demo-tool alpha
seed demo-tool alpha
seed demo-tool alpha
seed demo-tool apricot

if [[ "$DAEMON_MODE" == 1 ]]; then
  "$BIN" daemon start >/dev/null || { print -u2 "daemon did not start"; exit 1 }
fi

cleanup() {
  local rc=$?
  if [[ "$DAEMON_MODE" == 1 ]]; then
    "$BIN" daemon stop >/dev/null 2>&1
  fi
  zpty -d pty 2>/dev/null
  return $rc
}
trap cleanup EXIT

CTRL_X=$'\x18'

# ---- inner shell bootstrap ----------------------------------------------------
cat > "$RC_DIR/.zshrc" <<'RC'
bindkey -e
PS1='ptystate> '
stty rows 30 cols 100 2>/dev/null
eval "$("$ADAPTIVE_PTY_BIN" init zsh)"

# State probe: appends one line of genuine editor state. Backslashes, tabs,
# and newlines are escaped so every record stays a single parseable line;
# `words` counts BUFFER the way zsh would parse it as a command line
# (${(z)} — quotes and backslash escapes respected), which is exactly how an
# accepted completion lands on the command line.
typeset -gi ADAPTIVE_PTY_SEQ=0
adaptive-pty-dump() {
  local b=$BUFFER p=$POSTDISPLAY
  b=${b//\\/\\\\}; b=${b//$'\n'/\\n}; b=${b//$'\r'/\\r}; b=${b//$'\t'/\\t}
  p=${p//\\/\\\\}; p=${p//$'\n'/\\n}; p=${p//$'\r'/\\r}; p=${p//$'\t'/\\t}
  local -a w
  w=(${(z)BUFFER})
  (( ++ADAPTIVE_PTY_SEQ ))
  [[ -n "$ADAPTIVE_PTY_DUMP_FILE" ]] || return 0
  print -r -- "seq=$ADAPTIVE_PTY_SEQ mode=$ADAPTIVE_QUERY_MODE buf=[$b] post=[$p] sel=$ADAPTIVE_SELECTOR_ACTIVE idx=$ADAPTIVE_INDEX lidx=$ADAPTIVE_LIST_INDEX sup=$ADAPTIVE_SUPPRESSED cursor=$CURSOR words=$#w" >>! "$ADAPTIVE_PTY_DUMP_FILE"
}
zle -N adaptive-pty-dump
# F5, chosen deliberately: a bare control key would collide with emacs
# multikey prefixes (^X is one -- ^X^K is kill-buffer -- so a ^K probe right
# after Ctrl-X would wipe the line instead of dumping state).
bindkey '\e[15~' adaptive-pty-dump
# Binding inventory for post-mortem (tiny artifact, invaluable when a key
# mysteriously does not do what this harness expects).
bindkey -L >! "$ADAPTIVE_PTY_DUMP_FILE.bindings" 2>/dev/null
print PTY_READY
RC

cd "$RUN_DIR" || exit 1
export ZDOTDIR=$RC_DIR
export ADAPTIVE_PTY_DUMP_FILE=$DUMP_FILE
zpty -b pty zsh -d -i

# ---- helpers ------------------------------------------------------------------

FAILURES=0
SEQ=0

# Drains whatever the child has written so far into raw.log. zpty -r never
# blocks -- it fails as soon as no complete read is pending -- so this simply
# empties the pty buffer; called between every driver step, the buffer stays
# far from full and no background reader is needed.
pump() {
  local chunk
  while zpty -r pty chunk 2>/dev/null; do
    print -rn -- "$chunk" >> "$RAW_LOG"
  done
}

# Sleep, then collect anything the child produced meanwhile (async query
# completions render while the driver idles).
nap() { sleep "$1"; pump }

ok() { print "ok   $*"; }
fail() { print -u2 "FAIL $*"; (( ++FAILURES )); }
expect_eq() {
  local label=$1 got=$2 want=$3
  if [[ "$got" == "$want" ]]; then
    ok "$label"
  else
    fail "$label: got [$got] want [$want]"
  fi
}
die() {
  print -u2 -- "ABORT: $*"
  print -u2 -- "--- raw.log tail ---"
  tail -c 2000 "$RAW_LOG" 2>/dev/null >&2 || true
  print -u2 -- "--- state.dump tail ---"
  tail -5 "$DUMP_FILE" 2>/dev/null >&2 || true
  print -u2 -- "--- frontend trace tail ---"
  tail -15 "$OUT_DIR/frontend.log" 2>/dev/null >&2 || true
  print -u2 -- "--- live processes ---"
  ps -eo pid,ppid,pgid,stat,command 2>/dev/null | grep -E "adaptive|zsh -d -i" | grep -v grep >&2 || true
  print -u2 -- "--- out dir ---"
  ls -la "$OUT_DIR" >&2 || true
  print -u2 -- "--- dump file var ---"
  [[ -f "$DUMP_FILE" ]] && wc -l "$DUMP_FILE" >&2 || print -u2 -- "dump file missing: $DUMP_FILE"
  exit 1
}

press() { zpty -w -n pty "$1"; }

type_text() {
  local text=$1 i
  for (( i = 1; i <= $#text; i++ )); do
    press "$text[i]"
    sleep 0.02
  done
  pump
}

clear_line() {
  press $'\C-u'
  nap 0.05
}

# Waits for dump line `n`. Non-fatal: returns 1 when it does not show up in
# time (callers may retry).
poll_seq() {
  local want=$1 line deadline=$(( EPOCHSECONDS + 2 ))
  while (( EPOCHSECONDS < deadline )); do
    pump
    line=$(grep "^seq=$want " "$DUMP_FILE" 2>/dev/null | tail -1)
    if [[ -n "$line" ]]; then
      LAST_LINE=$line
      return 0
    fi
    sleep 0.05
  done
  return 1
}

# Waits for dump line `n`; fatal when lost, since every later assertion would
# be meaningless.
wait_seq() {
  poll_seq "$1" || die "timed out waiting for dump seq=$1"
}

dump_now() {
  local want=$(( SEQ + 1 )) try
  # A keystroke can be consumed by a zle -M message redraw (Ctrl-X shows one)
  # or by KEYTIMEOUT merging, so the press is retried until its dump lands.
  for (( try = 0; try < 4; try++ )); do
    press $'\e[15~'
    if poll_seq "$want"; then
      SEQ=$want
      return 0
    fi
    nap 0.2
  done
  die "dump seq=$want never appeared after retries"
}

# Bracketed field -> REPLY (values are escape-sanitized, never contain ']').
field() {
  local rest=${1#*$2=\[}
  if [[ "$rest" == "$1" ]]; then
    REPLY=""
    return 1
  fi
  REPLY=${rest%%]*}
}

# Scalar field -> REPLY.
flagv() {
  local rest=${1#*$2=}
  if [[ "$rest" == "$1" ]]; then
    REPLY=""
    return 1
  fi
  REPLY=${rest%% *}
}

# Types a prefix, then dumps repeatedly until a ghost renders. Fatal when the
# suggestion pipeline produces nothing within budget.
wait_ghost() {
  local i post
  for (( i = 0; i < 40; i++ )); do
    dump_now
    field "$LAST_LINE" post && post=$REPLY
    if [[ -n "$post" ]]; then
      return 0
    fi
    nap 0.15
  done
  die "ghost never appeared"
}

# Waits until the selector view is open.
wait_selector() {
  local i sel
  for (( i = 0; i < 40; i++ )); do
    dump_now
    flagv "$LAST_LINE" sel && sel=$REPLY
    if [[ "$sel" == 1 ]]; then
      return 0
    fi
    nap 0.15
  done
  die "selector never opened"
}

# Waits for the inner shell to finish booting.
local ready_deadline=$(( EPOCHSECONDS + 15 ))
until grep -q PTY_READY "$RAW_LOG" 2>/dev/null; do
  (( EPOCHSECONDS < ready_deadline )) || die "inner shell never became ready"
  nap 0.1
done

# ---- scenarios ---------------------------------------------------------------

# S0 boot + transport: the very first line-init query must have chosen the
# transport this run is meant to exercise.
dump_now
flagv "$LAST_LINE" mode
if [[ "$DAEMON_MODE" == 1 ]]; then
  expect_eq "boot transport is the daemon socket" "$REPLY" sock
else
  expect_eq "boot transport is fork" "$REPLY" fork
fi
flagv "$LAST_LINE" cursor
expect_eq "fresh prompt cursor at origin" "$REPLY" 0

# S1 ghost + TAB accept end to end: typed prefix ghosts the learned verb,
# TAB inserts value plus separating space, word count proves one command.
type_text "demo-tool al"
wait_ghost
field "$LAST_LINE" post
expect_eq "ghost suffix for typed prefix" "$REPLY" 'pha'
flagv "$LAST_LINE" idx
expect_eq "top-ranked habit ghosts first" "$REPLY" 1
press $'\t'
nap 0.2
dump_now
field "$LAST_LINE" buf
expect_eq "TAB accepts ghost into buffer" "$REPLY" 'demo-tool alpha '
flagv "$LAST_LINE" words
expect_eq "accepted buffer is two words" "$REPLY" 2

# S2 arrow cycling: with two prefix-consistent verbs the ghost shows its best
# guess and alternatives sit one Right away; accepting inserts the highlighted
# alternative, not always the top rank.
clear_line
type_text "demo-tool a"
wait_ghost
field "$LAST_LINE" post
expect_eq "top-ranked habit ghosts first" "$REPLY" 'lpha'
press $'\e[C'
nap 0.1
dump_now
flagv "$LAST_LINE" idx
expect_eq "Right moves to the next candidate" "$REPLY" 2
press $'\t'
nap 0.2
dump_now
field "$LAST_LINE" buf
expect_eq "accept inserts the cycled candidate" "$REPLY" 'demo-tool apricot '

# S3 shell-escaped directory (the P1 regression): the filesystem provider
# emits `spaced\ dir/`; the ghost, the wire, and the accept must preserve the
# backslash so the buffer stays TWO words, and a directory value takes no
# trailing space.
clear_line
type_text "cd spa"
wait_ghost
field "$LAST_LINE" post
expect_eq "ghost preserves escaped form" "$REPLY" 'ced\\ dir/'
press $'\t'
nap 0.2
dump_now
field "$LAST_LINE" buf
expect_eq "accept keeps the backslash" "$REPLY" 'cd spaced\\ dir/'
flagv "$LAST_LINE" words
expect_eq "escaped path remains one argument" "$REPLY" 2

# S4 Shift-TAB browse from a bare command word: selector opens over the ranked
# list, arrows move the highlight (wrapping included), TAB accepts the row.
clear_line
type_text "demo-tool "
press $'\e[Z'
wait_selector
flagv "$LAST_LINE" lidx
expect_eq "selector opens on the top row" "$REPLY" 1
field "$LAST_LINE" post
# Both learned verbs must appear as browsable rows. History patterns and
# schema verbs currently surface as separate rows with equal values; dedup is
# upstream's call, so assert on content, not row count.
if [[ "$REPLY" == '\n'* && "$REPLY" == *alpha* && "$REPLY" == *apricot* ]]; then
  ok "selector renders both verbs below the prompt"
else
  fail "selector rows unexpected: [$REPLY]"
fi
press $'\e[C'
nap 0.1
dump_now
flagv "$LAST_LINE" lidx
expect_eq "Right highlights the second row" "$REPLY" 2
press $'\t'
nap 0.2
dump_now
field "$LAST_LINE" buf
expect_eq "selector accept inserts the chosen row" "$REPLY" 'demo-tool apricot '
flagv "$LAST_LINE" sel
expect_eq "selector closed after accept" "$REPLY" 0

# S5 Ctrl-X session suppression toggles. The zle -M confirmation message can
# consume or delay the immediately following dump keystroke, so state is
# polled across several dumps rather than compared once.
press "$CTRL_X"
local flipped=0
for (( _try = 0; _try < 6; _try++ )); do
  dump_now
  flagv "$LAST_LINE" sup
  if [[ "$REPLY" == 1 ]]; then
    flipped=1
    break
  fi
done
(( flipped )) && ok "Ctrl-X suppresses suggestions" || fail "Ctrl-X never suppressed suggestions"
press "$CTRL_X"
flipped=0
for (( _try = 0; _try < 6; _try++ )); do
  dump_now
  flagv "$LAST_LINE" sup
  if [[ "$REPLY" == 0 ]]; then
    flipped=1
    break
  fi
done
(( flipped )) && ok "Ctrl-X again restores suggestions" || fail "Ctrl-X never restored suggestions"

# S6 ESC dismisses a visible ghost without touching the buffer. The lone ESC
# needs longer than zsh's KEYTIMEOUT before the next key so the two never
# merge into an unrecognized sequence.
clear_line
type_text "demo-tool al"
wait_ghost
press $'\e'
nap 0.6
# Responsiveness probe: a plain character must echo and land in the buffer.
# Distinguishes "dump widget stopped firing" from "editor stopped running".
press 'X'
nap 0.2
dump_now
field "$LAST_LINE" buf
expect_eq "ESC leaves the typed buffer intact" "$REPLY" 'demo-tool alX'
field "$LAST_LINE" post
expect_eq "ESC clears the ghost" "$REPLY" ""

# ---- report -------------------------------------------------------------------

if (( FAILURES )); then
  print -u2 "$FAILURES scenario assertion(s) failed"
  exit 1
fi
print "all interactive ZLE scenarios passed (daemon_mode=$DAEMON_MODE)"
