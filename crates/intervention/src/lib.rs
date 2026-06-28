//! Intervention — turning a drift verdict back into alignment.
//!
//! Two deterministic, model-free outputs (M3a):
//!
//! * **Snapshot** — a markdown reset, built from the baseline (`goal`, active
//!   `constraints`, held/rejected `decisions`) plus a one-line state summary,
//!   ready to paste into a fresh thread when the current one has rotted.
//! * **Preamble** — a short reminder of the binding constraints (and the most
//!   recent violation) to prepend to the next turn, nudging the model back
//!   without starting over.
//!
//! Both are pure functions of data Drifterr already has — no LLM, no network —
//! so they are cheap, private, and fully testable. The proxy exposes them via
//! the control API; the menubar's "Re-anchor" button renders them.

use drifterr_engine::baseline::{Baseline, Constraint, ConstraintType, Decision};
use drifterr_engine::signals::State;
use serde::Serialize;

/// A minimal summary of the triggering signal, so the snapshot/preamble can name
/// the most recent problem. Mirrors the proxy's per-signal view without coupling
/// to it.
#[derive(Debug, Clone, Default)]
pub struct Trigger {
    pub signal: String,
    pub detail: String,
}

/// The intervention outputs for one session.
#[derive(Debug, Clone, Serialize)]
pub struct Reanchor {
    /// Full markdown reset to paste into a new thread.
    pub snapshot: String,
    /// Short reminder to prepend to the next turn in the same thread.
    pub preamble: String,
}

/// Build both intervention outputs.
pub fn reanchor(
    baseline: &Baseline,
    state: State,
    saturation_pct: u32,
    exact: bool,
    trigger: Option<&Trigger>,
) -> Reanchor {
    Reanchor {
        snapshot: snapshot_markdown(baseline, state, saturation_pct, exact, trigger),
        preamble: preamble(baseline, trigger),
    }
}

fn state_label(state: State) -> &'static str {
    match state {
        State::Green => "Aligned",
        State::Amber => "Watch",
        State::Red => "Drifting",
    }
}

fn type_label(t: ConstraintType) -> &'static str {
    match t {
        ConstraintType::Tech => "tech",
        ConstraintType::Format => "format",
        ConstraintType::Tone => "tone",
        ConstraintType::Other => "other",
    }
}

fn active_constraints(baseline: &Baseline) -> Vec<&Constraint> {
    baseline.constraints.iter().filter(|c| c.active).collect()
}

/// The pasteable reset document.
fn snapshot_markdown(
    baseline: &Baseline,
    state: State,
    saturation_pct: u32,
    exact: bool,
    trigger: Option<&Trigger>,
) -> String {
    let goal = if baseline.goal.trim().is_empty() {
        "(no explicit goal captured)"
    } else {
        baseline.goal.trim()
    };

    let mut md = String::new();
    md.push_str("# Re-anchor\n\n");
    md.push_str(
        "This session drifted from its original intent. Here is the ground truth \
         to continue from — please honor it from your next reply.\n\n",
    );

    md.push_str("## Goal\n");
    md.push_str(goal);
    md.push_str("\n\n");

    md.push_str("## Active constraints\n");
    let constraints = active_constraints(baseline);
    if constraints.is_empty() {
        md.push_str("_None recorded._\n");
    } else {
        for c in &constraints {
            md.push_str(&format!("- [{}] {}\n", type_label(c.kind), c.text));
        }
    }
    md.push('\n');

    let (kept, rejected): (Vec<&Decision>, Vec<&Decision>) =
        baseline.decisions.iter().partition(|d| !d.rejected);
    if !kept.is_empty() || !rejected.is_empty() {
        md.push_str("## Decisions\n");
        for d in &kept {
            md.push_str(&format!("- Keep: {}\n", d.text));
        }
        for d in &rejected {
            md.push_str(&format!("- Rejected (do not reintroduce): {}\n", d.text));
        }
        md.push('\n');
    }

    md.push_str("## Where we are\n");
    let trigger_line = match trigger {
        Some(t) if !t.detail.is_empty() => t.detail.clone(),
        _ => "no specific trigger".to_string(),
    };
    md.push_str(&format!(
        "- Status: {} — {}\n",
        state_label(state),
        trigger_line
    ));
    md.push_str(&format!(
        "- Context: {}% full ({})\n\n",
        saturation_pct,
        if exact { "exact" } else { "estimated" }
    ));

    md.push_str("## What I need from you\n");
    md.push_str(
        "Resume the goal above, honoring every active constraint. Re-read them \
         before answering, and call out if any instruction conflicts.\n",
    );

    md
}

/// The short in-thread reminder.
fn preamble(baseline: &Baseline, trigger: Option<&Trigger>) -> String {
    let mut out = String::from("[Drifterr re-anchor] Binding constraints for this task:\n");
    let constraints = active_constraints(baseline);
    if constraints.is_empty() {
        out.push_str(&format!("- Stay on goal: {}\n", baseline.goal.trim()));
    } else {
        for c in &constraints {
            out.push_str(&format!("- {}\n", c.text));
        }
    }
    if let Some(t) = trigger {
        if !t.detail.is_empty() {
            out.push_str(&format!("Most recent issue: {}\n", t.detail));
        }
    }
    out.push_str("Please honor these from now on.");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use drifterr_engine::baseline::{Checkable, Constraint, ConstraintType, Decision};

    fn baseline() -> Baseline {
        Baseline {
            goal: "Refactor auth in strict TypeScript".into(),
            constraints: vec![
                Constraint {
                    id: "c1".into(),
                    text: "TypeScript only, no JS files".into(),
                    kind: ConstraintType::Tech,
                    checkable: Checkable::Deterministic,
                    active: true,
                    rule: None,
                },
                Constraint {
                    id: "c2".into(),
                    text: "No comments in code".into(),
                    kind: ConstraintType::Format,
                    checkable: Checkable::Deterministic,
                    active: false, // retired — must NOT appear
                    rule: None,
                },
            ],
            decisions: vec![
                Decision {
                    text: "use argon2".into(),
                    rejected: false,
                },
                Decision {
                    text: "bcrypt".into(),
                    rejected: true,
                },
            ],
        }
    }

    #[test]
    fn snapshot_contains_goal_active_constraints_and_decisions() {
        let r = reanchor(
            &baseline(),
            State::Red,
            63,
            true,
            Some(&Trigger {
                signal: "constraint".into(),
                detail: "constraint c1 violated: \".js\"".into(),
            }),
        );
        let s = &r.snapshot;
        assert!(s.contains("Refactor auth in strict TypeScript"));
        assert!(s.contains("TypeScript only, no JS files"));
        assert!(
            !s.contains("No comments in code"),
            "inactive constraint excluded"
        );
        assert!(s.contains("Keep: use argon2"));
        assert!(s.contains("Rejected (do not reintroduce): bcrypt"));
        assert!(s.contains("Drifting"));
        assert!(s.contains("63% full (exact)"));
        assert!(s.contains("constraint c1 violated"));
    }

    #[test]
    fn preamble_lists_active_constraints_and_recent_issue() {
        let r = reanchor(
            &baseline(),
            State::Red,
            10,
            false,
            Some(&Trigger {
                signal: "constraint".into(),
                detail: "c1 violated".into(),
            }),
        );
        assert!(r.preamble.contains("TypeScript only, no JS files"));
        assert!(!r.preamble.contains("No comments in code"));
        assert!(r.preamble.contains("Most recent issue: c1 violated"));
    }

    #[test]
    fn handles_empty_baseline_gracefully() {
        let empty = Baseline {
            goal: "".into(),
            constraints: vec![],
            decisions: vec![],
        };
        let r = reanchor(&empty, State::Amber, 0, false, None);
        assert!(r.snapshot.contains("no explicit goal captured"));
        assert!(r.snapshot.contains("_None recorded._"));
        assert!(r.preamble.contains("Stay on goal"));
    }
}
