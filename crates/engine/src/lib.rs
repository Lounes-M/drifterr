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

/// Evaluate every hard signal for one turn of a conversation.
///
/// Returns a [`Verdict`] carrying each signal's event and the instantaneous
/// worst state. Feed the verdict to a [`state_machine::SessionMonitor`] to get
/// the anti-flickered state shown to the user.
pub fn evaluate(conv: &Conversation, baseline: &Baseline) -> Verdict {
    let mut events = Vec::new();

    // Signal 1 — constraint adherence (hard, may be empty when all hold).
    events.extend(constraints::evaluate(baseline, conv.last_assistant()));

    // Signal 4 — saturation (hard, always one event).
    events.push(saturation::evaluate(conv));

    let state = events
        .iter()
        .map(|e| e.state)
        .fold(State::Green, State::worst);

    Verdict { state, events }
}
