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

## Upgrading to a semantic ONNX model

For true semantic similarity, add an ONNX-backed `Embedder` behind a feature
flag and return it from `default_embedder()`. Design constraints to keep:
local-first (no auto-download — the model ships with the app or is pointed at by
config), deterministic, and a graceful fallback to `BagEmbedder`.

**Recommended model:** `bge-small-en-v1.5` (384-dim, ~33 MB int8) for English, or
`paraphrase-multilingual-MiniLM-L12-v2` if you want FR/EN parity. Export to ONNX.

**Steps:**

1. `Cargo.toml`:
   ```toml
   [features]
   onnx = ["dep:ort", "dep:tokenizers"]
   [dependencies]
   ort = { version = "2", optional = true }          # ONNX Runtime
   tokenizers = { version = "0.20", optional = true } # HF tokenizer
   ```
2. New `src/onnx.rs` (compiled only with `--features onnx`): load `model.onnx` +
   `tokenizer.json` from `DRIFTERR_EMBED_MODEL` (a directory), run the encoder,
   mean-pool the last hidden state over the attention mask, L2-normalize. Cache
   the session in the struct (it's `Send + Sync`).
3. In `default_embedder()`:
   ```rust
   #[cfg(feature = "onnx")]
   if let Ok(dir) = std::env::var("DRIFTERR_EMBED_MODEL") {
       match onnx::OnnxEmbedder::load(&dir) {
           Ok(e) => return Box::new(e),
           Err(err) => eprintln!("drifterr: ONNX embedder load failed ({err}); using local"),
       }
   }
   ```
4. Ship the model with the Tauri app (a resource) and set `DRIFTERR_EMBED_MODEL`
   to its resource path at startup, so it's offline and per-machine.

**Why it's not done yet:** it pulls a native runtime (ONNX Runtime) and a model
file, which can't be built or validated in the CI sandbox. It's isolated behind
the `onnx` feature so the default build, CI and the shipped app are unaffected
until a model is bundled and the path is validated on real hardware.

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
