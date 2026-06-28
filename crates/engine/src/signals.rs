//! Signals — the measurable dimensions of drift.
//!
//! There are five signals in the full product. They are *never* fused into a
//! single opaque score: each keeps its own state and its own **evidence** (the
//! turn index, the constraint id, the offending span) so the UI can name the
//! triggering signal rather than show a meaningless "-14%".
//!
//! This module defines the shared vocabulary (state, signal identity, evidence)
//! and the two **hard** signals that ship in M1:
//!
//! * Signal 1 — constraint adherence (deterministic rules)
//! * Signal 4 — context saturation
//!
//! Hard signals are quasi-deterministic and are the only signals allowed to
//! drive a RED state. The soft signals (2 goal alignment, 3 decision
//! coherence, 5 degradation) arrive in later milestones and only ever act as
//! support.

use serde::{Deserialize, Serialize};

/// The traffic-light state of a signal or of the whole session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    /// Aligned.
    Green,
    /// Watch — rising saturation or a single soft signal slipping.
    Amber,
    /// A hard signal fired, or several soft signals converged.
    Red,
}

impl State {
    /// The more severe of two states (`Red` > `Amber` > `Green`).
    pub fn worst(self, other: State) -> State {
        self.max(other)
    }
}

/// Which signal an event came from. Numbered to match the brief so logs,
/// storage, and UI line up across the codebase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalKind {
    /// Signal 1 — constraint adherence.
    Constraint,
    /// Signal 2 — goal alignment (soft, later milestone).
    GoalAlignment,
    /// Signal 3 — decision coherence (later milestone).
    DecisionCoherence,
    /// Signal 4 — context saturation.
    Saturation,
    /// Signal 5 — degradation symptoms (soft, later milestone).
    Degradation,
}

/// The proof attached to every signal event. Without evidence a signal is just
/// an opinion; with it the user can verify the call in one glance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    /// Human-readable explanation, e.g. "constraint c1 violated: found `.js`".
    pub detail: String,
    /// The turn this evidence points at, if applicable.
    #[serde(rename = "turnIndex", skip_serializing_if = "Option::is_none")]
    pub turn_index: Option<usize>,
    /// The constraint this evidence points at, if applicable.
    #[serde(rename = "constraintId", skip_serializing_if = "Option::is_none")]
    pub constraint_id: Option<String>,
    /// The exact offending substring, if the rule isolated one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span: Option<String>,
}

/// One observation from one signal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignalEvent {
    pub signal: SignalKind,
    pub state: State,
    pub evidence: Evidence,
}

impl SignalEvent {
    pub fn new(signal: SignalKind, state: State, evidence: Evidence) -> Self {
        Self {
            signal,
            state,
            evidence,
        }
    }
}

pub mod constraints;
pub mod degradation;
pub mod goal;
pub mod saturation;
