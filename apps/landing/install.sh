#!/bin/sh
# Drifterr installer — downloads the latest release for your OS.
#   curl -fsSL https://drifterr.app/install.sh | sh
# macOS: installs Drifterr.app into ~/Applications (no admin).
# Linux: installs the AppImage to ~/.local/bin/drifterr.
set -eu

REPO="Lounes-M/drifterr"
API="https://api.github.com/repos/$REPO/releases/latest"

say() { printf '\033[1m%s\033[0m\n' "$*"; }
err() { printf '\033[31m%s\033[0m\n' "$*" >&2; }

command -v curl >/dev/null 2>&1 || { err "curl is required."; exit 1; }

say "Drifterr — fetching the latest release…"
JSON=$(curl -fsSL "$API") || { err "Could not reach GitHub releases."; exit 1; }

# Pull the first browser_download_url whose filename matches the pattern ($1).
asset_url() {
  printf '%s' "$JSON" \
    | grep -o '"browser_download_url": *"[^"]*"' \
    | sed 's/.*"\(http[^"]*\)".*/\1/' \
    | grep -i "$1" \
    | head -1
}

OS=$(uname -s)
case "$OS" in
  Darwin)
    URL=$(asset_url '\.dmg$') || true
    [ -n "${URL:-}" ] || { err "No macOS build found in the latest release yet."; exit 1; }
    TMP=$(mktemp -d)
    say "Downloading $(basename "$URL")…"
    curl -fsSL "$URL" -o "$TMP/Drifterr.dmg"
    say "Mounting…"
    VOL=$(hdiutil attach "$TMP/Drifterr.dmg" -nobrowse | grep -o '/Volumes/[^ ]*' | tail -1)
    DEST="$HOME/Applications"; mkdir -p "$DEST"
    rm -rf "$DEST/Drifterr.app"
    cp -R "$VOL"/*.app "$DEST/"
    hdiutil detach "$VOL" >/dev/null
    rm -rf "$TMP"
    say "Installed → $DEST/Drifterr.app"
    say "First launch: right-click the app → Open (it's unsigned)."
    ;;
  Linux)
    URL=$(asset_url '\.AppImage$') || true
    [ -n "${URL:-}" ] || { err "No Linux build found in the latest release yet."; exit 1; }
    DEST="$HOME/.local/bin"; mkdir -p "$DEST"
    say "Downloading $(basename "$URL")…"
    curl -fsSL "$URL" -o "$DEST/drifterr"
    chmod +x "$DEST/drifterr"
    say "Installed → $DEST/drifterr"
    case ":$PATH:" in *":$DEST:"*) : ;; *) say "Add ~/.local/bin to your PATH to run 'drifterr'." ;; esac
    ;;
  *)
    err "Unsupported OS: $OS. See the download page for options."
    exit 1
    ;;
esac

say "Done. Launch Drifterr and point your AI tool at it."
