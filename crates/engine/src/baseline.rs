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

    /// Active judge-checkable (fuzzy) constraints — checked off the response path
    /// by the judge, never the deterministic engine. These only ever raise AMBER.
    pub fn judge_constraints(&self) -> impl Iterator<Item = &Constraint> {
        self.constraints
            .iter()
            .filter(|c| c.active && c.checkable == Checkable::Judge)
    }

    /// Add a fuzzy, judge-checkable constraint discovered by LLM extraction.
    /// De-duplicated case-insensitively against existing constraint texts so a
    /// re-stated rule doesn't pile up. Returns `true` if it was newly added.
    ///
    /// The id follows the same `c{n}` scheme as [`absorb`](Self::absorb), so the
    /// evidence trail is uniform across deterministic and judge constraints.
    pub fn add_judge_constraint(&mut self, text: &str) -> bool {
        let text = text.trim();
        if text.is_empty() {
            return false;
        }
        let lc = text.to_lowercase();
        if self.constraints.iter().any(|c| c.text.to_lowercase() == lc) {
            return false;
        }
        let id = format!("c{}", self.constraints.len() + 1);
        self.constraints.push(Constraint {
            id,
            text: text.to_string(),
            kind: ConstraintType::Other,
            checkable: Checkable::Judge,
            active: true,
            rule: None,
        });
        true
    }

    /// Apply a **user-declared** intent: an explicit goal and a list of
    /// constraint phrases the user typed themselves (onboarding / the intent
    /// editor). This is the highest-authority baseline — unlike [`extract`], the
    /// goal isn't guessed from turn 1, it's stated.
    ///
    /// Each phrase becomes a constraint: a [`Checkable::Deterministic`] one when a
    /// rule can be inferred from it (so it can drive RED, like any hard
    /// constraint), otherwise a [`Checkable::Judge`] fuzzy one (AMBER-only).
    /// De-duplicated against existing constraints so re-declaring is idempotent.
    /// An empty `goal` leaves the existing goal untouched; empty phrases are
    /// skipped. Declared constraints are marked so later [`absorb`](Self::absorb)
    /// passes never retire them.
    pub fn declare(&mut self, goal: &str, constraints: &[String]) {
        let goal = goal.trim();
        if !goal.is_empty() {
            self.goal = goal.to_string();
        }
        for phrase in constraints {
            self.declare_constraint(phrase);
        }
    }

    /// Add one user-declared constraint phrase, choosing deterministic vs judge
    /// by whether a rule can be inferred. Returns `true` if newly added.
    pub fn declare_constraint(&mut self, phrase: &str) -> bool {
        let phrase = phrase.trim();
        if phrase.is_empty() {
            return false;
        }
        let lc = phrase.to_lowercase();
        // Dedup on either the visible text or an equivalent inferred rule.
        let rule = crate::infer::infer_rule(phrase);
        let dup = self
            .constraints
            .iter()
            .any(|c| c.text.to_lowercase() == lc || (rule.is_some() && c.rule == rule));
        if dup {
            return false;
        }
        let (text, kind, checkable) = match &rule {
            Some(r) => {
                let (t, k) = crate::infer::describe(r);
                (t.to_string(), k, Checkable::Deterministic)
            }
            None => (phrase.to_string(), ConstraintType::Other, Checkable::Judge),
        };
        let id = format!("c{}", self.constraints.len() + 1);
        self.constraints.push(Constraint {
            id,
            text,
            kind,
            checkable,
            active: true,
            rule,
        });
        true
    }

    /// Extract a baseline from a conversation's turns without an LLM.
    ///
    /// Used by channels that have no externally-supplied baseline (the proxy):
    /// the goal is the first user message, and constraints are the deterministic
    /// rules inferred from the user's own messages. This keeps M2 detection free
    /// and verifiable; richer judge-based extraction is a later milestone.
    pub fn extract(turns: &[crate::conversation::Turn]) -> Baseline {
        use crate::conversation::Role;

        let goal = turns
            .iter()
            .find(|t| t.role == Role::User)
            .map(|t| t.content.clone())
            .unwrap_or_default();

        let mut baseline = Baseline {
            goal,
            constraints: Vec::new(),
            decisions: Vec::new(),
        };
        baseline.absorb(turns);
        baseline
    }

    /// Merge any newly-stated deterministic constraints from `turns` into this
    /// baseline, preserving existing constraint ids.
    ///
    /// Constraints can be posed mid-session, not just at the start, so the proxy
    /// re-runs this each turn. Rules already present (compared structurally) are
    /// skipped, so ids — and therefore the evidence trail — stay stable.
    pub fn absorb(&mut self, turns: &[crate::conversation::Turn]) {
        use crate::conversation::Role;
        use crate::infer::{describe, infer_rules};

        if self.goal.is_empty() {
            if let Some(t) = turns.iter().find(|t| t.role == Role::User) {
                self.goal = t.content.clone();
            }
        }

        for turn in turns.iter().filter(|t| t.role == Role::User) {
            for rule in infer_rules(&turn.content) {
                if self
                    .constraints
                    .iter()
                    .any(|c| c.rule.as_ref() == Some(&rule))
                {
                    continue;
                }
                let (text, kind) = describe(&rule);
                let id = format!("c{}", self.constraints.len() + 1);
                self.constraints.push(Constraint {
                    id,
                    text: text.to_string(),
                    kind,
                    checkable: Checkable::Deterministic,
                    active: true,
                    rule: Some(rule),
                });
            }

            // Capture explicitly *rejected* decisions ("don't use X", "no X",
            // "avoid X", "pas de X"). These feed Signal 3 (decision coherence):
            // if a later reply reintroduces one, the judge can flag it.
            for text in crate::infer::infer_rejected_decisions(&turn.content) {
                if self.decisions.iter().any(|d| d.text == text) {
                    continue;
                }
                self.decisions.push(Decision {
                    text,
                    rejected: true,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::{Role, Turn};

    fn user(content: &str) -> Turn {
        Turn {
            index: 0,
            role: Role::User,
            content: content.to_string(),
            tokens: 0,
            timestamp: 0,
        }
    }

    #[test]
    fn extract_sets_goal_and_constraints() {
        let turns = vec![user(
            "Refactor auth in strict TypeScript, no JS, no comments",
        )];
        let b = Baseline::extract(&turns);
        assert!(b.goal.starts_with("Refactor auth"));
        assert_eq!(b.constraints.len(), 2);
        assert!(b
            .constraints
            .iter()
            .all(|c| c.checkable == Checkable::Deterministic));
    }

    #[test]
    fn absorb_is_idempotent_and_stable() {
        let turns = vec![user("TypeScript only, no JS")];
        let mut b = Baseline::extract(&turns);
        let first_id = b.constraints[0].id.clone();
        b.absorb(&turns); // same input again
        assert_eq!(b.constraints.len(), 1, "no duplicate constraint");
        assert_eq!(b.constraints[0].id, first_id, "id preserved");
    }

    #[test]
    fn absorb_picks_up_mid_session_constraint() {
        let mut b = Baseline::extract(&[user("TypeScript only, no JS")]);
        assert_eq!(b.constraints.len(), 1);
        b.absorb(&[user("also no comments in code")]);
        assert_eq!(b.constraints.len(), 2);
    }

    #[test]
    fn declare_sets_goal_and_classifies_constraints() {
        let mut b = Baseline::extract(&[user("hi")]);
        b.declare(
            "Ship the billing API",
            &[
                "TypeScript only, no JS".to_string(), // → deterministic (rule)
                "Keep the tone formal".to_string(),   // → judge (fuzzy)
            ],
        );
        assert_eq!(b.goal, "Ship the billing API");
        assert_eq!(b.deterministic_constraints().count(), 1);
        assert_eq!(b.judge_constraints().count(), 1);
        // Idempotent: re-declaring the same phrases adds nothing.
        let n = b.constraints.len();
        b.declare("", &["TypeScript only, no JS".to_string()]);
        assert_eq!(b.constraints.len(), n, "no duplicate on re-declare");
        // Empty goal leaves the declared goal intact.
        assert_eq!(b.goal, "Ship the billing API");
    }

    #[test]
    fn add_judge_constraint_dedups_and_ids() {
        let mut b = Baseline::extract(&[user("TypeScript only, no JS")]);
        assert_eq!(b.constraints.len(), 1);
        assert!(b.add_judge_constraint("Keep the tone formal"));
        assert_eq!(b.constraints.len(), 2);
        assert_eq!(b.constraints[1].id, "c2");
        assert_eq!(b.constraints[1].checkable, Checkable::Judge);
        // Case-insensitive dedup, and empty is ignored.
        assert!(!b.add_judge_constraint("keep the TONE formal"));
        assert!(!b.add_judge_constraint("   "));
        assert_eq!(b.constraints.len(), 2);
        // The new constraint is surfaced by judge_constraints(), not the
        // deterministic iterator.
        assert_eq!(b.judge_constraints().count(), 1);
        assert_eq!(b.deterministic_constraints().count(), 1);
    }
}
