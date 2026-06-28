//! Enum ⇄ string conversions for SQLite columns.
//!
//! SQLite has no enum type, so the engine's enums are stored as the same
//! lowercase strings used in their serde representation. Keeping the mapping in
//! one place means a new enum variant fails loudly in exactly one file.

use drifterr_engine::baseline::{Checkable, ConstraintType};
use drifterr_engine::conversation::{Role, Source};
use drifterr_engine::signals::{SignalKind, State};

pub fn role_str(r: Role) -> &'static str {
    match r {
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    }
}

pub fn parse_role(s: &str) -> Role {
    match s {
        "user" => Role::User,
        "tool" => Role::Tool,
        _ => Role::Assistant,
    }
}

pub fn source_str(s: Source) -> &'static str {
    match s {
        Source::Proxy => "proxy",
        Source::File => "file",
        Source::Browser => "browser",
    }
}

pub fn parse_source(s: &str) -> Source {
    match s {
        "file" => Source::File,
        "browser" => Source::Browser,
        _ => Source::Proxy,
    }
}

pub fn constraint_type_str(t: ConstraintType) -> &'static str {
    match t {
        ConstraintType::Tech => "tech",
        ConstraintType::Format => "format",
        ConstraintType::Tone => "tone",
        ConstraintType::Other => "other",
    }
}

pub fn parse_constraint_type(s: &str) -> ConstraintType {
    match s {
        "tech" => ConstraintType::Tech,
        "format" => ConstraintType::Format,
        "tone" => ConstraintType::Tone,
        _ => ConstraintType::Other,
    }
}

pub fn checkable_str(c: Checkable) -> &'static str {
    match c {
        Checkable::Deterministic => "deterministic",
        Checkable::Judge => "judge",
    }
}

pub fn parse_checkable(s: &str) -> Checkable {
    match s {
        "judge" => Checkable::Judge,
        _ => Checkable::Deterministic,
    }
}

pub fn signal_kind_str(k: SignalKind) -> &'static str {
    match k {
        SignalKind::Constraint => "constraint",
        SignalKind::GoalAlignment => "goal_alignment",
        SignalKind::DecisionCoherence => "decision_coherence",
        SignalKind::Saturation => "saturation",
        SignalKind::Degradation => "degradation",
    }
}

pub fn parse_signal_kind(s: &str) -> SignalKind {
    match s {
        "goal_alignment" => SignalKind::GoalAlignment,
        "decision_coherence" => SignalKind::DecisionCoherence,
        "saturation" => SignalKind::Saturation,
        "degradation" => SignalKind::Degradation,
        _ => SignalKind::Constraint,
    }
}

pub fn state_str(s: State) -> &'static str {
    match s {
        State::Green => "green",
        State::Amber => "amber",
        State::Red => "red",
    }
}

pub fn parse_state(s: &str) -> State {
    match s {
        "red" => State::Red,
        "amber" => State::Amber,
        _ => State::Green,
    }
}
