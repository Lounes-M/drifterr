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

/// "no js", "not js", "avoid js", "don't use javascript", "pas de js",
/// "sans javascript", "évite le js" — but NOT "no json" (the `\b` after
/// `js`/`javascript` forbids a following word char).
fn no_js_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:no|not|avoid|don'?t\s+use|do\s+not\s+use|pas\s+de|sans|évite(?:\s+le)?)\s+\.?(?:js|javascript)\b",
        )
        .unwrap()
    })
}

/// A word cap: "under 200 words", "at most 200 words", "no more than 200 words",
/// "max 200 words", "within 200 words", "less than 200 words", plus FR "moins de
/// 200 mots" / "au plus 200 mots". Captures the number.
fn max_words_prefix_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:under|below|at\s+most|no\s+more\s+than|max(?:imum)?(?:\s+of)?|within|less\s+than|fewer\s+than|moins\s+de|au\s+plus|maximum\s+de)\s+(\d{1,5})\s+(?:words?|mots?)\b",
        )
        .unwrap()
    })
}

/// The suffix form: "200 words max", "200 words or fewer", "200 mots max".
fn max_words_suffix_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            r"(?i)\b(\d{1,5})\s+(?:words?|mots?)\s+(?:max|maximum|or\s+(?:fewer|less)|ou\s+moins)\b",
        )
        .unwrap()
    })
}

/// The tightest inferable word cap in `text`, if any.
fn infer_max_words(text: &str) -> Option<usize> {
    let mut best: Option<usize> = None;
    for re in [max_words_prefix_re(), max_words_suffix_re()] {
        for caps in re.captures_iter(text) {
            if let Ok(n) = caps[1].parse::<usize>() {
                if n > 0 {
                    best = Some(best.map_or(n, |b| b.min(n)));
                }
            }
        }
    }
    best
}

/// "no comments", "aucun commentaire", "pas de commentaire(s)".
fn no_comments_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"(?i)\b(no comment|aucun commentaire|pas de commentaire)").unwrap()
    })
}

/// "no TODOs", "no TODO/FIXME", "no placeholders", "pas de TODO". A common
/// Claude Code rule ("finish it, no TODOs left"). We map it to a code-scoped
/// check so the word "todo" in prose never fires it.
fn no_todo_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:no|not|avoid|don'?t\s+(?:use|leave)|do\s+not\s+(?:use|leave)|without|pas\s+de|sans|aucun)\s+(?:todos?|fixmes?|placeholders?)\b",
        )
        .unwrap()
    })
}

/// "no console.log(s)", "remove console logs", "no console statements", "pas de
/// console.log". Scoped to JS/TS debug logging via the literal `console` so it
/// stays precise.
fn no_console_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:no|not|avoid|don'?t\s+use|do\s+not\s+use|remove|strip|pas\s+de|sans)\s+console(?:\.\w+|\s+(?:logs?|statements?|calls?))?\b",
        )
        .unwrap()
    })
}

/// "no any type", "don't use any", "avoid any types", "pas de any" — the
/// TypeScript `any` prohibition. Requires the type context ("any type", "use
/// any", backticked `any`) so the everyday English word "any" never trips it.
fn no_any_type_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        // Requires the TYPE context — "any type(s)", a backticked `any`, or the
        // explicit FR "pas de any" — so the everyday word "any" never fires.
        Regex::new(
            r"(?i)(?:\b(?:no|avoid|don'?t\s+use|do\s+not\s+use)\s+(?:`any`|\bany\b)\s+types?\b|\b(?:no|avoid|don'?t\s+use|do\s+not\s+use)\s+`any`|\bpas\s+de\s+(?:`any`|\bany\b))",
        )
        .unwrap()
    })
}

/// "no new dependencies", "don't add packages/libraries", "no new deps",
/// "pas de nouvelle dépendance", "n'ajoute pas de dépendances". A very common
/// Claude Code guardrail. Checked by forbidding package-manager install commands
/// (that name a package) inside code — see [`INSTALL_CMD_PATTERN`].
fn no_new_deps_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:no\s+(?:new\s+|additional\s+|extra\s+)?(?:dependenc(?:y|ies)|deps?|packages?|libraries|libs?|modules?)|(?:don'?t|do\s+not|never|no\s+need\s+to)\s+(?:add|install|introduce|pull\s+in)\s+(?:any\s+|a\s+|new\s+)?(?:dependenc(?:y|ies)|deps?|packages?|libraries|libs?)|(?:n'?ajoute[rz]?\s+pas|sans|pas\s+de)\s+(?:nouvelles?\s+)?(?:d[ée]pendances?|paquets?|librairies?))\b",
        )
        .unwrap()
    })
}

/// A package-manager install command that names a package (so `npm install`
/// alone — reinstalling existing deps — never fires). Only whitelisted install
/// flags are tolerated before the package, so `pip install -r requirements.txt`
/// (installing *existing* deps from a file) does NOT match. Line-anchored and
/// checked inside code blocks only, keeping it false-positive-free.
const INSTALL_CMD_PATTERN: &str = r"(?im)^[ \t]*(?:sudo[ \t]+)?(?:npm[ \t]+(?:install|i|add)|yarn[ \t]+add|pnpm[ \t]+(?:add|install)|bun[ \t]+add|pip3?[ \t]+install|cargo[ \t]+add|go[ \t]+get|gem[ \t]+install|composer[ \t]+require|poetry[ \t]+add)[ \t]+(?:(?:--save(?:-dev|-exact)?|--global|--dev|--user|--production|--upgrade|-[DgUSEPw])[ \t]+)*[@a-zA-Z0-9][\w@./+-]*";

/// "no eval", "don't use eval", "avoid eval", "pas de eval" — forbid the
/// dynamic-code `eval(` call in code (a classic security/quality constraint).
fn no_eval_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:no|don'?t\s+use|do\s+not\s+use|avoid|never\s+use|pas\s+de|sans)\s+eval\b",
        )
        .unwrap()
    })
}

/// "no hardcoded secrets/keys/passwords", "don't hardcode credentials", "no
/// secrets in code", "pas de secrets en dur". Checked by scanning code for
/// unambiguous secret *shapes* (see [`SECRET_PATTERN`]), never a loose
/// `password = "..."` (which would false-positive on placeholders).
fn no_secrets_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            r#"(?i)(?:\bno\s+hard[\s-]?coded?\s+(?:secrets?|keys?|api[\s-]?keys?|passwords?|credentials?|tokens?)|(?:don'?t|do\s+not|never)\s+hard[\s-]?code|\bno\s+(?:secrets?|api[\s-]?keys?|credentials?|tokens?)\s+(?:in\s+(?:the\s+)?code|committed|hard[\s-]?coded)|\bpas\s+de\s+(?:secrets?|cl[ée]s?|mots?\s+de\s+passe|identifiants?)\s+en\s+dur)"#,
        )
        .unwrap()
    })
}

/// Unambiguous secret shapes: AWS access-key id, a PEM private-key header, and
/// common provider token prefixes (GitHub, Slack, OpenAI, Google). Each is
/// specific enough to be false-positive-free; deliberately *not* a generic
/// `KEY = "..."` assignment, which trips on example/placeholder values.
const SECRET_PATTERN: &str = r#"(?:AKIA[0-9A-Z]{16}|-----BEGIN (?:RSA |EC |DSA |OPENSSH |PGP )?PRIVATE KEY-----|ghp_[0-9A-Za-z]{36}|github_pat_[0-9A-Za-z_]{20,}|xox[baprs]-[0-9A-Za-z-]{10,}|sk-[A-Za-z0-9]{20,}|AIza[0-9A-Za-z_-]{35})"#;

/// "don't touch/modify/edit/change X", "leave X alone", "ne touche pas à X" —
/// where X names a file or path. Captures the file so a per-file protected rule
/// can be built. Deliberately high-precision: the captured token must look like a
/// file (has an extension or a path separator), checked by the caller.
fn protected_file_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:(?:don'?t|do\s+not|never|please\s+don'?t)\s+(?:ever\s+)?(?:touch|modify|change|edit|alter|rewrite|refactor)\s+(?:the\s+|module\s+|file\s+)*(\.?[A-Za-z0-9_][\w./-]*)|\bleave\s+(?:the\s+)?(\.?[A-Za-z0-9_][\w./-]*)\s+(?:alone|untouched|as[\s-]is)|\bne\s+(?:touche[rz]?|modifie[rz]?|change[rz]?|[ée]dite[rz]?)\s+pas\s+(?:à\s+la\s+|à\s+|au\s+|le\s+|la\s+|l')*(\.?[A-Za-z0-9_][\w./-]*))",
        )
        .unwrap()
    })
}

/// The tightest inferable *line* cap in `text` (mirrors [`infer_max_words`] for
/// "under 50 lines" / "50 lines max" / "moins de 50 lignes"), if any.
fn infer_max_lines(text: &str) -> Option<usize> {
    static PREFIX: OnceLock<Regex> = OnceLock::new();
    static SUFFIX: OnceLock<Regex> = OnceLock::new();
    let prefix = PREFIX.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:under|below|at\s+most|no\s+more\s+than|max(?:imum)?(?:\s+of)?|within|less\s+than|fewer\s+than|moins\s+de|au\s+plus|maximum\s+de)\s+(\d{1,6})\s+(?:lines?|lignes?)(?:\s+of\s+code)?\b",
        )
        .unwrap()
    });
    let suffix = SUFFIX.get_or_init(|| {
        Regex::new(
            r"(?i)\b(\d{1,6})\s+(?:lines?|lignes?)(?:\s+of\s+code)?\s+(?:max|maximum|or\s+(?:fewer|less)|ou\s+moins)\b",
        )
        .unwrap()
    });
    let mut best: Option<usize> = None;
    for re in [prefix, suffix] {
        for caps in re.captures_iter(text) {
            if let Ok(n) = caps[1].parse::<usize>() {
                if n > 0 {
                    best = Some(best.map_or(n, |b| b.min(n)));
                }
            }
        }
    }
    best
}

/// Strip *trailing* sentence punctuation/quotes from a captured file token, so
/// "config.py." or "package.json," yields the bare path. Trailing-only, so a
/// leading dot (`.env`, `.gitignore`) is preserved.
fn clean_file_token(tok: &str) -> &str {
    tok.trim_end_matches(['.', ',', ';', ':', '!', '?', ')', '"', '\''])
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

    // "No TODOs / FIXMEs" — forbid leftover markers in code.
    if no_todo_re().is_match(text) {
        rules.push(Rule::ForbidInCode {
            pattern: r"\b(?:TODO|FIXME)\b".to_string(),
        });
    }

    // "No console.log" — forbid JS/TS debug logging left in code.
    if no_console_re().is_match(text) {
        rules.push(Rule::ForbidInCode {
            pattern: r"console\.(?:log|debug|info|warn|error)\b".to_string(),
        });
    }

    // "No `any` type" — forbid TypeScript `any` annotations in code.
    if no_any_type_re().is_match(text) {
        rules.push(Rule::ForbidInCode {
            pattern: r":\s*any\b".to_string(),
        });
    }

    // "under 200 words" / "200 words max" — a hard length cap.
    if let Some(max) = infer_max_words(text) {
        rules.push(Rule::MaxWords { max });
    }

    // "under 50 lines" / "50 lines max" — a per-code-block length cap.
    if let Some(max) = infer_max_lines(text) {
        rules.push(Rule::MaxLines { max });
    }

    // "No new dependencies" — forbid install commands that name a package.
    if no_new_deps_re().is_match(text) {
        rules.push(Rule::ForbidInCode {
            pattern: INSTALL_CMD_PATTERN.to_string(),
        });
    }

    // "No eval" — forbid the dynamic-code eval() call in code.
    if no_eval_re().is_match(text) {
        rules.push(Rule::ForbidInCode {
            pattern: r"\beval\s*\(".to_string(),
        });
    }

    // "No hardcoded secrets" — scan code for unambiguous secret shapes.
    if no_secrets_re().is_match(text) {
        rules.push(Rule::ForbidInCode {
            pattern: SECRET_PATTERN.to_string(),
        });
    }

    // "Don't touch <file>" — forbid a unified-diff header naming that file.
    // Parameterized per captured path; a prose mention won't match a diff line,
    // and tool-only edits (no diff text) are under-claimed by design.
    for caps in protected_file_re().captures_iter(text) {
        let Some(tok) = caps.get(1).or_else(|| caps.get(2)).or_else(|| caps.get(3)) else {
            continue;
        };
        let file = clean_file_token(tok.as_str());
        // Must look like a file: an extension or a path separator. This keeps
        // "don't touch it/that" from ever producing a rule.
        if !(file.contains('/') || file.contains('.')) || file.len() < 3 {
            continue;
        }
        let esc = regex::escape(file);
        let pattern = format!(r"(?m)^(?:diff --git |\+\+\+ |--- )[ab]/(?:\S*/)?{esc}\b");
        let rule = Rule::ForbidPattern { pattern };
        if !rules.contains(&rule) {
            rules.push(rule);
        }
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
            if lc == "js" || lc == "javascript" || lc.starts_with("comment") {
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
        // ForbidPattern covers the no-JS rule and the parameterized protected-file
        // rule; the diff-header pattern distinguishes the latter.
        Rule::ForbidPattern { pattern } if pattern.contains("diff --git") => (
            "A protected file must not be modified",
            ConstraintType::Tech,
        ),
        Rule::ForbidPattern { .. } => ("TypeScript only, no JS files", ConstraintType::Tech),
        // Several distinct code rules share the ForbidInCode mechanism; name each
        // by its pattern so the panel can state the actual cause, not a generic
        // "no comments". Keep these substrings in sync with `infer_rules`.
        Rule::ForbidInCode { pattern } if pattern.contains("TODO") => {
            ("No TODOs or FIXMEs in code", ConstraintType::Format)
        }
        Rule::ForbidInCode { pattern } if pattern.contains("console") => {
            ("No console logging in code", ConstraintType::Format)
        }
        Rule::ForbidInCode { pattern } if pattern.contains(":\\s*any") => {
            ("No `any` type in code", ConstraintType::Tech)
        }
        Rule::ForbidInCode { pattern } if pattern.contains("npm") => {
            ("No new dependencies", ConstraintType::Tech)
        }
        Rule::ForbidInCode { pattern } if pattern.contains("eval") => {
            ("No eval() calls in code", ConstraintType::Tech)
        }
        Rule::ForbidInCode { pattern } if pattern.contains("AKIA") => {
            ("No hardcoded secrets in code", ConstraintType::Tech)
        }
        Rule::ForbidInCode { .. } => ("No comments in code", ConstraintType::Format),
        Rule::RequirePattern { .. } => ("Required pattern must be present", ConstraintType::Tech),
        Rule::MaxWords { .. } => ("Stay within the word limit", ConstraintType::Format),
        Rule::MaxLines { .. } => ("Stay within the code line limit", ConstraintType::Format),
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

    #[test]
    fn no_js_broadened_phrasings() {
        for s in [
            "avoid javascript entirely",
            "don't use js here",
            "do not use JavaScript",
            "évite le js",
            "sans javascript",
        ] {
            assert!(
                infer_rules(s)
                    .iter()
                    .any(|r| matches!(r, Rule::ForbidPattern { .. })),
                "should infer no-JS from: {s}"
            );
        }
        // "jsonify" / "no json" still must not fire.
        assert!(infer_rules("please jsonify the output").is_empty());
    }

    #[test]
    fn word_limit_inference() {
        let cases = [
            ("keep it under 200 words", 200),
            ("at most 50 words please", 50),
            ("no more than 1000 words", 1000),
            ("answer in 30 words max", 30),
            ("réponds en moins de 100 mots", 100),
            ("120 mots max", 120),
        ];
        for (text, expect) in cases {
            let max = infer_rules(text).into_iter().find_map(|r| match r {
                Rule::MaxWords { max } => Some(max),
                _ => None,
            });
            assert_eq!(max, Some(expect), "for: {text}");
        }
        // The tightest cap wins when several are stated.
        let max = infer_rules("under 500 words, ideally 200 words max")
            .into_iter()
            .find_map(|r| match r {
                Rule::MaxWords { max } => Some(max),
                _ => None,
            });
        assert_eq!(max, Some(200));
        // Numbers unrelated to a word cap don't fire.
        assert!(infer_rules("use port 8080 and 3 retries").is_empty());
    }

    /// Helper: does `text` infer a code rule whose forbid pattern contains `frag`?
    fn infers_code_pattern(text: &str, frag: &str) -> bool {
        infer_rules(text)
            .iter()
            .any(|r| matches!(r, Rule::ForbidInCode { pattern } if pattern.contains(frag)))
    }

    #[test]
    fn no_todo_inference() {
        for s in [
            "no TODOs left please",
            "finish it, no todo",
            "don't leave FIXMEs",
            "no placeholders",
            "pas de todo",
            "sans placeholder",
        ] {
            assert!(
                infers_code_pattern(s, "TODO"),
                "should infer no-TODO from: {s}"
            );
        }
        // "today" / prose must not fire (word-boundary + specific words).
        assert!(!infers_code_pattern("ship it today", "TODO"));
        assert!(infer_rules("what should I do today?").is_empty());
    }

    #[test]
    fn no_console_inference() {
        for s in [
            "no console.log",
            "remove console logs",
            "don't use console.error",
            "strip console statements",
            "pas de console.log",
        ] {
            assert!(
                infers_code_pattern(s, "console"),
                "should infer no-console from: {s}"
            );
        }
        // A game console mention without a prohibition doesn't fire.
        assert!(!infers_code_pattern(
            "the console shows the score",
            "console"
        ));
    }

    #[test]
    fn no_any_type_inference() {
        for s in [
            "no any type",
            "don't use any type",
            "do not use any types",
            "avoid any types",
            "no `any`",
            "pas de any",
        ] {
            assert!(
                infers_code_pattern(s, "any"),
                "should infer no-any from: {s}"
            );
        }
        // Bare "any" without the type context stays quiet (precision).
        assert!(!infers_code_pattern("use any approach you prefer", "any"));
        // The everyday word "any" in prose must never trip it.
        assert!(!infers_code_pattern(
            "let me know if you have any questions",
            "any"
        ));
        assert!(infer_rules("pick any font you like").is_empty());
    }

    #[test]
    fn no_new_deps_inference() {
        for s in [
            "no new dependencies",
            "don't add any dependencies",
            "no new packages please",
            "never install new libraries",
            "pas de nouvelles dépendances",
            "n'ajoute pas de dépendances",
        ] {
            assert!(
                infer_rules(s).iter().any(
                    |r| matches!(r, Rule::ForbidInCode { pattern } if pattern.contains("npm"))
                ),
                "should infer no-new-deps from: {s}"
            );
        }
        // A plain feature ask must not fire.
        assert!(infer_rules("add a login page").is_empty());
        assert!(infer_rules("write the dependency injection container").is_empty());
    }

    #[test]
    fn no_eval_inference() {
        for s in ["no eval", "don't use eval", "avoid eval", "pas de eval"] {
            assert!(
                infers_code_pattern(s, "eval"),
                "should infer no-eval from: {s}"
            );
        }
        // "evaluate" / "retrieval" must not fire (word boundary in the cue).
        assert!(infer_rules("please evaluate the tradeoffs").is_empty());
    }

    #[test]
    fn no_secrets_inference() {
        for s in [
            "no hardcoded secrets",
            "don't hardcode API keys",
            "no credentials in code",
            "pas de secrets en dur",
        ] {
            assert!(
                infers_code_pattern(s, "AKIA"),
                "should infer no-secrets from: {s}"
            );
        }
        assert!(infer_rules("keep the design secret for now").is_empty());
    }

    #[test]
    fn protected_file_inference() {
        // A named file/path yields a diff-header ForbidPattern for that file.
        let rules = infer_rules("please don't touch package.json");
        assert!(rules
            .iter()
            .any(|r| matches!(r, Rule::ForbidPattern { pattern }
                if pattern.contains(r"package\.json") && pattern.contains("diff --git"))));
        // A leading dot (dotfile) is preserved, not stripped.
        assert!(infer_rules("don't modify .env")
            .iter()
            .any(|r| matches!(r, Rule::ForbidPattern { pattern } if pattern.contains(r"\.env"))));
        // Path with separators works too.
        assert!(infer_rules("do not edit src/db/schema.sql").iter().any(
            |r| matches!(r, Rule::ForbidPattern { pattern } if pattern.contains(r"schema\.sql"))
        ));
        // FR + "leave X alone".
        assert!(!infer_rules("ne touche pas à config.toml").is_empty());
        assert!(!infer_rules("leave Cargo.lock alone").is_empty());
        // A non-file object ("it", a bare word) produces no rule (precision).
        assert!(infer_rules("don't touch it").is_empty());
        assert!(infer_rules("don't change the plan").is_empty());
    }

    #[test]
    fn line_limit_inference() {
        for (text, expect) in [
            ("keep functions under 40 lines", 40usize),
            ("no more than 100 lines", 100),
            ("50 lines max", 50),
            ("moins de 30 lignes", 30),
            ("under 200 lines of code", 200),
        ] {
            let max = infer_rules(text).into_iter().find_map(|r| match r {
                Rule::MaxLines { max } => Some(max),
                _ => None,
            });
            assert_eq!(max, Some(expect), "for: {text}");
        }
        // The tightest cap wins; word caps stay a separate rule.
        assert!(infer_rules("under 200 words")
            .iter()
            .all(|r| !matches!(r, Rule::MaxLines { .. })));
    }

    #[test]
    fn new_code_rules_are_named_distinctly() {
        use crate::baseline::Rule;
        let todo = describe(&Rule::ForbidInCode {
            pattern: r"\b(?:TODO|FIXME)\b".into(),
        })
        .0;
        let console = describe(&Rule::ForbidInCode {
            pattern: r"console\.(?:log|debug|info|warn|error)\b".into(),
        })
        .0;
        let any = describe(&Rule::ForbidInCode {
            pattern: r":\s*any\b".into(),
        })
        .0;
        let comments = describe(&Rule::ForbidInCode {
            pattern: r"(//|/\*)".into(),
        })
        .0;
        // Each ForbidInCode rule names its own cause — no generic collision.
        assert!(todo.contains("TODO"));
        assert!(console.to_lowercase().contains("console"));
        assert!(any.contains("any"));
        assert!(comments.to_lowercase().contains("comment"));
        assert_ne!(todo, comments);
        assert_ne!(console, comments);
        assert_ne!(any, comments);
    }
}
