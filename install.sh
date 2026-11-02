#!/bin/sh
# Install Calinator for the current user (no sudo).
#
#   curl -fsSL https://raw.githubusercontent.com/smanookian/calinator/main/install.sh | sh
#
# Run from a source checkout to build and install that instead.
# While the repository is private, the download uses the GitHub CLI (gh).
# INSTALL_DIR changes where the binary goes (default ~/.local/bin).
set -eu

REPO="smanookian/calinator"
ASSET="calinator-x86_64-linux.tar.gz"
BIN_DIR="${INSTALL_DIR:-$HOME/.local/bin}"
APP_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/applications"

say() { printf '%s\n' "$*"; }
fail() { say "calinator install: $*" >&2; exit 1; }

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

# Build from source only when run as a file from a checkout (./install.sh),
# not when piped into sh (then $0 is "sh" and the current folder is unrelated).
HERE=""
case "$0" in
  *install.sh) HERE=$(cd "$(dirname "$0")" 2>/dev/null && pwd || true) ;;
esac
if [ -n "$HERE" ] && [ -f "$HERE/Cargo.toml" ] && grep -q '^name = "calinator"' "$HERE/Cargo.toml"; then
  say "Building from source in $HERE"
  command -v cargo >/dev/null || fail "cargo is needed to build from source"
  (cd "$HERE" && cargo build --release --locked -p calinator)
  cp "$HERE/target/release/calinator" "$TMP/calinator"
  DESKTOP="$HERE/packaging/calinator.desktop"
else
  case "$(uname -s)-$(uname -m)" in
    Linux-x86_64) ;;
    *) fail "release builds are Linux x86_64 only; build from a source checkout instead" ;;
  esac
  URL="https://github.com/$REPO/releases/latest/download/$ASSET"
  say "Downloading $URL"
  if ! curl -fsSL "$URL" -o "$TMP/c.tar.gz" 2>/dev/null; then
    # Private repository: download with the GitHub CLI, which is signed in.
    command -v gh >/dev/null || fail "download failed (private repository? install gh and run: gh auth login)"
    gh release download -R "$REPO" -p "$ASSET" -O "$TMP/c.tar.gz" || fail "download failed"
  fi
  tar -xzf "$TMP/c.tar.gz" -C "$TMP"
  DESKTOP="$TMP/calinator.desktop"
fi

mkdir -p "$BIN_DIR" "$APP_DIR"
install -m 755 "$TMP/calinator" "$BIN_DIR/calinator"
if [ -f "$DESKTOP" ]; then
  # Point the launcher at this binary, so it works without ~/.local/bin in PATH.
  sed "s|^Exec=calinator|Exec=$BIN_DIR/calinator|" "$DESKTOP" > "$APP_DIR/calinator.desktop"
  # Open .ics files and webcal: links with Calinator.
  if command -v xdg-mime >/dev/null; then
    xdg-mime default calinator.desktop text/calendar x-scheme-handler/webcal x-scheme-handler/webcals || true
  fi
  command -v update-desktop-database >/dev/null && update-desktop-database "$APP_DIR" 2>/dev/null || true
fi

say "Installed $BIN_DIR/calinator"
case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *) say "Add $BIN_DIR to your PATH." ;;
esac
say "Run: calinator   (check your setup with: calinator doctor)"
say "Reminders with the app closed: calinator daemon --install"
