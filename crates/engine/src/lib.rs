//! # Drifterr engine
//!
//! The channel-agnostic detection core. It only ever sees the normalized
//! [`Conversation`] format and a [`Baseline`] (the user's intention
//! fingerprint), and emits [`SignalEvent`]s plus a committed session [`State`].
//!
//! This crate is the product's go/no-go: if drift detection is not convincing
//! here — validated on fixtures, with no real channel attached — nothing else
//! matters. It ships the two **hard** signals (constraint adherence and
//! saturation); the soft signals and the judge path arrive in later milestones
//! behind the same `SignalEvent` interface.
//!
//! ## Usage
//!
//! ```
//! use drifterr_engine::{evaluate, state_machine::SessionMonitor};
//! # use drifterr_engine::{baseline::Baseline, conversation::*};
//! # let baseline = Baseline { goal: "g".into(), constraints: vec![], decisions: vec![] };
//! # let conv = Conversation {
//! #   session_id: "s".into(), model: "claude-opus-4-x".into(), turns: vec![],
//! #   context: ContextState { window_size: 1000, used_tokens: 100, exact: true,
//! #                           occupancy_known: true, tool_call_count: 0 },
//! #   source: Source::Proxy,
//! # };
//! let mut monitor = SessionMonitor::default();
//! let verdict = evaluate(&conv, &baseline);   // per-turn signals
//! let state = monitor.observe(&verdict);       // anti-flicker committed state
//! ```

pub mod baseline;
pub mod conversation;
pub mod infer;
pub mod rules_file;
pub mod signals;
pub mod state_machine;

use baseline::Baseline;
use conversation::Conversation;
use signals::{constraints, saturation, State};
use state_machine::Verdict;

pub use signals::{SignalEvent, SignalKind};

/// Evaluate every signal for one turn of a conversation.
///
/// Returns a [`Verdict`] carrying each signal's event and the instantaneous
/// worst state. Feed the verdict to a [`state_machine::SessionMonitor`] to get
/// the anti-flickered state shown to the user.
///
/// **Hard vs soft.** Hard signals (constraints, saturation) may reach RED. Soft
/// signals (goal alignment, degradation) only ever emit AMBER, so a soft signal
/// can never drive RED on its own — the verdict state is the worst across all
/// events, and soft maxes out at AMBER by construction.
pub fn evaluate(conv: &Conversation, baseline: &Baseline) -> Verdict {
    let mut events = Vec::new();

    // Signal 1 — constraint adherence (hard, may be empty when all hold).
    events.extend(constraints::evaluate(baseline, conv.last_assistant()));

    // Signal 4 — saturation (hard, always one event).
    events.push(saturation::evaluate(conv));

    // Soft signals (support only; AMBER ceiling). The embedder comes from the
    // factory — a local, deterministic hybrid-lexical model by default; a real
    // ONNX model can be slotted in there without touching this code.
    let embedder = drifterr_embeddings::default_embedder();
    if let Some(e) = signals::goal::evaluate(baseline, conv, embedder.as_ref()) {
        events.push(e);
    }
    if let Some(e) = signals::degradation::evaluate(conv, embedder.as_ref()) {
        events.push(e);
    }

    let state = events
        .iter()
        .map(|e| e.state)
        .fold(State::Green, State::worst);

    Verdict { state, events }
}

#[cfg(test)]
mod tests {
    use super::*;
    use baseline::Baseline;
    use conversation::{ContextState, Conversation, Role, Source, Turn};

    fn assistant(i: usize, content: &str) -> Turn {
        Turn {
            index: i,
            role: Role::Assistant,
            content: content.to_string(),
            tokens: 0,
            timestamp: 0,
        }
    }

    /// The guardrail: soft signals — even several at once, with no hard signal —
    /// must never push the verdict to RED. They cap at AMBER.
    #[test]
    fn soft_signals_never_drive_red() {
        let baseline = Baseline {
            goal: "build a rust web server with axum".into(),
            constraints: vec![],
            decisions: vec![],
        };
        // On-topic, then identical off-topic replies: trips BOTH goal-drift and
        // the looping degradation symptom at once.
        let conv = Conversation {
            session_id: "s".into(),
            model: "claude-opus-4-x".into(),
            turns: vec![
                assistant(0, "here is the axum rust web server code"),
                assistant(1, "the rust server request handler is ready"),
                assistant(2, "anyway here are some banana smoothie recipes for summer"),
                assistant(3, "anyway here are some banana smoothie recipes for summer"),
            ],
            // Low occupancy ⇒ saturation GREEN (no hard signal in play).
            context: ContextState {
                window_size: 200_000,
                used_tokens: 5_000,
                exact: true,
                occupancy_known: true,
                tool_call_count: 0,
            },
            source: Source::Proxy,
        };

        let verdict = evaluate(&conv, &baseline);
        let soft = verdict
            .events
            .iter()
            .filter(|e| {
                matches!(
                    e.signal,
                    SignalKind::GoalAlignment
                        | SignalKind::DecisionCoherence
                        | SignalKind::Degradation
                ) && e.state != State::Green
            })
            .count();
        assert!(soft >= 1, "expected at least one soft signal to fire");
        assert_ne!(verdict.state, State::Red, "soft signals must not reach RED");
        assert_eq!(verdict.state, State::Amber);
    }

    /// A tiny deterministic PRNG (SplitMix64) so the property test is reproducible
    /// across runs — `rand` isn't a dependency and randomness must be seedable.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }
        fn pick<T: Copy>(&mut self, xs: &[T]) -> T {
            xs[(self.next() % xs.len() as u64) as usize]
        }
    }

    /// Property (fuzzed): across many random conversations and baselines, NO soft
    /// signal (goal alignment, degradation, decision coherence) ever emits RED —
    /// the hard/soft rule holds by construction, not just on the hand-picked case
    /// above. A single counterexample here would mean a soft signal could drive a
    /// red alert, the one thing the brief forbids.
    #[test]
    fn soft_signals_never_emit_red_property() {
        // Varied content pools: on-topic, off-topic, looping, hedging, code.
        let phrases = [
            "here is the axum rust web server code",
            "anyway here are some banana smoothie recipes",
            "i'm not sure, it depends, maybe, perhaps this could work",
            "the handler is ready and wired to the router",
            "let me refactor the auth module in typescript",
            "peut-être que ça dépend, je ne suis pas sûr",
            "creating auth.js with console.log for now",
            "the same identical sentence repeated over and over",
            "a short reply",
            "a very long rambling reply that keeps going and going and restates the same point many times without adding anything new to the discussion at hand really",
        ];
        let goals = [
            "build a rust web server with axum",
            "write the billing API in strict typescript",
            "summarize the incident in plain english",
            "",
        ];

        let mut rng = Rng(0xD1CE_F00D_1234_5678);
        for _ in 0..3000 {
            let goal = rng.pick(&goals).to_string();
            let n = 1 + (rng.next() % 6) as usize; // 1..=6 turns
            let turns: Vec<_> = (0..n).map(|i| assistant(i, rng.pick(&phrases))).collect();
            let used = (rng.next() % 210_000) as usize; // spans low → over-full
            let conv = Conversation {
                session_id: "prop".into(),
                model: "m".into(),
                turns,
                context: ContextState {
                    window_size: 200_000,
                    used_tokens: used,
                    exact: true,
                    occupancy_known: true,
                    tool_call_count: 0,
                },
                source: Source::Proxy,
            };
            let baseline = Baseline {
                goal,
                constraints: vec![],
                decisions: vec![],
            };
            let verdict = evaluate(&conv, &baseline);
            for e in &verdict.events {
                let is_soft = matches!(
                    e.signal,
                    SignalKind::GoalAlignment
                        | SignalKind::DecisionCoherence
                        | SignalKind::Degradation
                );
                assert!(
                    !(is_soft && e.state == State::Red),
                    "soft signal {:?} emitted RED — hard/soft invariant broken",
                    e.signal
                );
            }
            // And the corollary: any RED must be owned by a hard signal.
            if verdict.state == State::Red {
                assert!(
                    verdict.events.iter().any(|e| matches!(
                        e.signal,
                        SignalKind::Constraint | SignalKind::Saturation
                    ) && e.state == State::Red),
                    "verdict is RED with no hard signal RED"
                );
            }
        }
    }
}
