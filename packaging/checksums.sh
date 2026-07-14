#!/usr/bin/env bash
# Print the SHA256 checksums for a Drifterr release's macOS DMG and Windows EXE,
# ready to paste into the Homebrew cask and the winget installer manifest.
#
# Usage:  packaging/checksums.sh 0.2.3
set -euo pipefail

VERSION="${1:-}"
if [[ -z "$VERSION" ]]; then
  echo "usage: $0 <version>   (e.g. 0.2.3)" >&2
  exit 2
fi

REPO="Lounes-M/drifterr"
BASE="https://github.com/${REPO}/releases/download/v${VERSION}"
DMG="Drifterr_${VERSION}_universal.dmg"
EXE="Drifterr_${VERSION}_x64-setup.exe"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

hash_of() {
  local name="$1"
  echo "  downloading $name…" >&2
  curl -fsSL -o "$tmp/$name" "$BASE/$name"
  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$tmp/$name" | awk '{print $1}'
  else
    sha256sum "$tmp/$name" | awk '{print $1}'
  fi
}

DMG_SHA="$(hash_of "$DMG")"
EXE_SHA="$(hash_of "$EXE")"

cat <<EOF

Drifterr v${VERSION} — checksums
──────────────────────────────────────────────
Homebrew cask (packaging/homebrew/drifterr.rb):
  version "${VERSION}"
  sha256 "${DMG_SHA}"

winget installer (packaging/winget/Drifterr.Drifterr.installer.yaml):
  PackageVersion: ${VERSION}
  InstallerSha256: ${EXE_SHA}
──────────────────────────────────────────────
Also bump PackageVersion in the other two winget files.
EOF
