#!/bin/sh
set -eu

REPO="jacobpowaza/adaptive-zsh-completions"
INSTALL_DIR=${ADAPTIVE_INSTALL_DIR:-"$HOME/.local/bin"}
ZSHRC=${ADAPTIVE_ZSHRC:-"$HOME/.zshrc"}
BASHRC=${ADAPTIVE_BASHRC:-"$HOME/.bashrc"}
# Which shell(s) to wire up: auto (detect), bash, zsh, or both.
SHELL_TARGET=${ADAPTIVE_SHELL:-auto}
VERSION=${ADAPTIVE_VERSION:-latest}
ACCEPT_KEY=${ADAPTIVE_ACCEPT_KEY:-'^I'}
MENU_NEXT_KEY=${ADAPTIVE_MENU_NEXT_KEY:-'^[[C'}
BROWSE_KEY=${ADAPTIVE_BROWSE_KEY:-'^[[Z'}
GHOST_STYLE=${ADAPTIVE_GHOST_STYLE:-'fg=245'}
# Bash frontends use their own knobs: the Readline frontend accepts a
# suggestion with Right (or End), the ble.sh frontend with Tab.
BASH_ACCEPT_KEY=${ADAPTIVE_BASH_ACCEPT_KEY:-''}
BASH_BROWSE_KEY=${ADAPTIVE_BASH_BROWSE_KEY:-'^[[Z'}

say() { printf '%s\n' "$*"; }
fail() { say "adaptive installer: $*" >&2; exit 1; }
command -v curl >/dev/null 2>&1 || fail "curl is required"
[ -n "$ACCEPT_KEY" ] || fail "ADAPTIVE_ACCEPT_KEY cannot be empty"
[ -n "$MENU_NEXT_KEY" ] || fail "ADAPTIVE_MENU_NEXT_KEY cannot be empty"
[ -n "$BROWSE_KEY" ] || fail "ADAPTIVE_BROWSE_KEY cannot be empty"
[ -n "$GHOST_STYLE" ] || fail "ADAPTIVE_GHOST_STYLE cannot be empty"
[ "${#ACCEPT_KEY}" -le 32 ] || fail "ADAPTIVE_ACCEPT_KEY is too long"
[ "${#MENU_NEXT_KEY}" -le 32 ] || fail "ADAPTIVE_MENU_NEXT_KEY is too long"
[ "${#BROWSE_KEY}" -le 32 ] || fail "ADAPTIVE_BROWSE_KEY is too long"
[ "${#GHOST_STYLE}" -le 64 ] || fail "ADAPTIVE_GHOST_STYLE is too long"
[ "$(printf '%s' "$ACCEPT_KEY" | tr -d '\r\n')" = "$ACCEPT_KEY" ] || fail "ADAPTIVE_ACCEPT_KEY must be one line"
[ "$(printf '%s' "$MENU_NEXT_KEY" | tr -d '\r\n')" = "$MENU_NEXT_KEY" ] || fail "ADAPTIVE_MENU_NEXT_KEY must be one line"
[ "$(printf '%s' "$BROWSE_KEY" | tr -d '\r\n')" = "$BROWSE_KEY" ] || fail "ADAPTIVE_BROWSE_KEY must be one line"
[ "$(printf '%s' "$GHOST_STYLE" | tr -d '\r\n')" = "$GHOST_STYLE" ] || fail "ADAPTIVE_GHOST_STYLE must be one line"
[ "$(printf '%s' "$BASH_BROWSE_KEY" | tr -d '\r\n')" = "$BASH_BROWSE_KEY" ] || fail "ADAPTIVE_BASH_BROWSE_KEY must be one line"
[ "${#BASH_BROWSE_KEY}" -le 32 ] || fail "ADAPTIVE_BASH_BROWSE_KEY is too long"
[ "$(printf '%s' "$BASH_ACCEPT_KEY" | tr -d '\r\n')" = "$BASH_ACCEPT_KEY" ] || fail "ADAPTIVE_BASH_ACCEPT_KEY must be one line"
[ "${#BASH_ACCEPT_KEY}" -le 32 ] || fail "ADAPTIVE_BASH_ACCEPT_KEY is too long"
accept_key_escaped=$(printf '%s' "$ACCEPT_KEY" | sed "s/'/'\\\\''/g")
menu_next_key_escaped=$(printf '%s' "$MENU_NEXT_KEY" | sed "s/'/'\\\\''/g")
browse_key_escaped=$(printf '%s' "$BROWSE_KEY" | sed "s/'/'\\\\''/g")
ghost_style_escaped=$(printf '%s' "$GHOST_STYLE" | sed "s/'/'\\\\''/g")
bash_accept_key_escaped=$(printf '%s' "$BASH_ACCEPT_KEY" | sed "s/'/'\\\\''/g")
bash_browse_key_escaped=$(printf '%s' "$BASH_BROWSE_KEY" | sed "s/'/'\\\\''/g")
mkdir -p "$INSTALL_DIR"

install_from_source() {
  source_dir=${ADAPTIVE_SOURCE_DIR:-}
  temporary=""
  if [ -z "$source_dir" ]; then
    command -v cargo >/dev/null 2>&1 || fail "no release binary was available and Rust/Cargo is not installed"
    temporary=$(mktemp -d "${TMPDIR:-/tmp}/adaptive-source.XXXXXX")
    trap 'rm -rf "$temporary"' EXIT HUP INT TERM
    curl -fsSL "https://github.com/$REPO/archive/refs/heads/main.tar.gz" -o "$temporary/source.tar.gz"
    tar -xzf "$temporary/source.tar.gz" -C "$temporary"
    source_dir=$(find "$temporary" -mindepth 1 -maxdepth 1 -type d | head -1)
  fi
  command -v cargo >/dev/null 2>&1 || fail "Cargo is required for a source installation"
  say "Building Adaptive from source..."
  cargo build --release --locked --manifest-path "$source_dir/Cargo.toml"
  install -m 755 "$source_dir/target/release/adaptive" "$INSTALL_DIR/adaptive"
}

install_release() {
  os=$(uname -s); arch=$(uname -m)
  case "$os:$arch" in
    Darwin:x86_64) target=x86_64-apple-darwin ;;
    Darwin:arm64|Darwin:aarch64) target=aarch64-apple-darwin ;;
    Linux:x86_64) target=x86_64-unknown-linux-gnu ;;
    Linux:aarch64|Linux:arm64) target=aarch64-unknown-linux-gnu ;;
    *) return 1 ;;
  esac
  temporary=$(mktemp -d "${TMPDIR:-/tmp}/adaptive-install.XXXXXX")
  trap 'rm -rf "$temporary"' EXIT HUP INT TERM
  if [ "$VERSION" = latest ]; then
    base="https://github.com/$REPO/releases/latest/download"
  else
    base="https://github.com/$REPO/releases/download/$VERSION"
  fi
  archive="adaptive-$target.tar.gz"
  curl -fsSL "$base/$archive" -o "$temporary/$archive" || return 1
  curl -fsSL "$base/checksums.txt" -o "$temporary/checksums.txt" || return 1
  expected=$(awk -v file="$archive" '$2 == file {print $1}' "$temporary/checksums.txt")
  [ -n "$expected" ] || fail "release checksum is missing for $archive"
  if command -v shasum >/dev/null 2>&1; then actual=$(shasum -a 256 "$temporary/$archive" | awk '{print $1}');
  elif command -v sha256sum >/dev/null 2>&1; then actual=$(sha256sum "$temporary/$archive" | awk '{print $1}');
  else fail "shasum or sha256sum is required"; fi
  [ "$actual" = "$expected" ] || fail "checksum verification failed"
  tar -xzf "$temporary/$archive" -C "$temporary"
  install -m 755 "$temporary/adaptive" "$INSTALL_DIR/adaptive"
}

if [ -n "${ADAPTIVE_SOURCE_DIR:-}" ]; then install_from_source
elif ! install_release; then say "Prebuilt release unavailable; falling back to source."; install_from_source
fi

carapace_manual_hint() {
  cat <<'CARAPACE_EOF'
Manual install options:
  macOS            brew install carapace-bin
  Arch Linux       sudo pacman -S carapace-bin
  Fedora           sudo dnf install carapace-bin
  Alpine           sudo apk add carapace-bin
  Debian/Ubuntu    https://carapace-sh.github.io/carapace-bin/setup.html

Adaptive works fully without it; carapace only adds extra command knowledge.
Toggling later: adaptive carapace enable | disable | status
CARAPACE_EOF
}

try_install_carapace() {
  if command -v brew >/dev/null 2>&1; then
    brew install carapace-bin
  elif command -v pacman >/dev/null 2>&1; then
    sudo pacman -S --noconfirm carapace-bin
  elif command -v dnf >/dev/null 2>&1; then
    sudo dnf install -y carapace-bin
  elif command -v apk >/dev/null 2>&1; then
    sudo apk add carapace-bin
  elif command -v apt-get >/dev/null 2>&1; then
    sudo apt-get update -qq && sudo apt-get install -y carapace-bin
  else
    return 1
  fi
}

if command -v carapace >/dev/null 2>&1; then
  say "Carapace detected at $(command -v carapace): merged into Adaptive completions."
else
  case "${ADAPTIVE_INSTALL_CARAPACE:-auto}" in
    yes)
      say "Installing optional carapace-bin..."
      if try_install_carapace && command -v carapace >/dev/null 2>&1; then
        say "Carapace installed."
      else
        say "Automatic carapace install failed or unsupported here."
        carapace_manual_hint
      fi
      ;;
    ask)
      # Prompts only on a real terminal; piped/CI runs get instructions.
      if [ -t 0 ] && [ -t 2 ]; then
        printf '%s' 'Install optional carapace-bin for thousands of extra completers? [y/N] '
        read -r answer || answer=""
        case "$answer" in
          y*|Y*) try_install_carapace || carapace_manual_hint ;;
          *) carapace_manual_hint ;;
        esac
      else
        say "Optional: carapace-bin not found."
        carapace_manual_hint
      fi
      ;;
    *)
      say "Optional: carapace-bin not found (adds completers for many CLIs; see docs)."
      ;;
  esac
fi

# ---------------------------------------------------------------------------
# shell integration
# ---------------------------------------------------------------------------

# Which shells to wire up. `auto` follows $SHELL; when that is ambiguous the
# integration is written for every shell that is actually installed, because
# each block is independent and idempotent.
target_shells() {
  case "$SHELL_TARGET" in
    bash|zsh|both)
      printf '%s\n' "$SHELL_TARGET"
      ;;
    auto)
      base=$(basename "${SHELL:-}")
      case "$base" in
        bash) printf 'bash\n' ;;
        zsh) printf 'zsh\n' ;;
        *)
          found=''
          command -v bash >/dev/null 2>&1 && found="$found bash"
          command -v zsh >/dev/null 2>&1 && found="$found zsh"
          [ -n "$found" ] || found=' zsh'
          # shellcheck disable=SC2086  # deliberate word splitting
          printf '%s\n' $found
          ;;
      esac
      ;;
    *) fail "ADAPTIVE_SHELL must be one of: auto, bash, zsh, both" ;;
  esac
}

# Translate a caret-notation key (the Zsh bindkey form, e.g. '^[[Z') into the
# literal bytes a Bash frontend expects.
bash_key_bytes() {
  printf '%s' "$1" | awk '
    {
      out = "";
      n = length($0);
      for (i = 1; i <= n; i++) {
        c = substr($0, i, 1);
        if (c == "^" && i < n) {
          d = substr($0, ++i, 1);
          if (d == "[") out = out "\033";
          else if (d == "I") out = out "\t";
          else out = out toupper(d);
        } else out = out c;
      }
      printf "%s", out;
    }'
}

# Render a value as a Bash $'...' literal, so escape sequences in it (the ESC
# of a cursor key, for instance) survive the round trip through the rc file.
bash_dollar_quote() {
  printf "$'%s'" "$(printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e "s/'/\\\\'/g" -e 's/\x1b/\\e/g')"
}

# The path of a generated integration file, as printed by `adaptive init`.
init_path() {
  printf '%s\n' "$("$INSTALL_DIR/adaptive" init "$1" || true)" |
    sed -n "s/^source '\(.*\)'$/\1/p"
}

# Write the managed block into an rc file.
#
# When the block is already there it is *replaced in place*, not skipped: an
# upgrade has to upgrade, and a block written by an older release would
# otherwise survive every reinstall. Everything outside the markers -- the
# user's own settings, aliases and PROMPT_COMMAND -- is left exactly as it
# was, and the previous file is kept as a timestamped backup.
#
# The file is rewritten through a temporary file but copied back with `cat`,
# so its permissions and ownership survive.
replace_block() {
  file=$1
  block=$2
  touch "$file"
  backup="$file.adaptive-backup.$(date +%Y%m%d%H%M%S).$$"
  cp "$file" "$backup"
  temporary=$(mktemp "${TMPDIR:-/tmp}/adaptive-rc.XXXXXX")
  if grep -q '^# >>> adaptive initialize >>>$' "$file"; then
    awk -v blockfile="$block" '
      BEGIN { while ((getline line < blockfile) > 0) lines[++n] = line }
      /^# >>> adaptive initialize >>>$/ {
        managed = 1
        for (i = 1; i <= n; i++) print lines[i]
        next
      }
      /^# <<< adaptive initialize <<<$/ { managed = 0; next }
      !managed { print }
    ' "$file" > "$temporary"
  else
    cat "$file" > "$temporary"
    printf '\n' >> "$temporary"
    cat "$block" >> "$temporary"
  fi
  cat "$temporary" > "$file"
  rm -f "$temporary"
  say "Updated $file (backup: $backup)"
}

install_zsh_block() {
  touch "$ZSHRC"
  # Pre-generate the cached integration so the very next shell sources a
  # file instead of launching the binary, and learn where the data dir
  # landed (platform-dependent) from the printed source line.
  init_file=$(init_path zsh)
  block=$(mktemp "${TMPDIR:-/tmp}/adaptive-block.XXXXXX")
  {
    printf '# >>> adaptive initialize >>>\n'
    printf 'case ":$PATH:" in\n'
    printf '  *":%s:"*) ;;\n' "$INSTALL_DIR"
    printf '  *) export PATH="%s:$PATH" ;;\n' "$INSTALL_DIR"
    printf 'esac\n'
    printf "export ADAPTIVE_ACCEPT_KEY='%s'\n" "$accept_key_escaped"
    printf "export ADAPTIVE_MENU_NEXT_KEY='%s'\n" "$menu_next_key_escaped"
    printf "export ADAPTIVE_BROWSE_KEY='%s'\n" "$browse_key_escaped"
    printf "export ADAPTIVE_GHOST_STYLE='%s'\n" "$ghost_style_escaped"
    if [ -n "$init_file" ]; then
      printf "export ADAPTIVE_INIT_FILE='%s'\n" "$(printf '%s' "$init_file" | sed "s/'/'\\\\''/g")"
      printf 'if [[ -r "$ADAPTIVE_INIT_FILE" ]]; then\n'
      printf '  source "$ADAPTIVE_INIT_FILE"\n'
      printf 'else\n'
      printf '  eval "$(adaptive init zsh)"\n'
      printf 'fi\n'
    else
      printf 'eval "$(adaptive init zsh)"\n'
    fi
    printf '# <<< adaptive initialize <<<\n'
  } > "$block"
  replace_block "$ZSHRC" "$block"
  rm -f "$block"
}

install_bash_block() {
  touch "$BASHRC"
  # Pre-generate both Bash frontends so the very next shell sources a file
  # instead of launching the binary.
  bash_init_file=$(init_path bash)
  blesh_init_file=$(init_path blesh)
  browse_literal=$(bash_dollar_quote "$(bash_key_bytes "$BASH_BROWSE_KEY")")
  bash_init_quoted=''
  if [ -n "$bash_init_file" ]; then
    bash_init_quoted=$(printf "'%s'" "$(printf '%s' "$bash_init_file" | sed "s/'/'\\\\''/g")")
  fi
  blesh_init_quoted=''
  if [ -n "$blesh_init_file" ]; then
    blesh_init_quoted=$(printf "'%s'" "$(printf '%s' "$blesh_init_file" | sed "s/'/'\\\\''/g")")
  fi
  block=$(mktemp "${TMPDIR:-/tmp}/adaptive-block.XXXXXX")
  {
    printf '# >>> adaptive initialize >>>\n'
    printf '# Only interactive shells: the frontends bind keys and readline\n'
    printf '# state, neither of which means anything to a script.\n'
    printf 'if [[ $- == *i* ]]; then\n'
    printf '  case ":$PATH:" in\n'
    printf '    *":%s:"*) ;;\n' "$INSTALL_DIR"
    printf '    *) export PATH="%s:$PATH" ;;\n' "$INSTALL_DIR"
    printf '  esac\n'
    printf '  if command -v adaptive >/dev/null 2>&1; then\n'
    printf "    export ADAPTIVE_BROWSE_KEY=%s\n" "$browse_literal"
    if [ -n "$BASH_ACCEPT_KEY" ]; then
      printf "    export ADAPTIVE_GHOST_ACCEPT_KEY=%s\n" \
        "$(bash_dollar_quote "$(bash_key_bytes "$BASH_ACCEPT_KEY")")"
    fi
    printf '    # ble.sh replaces Readline with a line editor that renders styled\n'
    printf '    # regions, so the inline suggestion can be dimmed. Load it when it is\n'
    printf '    # installed; otherwise the Readline frontend below takes over.\n'
    printf '    if [[ -r ${XDG_DATA_HOME:-$HOME/.local/share}/blesh/ble.sh ]]; then\n'
    printf '      [[ -n ${_ble_version:-} ]] || source ${XDG_DATA_HOME:-$HOME/.local/share}/blesh/ble.sh\n'
    printf '    elif [[ -r $HOME/.blesh/ble.sh ]]; then\n'
    printf '      [[ -n ${_ble_version:-} ]] || source $HOME/.blesh/ble.sh\n'
    printf '    fi\n'
    printf '    if [[ -n ${_ble_version:-} ]] || declare -F ble/util/idle.push >/dev/null 2>&1; then\n'
    if [ -n "$blesh_init_quoted" ]; then
      printf '      if [[ -r %s ]]; then\n' "$blesh_init_quoted"
      printf '        source %s\n' "$blesh_init_quoted"
      printf '      else\n'
      printf '        eval "$(adaptive init blesh)"\n'
      printf '      fi\n'
    else
      printf '      eval "$(adaptive init blesh)"\n'
    fi
    printf '    elif [[ -r %s ]]; then\n' "${bash_init_quoted:-/dev/null}"
    if [ -n "$bash_init_quoted" ]; then
      printf '      source %s\n' "$bash_init_quoted"
    fi
    printf '    else\n'
    printf '      eval "$(adaptive init bash)"\n'
    printf '    fi\n'
    printf '  fi\n'
    printf 'fi\n'
    printf '# <<< adaptive initialize <<<\n'
  } > "$block"
  replace_block "$BASHRC" "$block"
  rm -f "$block"
}

for target in $(target_shells); do
  case "$target" in
    zsh) install_zsh_block ;;
    bash) install_bash_block ;;
    both)
      install_zsh_block
      install_bash_block
      ;;
  esac
done

say "Installed $INSTALL_DIR/adaptive"
case "$SHELL_TARGET" in
  bash) say "Restart Bash or run: exec bash" ;;
  zsh) say "Restart Zsh or run: exec zsh" ;;
  both) say "Restart your shell (exec bash / exec zsh)" ;;
  *) say "Restart your shell (exec bash / exec zsh)" ;;
esac
