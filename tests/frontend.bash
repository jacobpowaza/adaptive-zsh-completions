#!/usr/bin/env bash
# Functional tests for the Bash frontend: sources the shared runtime and the
# Readline frontend, drives the completion handler with simulated readline
# state, and exercises the real engine end to end through shim CLIs built
# from captured help output.
set -u

cd "$(dirname "$0")/.." || exit 1
root=$PWD

if (( BASH_VERSINFO[0] < 4 )); then
  echo "bash >= 4 required; skipping frontend tests"
  exit 0
fi

bin=${ADAPTIVE_BIN:-$root/target/debug/adaptive}
[[ -x "$bin" ]] || bin=$root/target/release/adaptive
[[ -x "$bin" ]] || { echo "no adaptive binary; run cargo build first" >&2; exit 1; }

tmp=$(mktemp -d "${TMPDIR:-/tmp}/adaptive-frontend.XXXXXX") || exit 1
trap 'rm -rf "$tmp"' EXIT

shims=$tmp/bin
mkdir -p "$shims"
cat >"$shims/kubectl" <<SHIM
#!/bin/sh
case "\$1" in
  "") exec cat "$root/tests/fixtures/kubectl_help.txt" ;;
  rollout) exec cat "$root/tests/fixtures/kubectl_rollout_help.txt" ;;
esac
exec cat "$root/tests/fixtures/kubectl_help.txt"
SHIM
cat >"$shims/poetry" <<'SHIM'
#!/bin/sh
if [ "$#" -eq 0 ]; then exit 0; fi
case "$1" in
  list) cat tests/fixtures/poetry_list.txt ;;
  *) cat tests/fixtures/poetry_help.txt ;;
esac
SHIM
chmod +x "$shims/kubectl" "$shims/poetry"

export PATH="$shims:$tmp/real:$PATH"
mkdir -p "$tmp/real"
ln -sf "$bin" "$tmp/real/adaptive"
export ADAPTIVE_CACHE_DIR=$tmp/cache ADAPTIVE_DATA_DIR=$tmp/data ADAPTIVE_CONFIG=$tmp/config.toml

# The frontend expects the shared runtime (locations, query protocol, cache)
# to be sourced first -- that is exactly what the generated init script does.
# shellcheck source=../shell/adaptive-core.bash
source "$root/shell/adaptive-core.bash"
# shellcheck source=../shell/adaptive.bash
source "$root/shell/adaptive.bash"

failures=0
check() {
  local label=$1 expected=$2 buffer=$3
  # Readline's cursor sits at the end of what was typed.
  local cursor=${#buffer}
  COMP_LINE=$buffer COMP_POINT=$cursor COMP_CWORD=0 COMPREPLY=()
  _adaptive_complete
  local found=0 item
  for item in ${COMPREPLY[@]+"${COMPREPLY[@]}"}; do
    [[ "$item" == "$expected" ]] && found=1
  done
  if (( found )); then
    echo "ok   $label ($buffer)"
  else
    echo "FAIL $label: '$expected' not offered by '$buffer'" \
      "(got: ${COMPREPLY[*]:-nothing})"
    failures=$((failures + 1))
  fi
}

check "kubectl top-level subcommand" get "kubectl "
check "kubectl grouped-section verb" describe "kubectl "
check "kubectl nested path" status "kubectl rollout "
check "symfony list fallback (poetry)" install "poetry "
check "flag completion" --amend "git commit --am"
check "subcommand after prefix" checkout "git chec"

COMP_LINE="" COMP_POINT=0 COMP_CWORD=0 COMPREPLY=()
rc=0
_adaptive_complete || rc=$?
if (( rc == 0 )) || [[ -z "${ADAPTIVE_DEBUG:-}" && $rc -eq 124 ]]; then
  echo "ok   empty prompt falls back or predicts (rc=$rc)"
else
  echo "FAIL empty prompt returned rc=$rc"
  failures=$((failures + 1))
fi

COMP_LINE="definitelynotarealcommand " COMP_POINT=26 COMP_CWORD=0 COMPREPLY=()
rc=0
_adaptive_complete 2>/dev/null || rc=$?
if (( rc == 124 )) || (( rc == 0 )); then
  echo "ok   unknown command degrades gracefully (rc=$rc)"
else
  echo "FAIL unknown command returned rc=$rc"
  failures=$((failures + 1))
fi

# Shift-Tab walks the candidate list the other way: Readline consumes the
# matches in list order, so the gesture is implemented by presenting the
# ranking reversed.
COMP_LINE="kubectl " COMP_POINT=8 COMP_CWORD=0 COMPREPLY=()
_adaptive_complete
forward=(${COMPREPLY[@]+"${COMPREPLY[@]}"})
COMP_LINE="kubectl " COMP_POINT=8 COMP_CWORD=0 COMPREPLY=()
_adaptive_complete_backward
backward=(${COMPREPLY[@]+"${COMPREPLY[@]}"})
expected_backward=()
for (( i = ${#forward[@]} - 1; i >= 0; i-- )); do
  expected_backward+=("${forward[i]}")
done
if [[ "${backward[*]:-}" == "${expected_backward[*]:-}" ]] && (( ${#forward[@]} > 1 )); then
  echo "ok   shift-tab reverses the candidate order"
else
  echo "FAIL shift-tab order: got '${backward[*]:-}' want '${expected_backward[*]:-}'"
  failures=$((failures + 1))
fi

# The suggestion lives inside the line buffer (Readline cannot paint
# outside it), so the typed text stays untouched and the cursor sits on the
# boundary between the two.
READLINE_LINE="kubectl " READLINE_POINT=8
_adaptive_refresh
ghost=${READLINE_LINE:8}
if [[ -n $ghost && ${READLINE_LINE:0:8} == "kubectl " && $READLINE_POINT == 8 ]]; then
  echo "ok   ghost suggestion is planted after the typed text"
else
  echo "FAIL ghost suggestion: line='$READLINE_LINE' point=$READLINE_POINT ghost='$ghost'"
  failures=$((failures + 1))
fi

# Rejecting the suggestion drops the tail and leaves the typed text alone.
_adaptive_drop_ghost
if [[ $READLINE_LINE == "kubectl " && $READLINE_POINT == 8 && -z ${_ADAPTIVE_GHOST:-} ]]; then
  echo "ok   rejecting the suggestion restores the line"
else
  echo "FAIL reject: line='$READLINE_LINE' point=$READLINE_POINT"
  failures=$((failures + 1))
fi

if (( failures )); then
  echo "$failures failure(s)"
  exit 1
fi
echo "all frontend tests passed"
