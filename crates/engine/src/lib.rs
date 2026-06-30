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
//! #   context: ContextState { window_size: 1000, used_tokens: 100, exact: true, tool_call_count: 0 },
//! #   source: Source::Proxy,
//! # };
//! let mut monitor = SessionMonitor::default();
//! let verdict = evaluate(&conv, &baseline);   // per-turn signals
//! let state = monitor.observe(&verdict);       // anti-flicker committed state
//! ```

pub mod baseline;
pub mod conversation;
pub mod infer;
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
}
