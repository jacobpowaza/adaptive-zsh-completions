#!/bin/sh
set -eu
ROOT=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
TEST_ROOT=$(mktemp -d "${TMPDIR:-/tmp}/adaptive-install-test.XXXXXX")
trap 'rm -rf "$TEST_ROOT"' EXIT HUP INT TERM
mkdir -p "$TEST_ROOT/home" "$TEST_ROOT/bin"
printf 'bindkey -e\nexport EXISTING_SETTING=kept\n' > "$TEST_ROOT/home/.zshrc"
printf 'export EXISTING_BASH_SETTING=kept\nPROMPT_COMMAND="echo user-hook"\n' > "$TEST_ROOT/home/.bashrc"

# The installer runs `adaptive init`, which writes the cached frontends into
# the data directory. Pointing that at the test directory keeps the test
# hermetic: nothing outside $TEST_ROOT is written. HOME itself is left alone
# so the Rust toolchain (and cargo, when building from source) still works.
export ADAPTIVE_DATA_DIR="$TEST_ROOT/home/.local/share/adaptive"

run_installer() {
  env "$@" \
    ADAPTIVE_SOURCE_DIR="$ROOT" \
    ADAPTIVE_INSTALL_DIR="$TEST_ROOT/bin" \
    ADAPTIVE_ZSHRC="$TEST_ROOT/home/.zshrc" \
    ADAPTIVE_BASHRC="$TEST_ROOT/home/.bashrc" \
    ADAPTIVE_INSTALL_CARAPACE=skip \
    "$ROOT/install.sh"
}

# --- zsh ------------------------------------------------------------------
run_installer ADAPTIVE_ACCEPT_KEY='^F' ADAPTIVE_SHELL=zsh
test -x "$TEST_ROOT/bin/adaptive"
test "$(grep -c '^# >>> adaptive initialize >>>$' "$TEST_ROOT/home/.zshrc")" -eq 1
grep -q 'EXISTING_SETTING=kept' "$TEST_ROOT/home/.zshrc"
grep -q "ADAPTIVE_ACCEPT_KEY='\^F'" "$TEST_ROOT/home/.zshrc"
grep -q "ADAPTIVE_BROWSE_KEY='\^\[\[Z'" "$TEST_ROOT/home/.zshrc"
grep -q "ADAPTIVE_GHOST_STYLE='fg=245'" "$TEST_ROOT/home/.zshrc"
# A second install must not add a second block.
run_installer ADAPTIVE_ACCEPT_KEY='^F' ADAPTIVE_SHELL=zsh
test "$(grep -c '^# >>> adaptive initialize >>>$' "$TEST_ROOT/home/.zshrc")" -eq 1

# --- bash -----------------------------------------------------------------
run_installer ADAPTIVE_SHELL=bash
test "$(grep -c '^# >>> adaptive initialize >>>$' "$TEST_ROOT/home/.bashrc")" -eq 1
grep -q 'EXISTING_BASH_SETTING=kept' "$TEST_ROOT/home/.bashrc"
grep -q 'PROMPT_COMMAND="echo user-hook"' "$TEST_ROOT/home/.bashrc"
# The doctor looks for this string to detect a Bash integration.
grep -q 'adaptive init bash' "$TEST_ROOT/home/.bashrc"
grep -q 'adaptive init blesh' "$TEST_ROOT/home/.bashrc"
# The caret-notation browse key has to reach Bash as the raw escape sequence.
grep -q "ADAPTIVE_BROWSE_KEY=\$'\\\\e\[Z'" "$TEST_ROOT/home/.bashrc"
# The cached frontends are generated up front, so the first shell sources a
# file instead of launching the binary.
grep -q 'init.bash' "$TEST_ROOT/home/.bashrc"
grep -q 'init.blesh' "$TEST_ROOT/home/.bashrc"
# The block must not carry the workaround for the ble.sh keymap bug that
# v0.5.0 shipped: it corrupted ble.sh's own keymap array.
! grep -q '_ble_complete_auto_source' "$TEST_ROOT/home/.bashrc"
# The block is valid Bash.
bash -n "$TEST_ROOT/home/.bashrc"

# A second install must not add a second block, and must not change the
# block either: the installer is idempotent.
cp "$TEST_ROOT/home/.bashrc" "$TEST_ROOT/bashrc.before"
run_installer ADAPTIVE_SHELL=bash
test "$(grep -c '^# >>> adaptive initialize >>>$' "$TEST_ROOT/home/.bashrc")" -eq 1
cmp -s "$TEST_ROOT/bashrc.before" "$TEST_ROOT/home/.bashrc"

# An upgrade replaces the managed block in place, so a block written by an
# older release does not survive a reinstall.
sed -i 's/# >>> adaptive initialize >>>/# >>> adaptive initialize >>>\n# STALE BLOCK FROM AN OLD RELEASE/' "$TEST_ROOT/home/.bashrc"
run_installer ADAPTIVE_SHELL=bash
test "$(grep -c '^# >>> adaptive initialize >>>$' "$TEST_ROOT/home/.bashrc")" -eq 1
! grep -q 'STALE BLOCK FROM AN OLD RELEASE' "$TEST_ROOT/home/.bashrc"
grep -q 'EXISTING_BASH_SETTING=kept' "$TEST_ROOT/home/.bashrc"
bash -n "$TEST_ROOT/home/.bashrc"

# The install directory ends up on the PATH exactly once.
test "$(grep -cF '":'"$TEST_ROOT/bin"':"' "$TEST_ROOT/home/.bashrc")" -eq 1

# --- both shells at once --------------------------------------------------
run_installer ADAPTIVE_SHELL=both
test "$(grep -c '^# >>> adaptive initialize >>>$' "$TEST_ROOT/home/.zshrc")" -eq 1
test "$(grep -c '^# >>> adaptive initialize >>>$' "$TEST_ROOT/home/.bashrc")" -eq 1

# --- a fresh machine, with no rc files at all -----------------------------
rm -f "$TEST_ROOT/home/.zshrc" "$TEST_ROOT/home/.bashrc"
run_installer ADAPTIVE_SHELL=both
test "$(grep -c '^# >>> adaptive initialize >>>$' "$TEST_ROOT/home/.zshrc")" -eq 1
test "$(grep -c '^# >>> adaptive initialize >>>$' "$TEST_ROOT/home/.bashrc")" -eq 1
bash -n "$TEST_ROOT/home/.bashrc"

# Put the user's own settings back, so the uninstall check below can prove
# they survive.
printf 'bindkey -e\nexport EXISTING_SETTING=kept\n' > "$TEST_ROOT/home/.zshrc"
printf 'export EXISTING_BASH_SETTING=kept\nPROMPT_COMMAND="echo user-hook"\n' > "$TEST_ROOT/home/.bashrc"

# --- uninstall ------------------------------------------------------------
ADAPTIVE_INSTALL_DIR="$TEST_ROOT/bin" ADAPTIVE_ZSHRC="$TEST_ROOT/home/.zshrc" ADAPTIVE_BASHRC="$TEST_ROOT/home/.bashrc" "$ROOT/uninstall.sh"
test ! -e "$TEST_ROOT/bin/adaptive"
! grep -q '^# >>> adaptive initialize >>>$' "$TEST_ROOT/home/.zshrc"
! grep -q '^# >>> adaptive initialize >>>$' "$TEST_ROOT/home/.bashrc"
grep -q 'EXISTING_SETTING=kept' "$TEST_ROOT/home/.zshrc"
grep -q 'EXISTING_BASH_SETTING=kept' "$TEST_ROOT/home/.bashrc"

# --- reinstall after an uninstall ------------------------------------------
run_installer ADAPTIVE_SHELL=both
test "$(grep -c '^# >>> adaptive initialize >>>$' "$TEST_ROOT/home/.zshrc")" -eq 1
test "$(grep -c '^# >>> adaptive initialize >>>$' "$TEST_ROOT/home/.bashrc")" -eq 1
grep -q 'EXISTING_BASH_SETTING=kept' "$TEST_ROOT/home/.bashrc"

printf 'installer integration test passed\n'
