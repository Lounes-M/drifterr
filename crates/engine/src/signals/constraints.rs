//! Signal 1 — constraint adherence (deterministic).
//!
//! This is the credibility backbone: deterministic constraints are checked with
//! rules/regex, so a violation is a fact, not a guess — 0 false positives, 0
//! cost, and it is allowed to drive RED on its own.
//!
//! Each active deterministic constraint is checked against the latest assistant
//! turn (the delta). A violation produces a [`SignalEvent`] carrying the
//! constraint id, the turn index, and the offending span as evidence.
//!
//! Judge-checkable constraints are intentionally *not* handled here — they
//! require a model call and ship in a later milestone. They are skipped so the
//! hard signal stays free and exact.

use crate::baseline::{Baseline, Constraint, Rule};
use crate::conversation::Turn;
use crate::signals::{Evidence, SignalEvent, SignalKind, State};
use regex::Regex;

/// Evaluate Signal 1 against the most recent assistant turn.
///
/// Returns one event per violated deterministic constraint. An empty result
/// means every checked constraint held.
pub fn evaluate(baseline: &Baseline, last_assistant: Option<&Turn>) -> Vec<SignalEvent> {
    let Some(turn) = last_assistant else {
        return Vec::new();
    };

    let mut events = Vec::new();
    for c in baseline.deterministic_constraints() {
        if let Some(span) = check(c, &turn.content) {
            // A constraint the user stated is a fact when broken, and may drive RED.
            // A *proposed* one — inferred from a rules file rather than typed — caps
            // at AMBER until confirmed: the rule check is equally deterministic, but
            // whether the user ever asked for the rule was decided by reading
            // English, and a hard signal must not rest on that. See
            // `Constraint::proposed`.
            let (state, detail) = if c.proposed {
                (
                    State::Amber,
                    format!(
                        "proposed rule {} would be violated: \"{}\" — confirm it to enforce",
                        c.id, c.text
                    ),
                )
            } else {
                (
                    State::Red,
                    format!("constraint {} violated: \"{}\"", c.id, c.text),
                )
            };
            events.push(SignalEvent::new(
                SignalKind::Constraint,
                state,
                Evidence {
                    detail,
                    turn_index: Some(turn.index),
                    constraint_id: Some(c.id.clone()),
                    span,
                },
            ));
        }
    }
    events
}

/// Check one constraint against text. Returns the offending span (or an empty
/// span marker) on violation, `None` when satisfied.
///
/// If the constraint carries an explicit [`Rule`] we honor it; otherwise we
/// infer a rule from common phrasings. Inference is conservative — if we cannot
/// confidently derive a deterministic check we report "satisfied" rather than
/// risk a false positive, because a deterministic signal that cries wolf is
/// worse than one that stays quiet.
fn check(c: &Constraint, content: &str) -> Option<Option<String>> {
    let rule = c.rule.clone().or_else(|| crate::infer::infer_rule(&c.text));
    let rule = rule?;
    apply_rule(&rule, content)
}

/// Test-only view of [`check`], so sibling modules can assert end-to-end that an
/// inferred rule actually fires (or doesn't) on given content.
#[cfg(test)]
pub(crate) fn check_for_test(c: &Constraint, content: &str) -> Option<Option<String>> {
    check(c, content)
}

/// Apply a concrete rule. Outer `Option`: was it violated? Inner `Option`: the
/// span, when the rule could isolate one.
fn apply_rule(rule: &Rule, content: &str) -> Option<Option<String>> {
    match rule {
        Rule::ForbidPattern { pattern } => {
            let re = compile(pattern)?;
            re.find(content).map(|m| Some(m.as_str().to_string()))
        }
        Rule::RequirePattern { pattern } => {
            let re = compile(pattern)?;
            if re.is_match(content) {
                None
            } else {
                // Violation is the *absence* of a match — no span to point at.
                Some(None)
            }
        }
        Rule::ForbidInCode { pattern } => {
            let re = compile(pattern)?;
            for block in code_blocks(content) {
                if let Some(m) = re.find(block) {
                    return Some(Some(m.as_str().to_string()));
                }
            }
            None
        }
        Rule::MaxWords { max } => {
            let count = content.split_whitespace().count();
            if count > *max {
                Some(Some(format!("{count} words (limit {max})")))
            } else {
                None
            }
        }
        Rule::MaxLines { max } => {
            // Check each fenced block independently — a "keep it short" rule is
            // about the size of a given code unit, not the whole reply. No fences
            // ⇒ nothing to measure ⇒ satisfied (never guess prose is code).
            for block in code_blocks(content) {
                let lines = block.trim_matches('\n').lines().count();
                if lines > *max {
                    return Some(Some(format!("{lines} lines (limit {max})")));
                }
            }
            None
        }
        Rule::ForbidPathTouch { pattern } => {
            let re = compile(pattern)?;
            for path in touched_paths(content) {
                if re.is_match(path) {
                    return Some(Some(path.to_string()));
                }
            }
            None
        }
        Rule::ForbidLayerMarkers { label, pattern } => {
            let re = compile(pattern)?;
            for block in code_blocks(content) {
                if let Some(m) = re.find(block) {
                    // Name the boundary, not the regex: the panel shows this
                    // verbatim, and "useState(" alone doesn't explain the problem.
                    return Some(Some(format!("{} ({label})", m.as_str())));
                }
            }
            None
        }
        Rule::ForbidNewFiles => new_file_path(content).map(Some),
    }
}

/// Compile a regex, returning `None` (treated as "cannot check ⇒ no violation")
/// if the pattern is malformed. A bad rule must never crash the engine or
/// manufacture a violation.
fn compile(pattern: &str) -> Option<Regex> {
    Regex::new(pattern).ok()
}

/// Paths a reply's diff headers say were modified.
///
/// Only unified-diff and `diff --git` headers count. A path mentioned in prose is
/// discussion ("I looked at migrations/ but left it alone"), not modification, and
/// conflating the two is precisely how a hard signal loses its credibility. `/dev/null`
/// is skipped — it's the *absence* of a file, not a touched path.
fn touched_paths(content: &str) -> Vec<&str> {
    let mut out = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        let rest = if let Some(r) = line.strip_prefix("--- ") {
            r
        } else if let Some(r) = line.strip_prefix("+++ ") {
            r
        } else if let Some(r) = line.strip_prefix("diff --git ") {
            // "diff --git a/x b/x" — either side names the same file.
            r.split_whitespace().next().unwrap_or("")
        } else {
            continue;
        };
        // Strip the a/ b/ prefix and any trailing tab-separated timestamp.
        let path = rest
            .split('\t')
            .next()
            .unwrap_or("")
            .trim()
            .trim_start_matches("a/")
            .trim_start_matches("b/");
        if path.is_empty() || path == "/dev/null" {
            continue;
        }
        out.push(path);
    }
    out
}

/// The first newly-created file a reply's diff introduces, if any.
///
/// Two unambiguous git markers: `--- /dev/null` (the "before" side is nothing) and
/// an explicit `new file mode` line. Both are structural, so this needs no
/// heuristic about whether a path "looks new".
fn new_file_path(content: &str) -> Option<String> {
    let lines: Vec<&str> = content.lines().map(str::trim).collect();
    for (i, line) in lines.iter().enumerate() {
        if line.starts_with("new file mode") {
            // The following +++ header names it; fall back to the marker itself.
            let named = lines[i..]
                .iter()
                .find_map(|l| l.strip_prefix("+++ "))
                .map(|r| r.trim().trim_start_matches("b/").to_string());
            return Some(named.unwrap_or_else(|| "new file".to_string()));
        }
        if *line == "--- /dev/null" {
            let named = lines[i + 1..]
                .iter()
                .find_map(|l| l.strip_prefix("+++ "))
                .map(|r| r.trim().trim_start_matches("b/").to_string());
            return Some(named.unwrap_or_else(|| "new file".to_string()));
        }
    }
    None
}

/// Extract the contents of fenced code blocks (```...```), so code-scoped rules
/// ignore prose.
///
/// If no fences are present we return nothing rather than treating the whole
/// message as code. Guessing "this prose is actually code" is exactly how a
/// deterministic signal cries wolf (a `//` in a URL, a `#` in a sentence), and
/// a hard signal that produces false positives is worse than one that stays
/// quiet. Channels that emit bare code should wrap it in a fence.
fn code_blocks(content: &str) -> Vec<&str> {
    if !content.contains("```") {
        return Vec::new();
    }
    let mut blocks = Vec::new();
    let mut rest = content;
    while let Some(start) = rest.find("```") {
        // Skip the opening fence and its optional language tag line.
        let after = &rest[start + 3..];
        let body_start = after.find('\n').map(|n| n + 1).unwrap_or(after.len());
        let body = &after[body_start..];
        if let Some(end) = body.find("```") {
            blocks.push(&body[..end]);
            rest = &body[end + 3..];
        } else {
            blocks.push(body);
            break;
        }
    }
    blocks
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::baseline::{Checkable, ConstraintType};
    use crate::conversation::Role;

    fn turn(content: &str) -> Turn {
        Turn {
            index: 3,
            role: Role::Assistant,
            content: content.to_string(),
            tokens: 0,
            timestamp: 0,
        }
    }

    fn det(id: &str, text: &str, rule: Option<Rule>) -> Constraint {
        Constraint {
            id: id.to_string(),
            text: text.to_string(),
            kind: ConstraintType::Tech,
            checkable: Checkable::Deterministic,
            active: true,
            proposed: false,
            rule,
        }
    }

    #[test]
    fn forbid_pattern_violation_has_span() {
        let c = det("c1", "TypeScript only, no JS", None);
        let out = check(&c, "create app.js then build").unwrap();
        assert_eq!(out, Some(".js".to_string()));
    }

    #[test]
    fn forbid_pattern_satisfied() {
        let c = det("c1", "TypeScript only, no JS", None);
        assert!(check(&c, "create app.ts then build").is_none());
    }

    #[test]
    fn no_comments_only_in_code() {
        let c = det("c2", "No comments in code", None);
        // A // inside prose must NOT count; only inside a fence.
        let prose = "Use the path a//b in the URL description.";
        assert!(check(&c, prose).is_none());
        let code = "```ts\nconst x = 1; // oops\n```";
        assert!(check(&c, code).unwrap().is_some());
    }

    #[test]
    fn max_words_rule() {
        let c = det("c3", "Concise", Some(Rule::MaxWords { max: 3 }));
        assert!(check(&c, "one two three").is_none());
        assert!(check(&c, "one two three four").unwrap().is_some());
    }

    #[test]
    fn require_pattern_violation_when_absent() {
        let c = det(
            "c4",
            "Must export default",
            Some(Rule::RequirePattern {
                pattern: r"export default".to_string(),
            }),
        );
        assert_eq!(check(&c, "const x = 1").unwrap(), None); // violated, no span
        assert!(check(&c, "export default x").is_none()); // satisfied
    }

    #[test]
    fn evaluate_skips_judge_and_inactive() {
        let mut b = Baseline {
            goal: "g".into(),
            constraints: vec![
                det("c1", "no .js", None),
                Constraint {
                    checkable: Checkable::Judge,
                    ..det("c2", "concise tone", None)
                },
                Constraint {
                    active: false,
                    ..det("c3", "no .js", None)
                },
            ],
            decisions: vec![],
        };
        b.constraints[0].active = true;
        let events = evaluate(&b, Some(&turn("here is app.js")));
        // Only c1 fires: judge-checkable and inactive are skipped.
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].evidence.constraint_id.as_deref(), Some("c1"));
    }

    #[test]
    fn max_lines_only_in_code() {
        let c = det("cl", "keep it under 3 lines", None);
        // 4-line fenced block → violation; 2-line block → ok.
        let long = "```rs\nlet a = 1;\nlet b = 2;\nlet c = 3;\nlet d = 4;\n```";
        assert!(check(&c, long).unwrap().is_some());
        let short = "```rs\nlet a = 1;\nlet b = 2;\n```";
        assert!(check(&c, short).is_none());
        // No fence ⇒ nothing to measure ⇒ satisfied, even if the prose is long.
        let prose = "one\ntwo\nthree\nfour\nfive\nsix";
        assert!(check(&c, prose).is_none());
    }

    #[test]
    fn no_new_deps_fires_only_on_install_with_package() {
        let c = det("cd", "no new dependencies", None);
        // Install command that names a package → violation.
        assert!(check(&c, "```bash\nnpm install express\n```")
            .unwrap()
            .is_some());
        assert!(check(&c, "```sh\ncargo add serde\n```").unwrap().is_some());
        assert!(check(&c, "```\nnpm i -D typescript\n```")
            .unwrap()
            .is_some());
        // Reinstalling existing deps (no package) → no violation.
        assert!(check(&c, "```bash\nnpm install\n```").is_none());
        // Installing from a lockfile/requirements → no violation (existing deps).
        assert!(check(&c, "```bash\npip install -r requirements.txt\n```").is_none());
        // A prose mention without a fenced command → under-claimed, no violation.
        assert!(check(&c, "you could run npm install lodash").is_none());
    }

    #[test]
    fn protected_file_fires_on_diff_header_only() {
        let c = det("cf", "don't touch package.json", None);
        // A unified-diff header naming the file → violation.
        let diff = "Here's the change:\n```diff\n--- a/package.json\n+++ b/package.json\n@@\n```";
        assert!(check(&c, diff).unwrap().is_some());
        // Merely mentioning the file in prose → no violation (precision).
        assert!(check(&c, "I looked at package.json but left it as-is").is_none());
        // A different file's diff must not trip this constraint.
        assert!(check(&c, "```diff\n+++ b/README.md\n```").is_none());
    }

    #[test]
    fn no_eval_fires_in_code_only() {
        let c = det("ce", "no eval", None);
        assert!(check(&c, "```js\nconst x = eval(src);\n```")
            .unwrap()
            .is_some());
        // "evaluate" in prose (or code) is not eval(.
        assert!(check(&c, "```js\nconst r = evaluate(x);\n```").is_none());
        assert!(check(&c, "we should evaluate this").is_none());
    }

    #[test]
    fn no_secrets_fires_on_secret_shapes() {
        let c = det("cs", "no hardcoded secrets", None);
        let aws = "```py\nAWS_KEY = \"AKIAIOSFODNN7EXAMPLE\"\n```";
        assert!(check(&c, aws).unwrap().is_some());
        // A placeholder / ordinary assignment must NOT fire (no real secret shape).
        assert!(check(&c, "```py\napi_key = \"YOUR_KEY_HERE\"\n```").is_none());
    }

    #[test]
    fn malformed_regex_never_violates() {
        let c = det(
            "c5",
            "bad",
            Some(Rule::ForbidPattern {
                pattern: r"(".to_string(),
            }),
        );
        assert!(check(&c, "anything (").is_none());
    }

    /// A proposed constraint reports AMBER, never RED — the safety net under the
    /// importer. A parser mistake then costs a proposal the user glances at, not a
    /// red alert on a rule nobody wrote.
    #[test]
    fn a_proposed_constraint_caps_at_amber() {
        let stated = Constraint {
            id: "c1".into(),
            text: "No console.log".into(),
            kind: crate::baseline::ConstraintType::Format,
            checkable: crate::baseline::Checkable::Deterministic,
            active: true,
            proposed: false,
            rule: Some(Rule::ForbidInCode {
                pattern: r"console\.log".into(),
            }),
        };
        let proposed = Constraint {
            id: "claude-md-1".into(),
            proposed: true,
            ..stated.clone()
        };
        let turn = Turn {
            index: 3,
            role: crate::conversation::Role::Assistant,
            content: "```js\nconsole.log(1)\n```".into(),
            tokens: 5,
            timestamp: 0,
        };

        let red = evaluate(
            &Baseline {
                goal: "g".into(),
                constraints: vec![stated],
                decisions: vec![],
            },
            Some(&turn),
        );
        assert_eq!(red.len(), 1);
        assert_eq!(
            red[0].state,
            State::Red,
            "a stated constraint may drive RED"
        );

        let amber = evaluate(
            &Baseline {
                goal: "g".into(),
                constraints: vec![proposed],
                decisions: vec![],
            },
            Some(&turn),
        );
        assert_eq!(amber.len(), 1);
        assert_eq!(
            amber[0].state,
            State::Amber,
            "an imported, unconfirmed rule must never drive RED"
        );
        assert!(
            amber[0].evidence.detail.contains("confirm"),
            "and must say how to enforce it: {}",
            amber[0].evidence.detail
        );
    }
}
