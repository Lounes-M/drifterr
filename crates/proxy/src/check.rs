//! `drifterr-proxy check` — run the constraint rules over agent output in CI.
//!
//! # Why this exists
//!
//! The engine's constraint checks are the one part of Drifterr that is fully
//! deterministic, needs no model call, and is already false-positive-free by design. In
//! the desktop app they warn one person, once, in a menubar. The same checks over a pull
//! request's diff warn a whole team, on every change, in a place where "this rule was
//! broken" already has somewhere to go.
//!
//! It also reaches a buyer the desktop app cannot: a team that will not install a menubar
//! app on every laptop will happily add a CI step. That is the same engine earning its
//! keep twice.
//!
//! ```bash
//! # Check a diff against a rules file the repo already has
//! git diff origin/main... | drifterr-proxy check --rules CLAUDE.md
//!
//! # Or against a shared pack
//! git diff origin/main... | drifterr-proxy check --pack tight-scope
//! ```
//!
//! # What it is not
//!
//! Not a linter, and it must not become one. It checks *the rules the user stated*, and
//! only those it can verify — the same bar as the live engine. A CI step that invents
//! rules would be uninstalled within a day, so unenforceable rules are reported as
//! skipped rather than guessed at.

use drifterr_engine::baseline::{Baseline, Constraint};
use drifterr_engine::conversation::{ContextState, Conversation, Role, Source, Turn};
use drifterr_engine::signals::{SignalEvent, State};

/// The outcome of a check run.
#[derive(Debug, Default)]
pub struct CheckReport {
    /// Violations found, each naming the constraint and the offending span.
    pub violations: Vec<SignalEvent>,
    /// Constraints that were checked.
    pub checked: usize,
    /// Rules present but not verifiable — reported so their silence is never mistaken
    /// for a pass.
    pub skipped: Vec<String>,
}

impl CheckReport {
    pub fn ok(&self) -> bool {
        self.violations.is_empty()
    }

    /// Render for a terminal / CI log. GitHub Actions annotation syntax is used when
    /// `GITHUB_ACTIONS` is set, so violations surface inline on the pull request rather
    /// than only in a log nobody opens.
    pub fn render(&self, gha: bool) -> String {
        let mut out = String::new();
        if self.checked == 0 {
            out.push_str(
                "drifterr check: no verifiable rules found.\n\
                 Nothing was checked — this is NOT a pass. Point --rules at a file with \
                 checkable rules, or --pack at a rule pack.\n",
            );
            return out;
        }
        out.push_str(&format!(
            "drifterr check: {} rule(s) checked, {} violation(s).\n",
            self.checked,
            self.violations.len()
        ));
        for v in &self.violations {
            let cid = v.evidence.constraint_id.as_deref().unwrap_or("rule");
            let span = v.evidence.span.as_deref().unwrap_or("");
            if gha {
                // One annotation per violation. `::error` fails the step's log parsing
                // into a visible PR comment.
                out.push_str(&format!(
                    "::error title=Drifterr: {cid}::{}{}\n",
                    v.evidence.detail,
                    if span.is_empty() {
                        String::new()
                    } else {
                        format!(" — found: {span}")
                    }
                ));
            } else {
                out.push_str(&format!("  ✗ {cid}: {}", v.evidence.detail));
                if !span.is_empty() {
                    out.push_str(&format!("\n      found: {span}"));
                }
                out.push('\n');
            }
        }
        if !self.skipped.is_empty() {
            // Loud on purpose: a rule the user believes is enforced but isn't is worse
            // than a rule they know is advisory.
            out.push_str(&format!(
                "\n{} rule(s) could not be checked and were SKIPPED (not passed):\n",
                self.skipped.len()
            ));
            for s in &self.skipped {
                out.push_str(&format!("  – {s}\n"));
            }
        }
        out
    }
}

/// Check `content` — a diff, a patch, or an agent transcript — against `constraints`.
///
/// The content is handed to the engine as a single assistant turn, which is exactly what
/// it is: output produced by an agent, to be judged against rules the user set. Reusing
/// the live path rather than reimplementing it means CI and the desktop app can never
/// disagree about whether something is a violation.
pub fn check(content: &str, constraints: Vec<Constraint>) -> CheckReport {
    check_with(content, constraints, Vec::new())
}

/// As [`check`], carrying forward statements the loader found unverifiable so they can be
/// reported as skipped rather than silently passing.
pub fn check_with(
    content: &str,
    constraints: Vec<Constraint>,
    unchecked: Vec<String>,
) -> CheckReport {
    // A constraint with no inferable rule cannot fire; count it as skipped too.
    let mut skipped = unchecked;
    for c in &constraints {
        if c.active && c.rule.is_none() && drifterr_engine::infer::infer_rule(&c.text).is_none() {
            let line = format!("{} — {}", c.id, c.text);
            if !skipped.contains(&line) {
                skipped.push(line);
            }
        }
    }
    let checked = constraints
        .iter()
        .filter(|c| {
            c.active && (c.rule.is_some() || drifterr_engine::infer::infer_rule(&c.text).is_some())
        })
        .count();

    let baseline = Baseline {
        goal: String::new(),
        constraints,
        decisions: Vec::new(),
    };
    let conv = Conversation {
        session_id: "ci".into(),
        model: "ci".into(),
        turns: vec![Turn {
            index: 0,
            role: Role::Assistant,
            content: content.to_string(),
            tokens: 0,
            timestamp: 0,
        }],
        // Saturation is meaningless for a diff, so the context is empty and the signal
        // reads green. Only constraint violations matter here.
        context: ContextState {
            window_size: 1_000_000,
            used_tokens: 0,
            exact: false,
            occupancy_known: true,
            tool_call_count: 0,
        },
        source: Source::File,
    };

    let verdict = drifterr_engine::evaluate(&conv, &baseline);
    let violations = verdict
        .events
        .into_iter()
        .filter(|e| e.state == State::Red && e.signal == drifterr_engine::SignalKind::Constraint)
        .collect();

    CheckReport {
        violations,
        checked,
        skipped,
    }
}

/// Load constraints from a rules file's contents, a pack id, or an inline pack.
pub fn constraints_from(
    rules_text: Option<&str>,
    pack_id: Option<&str>,
    pack_json: Option<&str>,
) -> Result<Vec<Constraint>, String> {
    Ok(load(rules_text, pack_id, pack_json)?.0)
}

/// As [`constraints_from`], also returning statements that read like rules but that the
/// engine cannot verify.
///
/// CI is the one place this matters most: a rule sitting in someone's `CLAUDE.md` that
/// Drifterr silently ignores is a false assurance, and a green check would confirm it.
pub fn load(
    rules_text: Option<&str>,
    pack_id: Option<&str>,
    pack_json: Option<&str>,
) -> Result<(Vec<Constraint>, Vec<String>), String> {
    let mut out = Vec::new();
    let mut unchecked = Vec::new();
    if let Some(text) = rules_text {
        out.extend(drifterr_engine::rules_file::constraints_from_text(
            text, "rules",
        ));
        unchecked.extend(drifterr_engine::rules_file::unchecked_statements(text));
    }
    if let Some(id) = pack_id {
        let (pid, pack) = drifterr_engine::pack::builtin()
            .into_iter()
            .find(|(pid, _)| *pid == id)
            .ok_or_else(|| {
                let ids: Vec<&str> = drifterr_engine::pack::builtin()
                    .into_iter()
                    .map(|(i, _)| i)
                    .collect();
                format!("no such pack '{id}'. Available: {}", ids.join(", "))
            })?;
        let applied = pack.apply(pid);
        unchecked.extend(
            applied
                .advisory
                .iter()
                .map(|id| format!("pack rule '{id}' is not verifiable")),
        );
        out.extend(applied.enforced);
    }
    if let Some(json) = pack_json {
        let pack = drifterr_engine::pack::Pack::from_json(json)?;
        let applied = pack.apply("imported");
        unchecked.extend(
            applied
                .advisory
                .iter()
                .map(|id| format!("pack rule '{id}' is not verifiable")),
        );
        out.extend(applied.enforced);
    }
    Ok((out, unchecked))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RULES: &str = "- No new dependencies\n- Don't touch package.json\n- No console.log\n";

    #[test]
    fn a_clean_diff_passes() {
        let cs = constraints_from(Some(RULES), None, None).unwrap();
        let diff = "```diff\n--- a/src/app.ts\n+++ b/src/app.ts\n@@\n+const x = 1;\n```";
        let r = check(diff, cs);
        assert!(r.ok(), "clean diff must pass: {:?}", r.violations);
        assert!(r.checked >= 3);
    }

    #[test]
    fn a_violating_diff_fails_and_names_the_rule() {
        let cs = constraints_from(Some(RULES), None, None).unwrap();
        let diff =
            "```diff\n--- a/package.json\n+++ b/package.json\n@@\n+  \"lodash\": \"^4\"\n```";
        let r = check(diff, cs);
        assert!(!r.ok(), "touching a protected file must fail");
        let out = r.render(false);
        assert!(out.contains("package.json"), "{out}");
    }

    #[test]
    fn github_annotations_are_emitted_in_actions() {
        let cs = constraints_from(Some("- No console.log\n"), None, None).unwrap();
        let r = check("```ts\nconsole.log('x')\n```", cs);
        assert!(!r.ok());
        let gha = r.render(true);
        assert!(gha.starts_with("drifterr check:"));
        assert!(
            gha.contains("::error title=Drifterr:"),
            "must annotate so it surfaces on the PR: {gha}"
        );
    }

    #[test]
    fn nothing_to_check_is_not_reported_as_a_pass() {
        // The dangerous case: a rules file with no verifiable rules would otherwise exit
        // 0 and read as "all clear", which is a false assurance in CI.
        let (cs, unchecked) = load(
            Some("- Always prefer composition over inheritance\n"),
            None,
            None,
        )
        .unwrap();
        let r = check_with("anything", cs, unchecked);
        assert_eq!(r.checked, 0);
        let out = r.render(false);
        assert!(out.contains("NOT a pass"), "{out}");
    }

    #[test]
    fn unverifiable_rules_are_listed_as_skipped_not_silently_passed() {
        // The false-assurance case: a rule the user wrote that Drifterr cannot check must
        // be named, or a green run confirms protection that doesn't exist.
        let rules = "- No console.log\n- Always prefer composition over inheritance\n";
        let (cs, unchecked) = load(Some(rules), None, None).unwrap();
        let r = check_with("```ts\nconst a = 1;\n```", cs, unchecked);
        assert!(r.ok());
        assert!(!r.skipped.is_empty(), "skipped: {:?}", r.skipped);
        let out = r.render(false);
        assert!(out.contains("SKIPPED (not passed)"), "{out}");
        assert!(out.contains("composition"), "names the rule: {out}");
    }

    #[test]
    fn packs_can_supply_the_rules() {
        let cs = constraints_from(None, Some("tight-scope"), None).unwrap();
        assert!(!cs.is_empty());
        let r = check("```bash\nnpm install lodash\n```", cs);
        assert!(!r.ok(), "the pack's no-new-deps rule must fire");
    }

    #[test]
    fn an_unknown_pack_names_the_available_ones() {
        let err = constraints_from(None, Some("nope"), None).unwrap_err();
        assert!(err.contains("Available:"), "{err}");
        assert!(err.contains("tight-scope"), "{err}");
    }

    #[test]
    fn prose_mentioning_a_protected_file_does_not_fail_ci() {
        // Precision matters more here than anywhere: a CI check that fails on a commit
        // message mentioning a filename gets switched off immediately.
        let cs = constraints_from(Some("- Don't touch package.json\n"), None, None).unwrap();
        let r = check("I reviewed package.json but changed nothing.", cs);
        assert!(
            r.ok(),
            "a mention is not a modification: {:?}",
            r.violations
        );
    }
}
