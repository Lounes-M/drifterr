//! Deterministic rule inference from natural-language constraint phrasing.
//!
//! Shared by two callers:
//!
//! * [`crate::signals::constraints`] — to derive a check for a constraint that
//!   was given without an explicit [`Rule`].
//! * [`crate::baseline::Baseline::extract`] — to mine constraints out of the
//!   user's own messages when no baseline was supplied (the proxy channel).
//!
//! Inference is deliberately small and high-precision. It only recognizes
//! phrasings we can turn into a false-positive-free deterministic check;
//! everything else falls through to "no rule". A hard signal that cries wolf is
//! worse than one that stays quiet, so we under-claim rather than guess.

use crate::baseline::Rule;
use regex::Regex;
use std::sync::OnceLock;

/// "no js", "not js", "no .js", "pas de js", "sans js" — but NOT "no json"
/// (the `\b` after `js` forbids a following word char).
fn no_js_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?i)\b(no|not|pas de|sans)\s+\.?js\b").unwrap())
}

/// "no comments", "aucun commentaire", "pas de commentaire(s)".
fn no_comments_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"(?i)\b(no comment|aucun commentaire|pas de commentaire)").unwrap()
    })
}

/// Every deterministic rule we can confidently infer from `text`.
///
/// Returns all matches (a single message may state several constraints), so the
/// baseline extractor can capture more than one. Case-insensitive; recognizes
/// both English and French phrasings, matching the product's audience. Matching
/// is word-boundary-anchored so near-misses ("no json") don't fire — a hard
/// signal must not be built on a sloppy substring.
pub fn infer_rules(text: &str) -> Vec<Rule> {
    let mut rules = Vec::new();

    // "no JS" / "no .js" — forbid JavaScript file extensions.
    if no_js_re().is_match(text) {
        rules.push(Rule::ForbidPattern {
            pattern: r"\.js\b".to_string(),
        });
    }

    // "No comments in code" — forbid comment syntax inside code blocks.
    if no_comments_re().is_match(text) {
        rules.push(Rule::ForbidInCode {
            pattern: r"(//|/\*|^\s*#|<!--)".to_string(),
        });
    }

    rules
}

/// The first inferable rule, if any — the per-constraint fallback used by the
/// constraint checker.
pub fn infer_rule(text: &str) -> Option<Rule> {
    infer_rules(text).into_iter().next()
}

/// A stable human label and category for an inferred rule, used when the
/// extractor synthesizes a [`crate::baseline::Constraint`] from a bare rule.
pub fn describe(rule: &Rule) -> (&'static str, crate::baseline::ConstraintType) {
    use crate::baseline::ConstraintType;
    match rule {
        Rule::ForbidPattern { .. } => ("TypeScript only, no JS files", ConstraintType::Tech),
        Rule::ForbidInCode { .. } => ("No comments in code", ConstraintType::Format),
        Rule::RequirePattern { .. } => ("Required pattern must be present", ConstraintType::Tech),
        Rule::MaxWords { .. } => ("Stay within the word limit", ConstraintType::Format),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infers_both_from_one_message() {
        let rules = infer_rules("Use TypeScript only, no JS, and no comments please");
        assert_eq!(rules.len(), 2);
    }

    #[test]
    fn nothing_inferred_from_plain_text() {
        assert!(infer_rules("please make it fast and elegant").is_empty());
    }

    #[test]
    fn ts_abbreviation_and_no_json_guard() {
        // "TS, no JS" should infer the rule even without the word "typescript".
        assert_eq!(infer_rules("refactor in TS, no JS").len(), 1);
        // "no json" must NOT trigger the JS rule.
        assert!(infer_rules("return no json, just text").is_empty());
    }
}
