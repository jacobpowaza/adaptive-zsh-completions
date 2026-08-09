#!/bin/sh
set -eu
INSTALL_DIR=${ADAPTIVE_INSTALL_DIR:-"$HOME/.local/bin"}
ZSHRC=${ADAPTIVE_ZSHRC:-"$HOME/.zshrc"}
if [ -f "$ZSHRC" ] && grep -q '^# >>> adaptive initialize >>>$' "$ZSHRC"; then
  backup="$ZSHRC.adaptive-backup.$(date +%Y%m%d%H%M%S).$$"
  cp "$ZSHRC" "$backup"
  temporary=$(mktemp "${TMPDIR:-/tmp}/adaptive-zshrc.XXXXXX")
  awk 'BEGIN{managed=0} /^# >>> adaptive initialize >>>$/{managed=1;next} /^# <<< adaptive initialize <<<$/{managed=0;next} !managed{print}' "$ZSHRC" > "$temporary"
  mv "$temporary" "$ZSHRC"
  printf 'Removed Adaptive block from %s (backup: %s)\n' "$ZSHRC" "$backup"
fi
if [ -f "$INSTALL_DIR/adaptive" ]; then rm "$INSTALL_DIR/adaptive"; printf 'Removed %s/adaptive\n' "$INSTALL_DIR"; fi
printf 'Local cache and history were preserved. Remove them with `adaptive cache clear` and `adaptive history clear` before uninstalling if desired.\n'
