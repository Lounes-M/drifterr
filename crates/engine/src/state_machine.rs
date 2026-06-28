//! The session state machine.
//!
//! Per turn the engine produces a set of [`SignalEvent`]s. The state machine
//! turns that stream into the single state the UI shows, applying two rules
//! from the brief:
//!
//! * **Hard signals prime.** A deterministic constraint violation is a *fact*,
//!   so it commits to RED immediately — credibility demands we not sit on a
//!   provable violation.
//! * **Anti-flicker hysteresis.** Threshold-based signals (saturation) wobble
//!   around their boundaries, so a saturation-driven escalation must be
//!   confirmed for several consecutive turns before it commits, and the state
//!   only de-escalates after several consecutive clear turns. This prevents the
//!   widget from strobing.
//!
//! The machine is stateful across turns; create one [`SessionMonitor`] per
//! session and feed it each [`Verdict`] from [`crate::evaluate`].

use crate::signals::{SignalEvent, SignalKind, State};
use serde::{Deserialize, Serialize};

/// Tuning for the hysteresis behavior.
#[derive(Debug, Clone, Copy)]
pub struct Hysteresis {
    /// Consecutive saturation-driven RED observations required before the
    /// committed state escalates to RED. Hard constraint violations ignore this.
    pub red_confirmations: u32,
    /// Consecutive fully-GREEN observations required before the committed state
    /// de-escalates back to GREEN.
    pub clear_confirmations: u32,
}

impl Default for Hysteresis {
    fn default() -> Self {
        // Two confirmations is enough to kill boundary flicker without making
        // the alert feel laggy.
        Self {
            red_confirmations: 2,
            clear_confirmations: 2,
        }
    }
}

/// The result of evaluating one turn, before hysteresis.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verdict {
    /// The instantaneous worst state across all signals this turn.
    pub state: State,
    /// Every signal event observed this turn (including GREEN saturation).
    pub events: Vec<SignalEvent>,
}

impl Verdict {
    /// The single event the UI should name as the cause, if the state is not
    /// GREEN. Prefers a hard signal (constraint) over saturation, and the most
    /// severe state among them.
    pub fn triggering(&self) -> Option<&SignalEvent> {
        self.events
            .iter()
            .filter(|e| e.state != State::Green)
            .max_by_key(|e| (e.state, hard_rank(e.signal)))
    }

    /// Was any hard constraint violated this turn?
    fn has_constraint_violation(&self) -> bool {
        self.events
            .iter()
            .any(|e| e.signal == SignalKind::Constraint && e.state == State::Red)
    }
}

/// Rank used to break ties so hard signals win the "cause" slot.
fn hard_rank(signal: SignalKind) -> u8 {
    match signal {
        SignalKind::Constraint => 2,
        SignalKind::Saturation => 1,
        _ => 0,
    }
}

/// Stateful, anti-flicker session monitor. One per session.
#[derive(Debug, Clone)]
pub struct SessionMonitor {
    cfg: Hysteresis,
    committed: State,
    consecutive_red: u32,
    consecutive_green: u32,
}

impl Default for SessionMonitor {
    fn default() -> Self {
        Self::new(Hysteresis::default())
    }
}

impl SessionMonitor {
    pub fn new(cfg: Hysteresis) -> Self {
        Self {
            cfg,
            committed: State::Green,
            consecutive_red: 0,
            consecutive_green: 0,
        }
    }

    /// The state currently shown to the user.
    pub fn state(&self) -> State {
        self.committed
    }

    /// Feed one turn's verdict; returns the (possibly updated) committed state.
    pub fn observe(&mut self, verdict: &Verdict) -> State {
        // Track run lengths for hysteresis.
        match verdict.state {
            State::Red => {
                self.consecutive_red += 1;
                self.consecutive_green = 0;
            }
            State::Green => {
                self.consecutive_green += 1;
                self.consecutive_red = 0;
            }
            State::Amber => {
                self.consecutive_red = 0;
                self.consecutive_green = 0;
            }
        }

        // Escalation.
        if verdict.has_constraint_violation() {
            // Hard fact: commit immediately, no waiting.
            self.committed = State::Red;
            return self.committed;
        }
        if verdict.state == State::Red {
            // Saturation-driven RED: require confirmation.
            if self.consecutive_red >= self.cfg.red_confirmations {
                self.committed = State::Red;
            } else if self.committed < State::Amber {
                // Show AMBER while we wait for RED confirmation, rather than
                // hiding a building problem.
                self.committed = State::Amber;
            }
            return self.committed;
        }
        if verdict.state == State::Amber {
            // AMBER escalates without delay (it is itself the gentle warning),
            // but never downgrades a committed RED here.
            if self.committed != State::Red {
                self.committed = State::Amber;
            }
            return self.committed;
        }

        // De-escalation: only after enough consecutive clear turns.
        if verdict.state == State::Green
            && self.committed != State::Green
            && self.consecutive_green >= self.cfg.clear_confirmations
        {
            self.committed = State::Green;
        }
        self.committed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signals::{Evidence, SignalEvent};

    fn ev(signal: SignalKind, state: State) -> SignalEvent {
        SignalEvent::new(
            signal,
            state,
            Evidence {
                detail: "x".into(),
                turn_index: None,
                constraint_id: None,
                span: None,
            },
        )
    }

    fn v(events: Vec<SignalEvent>) -> Verdict {
        let state = events
            .iter()
            .map(|e| e.state)
            .fold(State::Green, State::worst);
        Verdict { state, events }
    }

    #[test]
    fn constraint_violation_commits_red_immediately() {
        let mut m = SessionMonitor::default();
        let s = m.observe(&v(vec![ev(SignalKind::Constraint, State::Red)]));
        assert_eq!(s, State::Red);
    }

    #[test]
    fn saturation_red_requires_confirmation() {
        let mut m = SessionMonitor::new(Hysteresis {
            red_confirmations: 2,
            clear_confirmations: 2,
        });
        // First RED: held at AMBER pending confirmation.
        assert_eq!(
            m.observe(&v(vec![ev(SignalKind::Saturation, State::Red)])),
            State::Amber
        );
        // Second consecutive RED: now commits.
        assert_eq!(
            m.observe(&v(vec![ev(SignalKind::Saturation, State::Red)])),
            State::Red
        );
    }

    #[test]
    fn de_escalation_needs_consecutive_greens() {
        let mut m = SessionMonitor::new(Hysteresis {
            red_confirmations: 1,
            clear_confirmations: 2,
        });
        m.observe(&v(vec![ev(SignalKind::Saturation, State::Red)]));
        assert_eq!(m.state(), State::Red);
        // One green: not enough.
        assert_eq!(m.observe(&v(vec![])), State::Red);
        // Second green: clears.
        assert_eq!(m.observe(&v(vec![])), State::Green);
    }

    #[test]
    fn triggering_prefers_constraint_over_saturation() {
        let verdict = v(vec![
            ev(SignalKind::Saturation, State::Red),
            ev(SignalKind::Constraint, State::Red),
        ]);
        assert_eq!(verdict.triggering().unwrap().signal, SignalKind::Constraint);
    }
}
