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
python3 scripts/gen_icons.py >/dev/null 2>&1 || echo "  (icons not regenerated — Pillow missing; using the committed set)" >&2

# Ship the whole of src/ rather than an enumerated list.
#
# The list was enumerated, and adding src/api.js to the extension without
# remembering to add it here would have produced a zip that loads and then fails
# at the first `importScripts` — in the store, not in CI. src/ holds only files
# the browser loads (tests live in tests/, tooling in scripts/), so "everything in
# src/" is both correct and self-maintaining.
rm -f "$OUT"
zip -r -q "$OUT" manifest.json src icons

# Guard the guard: every script the extension actually references must be inside
# the zip. Catches a renamed file, a typo in an importScripts path, and the class
# of mistake that motivated the change above.
missing=0
referenced=$(
  { grep -oE 'importScripts\("[^"]+"\)' src/*.js | sed -E 's/.*importScripts\("([^"]+)"\).*/\1/'
    grep -oE '<script src="[^"]+"' src/*.html | sed -E 's/.*src="([^"]+)".*/\1/'
    node -p 'const m=require("./manifest.json");[m.background?.service_worker,...(m.content_scripts||[]).flatMap(c=>c.js||[]),m.action?.default_popup].filter(Boolean).join("\n")'
  } | sort -u
)
for ref in $referenced; do
  # Paths in src/*.{js,html} are relative to src/; manifest paths are from the root.
  for candidate in "$ref" "src/$ref"; do
    if unzip -l "$OUT" | grep -qE "[[:space:]]${candidate}$"; then continue 2; fi
  done
  echo "✗ referenced but not packaged: $ref" >&2
  missing=1
done
[ "$missing" -eq 0 ] || { echo "Refusing to ship an incomplete zip." >&2; rm -f "$OUT"; exit 1; }

echo "✓ Packaged $OUT"
unzip -l "$OUT" | tail -n +4 | head -n -2 | awk '{print "   " $4}'
echo
echo "Next: upload $OUT at"
echo "  • Chrome  → https://chrome.google.com/webstore/devconsole"
echo "  • Firefox → https://addons.mozilla.org/developers/"
