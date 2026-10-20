#!/bin/sh
set -eu
ROOT=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
TEST_ROOT=$(mktemp -d "${TMPDIR:-/tmp}/adaptive-install-test.XXXXXX")
trap 'rm -rf "$TEST_ROOT"' EXIT HUP INT TERM
mkdir -p "$TEST_ROOT/home" "$TEST_ROOT/bin"
printf 'bindkey -e\nexport EXISTING_SETTING=kept\n' > "$TEST_ROOT/home/.zshrc"
printf 'export EXISTING_BASH_SETTING=kept\n' > "$TEST_ROOT/home/.bashrc"

# --- zsh ------------------------------------------------------------------
ADAPTIVE_SOURCE_DIR="$ROOT" ADAPTIVE_ACCEPT_KEY='^F' ADAPTIVE_INSTALL_DIR="$TEST_ROOT/bin" ADAPTIVE_ZSHRC="$TEST_ROOT/home/.zshrc" ADAPTIVE_SHELL=zsh ADAPTIVE_INSTALL_CARAPACE=skip "$ROOT/install.sh"
test -x "$TEST_ROOT/bin/adaptive"
test "$(grep -c '^# >>> adaptive initialize >>>$' "$TEST_ROOT/home/.zshrc")" -eq 1
grep -q 'EXISTING_SETTING=kept' "$TEST_ROOT/home/.zshrc"
grep -q "ADAPTIVE_ACCEPT_KEY='\^F'" "$TEST_ROOT/home/.zshrc"
grep -q "ADAPTIVE_BROWSE_KEY='\^\[\[Z'" "$TEST_ROOT/home/.zshrc"
grep -q "ADAPTIVE_GHOST_STYLE='fg=245'" "$TEST_ROOT/home/.zshrc"
ADAPTIVE_SOURCE_DIR="$ROOT" ADAPTIVE_INSTALL_DIR="$TEST_ROOT/bin" ADAPTIVE_ZSHRC="$TEST_ROOT/home/.zshrc" ADAPTIVE_SHELL=zsh ADAPTIVE_INSTALL_CARAPACE=skip "$ROOT/install.sh"
test "$(grep -c '^# >>> adaptive initialize >>>$' "$TEST_ROOT/home/.zshrc")" -eq 1

# --- bash -----------------------------------------------------------------
ADAPTIVE_SOURCE_DIR="$ROOT" ADAPTIVE_INSTALL_DIR="$TEST_ROOT/bin" ADAPTIVE_BASHRC="$TEST_ROOT/home/.bashrc" ADAPTIVE_SHELL=bash ADAPTIVE_INSTALL_CARAPACE=skip "$ROOT/install.sh"
test "$(grep -c '^# >>> adaptive initialize >>>$' "$TEST_ROOT/home/.bashrc")" -eq 1
grep -q 'EXISTING_BASH_SETTING=kept' "$TEST_ROOT/home/.bashrc"
# The doctor looks for this string to detect a Bash integration.
grep -q 'adaptive init bash' "$TEST_ROOT/home/.bashrc"
grep -q 'adaptive init blesh' "$TEST_ROOT/home/.bashrc"
# The caret-notation browse key has to reach Bash as the raw escape sequence.
grep -q "ADAPTIVE_BROWSE_KEY=\$'\\\\e\[Z'" "$TEST_ROOT/home/.bashrc"
# The cached frontends are generated up front, so the first shell sources a
# file instead of launching the binary.
grep -q 'init.bash' "$TEST_ROOT/home/.bashrc"
grep -q 'init.blesh' "$TEST_ROOT/home/.bashrc"
ADAPTIVE_SOURCE_DIR="$ROOT" ADAPTIVE_INSTALL_DIR="$TEST_ROOT/bin" ADAPTIVE_BASHRC="$TEST_ROOT/home/.bashrc" ADAPTIVE_SHELL=bash ADAPTIVE_INSTALL_CARAPACE=skip "$ROOT/install.sh"
test "$(grep -c '^# >>> adaptive initialize >>>$' "$TEST_ROOT/home/.bashrc")" -eq 1

# --- both shells at once --------------------------------------------------
ADAPTIVE_SOURCE_DIR="$ROOT" ADAPTIVE_INSTALL_DIR="$TEST_ROOT/bin" ADAPTIVE_ZSHRC="$TEST_ROOT/home/.zshrc" ADAPTIVE_BASHRC="$TEST_ROOT/home/.bashrc" ADAPTIVE_SHELL=both ADAPTIVE_INSTALL_CARAPACE=skip "$ROOT/install.sh"
test "$(grep -c '^# >>> adaptive initialize >>>$' "$TEST_ROOT/home/.zshrc")" -eq 1
test "$(grep -c '^# >>> adaptive initialize >>>$' "$TEST_ROOT/home/.bashrc")" -eq 1

# --- uninstall ------------------------------------------------------------
ADAPTIVE_INSTALL_DIR="$TEST_ROOT/bin" ADAPTIVE_ZSHRC="$TEST_ROOT/home/.zshrc" ADAPTIVE_BASHRC="$TEST_ROOT/home/.bashrc" "$ROOT/uninstall.sh"
test ! -e "$TEST_ROOT/bin/adaptive"
! grep -q '^# >>> adaptive initialize >>>$' "$TEST_ROOT/home/.zshrc"
! grep -q '^# >>> adaptive initialize >>>$' "$TEST_ROOT/home/.bashrc"
grep -q 'EXISTING_SETTING=kept' "$TEST_ROOT/home/.zshrc"
grep -q 'EXISTING_BASH_SETTING=kept' "$TEST_ROOT/home/.bashrc"

printf 'installer integration test passed\n'
