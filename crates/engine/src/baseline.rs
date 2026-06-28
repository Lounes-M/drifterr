//! The baseline — the user's intention fingerprint.
//!
//! This is Drifterr's ground truth: the `{ goal, constraints, decisions }`
//! the *user themselves* established. We never claim to detect "the model got
//! worse" (perceptual, unverifiable). We detect "this session diverged from
//! what you set" (measurable, verifiable) — and the baseline is the "what you
//! set".
//!
//! In production the baseline is extracted once from the opening turns (or read
//! from a rules file like `CLAUDE.md`). For M0/M1 we treat the baseline as a
//! given input — typically loaded from a fixture alongside the conversation —
//! so the engine can be validated independently of extraction quality.

use serde::{Deserialize, Serialize};

/// What kind of thing a constraint governs. Purely descriptive metadata used
/// by the UI to group violations; it does not affect detection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConstraintType {
    /// A technical rule (language, library, API).
    Tech,
    /// A formatting rule (no comments, max length, structure).
    Format,
    /// A tone / style rule.
    Tone,
    /// Anything else.
    #[serde(other)]
    Other,
}

/// How a constraint can be checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Checkable {
    /// Verifiable with a rule/regex/parse — 0 false positives, 0 cost. These
    /// are "hard" and may trigger a RED state on their own.
    Deterministic,
    /// Fuzzy — requires a judge (a short binary model call). Out of scope for
    /// the M1 hard-signal engine; represented here so the data model is stable.
    Judge,
}

/// A single constraint the user posed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Constraint {
    pub id: String,
    pub text: String,
    #[serde(rename = "type")]
    pub kind: ConstraintType,
    pub checkable: Checkable,
    /// Constraints can be retired mid-session (the user changes their mind).
    /// Defaults to active.
    #[serde(default = "default_true")]
    pub active: bool,
    /// Optional explicit rule binding. When present, this names the
    /// deterministic rule used to check the constraint. When absent for a
    /// deterministic constraint, the engine infers a rule from the text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule: Option<Rule>,
}

fn default_true() -> bool {
    true
}

/// A deterministic, machine-checkable rule.
///
/// Deterministic constraints are the credibility backbone of the product, so
/// their checks must be explicit and false-positive-free. Rather than guessing
/// from free text, a constraint can carry a precise [`Rule`]. The engine also
/// infers these from common phrasings when none is given.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Rule {
    /// Violated if the assistant turn contains this regex.
    ForbidPattern { pattern: String },
    /// Violated if the assistant turn does NOT contain this regex.
    RequirePattern { pattern: String },
    /// Violated if a fenced code block contains this regex (e.g. comment
    /// syntax). Scopes the check to code, ignoring prose.
    ForbidInCode { pattern: String },
    /// Violated if word count exceeds `max`.
    MaxWords { max: usize },
}

/// A decision made during the session. Tracked over time; `rejected` marks
/// ideas the user explicitly discarded so Signal 3 (decision coherence) can
/// later flag their reintroduction. Stored here for schema completeness;
/// Signal 3 itself is a milestone-M3 soft signal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    pub text: String,
    #[serde(default)]
    pub rejected: bool,
}

/// The intention fingerprint extracted at session start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Baseline {
    pub goal: String,
    #[serde(default)]
    pub constraints: Vec<Constraint>,
    #[serde(default)]
    pub decisions: Vec<Decision>,
}

impl Baseline {
    /// Active deterministic constraints — the only ones the M1 engine enforces.
    pub fn deterministic_constraints(&self) -> impl Iterator<Item = &Constraint> {
        self.constraints
            .iter()
            .filter(|c| c.active && c.checkable == Checkable::Deterministic)
    }
}
