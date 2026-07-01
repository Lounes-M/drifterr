//! Signal 1 (fuzzy half) — judge-checked constraint adherence.
//!
//! The deterministic constraint checker in the engine only fires on rules it can
//! verify with 0 false positives (regex/parse), and it drives RED. But most real
//! constraints a user states ("keep it concise", "no external libraries", "match
//! the existing style") are *fuzzy* — no regex captures them. Those are marked
//! [`Checkable::Judge`](drifterr_engine::baseline::Checkable) and handled here.
//!
//! One **batched** judge call checks every active judge-constraint against the
//! latest assistant turn at once. A flagged constraint emits **AMBER** — never
//! RED: a fuzzy call must under-claim, so a judge that cries wolf can only ever
//! raise a watch, not a wall. Fail-safe: a disabled judge or any error yields no
//! events.

use crate::Judge;
use drifterr_engine::baseline::{Checkable, Constraint};
use drifterr_engine::conversation::Turn;
use drifterr_engine::signals::{Evidence, SignalEvent, SignalKind, State};

/// Evaluate the fuzzy half of Signal 1 for the latest assistant turn.
///
/// Returns one AMBER event per active judge-checkable constraint the judge finds
/// violated. Empty when the judge is disabled, there are no such constraints, or
/// none look violated.
pub async fn constraint_adherence(
    last_assistant: &Turn,
    constraints: &[Constraint],
    judge: &Judge,
) -> Vec<SignalEvent> {
    if !judge.enabled() {
        return Vec::new();
    }

    // Only active, judge-checkable constraints — the deterministic ones are the
    // engine's job (and drive RED there).
    let judged: Vec<&Constraint> = constraints
        .iter()
        .filter(|c| c.active && c.checkable == Checkable::Judge)
        .collect();
    if judged.is_empty() {
        return Vec::new();
    }

    let texts: Vec<String> = judged.iter().map(|c| c.text.clone()).collect();
    let verdicts = judge.violations(&texts, &last_assistant.content).await;

    judged
        .iter()
        .zip(verdicts)
        .filter(|(_, violated)| *violated)
        .map(|(c, _)| {
            SignalEvent::new(
                SignalKind::Constraint,
                State::Amber,
                Evidence {
                    detail: format!("constraint {} looks violated: \"{}\"", c.id, c.text),
                    turn_index: Some(last_assistant.index),
                    constraint_id: Some(c.id.clone()),
                    span: None,
                },
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StubJudge;
    use drifterr_engine::baseline::ConstraintType;
    use drifterr_engine::conversation::Role;

    fn turn(content: &str) -> Turn {
        Turn {
            index: 5,
            role: Role::Assistant,
            content: content.to_string(),
            tokens: 0,
            timestamp: 0,
        }
    }

    fn judged(id: &str, text: &str) -> Constraint {
        Constraint {
            id: id.to_string(),
            text: text.to_string(),
            kind: ConstraintType::Other,
            checkable: Checkable::Judge,
            active: true,
            rule: None,
        }
    }

    #[tokio::test]
    async fn fires_amber_for_violated_judge_constraint() {
        let cs = vec![judged("c1", "Keep the tone formal")];
        let judge = Judge::Stub(StubJudge::new(&["lol"]));
        let events = constraint_adherence(&turn("lol sure thing dude"), &cs, &judge).await;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].state, State::Amber);
        assert_eq!(events[0].signal, SignalKind::Constraint);
        assert_eq!(events[0].evidence.constraint_id.as_deref(), Some("c1"));
    }

    #[tokio::test]
    async fn quiet_when_not_violated() {
        let cs = vec![judged("c1", "Keep the tone formal")];
        let judge = Judge::Stub(StubJudge::new(&["lol"]));
        let events =
            constraint_adherence(&turn("Certainly, here is the report."), &cs, &judge).await;
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn skips_deterministic_inactive_and_disabled() {
        let cs = vec![
            Constraint {
                checkable: Checkable::Deterministic,
                ..judged("c1", "no .js")
            },
            Constraint {
                active: false,
                ..judged("c2", "be concise")
            },
        ];
        let judge = Judge::Stub(StubJudge::new(&["anything"]));
        assert!(
            constraint_adherence(&turn("app.js and rambling"), &cs, &judge)
                .await
                .is_empty()
        );
        // Disabled judge → nothing even for a valid judge constraint.
        let cs2 = vec![judged("c3", "be concise")];
        assert!(
            constraint_adherence(&turn("whatever"), &cs2, &Judge::Disabled)
                .await
                .is_empty()
        );
    }
}
