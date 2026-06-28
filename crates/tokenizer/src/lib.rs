//! Token counting for Drifterr.
//!
//! Saturation (Signal 4) needs token counts. There are two regimes:
//!
//! * **Exact** — only the proxy channel can give exact counts, because it sees
//!   the real `messages` array sent to the provider and can run the provider's
//!   own tokenizer. Those exact counts are attached to turns upstream and the
//!   engine reads them directly; this crate is not involved.
//! * **Estimated** — the file watcher and browser channels never see the wire
//!   payload, so they estimate. This crate owns that estimation behind a
//!   provider-pluggable trait so the rest of the system never special-cases a
//!   model inline.
//!
//! The estimator here is a deliberately simple heuristic (a bytes-per-token
//! ratio tuned per provider family). It is good enough to drive the AMBER/RED
//! saturation thresholds when exact counts are unavailable. Swapping in a real
//! BPE tokenizer (`tiktoken-rs` for OpenAI, Anthropic's count-tokens endpoint)
//! later is a matter of adding a new `Tokenizer` impl — no caller changes.

/// Anything that can estimate the token count of a piece of text.
pub trait Tokenizer {
    /// Estimated number of tokens in `text`.
    fn count(&self, text: &str) -> usize;
}

/// Provider families we know rough token ratios for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    OpenAI,
    Anthropic,
    /// Unknown / generic model — uses a conservative default ratio.
    Generic,
}

impl Provider {
    /// Map a model identifier to a provider family.
    ///
    /// Matching is substring-based and case-insensitive so it survives the
    /// many naming variants (`gpt-4o`, `o1-preview`, `claude-opus-4-x`, …).
    pub fn from_model(model: &str) -> Provider {
        let m = model.to_ascii_lowercase();
        if m.contains("gpt") || m.starts_with("o1") || m.starts_with("o3") || m.contains("openai") {
            Provider::OpenAI
        } else if m.contains("claude") || m.contains("anthropic") {
            Provider::Anthropic
        } else {
            Provider::Generic
        }
    }

    /// Average bytes of UTF-8 text per token for this family.
    ///
    /// English prose lands around ~4 chars/token for both major BPE
    /// vocabularies; code and punctuation run denser. These ratios bias
    /// slightly conservative (fewer bytes per token ⇒ higher estimate) so we
    /// never *under*-report saturation, which would suppress a real alert.
    fn bytes_per_token(self) -> f32 {
        match self {
            Provider::OpenAI => 3.8,
            Provider::Anthropic => 3.9,
            Provider::Generic => 3.6,
        }
    }
}

/// Heuristic tokenizer: estimates from text length and a per-provider ratio.
///
/// This is the fallback used by non-proxy channels. It is intentionally cheap
/// (no model load, no allocation beyond the char count) and never panics.
#[derive(Debug, Clone, Copy)]
pub struct HeuristicTokenizer {
    provider: Provider,
}

impl HeuristicTokenizer {
    pub fn new(provider: Provider) -> Self {
        Self { provider }
    }

    /// Build an estimator appropriate for the given model identifier.
    pub fn for_model(model: &str) -> Self {
        Self::new(Provider::from_model(model))
    }
}

impl Tokenizer for HeuristicTokenizer {
    fn count(&self, text: &str) -> usize {
        if text.is_empty() {
            return 0;
        }
        // Count by char rather than byte so multi-byte UTF-8 (accents, CJK)
        // does not inflate the estimate.
        let chars = text.chars().count() as f32;
        (chars / self.provider.bytes_per_token()).ceil() as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_zero() {
        let t = HeuristicTokenizer::new(Provider::Generic);
        assert_eq!(t.count(""), 0);
    }

    #[test]
    fn longer_text_more_tokens() {
        let t = HeuristicTokenizer::new(Provider::OpenAI);
        let short = t.count("hello");
        let long = t.count("hello world this is a longer sentence with more words");
        assert!(long > short);
    }

    #[test]
    fn provider_detection() {
        assert_eq!(Provider::from_model("gpt-4o"), Provider::OpenAI);
        assert_eq!(Provider::from_model("o1-preview"), Provider::OpenAI);
        assert_eq!(Provider::from_model("claude-opus-4-x"), Provider::Anthropic);
        assert_eq!(Provider::from_model("llama-3-70b"), Provider::Generic);
    }

    #[test]
    fn ratio_is_reasonable() {
        // ~40 chars of English should land in a plausible token band.
        let t = HeuristicTokenizer::new(Provider::OpenAI);
        let n = t.count("The quick brown fox jumps over a lazy dog");
        assert!((8..=16).contains(&n), "got {n}");
    }
}
