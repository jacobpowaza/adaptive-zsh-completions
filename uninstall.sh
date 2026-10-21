#!/bin/sh
set -eu
INSTALL_DIR=${ADAPTIVE_INSTALL_DIR:-"$HOME/.local/bin"}
ZSHRC=${ADAPTIVE_ZSHRC:-"$HOME/.zshrc"}
BASHRC=${ADAPTIVE_BASHRC:-"$HOME/.bashrc"}

# Removes only Adaptive's managed block, leaving the rest of the rc file (and
# a timestamped backup of it) untouched.
#
# A file whose managed block has a start marker but no end marker is left
# alone: without the end marker there is no way to tell where Adaptive's
# block stops and the user's own configuration begins, and removing
# everything after the start marker would delete the user's settings.
remove_block() {
  file=$1
  label=$2
  [ -f "$file" ] || return 0
  grep -q '^# >>> adaptive initialize >>>$' "$file" || return 0
  if ! grep -q '^# <<< adaptive initialize <<<$' "$file"; then
    printf 'Left %s untouched: the Adaptive block has a start marker but no end marker, so its end cannot be found safely.\n' "$file"
    return 0
  fi
  backup="$file.adaptive-backup.$(date +%Y%m%d%H%M%S).$$"
  cp "$file" "$backup"
  temporary=$(mktemp "${TMPDIR:-/tmp}/adaptive-rc.XXXXXX")
  awk 'BEGIN{managed=0} /^# >>> adaptive initialize >>>$/{managed=1;next} /^# <<< adaptive initialize <<<$/{managed=0;next} !managed{print}' "$file" > "$temporary"
  cat "$temporary" > "$file"
  rm -f "$temporary"
  printf 'Removed Adaptive block from %s (backup: %s)\n' "$file" "$backup"
}

remove_block "$ZSHRC" "zsh"
remove_block "$BASHRC" "bash"

if [ -f "$INSTALL_DIR/adaptive" ]; then rm "$INSTALL_DIR/adaptive"; printf 'Removed %s/adaptive\n' "$INSTALL_DIR"; fi
printf 'Local cache and history were preserved. Remove them with `adaptive cache clear` and `adaptive history clear` before uninstalling if desired.\n'
