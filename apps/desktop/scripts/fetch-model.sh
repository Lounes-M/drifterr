#!/usr/bin/env bash
# Fetch the semantic embedding model Drifterr can bundle for true semantic drift
# detection ("car" ↔ "automobile") — the thing the default lexical embedder
# can't do. Off by default; see the "Semantic model" section in apps/desktop/README.
#
# Downloads a pre-exported ONNX bge-small-en-v1.5 (no Python/optimum needed) into
#   apps/desktop/src-tauri/models/embed/{model.onnx, tokenizer.json}
# This is the exact model measured in crates/embeddings/README.md (goal↔drift
# separation 0.05 → 0.26; eval accuracy 66.7% → 100%).
#
# Then build the semantic app (the committed default config is untouched — the
# model is injected as a resource only for this build):
#   cd apps/desktop/src-tauri
#   cargo tauri build --features semantic \
#     --config '{"bundle":{"resources":["models/embed/*"]}}'
set -euo pipefail

REPO="Xenova/bge-small-en-v1.5"
BASE="https://huggingface.co/${REPO}/resolve/main"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OUT="$HERE/../src-tauri/models/embed"

mkdir -p "$OUT"
echo "Fetching $REPO → $OUT"

curl -fSL --retry 3 -o "$OUT/tokenizer.json" "$BASE/tokenizer.json"
curl -fSL --retry 3 -o "$OUT/model.onnx"     "$BASE/onnx/model.onnx"

if [[ -s "$OUT/model.onnx" && -s "$OUT/tokenizer.json" ]]; then
  sz=$(du -h "$OUT/model.onnx" | cut -f1)
  echo "✓ Model ready ($sz) at $OUT"
  echo
  echo "Next — build the semantic app (default release config unchanged):"
  echo "  cd apps/desktop/src-tauri"
  echo "  cargo tauri build --features semantic \\"
  echo "    --config '{\"bundle\":{\"resources\":[\"models/embed/*\"]}}'"
else
  echo "✗ Download did not produce model.onnx + tokenizer.json" >&2
  exit 1
fi
