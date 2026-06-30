# drifterr-embeddings

Local text embeddings for the soft signals (goal alignment, degradation) and for
the decision-coherence retrieval step. Exposes the [`Embedder`] trait and a
default, dependency-free **hybrid-lexical** embedder (`BagEmbedder`):

- word unigrams + word bigrams + intra-word character trigrams,
- sub-linear TF damping, L2-normalized,
- deterministic, zero-network, zero-cost — local-first by construction.

The engine never names a concrete embedder: it calls `default_embedder() ->
Box<dyn Embedder>`. That factory is the **only** place to change to get semantic
embeddings.

## How good is the default?

Good enough for *relative* judgments (is this turn drifting from the goal vs
earlier turns; are two replies near-duplicates), which is all the soft signals
need — and they can never drive RED on their own. The char-trigram features mean
related word forms ("auth" ↔ "authentication") and short phrases now register,
where a plain bag-of-words scored them ~0. It is **not** truly semantic: it won't
match synonyms with no lexical overlap ("car" ↔ "automobile").

## Semantic ONNX model (implemented, behind the `onnx` feature)

For true semantic similarity ("car" ↔ "automobile"), the crate ships an
ONNX-backed embedder in [`src/onnx.rs`](src/onnx.rs), compiled only with
`--features onnx`. `default_embedder()` selects it when `DRIFTERR_EMBED_MODEL`
points at a model directory, and falls back to `BagEmbedder` on any load error —
so detection never breaks. It's local-first: the model is read from disk, nothing
is auto-downloaded at runtime.

**Recommended model:** `bge-small-en-v1.5` (384-dim, ~33 MB) for English, or
`paraphrase-multilingual-MiniLM-L12-v2` for FR/EN parity.

**1. Export the model + tokenizer:**
```bash
pip install "optimum[exporters]"
optimum-cli export onnx -m BAAI/bge-small-en-v1.5 ./bge-small-onnx
# → ./bge-small-onnx/{model.onnx, tokenizer.json}
```

**2. Build with the feature and point at the model:**
```bash
cargo run -p drifterr-proxy --features drifterr-embeddings/onnx
export DRIFTERR_EMBED_MODEL=/path/to/bge-small-onnx
```
(For the shipped app, bundle the model as a Tauri resource and set
`DRIFTERR_EMBED_MODEL` to its resource path at startup.)

**Validation caveat:** the `onnx` feature pulls a native runtime (ONNX Runtime,
via `ort`'s `download-binaries`) and is **not built in CI** — the default build,
CI and the shipped app are unaffected until you opt in. `ort` 2.x is a
prerelease pinned in `Cargo.toml`; the binding surface (`inputs!`,
`try_extract_raw_tensor`, `Session.inputs`) targets that pin — if you bump `ort`,
the run/extract calls in `src/onnx.rs` may need a minor adjustment. The logic
(tokenize → run → mean-pool → L2-normalize) is stable.

## Enabling the judge (decision-coherence, Signal 3)

The embedder only *retrieves* candidate rejected decisions; the final yes/no is
the **judge's** (an LLM call), so decision-coherence quality depends on the judge
being on. It's off by default and runs through the user's own provider:

```bash
OPENROUTER_API_KEY=sk-or-...        # or another key from_env accepts
DRIFTERR_JUDGE_MODEL=openai/gpt-4o-mini
```

With no key the judge is `Disabled` (fail-safe: always "no violation"), and
everything else keeps working. See `crates/judge`.
