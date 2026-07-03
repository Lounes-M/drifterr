#!/usr/bin/env bash
# Build a store-ready zip of the Drifterr extension — exactly the files the
# browser loads, nothing else (no node_modules, tests, or scripts).
#
#   apps/extension/scripts/package.sh   → apps/extension/drifterr-extension-<v>.zip
#
# Upload the zip to the Chrome Web Store / Firefox Add-ons dashboards.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
cd "$ROOT"

VERSION="$(node -p "require('./manifest.json').version" 2>/dev/null || echo "0.0.0")"
OUT="drifterr-extension-${VERSION}.zip"

# Regenerate icons so the zip is never stale.
python3 scripts/gen_icons.py >/dev/null

rm -f "$OUT"
zip -r -q "$OUT" \
  manifest.json \
  src/parse.js src/content.js src/background.js src/popup.html src/popup.js \
  icons

echo "✓ Packaged $OUT"
unzip -l "$OUT" | tail -n +4 | head -n -2 | awk '{print "   " $4}'
echo
echo "Next: upload $OUT at"
echo "  • Chrome  → https://chrome.google.com/webstore/devconsole"
echo "  • Firefox → https://addons.mozilla.org/developers/"
