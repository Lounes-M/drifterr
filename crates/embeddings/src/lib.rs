//! Local text embeddings for the soft signals (goal alignment, degradation).
//!
//! The soft signals only need a **relative** notion of similarity — is this turn
//! closer to or further from the goal than earlier ones; are two replies near-
//! duplicates? They never drive RED on their own, so the embedding does not need
//! to be a heavyweight semantic model.
//!
//! This crate exposes a pluggable [`Embedder`] trait and ships a default
//! [`BagEmbedder`]: a deterministic, dependency-free, zero-network feature-
//! hashing bag-of-words vector. It is local-first by construction (nothing
//! leaves the machine) and good enough to track trends and detect loops.
//!
//! Swapping in a real ONNX model (e.g. `fastembed-rs`/`bge-small`) later is a
//! matter of adding another `Embedder` impl behind a feature flag — the signal
//! code is unchanged.

/// Anything that maps text to a fixed-length vector.
///
/// `Send + Sync` so embedders can be held across `.await` in the async judge
/// phase (and shared across tasks).
pub trait Embedder: Send + Sync {
    /// Embed `text` into an L2-normalized vector. Empty/whitespace text yields a
    /// zero vector.
    fn embed(&self, text: &str) -> Vec<f32>;
}

/// Cosine similarity in `[-1, 1]` (`[0, 1]` for the non-negative bag vectors).
/// Returns `0.0` if either vector is zero or lengths differ.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    let mut dot = 0.0;
    let mut na = 0.0;
    let mut nb = 0.0;
    for i in 0..a.len() {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

/// Default embedder: feature-hashed, normalized term frequencies.
#[derive(Debug, Clone, Copy)]
pub struct BagEmbedder {
    dims: usize,
}

impl Default for BagEmbedder {
    fn default() -> Self {
        Self { dims: 256 }
    }
}

impl BagEmbedder {
    pub fn new(dims: usize) -> Self {
        Self { dims: dims.max(8) }
    }
}

impl Embedder for BagEmbedder {
    fn embed(&self, text: &str) -> Vec<f32> {
        let mut v = vec![0.0f32; self.dims];
        let mut any = false;
        for tok in tokenize(text) {
            if is_stopword(tok) {
                continue;
            }
            any = true;
            let idx = (fnv1a(tok) as usize) % self.dims;
            v[idx] += 1.0;
        }
        if !any {
            return v;
        }
        // L2-normalize so cosine == dot for callers.
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for x in &mut v {
                *x /= norm;
            }
        }
        v
    }
}

/// Lowercase tokenizer: alphanumeric runs of length ≥ 2.
pub fn tokenize(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() >= 2)
}

/// FNV-1a over the lowercased bytes — deterministic across runs (unlike the std
/// hasher), so embeddings are stable.
fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= b.to_ascii_lowercase() as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// A tiny English+French stopword set — dropping these sharpens the signal for
/// short turns without needing a real vocabulary.
fn is_stopword(t: &str) -> bool {
    matches!(
        t.to_ascii_lowercase().as_str(),
        "the"
            | "a"
            | "an"
            | "to"
            | "of"
            | "and"
            | "or"
            | "is"
            | "are"
            | "be"
            | "in"
            | "on"
            | "it"
            | "for"
            | "with"
            | "as"
            | "at"
            | "by"
            | "this"
            | "that"
            | "you"
            | "your"
            | "we"
            | "i"
            | "me"
            | "my"
            | "le"
            | "la"
            | "les"
            | "de"
            | "des"
            | "un"
            | "une"
            | "et"
            | "ou"
            | "est"
            | "dans"
            | "pour"
            | "avec"
            | "ce"
            | "que"
            | "qui"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_text_is_similar() {
        let e = BagEmbedder::default();
        let a = e.embed("refactor the auth module in typescript");
        let b = e.embed("refactor the auth module in typescript");
        assert!((cosine(&a, &b) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn unrelated_text_is_dissimilar() {
        let e = BagEmbedder::default();
        let a = e.embed("refactor the auth module in typescript");
        let b = e.embed("the weather today is sunny and warm outside");
        assert!(cosine(&a, &b) < 0.25, "got {}", cosine(&a, &b));
    }

    #[test]
    fn related_more_similar_than_unrelated() {
        let e = BagEmbedder::default();
        let goal = e.embed("refactor the authentication module in strict typescript");
        let on = e.embed("here is the refactored authentication module in typescript");
        let off = e.embed("let me tell you a story about a dragon and a castle");
        assert!(cosine(&goal, &on) > cosine(&goal, &off));
    }

    #[test]
    fn empty_is_zero_vector() {
        let e = BagEmbedder::default();
        let v = e.embed("   ");
        assert!(v.iter().all(|x| *x == 0.0));
        assert_eq!(cosine(&v, &v), 0.0);
    }

    #[test]
    fn deterministic() {
        let e = BagEmbedder::default();
        assert_eq!(e.embed("hello world"), e.embed("hello world"));
    }
}
