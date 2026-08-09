#!/bin/sh
set -eu
ROOT=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
TEST_ROOT=$(mktemp -d "${TMPDIR:-/tmp}/adaptive-install-test.XXXXXX")
trap 'rm -rf "$TEST_ROOT"' EXIT HUP INT TERM
mkdir -p "$TEST_ROOT/home" "$TEST_ROOT/bin"
printf 'bindkey -e\nexport EXISTING_SETTING=kept\n' > "$TEST_ROOT/home/.zshrc"
ADAPTIVE_SOURCE_DIR="$ROOT" ADAPTIVE_ACCEPT_KEY='^F' ADAPTIVE_INSTALL_DIR="$TEST_ROOT/bin" ADAPTIVE_ZSHRC="$TEST_ROOT/home/.zshrc" "$ROOT/install.sh"
test -x "$TEST_ROOT/bin/adaptive"
test "$(grep -c '^# >>> adaptive initialize >>>$' "$TEST_ROOT/home/.zshrc")" -eq 1
grep -q 'EXISTING_SETTING=kept' "$TEST_ROOT/home/.zshrc"
grep -q "ADAPTIVE_ACCEPT_KEY='\^F'" "$TEST_ROOT/home/.zshrc"
ADAPTIVE_SOURCE_DIR="$ROOT" ADAPTIVE_INSTALL_DIR="$TEST_ROOT/bin" ADAPTIVE_ZSHRC="$TEST_ROOT/home/.zshrc" "$ROOT/install.sh"
test "$(grep -c '^# >>> adaptive initialize >>>$' "$TEST_ROOT/home/.zshrc")" -eq 1
ADAPTIVE_INSTALL_DIR="$TEST_ROOT/bin" ADAPTIVE_ZSHRC="$TEST_ROOT/home/.zshrc" "$ROOT/uninstall.sh"
test ! -e "$TEST_ROOT/bin/adaptive"
! grep -q '^# >>> adaptive initialize >>>$' "$TEST_ROOT/home/.zshrc"
grep -q 'EXISTING_SETTING=kept' "$TEST_ROOT/home/.zshrc"
printf 'installer integration test passed\n'
