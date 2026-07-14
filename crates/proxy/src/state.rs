//! Live session state and the UI-facing status contract.
//!
//! The proxy keeps one [`SessionState`] per detected session (baseline +
//! anti-flicker monitor + last status) and exposes a snapshot via the control
//! API that the future menubar consumes. Detection mutates this off the
//! response path; the control endpoint only reads it.

use crate::provider::{ParsedRequest, ParsedResponse};
use drifterr_engine::baseline::Baseline;
use drifterr_engine::conversation::{ContextState, Conversation, Role, Source, Turn};
use drifterr_engine::signals::{SignalEvent, State};
use drifterr_engine::state_machine::SessionMonitor;
use drifterr_engine::{evaluate, SignalKind};
use drifterr_store::Store;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Mutex;

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
    fn from_baseline(b: &Baseline) -> Self {
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
                })
                .collect(),
            pending: false,
        }
    }
}

/// Where a session's goal came from — governs whether Auto-intent may overwrite
/// it and when it must instead ask the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GoalSource {
    /// Only the weak "first user message" heuristic so far — freely replaceable.
    Heuristic,
    /// The AI (Auto-intent) inferred it — refined silently, big shifts confirmed.
    Inferred,
    /// The user typed/confirmed it — never overwritten; a big shift is surfaced
    /// as a "did your goal change?" prompt, not a silent change.
    Declared,
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
    #[serde(rename = "updatedAt")]
    pub updated_at: i64,
}

/// Per-session live state, held in memory.
pub struct SessionState {
    pub baseline: Baseline,
    pub monitor: SessionMonitor,
    pub status: SessionStatus,
    /// Constraint texts already counted toward standing orders this session, so
    /// a recurring constraint counts once per session, not once per turn.
    bumped: std::collections::HashSet<String>,
    /// Rolling drift-score history (oldest→newest) for the session drift map.
    history: Vec<u8>,
    /// Provenance of `baseline.goal` — gates Auto-intent overwrites.
    goal_source: GoalSource,
    /// A goal shift Auto-intent detected, awaiting the user's confirm/deny.
    pending_shift: Option<IntentShift>,
    /// Turns observed so far, and the count at the last synthesis attempt — used
    /// to rate-limit Auto-intent's LLM calls.
    turns_seen: usize,
    last_synth_at: usize,
    /// Auto-intent cost control: how many judge synthesis calls this session has
    /// spent (capped by `judge_max_synth_per_session`), and a digest of the
    /// transcript at the last call so an unchanged transcript is never re-sent.
    synth_calls: usize,
    last_synth_hash: u64,
}

/// The proxy's shared core: every live session plus an optional durable store.
pub struct AppCore {
    sessions: HashMap<String, SessionState>,
    /// Order of first appearance so "current" is the most recently updated.
    last_updated: Option<String>,
    store: Option<Mutex<Store>>,
    /// A user-declared intent stated before any session exists yet (onboarding /
    /// the intent editor with no live session). Applied to — and cleared by — the
    /// next new session, so it seeds exactly one session, never leaks a goal
    /// across tasks.
    pending_intent: Option<(String, Vec<String>)>,
}

impl AppCore {
    pub fn new(store: Option<Store>) -> Self {
        Self {
            sessions: HashMap::new(),
            last_updated: None,
            store: store.map(Mutex::new),
            pending_intent: None,
        }
    }

    /// Apply a user-declared intent (explicit goal + constraint phrases) to a
    /// session's baseline. Resolves the target as: `session` if given, else the
    /// most recently updated session. If no session exists yet, the intent is
    /// stashed and seeds the next new session. Persists best-effort. Returns the
    /// resolved intent view so the caller can echo it back to the UI.
    pub fn set_intent(
        &mut self,
        session: Option<&str>,
        goal: &str,
        constraints: &[String],
    ) -> IntentView {
        let target = session
            .map(str::to_string)
            .or_else(|| self.last_updated.clone());

        match target.and_then(|id| self.sessions.get_mut(&id).map(|s| (id, s))) {
            Some((id, session)) => {
                session.baseline.declare(goal, constraints);
                // Typing a goal makes it authoritative and resolves any pending
                // Auto-intent shift prompt.
                if !goal.trim().is_empty() {
                    session.goal_source = GoalSource::Declared;
                    session.pending_shift = None;
                    session.status.intent_shift = None;
                }
                if let Some(store) = &self.store {
                    if let Ok(mut s) = store.lock() {
                        let _ = s.save_baseline(&id, &session.baseline);
                    }
                }
                IntentView::from_baseline(&session.baseline)
            }
            None => {
                // No live session — stash for the next one to inherit.
                self.pending_intent = Some((goal.to_string(), constraints.to_vec()));
                IntentView {
                    goal: goal.trim().to_string(),
                    constraints: Vec::new(),
                    pending: true,
                }
            }
        }
    }

    /// Retire a constraint by id on a session (the user removed it). Persists
    /// best-effort. Returns the updated intent view, or `None` if the session /
    /// constraint wasn't found.
    pub fn retire_constraint(&mut self, session: Option<&str>, id: &str) -> Option<IntentView> {
        let target = session
            .map(str::to_string)
            .or_else(|| self.last_updated.clone())?;
        let session = self.sessions.get_mut(&target)?;
        if !session.baseline.retire(id) {
            return None;
        }
        if let Some(store) = &self.store {
            if let Ok(mut s) = store.lock() {
                let _ = s.save_baseline(&target, &session.baseline);
            }
        }
        Some(IntentView::from_baseline(&session.baseline))
    }

    /// The current declared/inferred intent for a session (or the current one, or
    /// a pending not-yet-applied intent). `None` only when there is nothing at all
    /// to show.
    /// Build a false-positive feedback sample for the current (or given) session:
    /// the user is telling us "this wasn't drift". Captures the fired signal + its
    /// evidence and the baseline it was measured against, framed as the corrected
    /// expectation (green / none). Paired with the user's transcript it becomes an
    /// eval case — the false-positive→corpus loop. Returns `None` if there's no
    /// session or nothing was firing (nothing to correct).
    pub fn feedback_sample(
        &self,
        session: Option<&str>,
        note: Option<String>,
    ) -> Option<FeedbackSample> {
        let target = session
            .map(str::to_string)
            .or_else(|| self.last_updated.clone())?;
        let s = self.sessions.get(&target)?;
        let trig = s.status.triggering.as_ref()?; // nothing firing ⇒ nothing to correct
        let intent = IntentView::from_baseline(&s.baseline);
        Some(FeedbackSample {
            label: "not_drift",
            session: target,
            model: s.status.model.clone(),
            observed_state: format!("{:?}", s.status.state).to_lowercase(),
            triggering_signal: trig.signal.clone(),
            detail: trig.detail.clone(),
            constraint_id: trig.constraint_id.clone(),
            span: trig.span.clone(),
            goal: intent.goal,
            constraints: intent
                .constraints
                .iter()
                .filter(|c| c.active)
                .map(|c| c.text.clone())
                .collect(),
            note,
            corrected_state: "green",
        })
    }

    pub fn intent_of(&self, session: Option<&str>) -> Option<IntentView> {
        let target = session
            .map(str::to_string)
            .or_else(|| self.last_updated.clone());
        if let Some(s) = target.and_then(|id| self.sessions.get(&id)) {
            return Some(IntentView::from_baseline(&s.baseline));
        }
        self.pending_intent.as_ref().map(|(goal, cs)| IntentView {
            goal: goal.trim().to_string(),
            constraints: cs
                .iter()
                .filter(|c| !c.trim().is_empty())
                .map(|c| ConstraintView {
                    id: String::new(),
                    text: c.trim().to_string(),
                    kind: "other".into(),
                    checkable: "judge".into(),
                    active: true,
                })
                .collect(),
            pending: true,
        })
    }

    /// Snapshot of the most recently updated session.
    pub fn current(&self) -> Option<SessionStatus> {
        self.last_updated
            .as_ref()
            .and_then(|id| self.sessions.get(id))
            .map(|s| s.status.clone())
    }

    /// Snapshots of all known sessions.
    pub fn all(&self) -> Vec<SessionStatus> {
        self.sessions.values().map(|s| s.status.clone()).collect()
    }

    /// The decisions recorded for a session (clone), for the judge phase.
    pub fn decisions_for(&self, session_id: &str) -> Vec<drifterr_engine::baseline::Decision> {
        self.sessions
            .get(session_id)
            .map(|s| s.baseline.decisions.clone())
            .unwrap_or_default()
    }

    /// The active judge-checkable (fuzzy) constraints for a session (clone), for
    /// the async judge phase.
    pub fn judge_constraints_for(
        &self,
        session_id: &str,
    ) -> Vec<drifterr_engine::baseline::Constraint> {
        self.sessions
            .get(session_id)
            .map(|s| s.baseline.judge_constraints().cloned().collect())
            .unwrap_or_default()
    }

    /// Merge LLM-extracted fuzzy constraints into a session's baseline (deduped).
    /// Persists the updated baseline best-effort. Returns the texts that were
    /// newly added, so the caller can log/act only on genuinely new rules.
    pub fn add_judge_constraints(&mut self, session_id: &str, texts: Vec<String>) -> Vec<String> {
        let Some(session) = self.sessions.get_mut(session_id) else {
            return Vec::new();
        };
        let mut added = Vec::new();
        for t in texts {
            if session.baseline.add_judge_constraint(&t) {
                added.push(t);
            }
        }
        if !added.is_empty() {
            if let Some(store) = &self.store {
                if let Ok(mut s) = store.lock() {
                    let _ = s.save_baseline(session_id, &session.baseline);
                }
            }
        }
        added
    }

    /// Merge late, out-of-band signal events (e.g. the async judge's Signal 3)
    /// into a session's status. These are AMBER support signals: they append to
    /// the signal list, can lift a GREEN session to AMBER, and never downgrade a
    /// committed state. (They bypass the saturation hysteresis by design — a
    /// judge-confirmed finding is a confirmation, not a flickery threshold.)
    pub fn apply_extra_events(&mut self, session_id: &str, events: Vec<SignalEvent>) {
        let Some(session) = self.sessions.get_mut(session_id) else {
            return;
        };
        if events.is_empty() {
            return;
        }
        let worst_extra = events
            .iter()
            .map(|e| e.state)
            .fold(State::Green, State::worst);

        for e in &events {
            session.status.signals.push(view_of(e));
        }
        // Lift GREEN → (worst soft, capped at AMBER); never lower a stronger state.
        if session.status.state == State::Green && worst_extra != State::Green {
            session.status.state = worst_extra;
            if session.status.triggering.is_none() {
                if let Some(top) = events.iter().max_by_key(|e| e.state) {
                    session.status.triggering = Some(view_of(top));
                }
            }
        }
        session.status.updated_at = now_millis();

        if let Some(store) = &self.store {
            if let Ok(mut s) = store.lock() {
                let _ = s.record_events(session_id, &events);
                let _ = s.set_status(session_id, session.status.state);
            }
        }
        self.last_updated = Some(session_id.to_string());
    }

    /// The re-anchor preamble for a session **only when it is currently RED** —
    /// used by opt-in auto-re-anchor to nudge the next request. Conservative:
    /// AMBER/GREEN sessions get nothing, so we only ever touch a request when a
    /// hard signal is actively firing.
    pub fn auto_preamble(&self, session_id: &str) -> Option<String> {
        let s = self.sessions.get(session_id)?;
        if s.status.state != State::Red {
            return None;
        }
        let trigger = s
            .status
            .triggering
            .as_ref()
            .map(|t| drifterr_intervention::Trigger {
                signal: t.signal.clone(),
                detail: t.detail.clone(),
            });
        Some(
            drifterr_intervention::reanchor(
                &s.baseline,
                s.status.state,
                s.status.saturation_pct,
                s.status.exact,
                trigger.as_ref(),
            )
            .preamble,
        )
    }

    /// Build the re-anchor intervention (snapshot + preamble) for a session.
    /// With no id, uses the most recently updated session. `None` if unknown.
    pub fn reanchor(&self, session_id: Option<&str>) -> Option<drifterr_intervention::Reanchor> {
        let id = match session_id {
            Some(s) => s.to_string(),
            None => self.last_updated.clone()?,
        };
        let session = self.sessions.get(&id)?;
        let trigger = session
            .status
            .triggering
            .as_ref()
            .map(|t| drifterr_intervention::Trigger {
                signal: t.signal.clone(),
                detail: t.detail.clone(),
            });
        Some(drifterr_intervention::reanchor(
            &session.baseline,
            session.status.state,
            session.status.saturation_pct,
            session.status.exact,
            trigger.as_ref(),
        ))
    }

    /// Whether Auto-intent should run an inference for this session now. Cost
    /// controls, cheapest first (no transcript needed):
    /// * cadence — after ≥2 turns, then at most once every 3 turns;
    /// * budget — never more than `judge_max_synth_per_session` calls per session.
    ///
    /// A changed-content check (the cache) is separate — see
    /// [`synth_content_changed`](Self::synth_content_changed) — because it needs
    /// the transcript, which the caller only builds once this returns true.
    pub fn due_for_intent_synthesis(&self, session_id: &str) -> bool {
        let budget = judge_max_synth_per_session();
        self.sessions.get(session_id).is_some_and(|s| {
            s.synth_calls < budget
                && s.turns_seen >= 2
                && (s.last_synth_at == 0 || s.turns_seen - s.last_synth_at >= 3)
        })
    }

    /// The cache guard: has the transcript actually changed since the last
    /// synthesis? Skips a judge call (and its cost) when nothing material moved.
    pub fn synth_content_changed(&self, session_id: &str, transcript_hash: u64) -> bool {
        self.sessions
            .get(session_id)
            .is_some_and(|s| s.last_synth_hash != transcript_hash)
    }

    /// Record that a synthesis was skipped because the transcript was unchanged:
    /// advance the cadence clock (so we don't re-check every turn) WITHOUT
    /// spending budget — no judge call was made.
    pub fn note_synth_skipped(&mut self, session_id: &str) {
        if let Some(s) = self.sessions.get_mut(session_id) {
            s.last_synth_at = s.turns_seen;
        }
    }

    /// Apply an AI-inferred intent to a session (Auto-intent). Constraints are
    /// always merged as fuzzy (AMBER-only) and deduped. The goal is handled by
    /// provenance:
    /// * a big shift from the current goal → surfaced as a pending prompt (never a
    ///   silent overwrite), so a deliberate pivot and drift are told apart by the
    ///   user, not guessed;
    /// * otherwise the goal is refined silently only when it isn't user-declared.
    ///
    /// Records the attempt so the rate limiter advances even when nothing changes,
    /// spends one unit of the session's synthesis budget, and remembers the
    /// transcript digest so an identical transcript is never re-sent.
    pub fn apply_inferred_intent(
        &mut self,
        session_id: &str,
        intent: &drifterr_judge::InferredIntent,
        transcript_hash: u64,
    ) {
        let Some(session) = self.sessions.get_mut(session_id) else {
            return;
        };
        session.last_synth_at = session.turns_seen;
        session.synth_calls += 1;
        session.last_synth_hash = transcript_hash;

        for c in &intent.constraints {
            session.baseline.add_judge_constraint(c);
        }

        let new_goal = intent.goal.trim();
        if !new_goal.is_empty() {
            let cur = session.baseline.goal.trim().to_string();
            match session.goal_source {
                GoalSource::Heuristic => {
                    // First real inference upgrades the weak first-message guess.
                    session.baseline.goal = new_goal.to_string();
                    session.goal_source = GoalSource::Inferred;
                }
                GoalSource::Inferred | GoalSource::Declared => {
                    if !cur.is_empty() && goal_shifted(&cur, new_goal) {
                        // Ask the user: pivot or drift?
                        session.pending_shift = Some(IntentShift {
                            from: cur,
                            to: new_goal.to_string(),
                        });
                        session.status.intent_shift = session.pending_shift.clone();
                    } else if session.goal_source == GoalSource::Inferred {
                        session.baseline.goal = new_goal.to_string();
                    }
                }
            }
        }

        if let Some(store) = &self.store {
            if let Ok(mut s) = store.lock() {
                let _ = s.save_baseline(session_id, &session.baseline);
            }
        }
    }

    /// Resolve a pending Auto-intent goal shift. `accept = true` adopts the new
    /// goal (a deliberate pivot) and marks it user-confirmed; `false` keeps the
    /// old goal (the divergence stands and correctly reads as drift). Returns the
    /// resolved intent view, or `None` if there was no pending shift.
    pub fn resolve_intent_shift(
        &mut self,
        session_id: Option<&str>,
        accept: bool,
    ) -> Option<IntentView> {
        let id = match session_id {
            Some(s) => s.to_string(),
            None => self.last_updated.clone()?,
        };
        let session = self.sessions.get_mut(&id)?;
        let shift = session.pending_shift.take()?;
        session.status.intent_shift = None;
        if accept {
            session.baseline.goal = shift.to;
            session.goal_source = GoalSource::Declared;
            if let Some(store) = &self.store {
                if let Ok(mut s) = store.lock() {
                    let _ = s.save_baseline(&id, &session.baseline);
                }
            }
        }
        Some(IntentView::from_baseline(&session.baseline))
    }

    /// Run detection for one completed turn and update session state.
    ///
    /// This is the whole detection pipeline behind the proxy: recover the
    /// conversation, extract/absorb the baseline, evaluate the hard signals,
    /// pass through the anti-flicker monitor, persist, and store the status the
    /// control API will serve. Returns the new committed state.
    pub fn record_turn(
        &mut self,
        session_id: &str,
        req: &ParsedRequest,
        resp: &ParsedResponse,
    ) -> State {
        self.ingest(build_conversation(session_id, req, resp))
    }

    /// Ingest a fully-formed normalized [`Conversation`] from *any* channel
    /// (proxy or file watcher) and run detection. This is the single point where
    /// the engine meets a channel — proof that channels are interchangeable: the
    /// file watcher and the proxy both end here, no channel-specific branch.
    pub fn record_conversation(&mut self, conv: &Conversation) -> State {
        self.ingest(conv.clone())
    }

    /// The shared detection pipeline: get/create the session (extracting a
    /// baseline on first sight), keep the baseline current, evaluate, run the
    /// anti-flicker monitor, persist, and store the status. Keyed on
    /// `conv.session_id`.
    fn ingest(&mut self, conv: Conversation) -> State {
        let session_id = conv.session_id.clone();

        // On a brand-new session, seed it with the user's promoted standing
        // orders (the moat): rules they've accepted reappear automatically.
        let is_new = !self.sessions.contains_key(&session_id);
        let injected = if is_new {
            self.promoted_constraints()
        } else {
            Vec::new()
        };
        // A user-declared intent stated before this session existed seeds it once.
        let seed_intent = if is_new {
            self.pending_intent.take()
        } else {
            None
        };

        let entry = self.sessions.entry(session_id.clone());
        let session = entry.or_insert_with(|| {
            let mut baseline = Baseline::extract(&conv.turns);
            baseline.constraints.extend(injected);
            // A user-declared seed makes the goal authoritative; otherwise it's
            // only the weak first-message heuristic.
            let goal_source = if let Some((goal, constraints)) = &seed_intent {
                baseline.declare(goal, constraints);
                GoalSource::Declared
            } else {
                GoalSource::Heuristic
            };
            SessionState {
                baseline,
                monitor: SessionMonitor::default(),
                status: SessionStatus {
                    session_id: session_id.clone(),
                    model: conv.model.clone(),
                    state: State::Green,
                    saturation_pct: 0,
                    drift_score: 0,
                    exact: conv.context.exact,
                    triggering: None,
                    signals: Vec::new(),
                    intent_shift: None,
                    updated_at: 0,
                    history: Vec::new(),
                },
                bumped: std::collections::HashSet::new(),
                history: Vec::new(),
                goal_source,
                pending_shift: None,
                turns_seen: 0,
                last_synth_at: 0,
                synth_calls: 0,
                last_synth_hash: 0,
            }
        });

        session.turns_seen += 1;

        // Constraints / rejected decisions may be stated mid-session; keep the
        // baseline current.
        session.baseline.absorb(&conv.turns);

        // Count user-stated constraints toward standing orders — once per session
        // each, and never the injected (`so…`) ones.
        let to_bump: Vec<String> = session
            .baseline
            .constraints
            .iter()
            .filter(|c| c.active && !c.id.starts_with("so") && !session.bumped.contains(&c.text))
            .map(|c| c.text.clone())
            .collect();
        for t in &to_bump {
            session.bumped.insert(t.clone());
        }

        let verdict = evaluate(&conv, &session.baseline);
        let committed = session.monitor.observe(&verdict);

        // Append to the rolling drift-map history (cap to the last 60 turns).
        session.history.push(verdict.drift_score());
        if session.history.len() > 60 {
            let drop = session.history.len() - 60;
            session.history.drain(0..drop);
        }

        session.status = SessionStatus {
            session_id: session_id.clone(),
            model: conv.model.clone(),
            state: committed,
            saturation_pct: (conv.saturation_ratio() * 100.0).round() as u32,
            drift_score: verdict.drift_score(),
            exact: conv.context.exact,
            triggering: verdict.triggering().map(view_of),
            signals: verdict.events.iter().map(view_of).collect(),
            intent_shift: session.pending_shift.clone(),
            updated_at: now_millis(),
            history: session.history.clone(),
        };

        // Durable record (best-effort: persistence must never break ingestion).
        if let Some(store) = &self.store {
            if let Ok(mut s) = store.lock() {
                let _ = s.save_conversation(&conv);
                let _ = s.save_baseline(&session_id, &session.baseline);
                let _ = s.record_events(&session_id, &verdict.events);
                let _ = s.set_status(&session_id, committed);
                // Track recurring constraints across sessions.
                use drifterr_embeddings::{BagEmbedder, Embedder};
                let embedder = BagEmbedder::default();
                for text in &to_bump {
                    let _ = s.bump_standing_order(text, &embedder.embed(text));
                }
            }
        }

        self.last_updated = Some(session_id);
        committed
    }

    /// Promoted standing orders rendered as constraints to seed a new session.
    fn promoted_constraints(&self) -> Vec<drifterr_engine::baseline::Constraint> {
        use drifterr_engine::baseline::{Checkable, Constraint, ConstraintType};
        let Some(store) = &self.store else {
            return Vec::new();
        };
        let Ok(s) = store.lock() else {
            return Vec::new();
        };
        let orders = s.promoted_standing_orders().unwrap_or_default();
        orders
            .into_iter()
            .map(|o| {
                let rule = drifterr_engine::infer::infer_rule(&o.text);
                let (kind, checkable) = match &rule {
                    Some(r) => (
                        drifterr_engine::infer::describe(r).1,
                        Checkable::Deterministic,
                    ),
                    None => (ConstraintType::Other, Checkable::Judge),
                };
                Constraint {
                    id: format!("so{}", o.id),
                    text: o.text,
                    kind,
                    checkable,
                    active: true,
                    rule,
                }
            })
            .collect()
    }

    /// The recent flag events (amber/red) for a session — the activity journal.
    /// Resolves `session` (or the current one) and reads the local store. Empty
    /// without a durable store.
    pub fn journal(&self, session: Option<&str>, limit: usize) -> Vec<drifterr_store::FlagEvent> {
        let id = match session
            .map(str::to_string)
            .or_else(|| self.last_updated.clone())
        {
            Some(id) => id,
            None => return Vec::new(),
        };
        self.store
            .as_ref()
            .and_then(|s| s.lock().ok())
            .and_then(|s| s.recent_flags(&id, limit).ok())
            .unwrap_or_default()
    }

    /// Compact summaries of past sessions for the history/timeline view. Empty
    /// when there's no durable store (in-memory mode).
    pub fn session_history(&self, limit: usize) -> Vec<drifterr_store::SessionSummary> {
        self.store
            .as_ref()
            .and_then(|s| s.lock().ok())
            .and_then(|s| s.list_sessions(limit).ok())
            .unwrap_or_default()
    }

    /// All standing orders (for the control API).
    pub fn standing_orders(&self) -> Vec<drifterr_store::StandingOrder> {
        self.store
            .as_ref()
            .and_then(|s| s.lock().ok())
            .and_then(|s| s.list_standing_orders().ok())
            .unwrap_or_default()
    }

    /// Promote a standing order by id. Returns whether it succeeded.
    pub fn promote_standing_order(&self, id: i64) -> bool {
        self.store
            .as_ref()
            .and_then(|s| s.lock().ok())
            .map(|mut s| s.promote_standing_order(id).is_ok())
            .unwrap_or(false)
    }
}

/// Build a normalized conversation from a browser-extension payload: turns
/// scraped from the page DOM, with **estimated** tokens (no wire payload, so
/// `exact = false`), `source = Browser`.
pub fn browser_conversation(
    session_id: String,
    model: String,
    turns: Vec<(Role, String)>,
) -> Conversation {
    use drifterr_tokenizer::{context_window, HeuristicTokenizer, Tokenizer};
    let tok = HeuristicTokenizer::for_model(&model);
    let mut used = 0usize;
    let built: Vec<Turn> = turns
        .into_iter()
        .enumerate()
        .map(|(index, (role, content))| {
            let tokens = tok.count(&content);
            used += tokens;
            Turn {
                index,
                role,
                content,
                tokens,
                timestamp: now_millis(),
            }
        })
        .collect();
    Conversation {
        context: ContextState {
            window_size: context_window(&model),
            used_tokens: used,
            exact: false,
            tool_call_count: 0,
        },
        session_id,
        model,
        turns: built,
        source: Source::Browser,
    }
}

/// Auto-intent's per-session budget: the maximum number of judge synthesis calls
/// one session may spend. Configurable via `DRIFTERR_JUDGE_MAX_SYNTH_PER_SESSION`
/// (BYOK — the user pays their own provider, so we cap runaway cost). Defaults to
/// 30 (≈ every 3 turns over a ~90-turn session).
pub fn judge_max_synth_per_session() -> usize {
    std::env::var("DRIFTERR_JUDGE_MAX_SYNTH_PER_SESSION")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(30)
}

/// A stable digest of a transcript, used to skip a synthesis call when the
/// content hasn't changed since the last one.
pub fn transcript_digest(transcript: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    transcript.hash(&mut h);
    h.finish()
}

/// Render the most recent turns as a compact transcript for Auto-intent's
/// inference call. Caps the tail (recent context is what matters) and each turn's
/// length so a giant paste can't blow up the prompt.
pub fn transcript_for(req: &ParsedRequest, resp: &ParsedResponse) -> String {
    const MAX_TURNS: usize = 12;
    const MAX_CHARS: usize = 800;
    let clip = |s: &str| {
        let t = s.trim();
        if t.chars().count() > MAX_CHARS {
            t.chars().take(MAX_CHARS).collect::<String>() + "…"
        } else {
            t.to_string()
        }
    };
    let mut lines = Vec::new();
    let start = req.turns.len().saturating_sub(MAX_TURNS);
    for turn in &req.turns[start..] {
        let who = match turn.role {
            Role::User => "User",
            Role::Assistant => "Assistant",
            Role::Tool => "Tool",
        };
        let c = clip(&turn.content);
        if !c.is_empty() {
            lines.push(format!("{who}: {c}"));
        }
    }
    let a = clip(&resp.assistant_text);
    if !a.is_empty() {
        lines.push(format!("Assistant: {a}"));
    }
    lines.join("\n")
}

/// Build the normalized conversation from the request history plus the new
/// assistant turn, using exact provider usage for saturation when available.
fn build_conversation(
    session_id: &str,
    req: &ParsedRequest,
    resp: &ParsedResponse,
) -> Conversation {
    let mut turns = req.turns.clone();
    let index = turns.len();
    let out_tokens = resp.output_tokens.unwrap_or_else(|| {
        use drifterr_tokenizer::{HeuristicTokenizer, Tokenizer};
        HeuristicTokenizer::for_model(&req.model).count(&resp.assistant_text)
    });
    turns.push(Turn {
        index,
        role: Role::Assistant,
        content: resp.assistant_text.clone(),
        tokens: out_tokens,
        timestamp: now_millis(),
    });

    let exact = resp.has_exact_usage();
    let used_tokens = if exact {
        // input_tokens already covers the whole prompt; + output = post-turn
        // occupancy. This is the truest measure of context consumed.
        resp.input_tokens.unwrap() + resp.output_tokens.unwrap()
    } else {
        turns.iter().map(|t| t.tokens).sum()
    };

    Conversation {
        session_id: session_id.to_string(),
        model: req.model.clone(),
        turns,
        context: ContextState {
            window_size: drifterr_tokenizer::context_window(&req.model),
            used_tokens,
            exact,
            tool_call_count: req.tool_call_count,
        },
        source: Source::Proxy,
    }
}

/// Do two goals describe a materially different objective? A deliberately simple,
/// predictable Jaccard over content words (length ≥ 4, so "the"/"and"/"with" are
/// ignored): a refinement of the same goal keeps most words and stays below the
/// bar; a topic change shares almost none and trips it. Cheap and dependency-free
/// — no embeddings, so the pivot-vs-drift prompt is deterministic.
fn goal_shifted(old: &str, new: &str) -> bool {
    fn words(s: &str) -> std::collections::HashSet<String> {
        s.to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.chars().count() >= 4)
            .map(str::to_string)
            .collect()
    }
    let a = words(old);
    let b = words(new);
    if a.is_empty() || b.is_empty() {
        return false; // not enough signal to claim a shift
    }
    let inter = a.intersection(&b).count();
    let union = a.union(&b).count();
    (inter as f32 / union as f32) < 0.18
}

fn view_of(e: &SignalEvent) -> SignalView {
    SignalView {
        signal: signal_name(e.signal).to_string(),
        state: e.state,
        detail: e.evidence.detail.clone(),
        constraint_id: e.evidence.constraint_id.clone(),
        span: e.evidence.span.clone(),
    }
}

fn signal_name(k: SignalKind) -> &'static str {
    match k {
        SignalKind::Constraint => "constraint",
        SignalKind::GoalAlignment => "goal_alignment",
        SignalKind::DecisionCoherence => "decision_coherence",
        SignalKind::Saturation => "saturation",
        SignalKind::Degradation => "degradation",
    }
}

/// Derive a stable session id from the conversation's opening.
///
/// Requests are stateless — each carries the full, growing history — so the one
/// thing constant across a session's requests is its first user message. We
/// hash that (with the model) via FNV-1a, which is deterministic across runs
/// (unlike the std hasher's randomized state).
pub fn session_id_for(req: &ParsedRequest) -> String {
    let anchor = req
        .turns
        .iter()
        .find(|t| t.role == Role::User)
        .map(|t| t.content.as_str())
        .unwrap_or("default");
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in req
        .model
        .bytes()
        .chain(b"\0".iter().copied())
        .chain(anchor.bytes())
    {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("sess-{hash:016x}")
}

fn now_millis() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{parse_request, parse_response, Provider};

    fn req(body: &[u8]) -> ParsedRequest {
        parse_request(Provider::OpenAI, body)
    }

    #[test]
    fn file_channel_feeds_the_same_engine() {
        // A Claude Code session (file channel) that violates a constraint must
        // light up exactly like the proxy channel would — via the same ingest.
        let jsonl = concat!(
            r#"{"type":"user","sessionId":"file-x","message":{"role":"user","content":"Refactor in TS, no JS"}}"#,
            "\n",
            r#"{"type":"assistant","message":{"role":"assistant","model":"claude-opus-4-x","content":[{"type":"text","text":"Sure, creating auth.js now"}]}}"#,
        );
        let conv = drifterr_adapters::claude_code::parse_session(jsonl, "x").unwrap();
        let mut core = AppCore::new(None);
        let state = core.record_conversation(&conv);
        assert_eq!(state, State::Red);
        let status = core.current().unwrap();
        assert!(!status.exact, "file channel saturation is estimated");
        assert_eq!(status.triggering.unwrap().signal, "constraint");
    }

    #[test]
    fn constraint_violation_turns_red_with_exact_saturation() {
        let mut core = AppCore::new(None);
        let r = req(
            br#"{"model":"gpt-4o","messages":[{"role":"user","content":"refactor in TS, no JS"}]}"#,
        );
        let id = session_id_for(&r);
        let resp = ParsedResponse {
            assistant_text: "Sure, let's create auth.js".into(),
            input_tokens: Some(100),
            output_tokens: Some(20),
        };
        let state = core.record_turn(&id, &r, &resp);
        assert_eq!(state, State::Red);

        let status = core.current().unwrap();
        assert!(status.exact);
        // 120 / 128000 → rounds to 0%, but the trigger must be the constraint.
        assert_eq!(status.triggering.as_ref().unwrap().signal, "constraint");
        assert_eq!(
            status.triggering.unwrap().constraint_id.as_deref(),
            Some("c1")
        );
    }

    #[test]
    fn stable_session_id_across_growing_history() {
        let a = req(br#"{"model":"gpt-4o","messages":[{"role":"user","content":"start here"}]}"#);
        let b = req(br#"{"model":"gpt-4o","messages":[
            {"role":"user","content":"start here"},
            {"role":"assistant","content":"ok"},
            {"role":"user","content":"more"}]}"#);
        assert_eq!(session_id_for(&a), session_id_for(&b));
    }

    #[test]
    fn judge_constraints_round_trip_and_dedup() {
        let mut core = AppCore::new(None);
        let r = req(
            br#"{"model":"gpt-4o","messages":[{"role":"user","content":"write me a report"}]}"#,
        );
        let id = session_id_for(&r);
        let resp = ParsedResponse {
            assistant_text: "Here is the report.".into(),
            input_tokens: Some(10),
            output_tokens: Some(5),
        };
        core.record_turn(&id, &r, &resp);

        // No fuzzy constraints yet.
        assert!(core.judge_constraints_for(&id).is_empty());

        // Extraction adds two; a case-insensitive duplicate is dropped.
        let added = core.add_judge_constraints(
            &id,
            vec![
                "Keep the tone formal".into(),
                "keep the TONE formal".into(),
                "No bullet points".into(),
            ],
        );
        assert_eq!(added.len(), 2);
        let cs = core.judge_constraints_for(&id);
        assert_eq!(cs.len(), 2);
        assert!(cs
            .iter()
            .all(|c| c.checkable == drifterr_engine::baseline::Checkable::Judge && c.active));

        // Unknown session ids are inert, not a panic.
        assert!(core.judge_constraints_for("nope").is_empty());
        assert!(core
            .add_judge_constraints("nope", vec!["x".into()])
            .is_empty());
    }

    #[test]
    fn declared_intent_applies_to_current_session() {
        let mut core = AppCore::new(None);
        let r = req(br#"{"model":"gpt-4o","messages":[{"role":"user","content":"help me"}]}"#);
        let id = session_id_for(&r);
        let resp = ParsedResponse {
            assistant_text: "sure".into(),
            input_tokens: Some(10),
            output_tokens: Some(5),
        };
        core.record_turn(&id, &r, &resp);

        let view = core.set_intent(
            None,
            "Ship the billing API",
            &["TypeScript only, no JS".into(), "Keep it formal".into()],
        );
        assert!(!view.pending);
        assert_eq!(view.goal, "Ship the billing API");
        // One deterministic (inferred rule) + one judge (fuzzy).
        assert_eq!(
            view.constraints
                .iter()
                .filter(|c| c.checkable == "deterministic")
                .count(),
            1
        );
        assert_eq!(
            view.constraints
                .iter()
                .filter(|c| c.checkable == "judge")
                .count(),
            1
        );
        // Read-back matches.
        let got = core.intent_of(None).unwrap();
        assert_eq!(got.goal, "Ship the billing API");
    }

    #[test]
    fn pending_intent_seeds_the_next_session() {
        let mut core = AppCore::new(None);
        // Declared with no live session → pending.
        let view = core.set_intent(None, "Refactor auth", &["no JS".into()]);
        assert!(view.pending);
        assert!(core.current().is_none());

        // First turn creates the session, inheriting the declared intent.
        let r = req(br#"{"model":"gpt-4o","messages":[{"role":"user","content":"start"}]}"#);
        let id = session_id_for(&r);
        let resp = ParsedResponse {
            assistant_text: "creating auth.js".into(),
            input_tokens: Some(10),
            output_tokens: Some(5),
        };
        // The declared "no JS" constraint should now be enforced → RED.
        let state = core.record_turn(&id, &r, &resp);
        assert_eq!(state, State::Red);
        let got = core.intent_of(None).unwrap();
        assert_eq!(got.goal, "Refactor auth");
        // Pending was consumed (a second new session doesn't inherit it).
        assert!(core.pending_intent.is_none());
    }

    #[test]
    fn retire_constraint_removes_it_from_intent() {
        let mut core = AppCore::new(None);
        let r = req(
            br#"{"model":"gpt-4o","messages":[{"role":"user","content":"refactor in TS, no JS"}]}"#,
        );
        let id = session_id_for(&r);
        let resp = ParsedResponse {
            assistant_text: "ok".into(),
            input_tokens: Some(10),
            output_tokens: Some(5),
        };
        core.record_turn(&id, &r, &resp);
        let cid = core.intent_of(Some(&id)).unwrap().constraints[0].id.clone();
        let view = core.retire_constraint(Some(&id), &cid).unwrap();
        assert!(
            view.constraints.iter().all(|c| c.id != cid),
            "retired constraint is gone from the view"
        );
        // Unknown id → None.
        assert!(core.retire_constraint(Some(&id), "nope").is_none());
    }

    #[test]
    fn goal_shift_detection() {
        // Refinement of the same objective → not a shift.
        assert!(!goal_shifted(
            "Ship the billing API in strict TypeScript",
            "Ship the billing API"
        ));
        // Different topic → a shift.
        assert!(goal_shifted(
            "Ship the billing API in strict TypeScript",
            "Redesign the marketing landing page"
        ));
        // Empty side → never claim a shift.
        assert!(!goal_shifted("", "anything at all here"));
    }

    fn new_session(core: &mut AppCore, first_user: &str) -> String {
        let body = format!(
            r#"{{"model":"gpt-4o","messages":[{{"role":"user","content":"{first_user}"}}]}}"#
        );
        let r = req(body.as_bytes());
        let id = session_id_for(&r);
        let resp = ParsedResponse {
            assistant_text: "ok".into(),
            input_tokens: Some(10),
            output_tokens: Some(5),
        };
        core.record_turn(&id, &r, &resp);
        id
    }

    #[test]
    fn auto_intent_adopts_then_prompts_on_pivot() {
        use drifterr_judge::InferredIntent;
        let mut core = AppCore::new(None);
        let id = new_session(&mut core, "help me with auth");

        // First inference upgrades the weak heuristic goal silently + adds fuzzy
        // constraints.
        core.apply_inferred_intent(
            &id,
            &InferredIntent {
                goal: "Refactor the auth module to use argon2".into(),
                constraints: vec!["Keep the tone terse".into()],
            },
            1,
        );
        let v = core.intent_of(Some(&id)).unwrap();
        assert_eq!(v.goal, "Refactor the auth module to use argon2");
        assert!(v.constraints.iter().any(|c| c.checkable == "judge"));
        assert!(core.current().unwrap().intent_shift.is_none());

        // A big shift now → prompt, goal unchanged until resolved.
        core.apply_inferred_intent(
            &id,
            &InferredIntent {
                goal: "Build a React analytics dashboard".into(),
                constraints: vec![],
            },
            2,
        );
        let shift = core.current().unwrap().intent_shift.expect("pending shift");
        assert_eq!(shift.to, "Build a React analytics dashboard");
        assert_eq!(
            core.intent_of(Some(&id)).unwrap().goal,
            "Refactor the auth module to use argon2",
            "goal is not silently overwritten on a big shift"
        );

        // Accept → adopt the pivot; the prompt clears.
        let v = core.resolve_intent_shift(Some(&id), true).unwrap();
        assert_eq!(v.goal, "Build a React analytics dashboard");
        assert!(core.current().unwrap().intent_shift.is_none());
    }

    #[test]
    fn auto_intent_reject_keeps_goal() {
        use drifterr_judge::InferredIntent;
        let mut core = AppCore::new(None);
        let id = new_session(&mut core, "write the launch blog post");
        // Declare a goal (authoritative), then a divergent inference → prompt.
        core.set_intent(Some(&id), "Write the launch blog post", &[]);
        core.apply_inferred_intent(
            &id,
            &InferredIntent {
                goal: "Debug the CI pipeline".into(),
                constraints: vec![],
            },
            1,
        );
        assert!(core.current().unwrap().intent_shift.is_some());
        // Reject → keep the declared goal; the divergence stays (reads as drift).
        let v = core.resolve_intent_shift(Some(&id), false).unwrap();
        assert_eq!(v.goal, "Write the launch blog post");
        assert!(core.current().unwrap().intent_shift.is_none());
    }

    #[test]
    fn auto_intent_caches_unchanged_transcript() {
        use drifterr_judge::InferredIntent;
        let mut core = AppCore::new(None);
        let id = new_session(&mut core, "help me with the API");
        // Fresh session: any hash is "changed" (last_synth_hash starts at 0).
        assert!(core.synth_content_changed(&id, 42));
        core.apply_inferred_intent(
            &id,
            &InferredIntent {
                goal: "Ship the API".into(),
                constraints: vec![],
            },
            42,
        );
        // Same transcript ⇒ cached ⇒ no paid re-synthesis.
        assert!(
            !core.synth_content_changed(&id, 42),
            "an unchanged transcript must not trigger another judge call"
        );
        // A changed transcript ⇒ re-synthesize.
        assert!(core.synth_content_changed(&id, 43));
    }

    #[test]
    fn auto_intent_budget_caps_calls() {
        // Budget 0 ⇒ Auto-intent is never due, no matter the cadence.
        std::env::set_var("DRIFTERR_JUDGE_MAX_SYNTH_PER_SESSION", "0");
        let mut core = AppCore::new(None);
        let id = new_session(&mut core, "start the refactor");
        // Advance turns so cadence alone would say "due".
        let r = req(br#"{"model":"gpt-4o","messages":[{"role":"user","content":"more"}]}"#);
        let resp = ParsedResponse {
            assistant_text: "ok".into(),
            input_tokens: Some(10),
            output_tokens: Some(5),
        };
        core.record_turn(&id, &r, &resp);
        core.record_turn(&id, &r, &resp);
        assert!(
            !core.due_for_intent_synthesis(&id),
            "a spent budget must stop synthesis even when the cadence is ready"
        );
        std::env::remove_var("DRIFTERR_JUDGE_MAX_SYNTH_PER_SESSION");
    }

    #[test]
    fn feedback_sample_captures_the_firing_signal() {
        // A session that fires a hard constraint (no-JS violated).
        let jsonl = concat!(
            r#"{"type":"user","sessionId":"fb","message":{"role":"user","content":"Refactor in TS, no JS"}}"#,
            "\n",
            r#"{"type":"assistant","message":{"role":"assistant","model":"claude","content":[{"type":"text","text":"creating auth.js now"}]}}"#,
        );
        let conv = drifterr_adapters::claude_code::parse_session(jsonl, "fb").unwrap();
        let mut core = AppCore::new(None);
        assert_eq!(core.record_conversation(&conv), State::Red);

        let s = core
            .feedback_sample(None, Some("false alarm".into()))
            .expect("a firing session yields a sample");
        assert_eq!(s.label, "not_drift");
        assert_eq!(s.triggering_signal, "constraint");
        assert_eq!(s.observed_state, "red");
        assert_eq!(s.corrected_state, "green");
        assert!(!s.goal.is_empty(), "captures the baseline goal");
        assert_eq!(s.note.as_deref(), Some("false alarm"));

        // No session / nothing firing ⇒ nothing to correct.
        let empty = AppCore::new(None);
        assert!(empty.feedback_sample(None, None).is_none());
    }

    #[test]
    fn estimated_saturation_when_no_usage() {
        let mut core = AppCore::new(None);
        let r = req(br#"{"model":"gpt-4o","messages":[{"role":"user","content":"hi"}]}"#);
        let id = session_id_for(&r);
        let resp = parse_response(
            Provider::OpenAI,
            "application/json",
            b"{\"choices\":[{\"message\":{\"content\":\"hello\"}}]}",
        );
        core.record_turn(&id, &r, &resp);
        assert!(!core.current().unwrap().exact);
    }
}
