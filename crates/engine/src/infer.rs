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

/// "don't use X", "do not use X", "stop using X", "avoid X", "no longer use X",
/// "pas de X" — but NOT the JS/comments constraints (handled as rules above).
fn rejected_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:don'?t use|do not use|stop using|no longer use|avoid|n'utilise pas|pas de)\s+([a-z0-9][a-z0-9._+\-]{1,38})",
        )
        .unwrap()
    })
}

/// Extract decisions the user explicitly rejected, as short normalized phrases
/// (e.g. "use bcrypt"). High-precision by design — it only matches clear
/// "don't use X" style statements, so it rarely fires on prose.
pub fn infer_rejected_decisions(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for caps in rejected_re().captures_iter(text) {
        if let Some(obj) = caps.get(1) {
            let object = obj.as_str().trim().trim_end_matches(['.', ',', ';']).trim();
            // Drop phrasings that the JS/comments constraint rules already cover.
            let lc = object.to_ascii_lowercase();
            if lc == "js" || lc.starts_with("comment") {
                continue;
            }
            let phrase = format!("use {object}");
            if !out.contains(&phrase) {
                out.push(phrase);
            }
        }
    }
    out
}

/// A broad EN/FR cue that a user message *states a constraint* on the
/// assistant's work — imperative "must/only/always/never", prohibitions
/// ("don't", "no ", "avoid"), or explicit constraint words. Used as a cheap
/// local gate so we only spend an LLM extraction call on turns that plausibly
/// carry a rule, never on every message.
///
/// This is deliberately *recall-oriented* (unlike [`infer_rules`], which is
/// precision-oriented): a false positive here only costs one judge call that
/// returns `[]`, whereas a miss would silently drop a real fuzzy constraint. The
/// judge is the precision stage.
pub fn has_constraint_cue(text: &str) -> bool {
    constraint_cue_re().is_match(text)
}

fn constraint_cue_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:must(?:\s+not)?|only|always|never|do\s+not|don'?t|no\s+\w|without|avoid|ensure|make\s+sure|keep\s+it|stick\s+to|limit|at\s+most|at\s+least|constraint|require[ds]?|forbid|prohibit|toujours|jamais|uniquement|seulement|sans|évite|assure|limite|au\s+plus|au\s+moins|pas\s+de|ne\s+pas)\b",
        )
        .unwrap()
    })
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
    fn rejected_decisions_high_precision() {
        assert_eq!(
            infer_rejected_decisions("Please don't use bcrypt for hashing"),
            vec!["use bcrypt".to_string()]
        );
        assert_eq!(
            infer_rejected_decisions("avoid redux and stop using moment"),
            vec!["use redux".to_string(), "use moment".to_string()]
        );
        // Plain prose with no rejection phrasing → nothing.
        assert!(infer_rejected_decisions("we should ship this feature soon").is_empty());
    }

    #[test]
    fn constraint_cue_recall() {
        // Imperatives / prohibitions / explicit constraint words fire (EN + FR).
        assert!(has_constraint_cue("Keep it under 200 words please"));
        assert!(has_constraint_cue("You must always cite your sources"));
        assert!(has_constraint_cue("don't use any external libraries"));
        assert!(has_constraint_cue("réponds uniquement en français"));
        assert!(has_constraint_cue("sans jamais utiliser de jargon"));
        // Plain task requests with no rule phrasing stay quiet.
        assert!(!has_constraint_cue("can you help me write a poem"));
        assert!(!has_constraint_cue("what is the capital of France"));
    }

    #[test]
    fn ts_abbreviation_and_no_json_guard() {
        // "TS, no JS" should infer the rule even without the word "typescript".
        assert_eq!(infer_rules("refactor in TS, no JS").len(), 1);
        // "no json" must NOT trigger the JS rule.
        assert!(infer_rules("return no json, just text").is_empty());
    }
}
