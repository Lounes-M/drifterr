//! The shapes the control API serializes.
//!
//! Split out of `state.rs` so the API contract is readable on its own. What the
//! panel and the extension receive is a different concern from how it is computed,
//! and these types are the part of the proxy that other people's code depends on
//! staying stable.
//!
//! Each is flattened for a consumer — no `Rule` internals, no engine types — so a
//! change to the engine's representation is not automatically a breaking change to
//! the API.

use drifterr_engine::baseline::Baseline;
use drifterr_engine::signals::State;
use serde::Serialize;

/// A constraint as the intent editor sees it — the same data the engine holds,
/// flattened for the UI (no `Rule` internals).
#[derive(Debug, Clone, Serialize)]
pub struct ConstraintView {
    pub id: String,
    pub text: String,
    /// "tech" | "format" | "tone" | "other".
    pub kind: String,
    /// "deterministic" (can drive RED) | "judge" (fuzzy, AMBER-only).
    pub checkable: String,
    pub active: bool,
    /// True for a rule Drifterr imported from a project rules file rather than one
    /// the user stated. It is checked and flagged, but caps at AMBER until the user
    /// confirms it — so the panel must show it as a proposal with a way to accept.
    #[serde(default)]
    pub proposed: bool,
}

/// A user-reported false positive ("this wasn't drift"), captured locally so it
/// can later be folded into the eval corpus. Never leaves the machine — it's
/// written to a local JSONL file, the same local-first boundary as the SQLite
/// store. Shaped to line up with the eval annotation schema (see eval/SCHEMA.md).
#[derive(Debug, Clone, Serialize)]
pub struct FeedbackSample {
    /// Always "not_drift" for now — the one correction the button reports.
    pub label: &'static str,
    pub session: String,
    pub model: String,
    /// What the engine committed when the user disagreed.
    #[serde(rename = "observedState")]
    pub observed_state: String,
    /// The signal the engine named as the cause, and its evidence.
    #[serde(rename = "triggeringSignal")]
    pub triggering_signal: String,
    pub detail: String,
    #[serde(rename = "constraintId", skip_serializing_if = "Option::is_none")]
    pub constraint_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span: Option<String>,
    /// The baseline the drift was measured against.
    pub goal: String,
    pub constraints: Vec<String>,
    /// An optional free-text note from the user.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// The corrected expectation for annotation: the user says this is aligned.
    #[serde(rename = "correctedState")]
    pub corrected_state: &'static str,
}

/// The user-facing intent for a session: the stated goal plus its constraints.
#[derive(Debug, Clone, Serialize)]
pub struct IntentView {
    pub goal: String,
    pub constraints: Vec<ConstraintView>,
    /// True when this intent isn't attached to a live session yet — it will seed
    /// the next one. Lets the UI say "will apply to your next session".
    pub pending: bool,
}

impl IntentView {
    pub(crate) fn from_baseline(b: &Baseline) -> Self {
        use drifterr_engine::baseline::{Checkable, ConstraintType};
        let kind = |k: ConstraintType| match k {
            ConstraintType::Tech => "tech",
            ConstraintType::Format => "format",
            ConstraintType::Tone => "tone",
            ConstraintType::Other => "other",
        };
        IntentView {
            goal: b.goal.trim().to_string(),
            constraints: b
                .constraints
                .iter()
                .filter(|c| c.active)
                .map(|c| ConstraintView {
                    id: c.id.clone(),
                    text: c.text.clone(),
                    kind: kind(c.kind).into(),
                    checkable: match c.checkable {
                        Checkable::Deterministic => "deterministic",
                        Checkable::Judge => "judge",
                    }
                    .into(),
                    active: c.active,
                    proposed: c.proposed,
                })
                .collect(),
            pending: false,
        }
    }
}

/// A detected goal shift awaiting the user's call: is the new direction a
/// deliberate pivot (adopt it) or drift (keep the old goal, let it read red)?
#[derive(Debug, Clone, Serialize)]
pub struct IntentShift {
    pub from: String,
    pub to: String,
}

/// One detail line per signal, named — never a fused score.
#[derive(Debug, Clone, Serialize)]
pub struct SignalView {
    pub signal: String,
    pub state: State,
    pub detail: String,
    #[serde(rename = "constraintId", skip_serializing_if = "Option::is_none")]
    pub constraint_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span: Option<String>,
}

/// The serializable status the UI renders for a session.
#[derive(Debug, Clone, Serialize)]
pub struct SessionStatus {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    pub model: String,
    pub state: State,
    /// Context occupancy 0–100.
    #[serde(rename = "saturationPct")]
    pub saturation_pct: u32,
    /// 0–100 display aggregate of drift this turn (presentation only — the
    /// separate signals remain the source of truth for `state`/`triggering`).
    #[serde(rename = "driftScore")]
    pub drift_score: u8,
    /// Drift score per recorded turn (oldest→newest), for the session drift map.
    #[serde(default)]
    pub history: Vec<u8>,
    /// Whether saturation is exact (proxy usage) or estimated.
    pub exact: bool,
    /// The single signal the UI should name as the cause, if not green.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub triggering: Option<SignalView>,
    /// Every signal's current view (for the expanded panel).
    pub signals: Vec<SignalView>,
    /// A pending Auto-intent goal shift the user needs to confirm/deny, if any.
    #[serde(rename = "intentShift", skip_serializing_if = "Option::is_none")]
    pub intent_shift: Option<IntentShift>,
    /// The last re-anchor and whether it held — closes the loop on the product's
    /// headline action instead of leaving the user to guess whether it worked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reanchor: Option<ReanchorMark>,
    #[serde(rename = "updatedAt")]
    pub updated_at: i64,
}

/// A re-anchor, plus what happened afterwards.
///
/// # Why this exists
///
/// Re-anchor was the product's headline action and nothing measured whether it did
/// anything. The user clicked, pasted, and then had to *believe* it helped. That's
/// a broken loop in both directions: the user gets no confirmation, and the project
/// gets no evidence — which is exactly why the site had to reach for an invented
/// "52 min saved" figure instead of a real one.
///
/// So each re-anchor records the cause it was meant to fix, and subsequent turns are
/// checked against it. Either the cause stays quiet (the re-anchor held, and we can
/// say for how many turns) or it comes back (it didn't, and we say that too). The
/// honest answer is the useful one: "held for 12 turns" is worth something precisely
/// because "broke again on turn 3" is a possible outcome.
#[derive(Debug, Clone, Serialize)]
pub struct ReanchorMark {
    /// Turn count at the moment of re-anchoring.
    #[serde(rename = "atTurn")]
    pub at_turn: usize,
    /// The signal that was firing then — the thing the re-anchor was meant to fix.
    pub signal: String,
    /// The specific constraint, when the cause was a constraint violation. This is
    /// what makes "did it hold" a precise question rather than a vague one.
    #[serde(rename = "constraintId", skip_serializing_if = "Option::is_none")]
    pub constraint_id: Option<String>,
    /// Assistant turns observed since, with the cause staying quiet.
    #[serde(rename = "heldTurns")]
    pub held_turns: usize,
    /// The turn the same cause reappeared, if it did. `None` = still holding.
    #[serde(rename = "brokeAgainAtTurn", skip_serializing_if = "Option::is_none")]
    pub broke_again_at_turn: Option<usize>,
}

impl ReanchorMark {
    /// Did the re-anchor hold? `None` until enough turns have passed to mean
    /// anything — claiming success on zero subsequent turns would be the same kind
    /// of empty number this whole mechanism exists to replace.
    pub fn held(&self) -> Option<bool> {
        if self.broke_again_at_turn.is_some() {
            return Some(false);
        }
        (self.held_turns >= MIN_TURNS_TO_JUDGE_REANCHOR).then_some(true)
    }
}

/// Assistant turns that must pass before a re-anchor is called successful.
const MIN_TURNS_TO_JUDGE_REANCHOR: usize = 2;
