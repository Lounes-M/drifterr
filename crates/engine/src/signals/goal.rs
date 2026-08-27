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
use std::sync::OnceLock;

/// Tuning for the goal-alignment trend test.
///
/// # Why these are relative, not absolute
///
/// This test used to require `recent < 0.5` on top of a decline — an **absolute**
/// similarity floor. That was structurally wrong for two reasons, and between them
/// they gave the signal a recall near zero:
///
/// 1. **Absolute cosine scale is a property of the embedder, not of drift.** The
///    lexical bag embedder and the ONNX sentence model put on-topic pairs in
///    completely different ranges, so no single floor can be correct for both. The
///    same session could be "obviously drifting" under one and "fine" under the
///    other purely because of vector geometry.
/// 2. **It made the test conjunctive on an unrelated quantity.** A reply could fall
///    right off the goal — a large, unambiguous decline — and still be ignored
///    because it started from a high base and stayed above 0.5.
///
/// So the test is now purely about *change*: an absolute drop, plus a proportional
/// drop relative to the alignment the session had established. Proportional is the
/// scale-free part, and it's what makes the same thresholds meaningful under either
/// embedder.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GoalThresholds {
    /// Minimum assistant turns before the trend means anything.
    pub min_turns: usize,
    /// How many of the most recent replies form the "now" window.
    pub recent_window: usize,
    /// Absolute decline required (guards against noise on tiny similarities).
    pub min_drop: f32,
    /// Decline as a fraction of the early baseline. The scale-free test.
    pub min_rel_drop: f32,
}

impl Default for GoalThresholds {
    fn default() -> Self {
        Self {
            min_turns: 4,
            recent_window: 2,
            // Both must hold. The absolute floor stops noise firing when the whole
            // session sits near zero similarity; the relative one is the real test.
            min_drop: 0.05,
            min_rel_drop: 0.25,
        }
    }
}

impl GoalThresholds {
    /// Read overrides from the environment, falling back to [`Default`] per field.
    ///
    /// These exist so the thresholds can be **calibrated against a corpus** rather
    /// than argued about — see the eval harness's `--sweep` mode, which walks a grid
    /// of these values and reports precision/recall for each. Until there is a real
    /// annotated corpus to calibrate on, the defaults stay conservative.
    pub fn from_env() -> Self {
        let d = Self::default();
        let num = |key: &str, fallback: f32| -> f32 {
            std::env::var(key)
                .ok()
                .and_then(|v| v.trim().parse::<f32>().ok())
                .filter(|v| v.is_finite())
                .unwrap_or(fallback)
        };
        let count = |key: &str, fallback: usize| -> usize {
            std::env::var(key)
                .ok()
                .and_then(|v| v.trim().parse::<usize>().ok())
                .filter(|v| *v > 0)
                .unwrap_or(fallback)
        };
        Self {
            min_turns: count("DRIFTERR_GOAL_MIN_TURNS", d.min_turns),
            recent_window: count("DRIFTERR_GOAL_RECENT_WINDOW", d.recent_window),
            min_drop: num("DRIFTERR_GOAL_MIN_DROP", d.min_drop),
            min_rel_drop: num("DRIFTERR_GOAL_MIN_REL_DROP", d.min_rel_drop),
        }
    }

    /// Process-wide thresholds, read from the environment once.
    pub fn active() -> &'static GoalThresholds {
        static T: OnceLock<GoalThresholds> = OnceLock::new();
        T.get_or_init(GoalThresholds::from_env)
    }
}

/// Evaluate Signal 2 with the process-wide thresholds. Returns `Some(AMBER event)`
/// when recent replies have drifted from the goal, else `None`.
pub fn evaluate(
    baseline: &Baseline,
    conv: &Conversation,
    embedder: &dyn Embedder,
) -> Option<SignalEvent> {
    evaluate_with(baseline, conv, embedder, GoalThresholds::active())
}

/// Evaluate Signal 2 against explicit thresholds — the entry point the calibration
/// sweep drives.
pub fn evaluate_with(
    baseline: &Baseline,
    conv: &Conversation,
    embedder: &dyn Embedder,
    cfg: &GoalThresholds,
) -> Option<SignalEvent> {
    let goal = baseline.goal.trim();
    if goal.is_empty() {
        return None;
    }
    let turns: Vec<&crate::conversation::Turn> = conv.assistant_turns().collect();
    if turns.len() < cfg.min_turns {
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

    // Compare the earlier half against the most recent replies. `half` is at least
    // 1 because `min_turns` is at least 1 and enforced above.
    let half = (sims.len() / 2).max(1);
    let early_avg = mean(&sims[..half]);
    let window = cfg.recent_window.max(1).min(sims.len());
    let recent_avg = mean(&sims[sims.len() - window..]);

    // A session that never aligned with its goal has nothing to decline *from*;
    // reporting a trend there would be noise, not drift.
    if early_avg <= 0.0 {
        return None;
    }

    let drop = early_avg - recent_avg;
    let rel_drop = drop / early_avg;
    if drop >= cfg.min_drop && rel_drop >= cfg.min_rel_drop {
        let last = turns.last().unwrap();
        Some(SignalEvent::new(
            SignalKind::GoalAlignment,
            State::Amber,
            Evidence {
                detail: format!(
                    "replies drifting from the goal (alignment {recent_avg:.2} ↓ from {early_avg:.2}, −{:.0}%)",
                    rel_drop * 100.0
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
                occupancy_known: true,
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

    /// The regression this guards: the old test required `recent < 0.5` on top of a
    /// decline, so a session that started *well* aligned and fell a long way could
    /// still be ignored purely because it stayed above an arbitrary absolute line.
    /// The test must depend only on the size of the change.
    #[test]
    fn detects_a_large_decline_regardless_of_absolute_level() {
        let b = baseline("refactor the authentication module in strict typescript");
        // Both halves mention the goal's vocabulary, so absolute similarity stays
        // comparatively high throughout — yet the second half is clearly about
        // something else.
        let c = conv(
            &[
                "refactor the authentication module in strict typescript now",
                "the strict typescript authentication module refactor continues",
            ],
            &[
                "typescript is one topic; let us discuss deployment pipelines and kubernetes ingress",
                "kubernetes ingress controllers and helm charts need review before the release train",
            ],
        );
        let e = evaluate(&b, &c, &BagEmbedder::default())
            .expect("a large proportional decline must fire");
        assert_eq!(e.state, State::Amber);
        assert!(
            e.evidence.detail.contains('%'),
            "evidence should quote the proportional drop: {}",
            e.evidence.detail
        );
    }

    /// A session that never aligned with its goal has nothing to decline *from*.
    /// Reporting a trend there is noise, and dividing by a zero baseline would make
    /// the relative test meaningless.
    #[test]
    fn quiet_when_never_aligned() {
        let b = baseline("zzzz qqqq vvvv unrelated vocabulary");
        let c = conv(
            &[
                "completely different words here",
                "and more different words",
            ],
            &[
                "yet more unrelated content",
                "still nothing in common at all",
            ],
        );
        assert!(evaluate(&b, &c, &BagEmbedder::default()).is_none());
    }

    #[test]
    fn thresholds_are_configurable_and_monotonic() {
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
        let e = BagEmbedder::default();

        // Demanding an impossible decline silences it; demanding none fires.
        let strict = GoalThresholds {
            min_rel_drop: 0.99,
            ..GoalThresholds::default()
        };
        assert!(evaluate_with(&b, &c, &e, &strict).is_none());
        let loose = GoalThresholds {
            min_drop: 0.0,
            min_rel_drop: 0.0,
            ..GoalThresholds::default()
        };
        assert!(evaluate_with(&b, &c, &e, &loose).is_some());

        // min_turns still gates: this conversation has 4 assistant turns.
        let needs_more = GoalThresholds {
            min_turns: 5,
            ..GoalThresholds::default()
        };
        assert!(evaluate_with(&b, &c, &e, &needs_more).is_none());
    }

    #[test]
    fn env_overrides_parse_and_reject_garbage() {
        // Not using `active()` here — it caches process-wide, and a test must not
        // depend on which test ran first.
        //
        // But not reading the cache is only half of it: this test *mutates the
        // process environment*, and every other test that calls `evaluate()` goes
        // through `active()`, which reads the environment the first time anyone
        // asks. If that initialization happened to land inside the window below,
        // the whole process would run on `min_turns: 6` and the drift tests would
        // fail — which is exactly what happened under coverage instrumentation,
        // where the changed timing shifted the interleaving.
        //
        // Forcing the cache to resolve *before* touching the environment closes
        // that window: the OnceLock is already set to the defaults, so no
        // interleaving can poison it.
        let _ = GoalThresholds::active();

        let d = GoalThresholds::default();
        // A bad value must fall back rather than panic or zero the threshold.
        unsafe {
            std::env::set_var("DRIFTERR_GOAL_MIN_REL_DROP", "not-a-number");
            std::env::set_var("DRIFTERR_GOAL_MIN_TURNS", "0");
        }
        let t = GoalThresholds::from_env();
        assert_eq!(t.min_rel_drop, d.min_rel_drop);
        assert_eq!(
            t.min_turns, d.min_turns,
            "0 turns is nonsense; keep default"
        );
        unsafe {
            std::env::set_var("DRIFTERR_GOAL_MIN_REL_DROP", "0.4");
            std::env::set_var("DRIFTERR_GOAL_MIN_TURNS", "6");
        }
        let t = GoalThresholds::from_env();
        assert_eq!(t.min_rel_drop, 0.4);
        assert_eq!(t.min_turns, 6);
        unsafe {
            std::env::remove_var("DRIFTERR_GOAL_MIN_REL_DROP");
            std::env::remove_var("DRIFTERR_GOAL_MIN_TURNS");
        }
    }
}
