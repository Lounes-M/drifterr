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
    /// Whether saturation is exact (proxy usage) or estimated.
    pub exact: bool,
    /// The single signal the UI should name as the cause, if not green.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub triggering: Option<SignalView>,
    /// Every signal's current view (for the expanded panel).
    pub signals: Vec<SignalView>,
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
}

/// The proxy's shared core: every live session plus an optional durable store.
pub struct AppCore {
    sessions: HashMap<String, SessionState>,
    /// Order of first appearance so "current" is the most recently updated.
    last_updated: Option<String>,
    store: Option<Mutex<Store>>,
}

impl AppCore {
    pub fn new(store: Option<Store>) -> Self {
        Self {
            sessions: HashMap::new(),
            last_updated: None,
            store: store.map(Mutex::new),
        }
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
        let injected = if self.sessions.contains_key(&session_id) {
            Vec::new()
        } else {
            self.promoted_constraints()
        };

        let entry = self.sessions.entry(session_id.clone());
        let session = entry.or_insert_with(|| {
            let mut baseline = Baseline::extract(&conv.turns);
            baseline.constraints.extend(injected);
            SessionState {
                baseline,
                monitor: SessionMonitor::default(),
                status: SessionStatus {
                    session_id: session_id.clone(),
                    model: conv.model.clone(),
                    state: State::Green,
                    saturation_pct: 0,
                    exact: conv.context.exact,
                    triggering: None,
                    signals: Vec::new(),
                    updated_at: 0,
                },
                bumped: std::collections::HashSet::new(),
            }
        });

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

        session.status = SessionStatus {
            session_id: session_id.clone(),
            model: conv.model.clone(),
            state: committed,
            saturation_pct: (conv.saturation_ratio() * 100.0).round() as u32,
            exact: conv.context.exact,
            triggering: verdict.triggering().map(view_of),
            signals: verdict.events.iter().map(view_of).collect(),
            updated_at: now_millis(),
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
