//! Semantic embedder backed by a local ONNX sentence-transformer.
//!
//! Compiled only with `--features onnx`. It loads a model + tokenizer from a
//! directory (no network, no auto-download of the *model* — local-first), runs
//! the encoder, mean-pools the token embeddings over the attention mask and
//! L2-normalizes. The result drops straight into the [`Embedder`](crate::Embedder)
//! trait, so the engine is unchanged.
//!
//! Expected files in the directory pointed to by `DRIFTERR_EMBED_MODEL`:
//!   * `model.onnx`      — the encoder (e.g. `bge-small-en-v1.5`, 384-dim)
//!   * `tokenizer.json`  — the matching HuggingFace fast tokenizer
//!
//! Export example (Python):
//! ```text
//! pip install optimum[exporters]
//! optimum-cli export onnx -m BAAI/bge-small-en-v1.5 ./bge-small-onnx
//! # → ./bge-small-onnx/model.onnx + tokenizer.json
//! ```
//!
//! NOTE: this targets the `ort` 2.x and `tokenizers` 0.20 APIs. Those crates are
//! native and move fast — when you first build with `--features onnx`, you may
//! need to pin/adjust a minor version or tweak the run/extract calls below for
//! your exact `ort` release. The logic (tokenize → run → mean-pool → normalize)
//! is stable; only the binding surface changes.

use std::path::Path;
use std::sync::Mutex;

use ndarray::Array2;
use ort::session::{builder::GraphOptimizationLevel, Session};
use ort::value::Value;
use tokenizers::Tokenizer;

use crate::Embedder;

/// Max tokens fed to the encoder. Turns are short; this bounds cost and memory.
const MAX_TOKENS: usize = 256;

pub struct OnnxEmbedder {
    tokenizer: Tokenizer,
    // `Session::run` takes `&mut self` in ort 2.x, so guard it for `Sync`.
    session: Mutex<Session>,
    /// Whether the model expects a `token_type_ids` input (BERT-family do).
    wants_token_types: bool,
}

impl OnnxEmbedder {
    /// Load `model.onnx` + `tokenizer.json` from `dir`.
    pub fn load(dir: &Path) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let tokenizer = Tokenizer::from_file(dir.join("tokenizer.json"))?;
        let session = Session::builder()?
            .with_optimization_level(GraphOptimizationLevel::Level3)?
            .commit_from_file(dir.join("model.onnx"))?;
        // Detect the optional token_type_ids input by name.
        let wants_token_types = session.inputs.iter().any(|i| i.name == "token_type_ids");
        Ok(Self {
            tokenizer,
            session: Mutex::new(session),
            wants_token_types,
        })
    }

    fn embed_inner(
        &self,
        text: &str,
    ) -> Result<Vec<f32>, Box<dyn std::error::Error + Send + Sync>> {
        let enc = self.tokenizer.encode(text, true)?;
        let mut ids: Vec<i64> = enc.get_ids().iter().map(|&x| x as i64).collect();
        let mut mask: Vec<i64> = enc.get_attention_mask().iter().map(|&x| x as i64).collect();
        if ids.len() > MAX_TOKENS {
            ids.truncate(MAX_TOKENS);
            mask.truncate(MAX_TOKENS);
        }
        let n = ids.len().max(1);
        let id_arr = Array2::from_shape_vec((1, n), ids)?;
        let mask_arr = Array2::from_shape_vec((1, n), mask.clone())?;

        let mut session = self.session.lock().unwrap();
        // `ort::inputs!` builds the input vec directly (it is not fallible).
        let outputs = if self.wants_token_types {
            let types = Array2::<i64>::zeros((1, n));
            session.run(ort::inputs![
                "input_ids" => Value::from_array(id_arr)?,
                "attention_mask" => Value::from_array(mask_arr)?,
                "token_type_ids" => Value::from_array(types)?,
            ])?
        } else {
            session.run(ort::inputs![
                "input_ids" => Value::from_array(id_arr)?,
                "attention_mask" => Value::from_array(mask_arr)?,
            ])?
        };

        // Take the token-level hidden states (first output for encoder models).
        // `try_extract_tensor` yields (&Shape, &[T]); Shape derefs to [i64].
        let (shape, data) = outputs[0].try_extract_tensor::<f32>()?;
        let dims: &[i64] = shape;
        // Expected shape: [1, seq, hidden].
        let (seq, hidden) = match dims {
            [_, s, h] => (*s as usize, *h as usize),
            _ => return Err("unexpected model output shape".into()),
        };

        // Mean-pool over tokens, weighted by the attention mask.
        let mut pooled = vec![0.0f32; hidden];
        let mut denom = 0.0f32;
        for (t, &mi) in mask.iter().enumerate().take(seq.min(n)) {
            let m = mi as f32;
            if m == 0.0 {
                continue;
            }
            denom += m;
            let base = t * hidden;
            for (j, p) in pooled.iter_mut().enumerate() {
                *p += m * data[base + j];
            }
        }
        if denom > 0.0 {
            for p in &mut pooled {
                *p /= denom;
            }
        }
        // L2-normalize so cosine == dot.
        let norm: f32 = pooled.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for p in &mut pooled {
                *p /= norm;
            }
        }
        Ok(pooled)
    }
}

impl Embedder for OnnxEmbedder {
    fn embed(&self, text: &str) -> Vec<f32> {
        if text.trim().is_empty() {
            return Vec::new();
        }
        // Fail-safe: on any inference error, return an empty vector. Callers use
        // `cosine`, which treats a zero/empty vector as "no signal" (0.0), so a
        // hiccup degrades gracefully rather than crashing detection.
        self.embed_inner(text).unwrap_or_default()
    }
}
