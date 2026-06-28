//! Signal 2 — goal alignment (soft).
//!
//! Tracks whether recent assistant replies are drifting *away* from the goal,
//! by comparing the cosine similarity of each reply to the goal over time. This
//! is a **soft** signal: fuzzy by nature, so it never drives RED — it caps at
//! AMBER and only acts as support.
//!
//! We report on the **trend**, not an absolute score: a session whose replies
//! are simply terse shouldn't alarm; one whose replies were on-topic and are now
//! wandering should. We require enough turns and a real decline before firing,
//! preferring to stay quiet over crying wolf.

use crate::baseline::Baseline;
use crate::conversation::Conversation;
use crate::signals::{Evidence, SignalEvent, SignalKind, State};
use drifterr_embeddings::{cosine, Embedder};

/// Minimum assistant turns before the trend is meaningful.
const MIN_TURNS: usize = 4;
/// Recent similarity must fall this far below the early baseline to count.
const DECLINE: f32 = 0.12;
/// …and land below this floor (still reasonably aligned ⇒ don't alarm).
const FLOOR: f32 = 0.5;

/// Evaluate Signal 2. Returns `Some(AMBER event)` when recent replies have
/// drifted from the goal, else `None`.
pub fn evaluate(
    baseline: &Baseline,
    conv: &Conversation,
    embedder: &dyn Embedder,
) -> Option<SignalEvent> {
    let goal = baseline.goal.trim();
    if goal.is_empty() {
        return None;
    }
    let turns: Vec<&crate::conversation::Turn> = conv.assistant_turns().collect();
    if turns.len() < MIN_TURNS {
        return None;
    }

    let goal_vec = embedder.embed(goal);
    if goal_vec.iter().all(|x| *x == 0.0) {
        return None;
    }

    let sims: Vec<f32> = turns
        .iter()
        .map(|t| cosine(&goal_vec, &embedder.embed(&t.content)))
        .collect();

    // Compare the earlier half against the most recent two replies.
    let half = sims.len() / 2;
    let early_avg = mean(&sims[..half]);
    let recent = &sims[sims.len().saturating_sub(2)..];
    let recent_avg = mean(recent);

    if recent_avg + DECLINE < early_avg && recent_avg < FLOOR {
        let last = turns.last().unwrap();
        Some(SignalEvent::new(
            SignalKind::GoalAlignment,
            State::Amber,
            Evidence {
                detail: format!(
                    "replies drifting from the goal (alignment {:.2} ↓ from {:.2})",
                    recent_avg, early_avg
                ),
                turn_index: Some(last.index),
                constraint_id: None,
                span: None,
            },
        ))
    } else {
        None
    }
}

fn mean(xs: &[f32]) -> f32 {
    if xs.is_empty() {
        return 0.0;
    }
    xs.iter().sum::<f32>() / xs.len() as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::{ContextState, Role, Source, Turn};
    use drifterr_embeddings::BagEmbedder;

    fn conv(goal_turns_on: &[&str], drift_turns: &[&str]) -> Conversation {
        let mut turns = Vec::new();
        let mut push = |s: &str, i: &mut usize| {
            turns.push(Turn {
                index: *i,
                role: Role::Assistant,
                content: s.to_string(),
                tokens: 0,
                timestamp: 0,
            });
            *i += 1;
        };
        let mut i = 0;
        for s in goal_turns_on {
            push(s, &mut i);
        }
        for s in drift_turns {
            push(s, &mut i);
        }
        Conversation {
            session_id: "s".into(),
            model: "m".into(),
            turns,
            context: ContextState {
                window_size: 1000,
                used_tokens: 10,
                exact: true,
                tool_call_count: 0,
            },
            source: Source::Proxy,
        }
    }

    fn baseline(goal: &str) -> Baseline {
        Baseline {
            goal: goal.into(),
            constraints: vec![],
            decisions: vec![],
        }
    }

    #[test]
    fn fires_amber_on_drift() {
        let b = baseline("refactor the authentication module in strict typescript");
        let c = conv(
            &[
                "refactored the authentication module in typescript",
                "updated the typescript auth module types",
            ],
            &[
                "let me tell you about my favorite pizza toppings",
                "pineapple and mushrooms make a great pizza combo",
            ],
        );
        let e = evaluate(&b, &c, &BagEmbedder::default()).expect("should fire");
        assert_eq!(e.state, State::Amber);
        assert_eq!(e.signal, SignalKind::GoalAlignment);
    }

    #[test]
    fn quiet_when_on_topic() {
        let b = baseline("refactor the authentication module in strict typescript");
        let c = conv(
            &[
                "refactored the authentication module in typescript",
                "updated the typescript auth module types",
            ],
            &[
                "added typescript tests for the auth module",
                "the authentication module refactor in typescript is complete",
            ],
        );
        assert!(evaluate(&b, &c, &BagEmbedder::default()).is_none());
    }

    #[test]
    fn quiet_when_too_few_turns() {
        let b = baseline("refactor auth");
        let c = conv(&["off topic entirely about cats"], &[]);
        assert!(evaluate(&b, &c, &BagEmbedder::default()).is_none());
    }

    #[test]
    fn never_red() {
        // Even maximal drift only ever yields AMBER.
        let b = baseline("write a rust web server with axum");
        let c = conv(
            &[
                "here is the axum rust web server code",
                "the rust server handler is ready",
            ],
            &[
                "banana smoothie recipes for summer",
                "how to grow tomatoes in a garden",
            ],
        );
        if let Some(e) = evaluate(&b, &c, &BagEmbedder::default()) {
            assert_eq!(e.state, State::Amber);
        }
    }
}
