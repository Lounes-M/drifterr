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

# Verify a downloaded file against the release's SHA256SUMS.
#
# HTTPS proves the bytes came from GitHub. It proves nothing about whether they
# are the bytes we built — a compromised release, a wrong asset, a truncated
# download all look identical to a successful transfer. This script pipes into a
# shell and then runs a binary, which is exactly the situation that deserves a
# digest check rather than a shrug.
#
# A release with no SHA256SUMS is refused rather than installed unverified: this
# is the one place where "it probably worked" is not good enough, and every
# release from now on publishes one.
verify() {
  file="$1"; name="$2"
  if [ -z "${SUMS:-}" ]; then
    err "This release publishes no SHA256SUMS — refusing to install unverified."
    err "Download it by hand from https://github.com/$REPO/releases if you accept that."
    exit 1
  fi
  want=$(printf '%s\n' "$SUMS" | awk -v n="$name" '$2 == n || $2 == "*" n { print $1; exit }')
  if [ -z "$want" ]; then
    err "$name is not listed in this release's SHA256SUMS — refusing to install."
    exit 1
  fi
  if command -v sha256sum >/dev/null 2>&1; then
    got=$(sha256sum "$file" | awk '{print $1}')
  elif command -v shasum >/dev/null 2>&1; then
    got=$(shasum -a 256 "$file" | awk '{print $1}')
  else
    err "Neither sha256sum nor shasum is available — cannot verify the download."
    exit 1
  fi
  if [ "$got" != "$want" ]; then
    err "Checksum mismatch for $name."
    err "  expected $want"
    err "  got      $got"
    err "Not installing. Please report this."
    exit 1
  fi
  say "Verified $name (sha256 ok)."
}

say "Drifterr — fetching the latest release…"
JSON=$(curl -fsSL "$API") || { err "Could not reach GitHub releases."; exit 1; }

# The digest manifest, published by the release workflow alongside the binaries.
SUMS_URL=$(printf '%s' "$JSON" \
  | grep -o '"browser_download_url": *"[^"]*SHA256SUMS"' \
  | sed 's/.*"\(http[^"]*\)".*/\1/' | head -1)
SUMS=""
if [ -n "${SUMS_URL:-}" ]; then
  SUMS=$(curl -fsSL "$SUMS_URL" || true)
fi

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
    verify "$TMP/Drifterr.dmg" "$(basename "$URL")"
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
    verify "$DEST/drifterr" "$(basename "$URL")"
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
