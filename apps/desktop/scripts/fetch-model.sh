#!/usr/bin/env bash
# Export the semantic embedding model Drifterr can bundle for true semantic
# drift detection ("car" ↔ "automobile"), which the default lexical embedder
# can't do. Off by default — see the "Semantic model" section in
# apps/desktop/README (and crates/embeddings/README.md).
#
# Output: apps/desktop/src-tauri/models/embed/{model.onnx, tokenizer.json}
# Then build with:  cargo tauri build --features semantic
#
# Requires Python + optimum. ~33 MB for bge-small-en-v1.5.
set -euo pipefail

MODEL="${1:-BAAI/bge-small-en-v1.5}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OUT="$HERE/../src-tauri/models/embed"

echo "Exporting $MODEL → $OUT"
mkdir -p "$OUT"

if ! python3 -c "import optimum" 2>/dev/null; then
  echo "Installing optimum exporters (one-time)…"
  python3 -m pip install --quiet "optimum[exporters]"
fi

python3 -m optimum.exporters.onnx --model "$MODEL" --task feature-extraction "$OUT"

if [[ -f "$OUT/model.onnx" && -f "$OUT/tokenizer.json" ]]; then
  echo "✓ Model ready at $OUT"
  echo "  Now build the semantic app:  cargo tauri build --features semantic"
else
  echo "✗ Export did not produce model.onnx + tokenizer.json" >&2
  exit 1
fi
