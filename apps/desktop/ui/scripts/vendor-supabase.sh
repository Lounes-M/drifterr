#!/usr/bin/env bash
# Rebuild the vendored Supabase client.
#
# The panel used to `import { createClient } from "https://esm.sh/..."` at
# runtime, and the Tauri CSP was widened to permit it. That put a third-party CDN
# inside the trust boundary of a desktop app: a compromise or hijack of that host
# would execute arbitrary JavaScript in a webview holding the user's auth session,
# with reach into the local control API. For a product whose entire pitch is that
# nothing leaves your machine, fetching executable code from someone else's server
# on every launch is the wrong default.
#
# So the SDK is bundled once, committed, and loaded from disk. Re-run this to
# change versions; the pinned version below is the single source of truth.
#
#   apps/desktop/ui/scripts/vendor-supabase.sh
set -euo pipefail

VERSION="2.47.10"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
UI="$(cd "$HERE/.." && pwd)"
LANDING="$(cd "$UI/../../landing" && pwd)"

cd "$UI"
npm install --no-save --silent "@supabase/supabase-js@${VERSION}" esbuild

for out in "$UI/vendor/supabase.js" "$LANDING/vendor/supabase.js"; do
  mkdir -p "$(dirname "$out")"
  ./node_modules/.bin/esbuild \
    --bundle --format=esm --platform=browser --target=es2020 \
    --minify --legal-comments=none \
    --outfile="$out" \
    node_modules/@supabase/supabase-js/dist/module/index.js
done

# The whole point is that the bundle reaches for nothing at runtime. A remote
# import surviving it would silently restore the CDN dependency.
if grep -qE '(import|from)[[:space:]]*"https?://' "$UI/vendor/supabase.js"; then
  echo "the bundle still contains a remote import - refusing to ship it" >&2
  exit 1
fi

echo "vendored @supabase/supabase-js@${VERSION}"
ls -l "$UI/vendor/supabase.js" "$LANDING/vendor/supabase.js"
