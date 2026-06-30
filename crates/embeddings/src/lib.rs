//! Local text embeddings for the soft signals (goal alignment, degradation).
//!
//! The soft signals only need a **relative** notion of similarity — is this turn
//! closer to or further from the goal than earlier ones; are two replies near-
//! duplicates? They never drive RED on their own, so the embedding does not need
//! to be a heavyweight semantic model.
//!
//! This crate exposes a pluggable [`Embedder`] trait and ships a default
//! [`BagEmbedder`]: a deterministic, dependency-free, zero-network **hybrid
//! lexical** model (word unigrams + bigrams + intra-word character trigrams,
//! sub-linear TF, L2-normalized). It is local-first by construction (nothing
//! leaves the machine) and good enough to track trends, link related word forms
//! and detect loops.
//!
//! The engine selects its embedder via [`default_embedder`], so swapping in a
//! real ONNX model (e.g. `bge-small`) is a one-place change behind a feature
//! flag — the signal code is unchanged. See `README.md`.

/// Anything that maps text to a fixed-length vector.
///
/// `Send + Sync` so embedders can be held across `.await` in the async judge
/// phase (and shared across tasks).
pub trait Embedder: Send + Sync {
    /// Embed `text` into an L2-normalized vector. Empty/whitespace text yields a
    /// zero vector.
    fn embed(&self, text: &str) -> Vec<f32>;
}

/// The embedder the engine should use. This is the single place to swap the
/// implementation: today it returns the local [`BagEmbedder`]; a real ONNX model
/// (behind a feature flag) would be selected here, with a graceful fallback to
/// the local model when no model file is configured. The engine depends only on
/// the returned `dyn Embedder`, so nothing else changes.
pub fn default_embedder() -> Box<dyn Embedder> {
    // Future (feature = "onnx"): if DRIFTERR_EMBED_MODEL points at a model file,
    // return Box::new(OnnxEmbedder::load(path)?) and fall back to BagEmbedder on
    // any error. See README.md → "Upgrading to a semantic ONNX model".
    Box::new(BagEmbedder::default())
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

/// Default embedder: a deterministic, dependency-free **hybrid lexical** model.
///
/// It feature-hashes three feature families into one normalized vector:
/// * **word unigrams** — the core term-frequency signal;
/// * **word bigrams** — captures short phrases ("server side" ≠ "side server");
/// * **intra-word character trigrams** — captures morphology and typos, so
///   "auth" and "authentication" share signal even with no whole-word overlap.
///
/// Counts are damped with sub-linear TF (`1 + ln(count)`) so a repeated word
/// doesn't dominate, then the vector is L2-normalized so cosine == dot for
/// callers. This is a real step up from plain bag-of-words for the short-text
/// similarity the soft signals need, while staying local-first and zero-cost.
/// For true semantic similarity, swap in an ONNX model behind the [`Embedder`]
/// trait — see the crate-level docs.
#[derive(Debug, Clone, Copy)]
pub struct BagEmbedder {
    dims: usize,
}

/// Relative weights per feature family. Words dominate; char trigrams add a
/// softer morphological signal without drowning out whole words.
const W_WORD: f32 = 1.0;
const W_BIGRAM: f32 = 0.8;
const W_CHAR: f32 = 0.45;

impl Default for BagEmbedder {
    fn default() -> Self {
        Self { dims: 512 }
    }
}

impl BagEmbedder {
    pub fn new(dims: usize) -> Self {
        Self { dims: dims.max(8) }
    }
}

impl Embedder for BagEmbedder {
    fn embed(&self, text: &str) -> Vec<f32> {
        // Accumulate (raw count, strongest family weight) per hashed feature.
        let mut counts: std::collections::HashMap<usize, (f32, f32)> =
            std::collections::HashMap::new();
        let mut bump = |idx: usize, weight: f32| {
            let e = counts.entry(idx).or_insert((0.0, weight));
            e.0 += 1.0;
            if weight > e.1 {
                e.1 = weight;
            }
        };

        let mut prev: Option<&str> = None;
        let mut any = false;
        for tok in tokenize(text) {
            if is_stopword(tok) {
                prev = None; // don't bridge a bigram across a dropped stopword
                continue;
            }
            any = true;
            bump((fnv1a(tok) as usize) % self.dims, W_WORD);
            if let Some(p) = prev {
                let bigram = format!("{p}\u{1}{tok}");
                bump((fnv1a(&bigram) as usize) % self.dims, W_BIGRAM);
            }
            for tri in char_trigrams(tok) {
                bump((fnv1a(&tri) as usize) % self.dims, W_CHAR);
            }
            prev = Some(tok);
        }

        let mut v = vec![0.0f32; self.dims];
        if !any {
            return v;
        }
        for (idx, (count, weight)) in counts {
            // Sub-linear TF damping, scaled by the feature family's weight.
            v[idx] += weight * (1.0 + count.ln());
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

/// Character trigrams of a single (lowercased) token, with `^`/`$` boundary
/// markers so prefixes/suffixes count. Tokens shorter than a trigram yield their
/// bounded form as one feature.
fn char_trigrams(tok: &str) -> Vec<String> {
    let chars: Vec<char> = format!("^{}$", tok.to_ascii_lowercase()).chars().collect();
    if chars.len() < 3 {
        return vec![chars.into_iter().collect()];
    }
    chars.windows(3).map(|w| w.iter().collect()).collect()
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

    #[test]
    fn morphology_links_related_word_forms() {
        // Char trigrams let shared roots register even with no whole-word
        // overlap — the old pure bag-of-words scored these at ~0.
        let e = BagEmbedder::default();
        let related = cosine(&e.embed("authentication"), &e.embed("authenticate"));
        let unrelated = cosine(&e.embed("authentication"), &e.embed("dragon"));
        assert!(
            related > 0.3,
            "related word forms should share signal, got {related}"
        );
        assert!(
            related > unrelated + 0.2,
            "related ({related}) ≫ unrelated ({unrelated})"
        );
    }

    #[test]
    fn bigrams_capture_word_order() {
        // Identical word multisets, so unigram + char-trigram features are equal
        // for both candidates — only the bigrams differ. The candidate that
        // preserves the query's adjacent pairs must score higher.
        let e = BagEmbedder::default();
        let q = e.embed("server side rendering only");
        let keeps_pairs = cosine(&q, &e.embed("only server side rendering"));
        let breaks_pairs = cosine(&q, &e.embed("rendering side server only"));
        assert!(
            keeps_pairs > breaks_pairs,
            "order-preserving ({keeps_pairs}) > scrambled ({breaks_pairs})"
        );
    }

    #[test]
    fn distinct_replies_stay_below_loop_territory() {
        // Guards the degradation signal: two genuinely different replies must not
        // look like near-duplicates just because char trigrams add overlap.
        let e = BagEmbedder::default();
        let a = e.embed("I added the CSV export endpoint on the server with tests.");
        let b = e.embed("Let's discuss the color palette for the marketing page.");
        assert!(
            cosine(&a, &b) < 0.5,
            "distinct replies too similar: {}",
            cosine(&a, &b)
        );
    }
}
