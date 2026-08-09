#!/bin/sh
set -eu

REPO="jacobpowaza/adaptive-zsh-completions"
INSTALL_DIR=${ADAPTIVE_INSTALL_DIR:-"$HOME/.local/bin"}
ZSHRC=${ADAPTIVE_ZSHRC:-"$HOME/.zshrc"}
VERSION=${ADAPTIVE_VERSION:-latest}

say() { printf '%s\n' "$*"; }
fail() { say "adaptive installer: $*" >&2; exit 1; }
command -v curl >/dev/null 2>&1 || fail "curl is required"
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

touch "$ZSHRC"
if ! grep -q '^# >>> adaptive initialize >>>$' "$ZSHRC"; then
  backup="$ZSHRC.adaptive-backup.$(date +%Y%m%d%H%M%S).$$"
  cp "$ZSHRC" "$backup"
  {
    printf '\n# >>> adaptive initialize >>>\n'
    printf 'export PATH="%s:$PATH"\n' "$INSTALL_DIR"
    printf 'eval "$("%s/adaptive" init zsh)"\n' "$INSTALL_DIR"
    printf '# <<< adaptive initialize <<<\n'
  } >> "$ZSHRC"
  say "Updated $ZSHRC (backup: $backup)"
else
  say "Zsh integration already present in $ZSHRC"
fi
say "Installed $INSTALL_DIR/adaptive"
say "Restart Zsh or run: exec zsh"
