//! Drifterr local store — the SQLite persistence layer.
//!
//! Local-first by design: conversations live in a local SQLite file and are
//! never shipped to a Drifterr server. This crate persists and reloads the
//! normalized [`Conversation`], its [`Baseline`], and the [`SignalEvent`]s the
//! engine produces.
//!
//! M0 acceptance lives here: load a `Conversation` from a fixture and
//! persist/reload it without loss (see the round-trip test).

use drifterr_engine::baseline::{Baseline, Constraint, Decision, Rule};
use drifterr_engine::conversation::{ContextState, Conversation, Turn};
use drifterr_engine::signals::{Evidence, SignalEvent, State};
use rusqlite::{params, Connection};

mod convert;
use convert::*;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("data error: {0}")]
    Data(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// A handle to the local SQLite database.
pub struct Store {
    conn: Connection,
}

impl Store {
    /// Open (and migrate) a store at `path`. Creates the file if absent.
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    /// Open an ephemeral in-memory store — used by tests.
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::init(conn)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        conn.execute_batch(include_str!("schema.sql"))?;
        Ok(Self { conn })
    }

    // --- install metadata --------------------------------------------------

    /// Read an install-scoped metadata value. `None` when unset.
    pub fn meta(&self, key: &str) -> Result<Option<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT value FROM app_meta WHERE key = ?1")?;
        let mut rows = stmt.query(params![key])?;
        match rows.next()? {
            Some(r) => Ok(Some(r.get(0)?)),
            None => Ok(None),
        }
    }

    /// Write an install-scoped metadata value, replacing any prior one.
    pub fn set_meta(&mut self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO app_meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// Read a metadata value, writing `default` first if the key is unset.
    /// Returns the value now in force — so the *first* caller establishes it.
    ///
    /// This is how the local Pro trial gets its start timestamp: the first launch
    /// stamps it, every later launch reads the same one back, and nothing about it
    /// needs a network or an account.
    pub fn meta_or_init(&mut self, key: &str, default: &str) -> Result<String> {
        if let Some(v) = self.meta(key)? {
            return Ok(v);
        }
        self.set_meta(key, default)?;
        Ok(default.to_string())
    }

    // --- re-anchor outcomes -------------------------------------------------

    /// Record that a re-anchor happened, with the cause it was meant to fix.
    /// Returns the row id so the outcome can be filled in once it is known.
    pub fn record_reanchor(
        &mut self,
        session_id: &str,
        signal: &str,
        constraint_id: Option<&str>,
        ts: i64,
    ) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO reanchors (session_id, signal, constraint_id, ts, held)
             VALUES (?1, ?2, ?3, ?4, NULL)",
            params![session_id, signal, constraint_id, ts],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Set the outcome of the most recent re-anchor for a session.
    ///
    /// Only ever writes when the verdict is actually known — "too early to say" stays
    /// NULL rather than being rounded to success, because a re-anchor success rate
    /// that counts undecided cases as wins is the same kind of invented number this
    /// mechanism was built to replace.
    pub fn set_reanchor_outcome(&mut self, session_id: &str, held: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE reanchors SET held = ?2
             WHERE id = (SELECT id FROM reanchors WHERE session_id = ?1
                         ORDER BY ts DESC, id DESC LIMIT 1)",
            params![session_id, if held { 1 } else { 0 }],
        )?;
        Ok(())
    }

    /// Re-anchor tallies since `since_ms`: (total, held, broke). `total` counts every
    /// re-anchor; `held + broke` counts only the decided ones, so the undecided
    /// remainder is visible rather than hidden.
    pub fn reanchor_stats(&self, since_ms: i64) -> Result<(usize, usize, usize)> {
        let mut stmt = self.conn.prepare(
            "SELECT COUNT(*),
                    COALESCE(SUM(CASE WHEN held = 1 THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN held = 0 THEN 1 ELSE 0 END), 0)
             FROM reanchors WHERE ts >= ?1",
        )?;
        let row = stmt.query_row(params![since_ms], |r| {
            Ok((
                r.get::<_, i64>(0)? as usize,
                r.get::<_, i64>(1)? as usize,
                r.get::<_, i64>(2)? as usize,
            ))
        })?;
        Ok(row)
    }

    /// Persist a conversation (session + turns + context). Idempotent on
    /// session id: re-saving replaces the prior rows for that session.
    pub fn save_conversation(&mut self, conv: &Conversation) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO sessions (id, model, source, status) VALUES (?1, ?2, ?3, NULL)
             ON CONFLICT(id) DO UPDATE SET model = excluded.model, source = excluded.source",
            params![conv.session_id, conv.model, source_str(conv.source)],
        )?;

        tx.execute(
            "DELETE FROM turns WHERE session_id = ?1",
            params![conv.session_id],
        )?;
        for t in &conv.turns {
            tx.execute(
                "INSERT INTO turns (session_id, idx, role, content, tokens, ts)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    conv.session_id,
                    t.index as i64,
                    role_str(t.role),
                    t.content,
                    t.tokens as i64,
                    t.timestamp
                ],
            )?;
        }

        tx.execute(
            "INSERT INTO context_state (session_id, window_size, used_tokens, exact, tool_calls)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(session_id) DO UPDATE SET
               window_size = excluded.window_size, used_tokens = excluded.used_tokens,
               exact = excluded.exact, tool_calls = excluded.tool_calls",
            params![
                conv.session_id,
                conv.context.window_size as i64,
                conv.context.used_tokens as i64,
                conv.context.exact as i64,
                conv.context.tool_call_count as i64
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Reload a previously saved conversation by session id.
    pub fn load_conversation(&self, session_id: &str) -> Result<Conversation> {
        let (model, source): (String, String) = self
            .conn
            .query_row(
                "SELECT model, source FROM sessions WHERE id = ?1",
                params![session_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(|_| StoreError::Data(format!("no session {session_id}")))?;

        let mut stmt = self.conn.prepare(
            "SELECT idx, role, content, tokens, ts FROM turns WHERE session_id = ?1 ORDER BY idx",
        )?;
        let turns = stmt
            .query_map(params![session_id], |r| {
                Ok(Turn {
                    index: r.get::<_, i64>(0)? as usize,
                    role: parse_role(&r.get::<_, String>(1)?),
                    content: r.get(2)?,
                    tokens: r.get::<_, i64>(3)? as usize,
                    timestamp: r.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let context = self.conn.query_row(
            "SELECT window_size, used_tokens, exact, tool_calls FROM context_state WHERE session_id = ?1",
            params![session_id],
            |r| {
                Ok(ContextState {
                    window_size: r.get::<_, i64>(0)? as usize,
                    used_tokens: r.get::<_, i64>(1)? as usize,
                    exact: r.get::<_, i64>(2)? != 0,
                    tool_call_count: r.get::<_, i64>(3)? as usize,
                })
            },
        )?;

        Ok(Conversation {
            session_id: session_id.to_string(),
            model,
            turns,
            context,
            source: parse_source(&source),
        })
    }

    /// Persist a baseline (goal on the session row, plus constraints &
    /// decisions). Replaces any prior baseline rows for the session.
    pub fn save_baseline(&mut self, session_id: &str, baseline: &Baseline) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "UPDATE sessions SET goal = ?1 WHERE id = ?2",
            params![baseline.goal, session_id],
        )?;
        tx.execute(
            "DELETE FROM constraints WHERE session_id = ?1",
            params![session_id],
        )?;
        for c in &baseline.constraints {
            let rule_json = match &c.rule {
                Some(r) => Some(serde_json::to_string(r)?),
                None => None,
            };
            tx.execute(
                "INSERT INTO constraints (id, session_id, text, type, checkable, active, rule_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    c.id,
                    session_id,
                    c.text,
                    constraint_type_str(c.kind),
                    checkable_str(c.checkable),
                    c.active as i64,
                    rule_json
                ],
            )?;
        }
        tx.execute(
            "DELETE FROM decisions WHERE session_id = ?1",
            params![session_id],
        )?;
        for d in &baseline.decisions {
            tx.execute(
                "INSERT INTO decisions (session_id, text, rejected) VALUES (?1, ?2, ?3)",
                params![session_id, d.text, d.rejected as i64],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Reload a baseline for a session.
    pub fn load_baseline(&self, session_id: &str) -> Result<Baseline> {
        let goal: String = self
            .conn
            .query_row(
                "SELECT COALESCE(goal, '') FROM sessions WHERE id = ?1",
                params![session_id],
                |r| r.get(0),
            )
            .map_err(|_| StoreError::Data(format!("no session {session_id}")))?;

        let mut cstmt = self.conn.prepare(
            "SELECT id, text, type, checkable, active, rule_json FROM constraints WHERE session_id = ?1",
        )?;
        let constraints = cstmt
            .query_map(params![session_id], |r| {
                let rule_json: Option<String> = r.get(5)?;
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, i64>(4)? != 0,
                    rule_json,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .map(|(id, text, ty, ck, active, rule_json)| {
                let rule: Option<Rule> = match rule_json {
                    Some(j) => Some(serde_json::from_str(&j)?),
                    None => None,
                };
                Ok(Constraint {
                    id,
                    text,
                    kind: parse_constraint_type(&ty),
                    checkable: parse_checkable(&ck),
                    active,
                    rule,
                })
            })
            .collect::<Result<Vec<_>>>()?;

        let mut dstmt = self
            .conn
            .prepare("SELECT text, rejected FROM decisions WHERE session_id = ?1 ORDER BY id")?;
        let decisions = dstmt
            .query_map(params![session_id], |r| {
                Ok(Decision {
                    text: r.get(0)?,
                    rejected: r.get::<_, i64>(1)? != 0,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(Baseline {
            goal,
            constraints,
            decisions,
        })
    }

    /// Append signal events for a session (the audit trail behind every alert).
    pub fn record_events(&mut self, session_id: &str, events: &[SignalEvent]) -> Result<()> {
        let tx = self.conn.transaction()?;
        for e in events {
            tx.execute(
                "INSERT INTO signal_events (session_id, signal, state, evidence, turn_idx, ts)
                 VALUES (?1, ?2, ?3, ?4, ?5, NULL)",
                params![
                    session_id,
                    signal_kind_str(e.signal),
                    state_str(e.state),
                    serde_json::to_string(&e.evidence)?,
                    e.evidence.turn_index.map(|i| i as i64)
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// The recent *flag* events for a session (state ≠ green) — the readable
    /// "what fired and when" journal, newest first. `ts` isn't recorded, so the
    /// autoincrement id gives chronological order and `turn_idx` gives the turn.
    pub fn recent_flags(&self, session_id: &str, limit: usize) -> Result<Vec<FlagEvent>> {
        let mut stmt = self.conn.prepare(
            "SELECT signal, state, evidence, turn_idx FROM signal_events
             WHERE session_id = ?1 AND state != 'green'
             ORDER BY id DESC LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(params![session_id, limit as i64], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<i64>>(3)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|(signal, state, ev, turn_idx)| {
                let evidence: Evidence = serde_json::from_str(&ev)?;
                Ok(FlagEvent {
                    signal,
                    state,
                    detail: evidence.detail,
                    constraint_id: evidence.constraint_id,
                    span: evidence.span,
                    turn_index: turn_idx.map(|i| i as usize),
                })
            })
            .collect()
    }

    /// Read back the recorded events for a session, oldest first.
    pub fn load_events(&self, session_id: &str) -> Result<Vec<SignalEvent>> {
        let mut stmt = self.conn.prepare(
            "SELECT signal, state, evidence FROM signal_events WHERE session_id = ?1 ORDER BY id",
        )?;
        let rows = stmt
            .query_map(params![session_id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|(sig, st, ev)| {
                let evidence: Evidence = serde_json::from_str(&ev)?;
                Ok(SignalEvent::new(
                    parse_signal_kind(&sig),
                    parse_state(&st),
                    evidence,
                ))
            })
            .collect()
    }

    /// Update the cached traffic-light status on the session row.
    pub fn set_status(&mut self, session_id: &str, state: State) -> Result<()> {
        self.conn.execute(
            "UPDATE sessions SET status = ?1 WHERE id = ?2",
            params![state_str(state), session_id],
        )?;
        Ok(())
    }

    /// A compact summary of every persisted session, newest activity first, for
    /// the history/timeline view. Local-only, like everything else here.
    pub fn list_sessions(&self, limit: usize) -> Result<Vec<SessionSummary>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.id, s.model, s.goal, s.status,
                    COUNT(t.id) AS n, COALESCE(MAX(t.ts), 0) AS last_ts
             FROM sessions s LEFT JOIN turns t ON t.session_id = s.id
             GROUP BY s.id
             ORDER BY last_ts DESC
             LIMIT ?1",
        )?;
        let rows = stmt
            .query_map(params![limit as i64], |r| {
                Ok(SessionSummary {
                    session_id: r.get(0)?,
                    model: r.get(1)?,
                    goal: r.get(2)?,
                    status: r.get(3)?,
                    turns: r.get::<_, i64>(4)?,
                    last_ts: r.get::<_, i64>(5)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    // --- standing orders (the moat) ---------------------------------------

    /// Record one occurrence of a recurring constraint/correction. Deduplicates
    /// against existing orders by embedding cosine similarity (≥ [`SO_DEDUP_SIM`])
    /// so re-phrasings count as the same order. Returns the resulting order
    /// (with its updated occurrence count).
    pub fn bump_standing_order(&mut self, text: &str, embedding: &[f32]) -> Result<StandingOrder> {
        // Find the closest existing order.
        let mut best: Option<(i64, f32)> = None;
        {
            let mut stmt = self
                .conn
                .prepare("SELECT id, embedding FROM standing_orders")?;
            let rows = stmt.query_map([], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, Option<Vec<u8>>>(1)?))
            })?;
            for row in rows {
                let (id, blob) = row?;
                let sim = blob
                    .map(|b| drifterr_embeddings::cosine(embedding, &bytes_to_vec(&b)))
                    .unwrap_or(0.0);
                if sim >= SO_DEDUP_SIM && best.map(|(_, s)| sim > s).unwrap_or(true) {
                    best = Some((id, sim));
                }
            }
        }

        if let Some((id, _)) = best {
            self.conn.execute(
                "UPDATE standing_orders SET occurrences = occurrences + 1 WHERE id = ?1",
                params![id],
            )?;
            return self.standing_order(id);
        }

        self.conn.execute(
            "INSERT INTO standing_orders (text, embedding, occurrences, promoted)
             VALUES (?1, ?2, 1, 0)",
            params![text, vec_to_bytes(embedding)],
        )?;
        let id = self.conn.last_insert_rowid();
        self.standing_order(id)
    }

    fn standing_order(&self, id: i64) -> Result<StandingOrder> {
        Ok(self.conn.query_row(
            "SELECT id, text, occurrences, promoted FROM standing_orders WHERE id = ?1",
            params![id],
            |r| {
                Ok(StandingOrder {
                    id: r.get(0)?,
                    text: r.get(1)?,
                    occurrences: r.get(2)?,
                    promoted: r.get::<_, i64>(3)? != 0,
                })
            },
        )?)
    }

    /// All standing orders, most-recurring first.
    pub fn list_standing_orders(&self) -> Result<Vec<StandingOrder>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, text, occurrences, promoted FROM standing_orders
             ORDER BY occurrences DESC, id ASC",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(StandingOrder {
                    id: r.get(0)?,
                    text: r.get(1)?,
                    occurrences: r.get(2)?,
                    promoted: r.get::<_, i64>(3)? != 0,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Mark a standing order as promoted (an accepted persistent rule).
    pub fn promote_standing_order(&mut self, id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE standing_orders SET promoted = 1 WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }

    /// Promoted standing orders — the ones to auto-apply to new sessions.
    pub fn promoted_standing_orders(&self) -> Result<Vec<StandingOrder>> {
        Ok(self
            .list_standing_orders()?
            .into_iter()
            .filter(|s| s.promoted)
            .collect())
    }
}

/// Occurrences at/above which a standing order is a promotion candidate.
pub const SO_PROMOTE_THRESHOLD: i64 = 3;
/// Embedding cosine at/above which two corrections are "the same" standing order.
pub const SO_DEDUP_SIM: f32 = 0.55;

/// One recorded flag (an amber/red signal event) for the activity journal.
#[derive(Debug, Clone, PartialEq)]
pub struct FlagEvent {
    /// Signal kind id ("constraint", "saturation", …).
    pub signal: String,
    /// "amber" | "red".
    pub state: String,
    /// The evidence detail line.
    pub detail: String,
    pub constraint_id: Option<String>,
    pub span: Option<String>,
    /// The turn the flag fired on, if recorded.
    pub turn_index: Option<usize>,
}

/// A compact, per-session summary for the history/timeline view.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionSummary {
    pub session_id: String,
    pub model: String,
    /// The session's goal (declared or extracted), if any.
    pub goal: Option<String>,
    /// Last committed state ("green" | "amber" | "red"), if recorded.
    pub status: Option<String>,
    /// Number of turns recorded.
    pub turns: i64,
    /// Timestamp (ms) of the most recent turn, for ordering + display.
    pub last_ts: i64,
}

/// A recurring correction tracked across sessions — the seed of the personal
/// "standing orders" layer (the moat).
#[derive(Debug, Clone, PartialEq)]
pub struct StandingOrder {
    pub id: i64,
    pub text: String,
    pub occurrences: i64,
    pub promoted: bool,
}

impl StandingOrder {
    /// Recurring enough to propose as a persistent rule, not yet promoted.
    pub fn is_candidate(&self) -> bool {
        !self.promoted && self.occurrences >= SO_PROMOTE_THRESHOLD
    }
}

fn vec_to_bytes(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out
}

fn bytes_to_vec(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use drifterr_engine::conversation::{Role, Source};

    fn sample() -> Conversation {
        Conversation {
            session_id: "sess-1".into(),
            model: "claude-opus-4-x".into(),
            turns: vec![
                Turn {
                    index: 0,
                    role: Role::User,
                    content: "refactor auth".into(),
                    tokens: 5,
                    timestamp: 100,
                },
                Turn {
                    index: 1,
                    role: Role::Assistant,
                    content: "here is app.ts".into(),
                    tokens: 7,
                    timestamp: 200,
                },
            ],
            context: ContextState {
                window_size: 200_000,
                used_tokens: 1234,
                exact: true,
                tool_call_count: 2,
            },
            source: Source::Proxy,
        }
    }

    #[test]
    fn standing_orders_dedup_threshold_and_promote() {
        use drifterr_embeddings::{BagEmbedder, Embedder};
        let mut store = Store::open_in_memory().unwrap();
        let e = BagEmbedder::default();

        let text = "TypeScript only, no JS files";
        let emb = e.embed(text);
        // Same correction across three sessions → one order, occurrences = 3.
        let s1 = store.bump_standing_order(text, &emb).unwrap();
        assert_eq!(s1.occurrences, 1);
        assert!(!s1.is_candidate());
        store.bump_standing_order(text, &emb).unwrap();
        let s3 = store.bump_standing_order(text, &emb).unwrap();
        assert_eq!(s3.occurrences, 3);
        assert!(s3.is_candidate(), "3 occurrences ⇒ promotion candidate");

        // A clearly different correction is tracked separately.
        let other = "Never use console.log in committed code";
        store.bump_standing_order(other, &e.embed(other)).unwrap();
        assert_eq!(store.list_standing_orders().unwrap().len(), 2);

        // Promote it → no longer a candidate, and shows up in promoted list.
        store.promote_standing_order(s3.id).unwrap();
        let promoted = store.promoted_standing_orders().unwrap();
        assert_eq!(promoted.len(), 1);
        assert_eq!(promoted[0].text, text);
        assert!(!promoted[0].is_candidate());
    }

    #[test]
    fn meta_round_trips_and_init_is_first_write_wins() {
        let mut s = Store::open_in_memory().unwrap();
        assert_eq!(s.meta("trial_started_at").unwrap(), None);

        // First caller establishes the value.
        assert_eq!(
            s.meta_or_init("trial_started_at", "1700000000000").unwrap(),
            "1700000000000"
        );
        // Later callers read the same one back — a relaunch must not restart the
        // trial clock.
        assert_eq!(
            s.meta_or_init("trial_started_at", "9999999999999").unwrap(),
            "1700000000000"
        );
        assert_eq!(
            s.meta("trial_started_at").unwrap().as_deref(),
            Some("1700000000000")
        );

        // An explicit write still overwrites (e.g. tests, support tooling).
        s.set_meta("trial_started_at", "42").unwrap();
        assert_eq!(s.meta("trial_started_at").unwrap().as_deref(), Some("42"));
    }

    #[test]
    fn conversation_round_trip() {
        let mut store = Store::open_in_memory().unwrap();
        let conv = sample();
        store.save_conversation(&conv).unwrap();
        let back = store.load_conversation("sess-1").unwrap();
        assert_eq!(conv, back);
    }

    #[test]
    fn list_sessions_summarizes_newest_first() {
        let mut store = Store::open_in_memory().unwrap();
        store.save_conversation(&sample()).unwrap(); // sess-1, last turn ts=200
        store.set_status("sess-1", State::Red).unwrap();

        // A second, more recent session.
        let mut later = sample();
        later.session_id = "sess-2".into();
        later.turns[0].timestamp = 5000;
        later.turns[1].timestamp = 6000;
        store.save_conversation(&later).unwrap();

        let list = store.list_sessions(10).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].session_id, "sess-2", "newest activity first");
        assert_eq!(list[0].turns, 2);
        assert_eq!(list[0].last_ts, 6000);
        assert_eq!(list[1].session_id, "sess-1");
        assert_eq!(list[1].status.as_deref(), Some("red"));
        // The limit is honored.
        assert_eq!(store.list_sessions(1).unwrap().len(), 1);
    }

    #[test]
    fn baseline_round_trip() {
        use drifterr_engine::baseline::{Checkable, ConstraintType};
        let mut store = Store::open_in_memory().unwrap();
        store.save_conversation(&sample()).unwrap();
        let baseline = Baseline {
            goal: "Refactor auth in strict TS".into(),
            constraints: vec![Constraint {
                id: "c1".into(),
                text: "TypeScript only, no JS".into(),
                kind: ConstraintType::Tech,
                checkable: Checkable::Deterministic,
                active: true,
                rule: Some(Rule::ForbidPattern {
                    pattern: r"\.js\b".into(),
                }),
            }],
            decisions: vec![Decision {
                text: "use argon2".into(),
                rejected: false,
            }],
        };
        store.save_baseline("sess-1", &baseline).unwrap();
        let back = store.load_baseline("sess-1").unwrap();
        assert_eq!(baseline, back);
    }

    #[test]
    fn recent_flags_returns_non_green_newest_first() {
        use drifterr_engine::signals::SignalKind;
        let mut store = Store::open_in_memory().unwrap();
        store.save_conversation(&sample()).unwrap();
        let ev = |kind, state, detail: &str, turn| {
            SignalEvent::new(
                kind,
                state,
                Evidence {
                    detail: detail.into(),
                    turn_index: Some(turn),
                    constraint_id: None,
                    span: None,
                },
            )
        };
        store
            .record_events(
                "sess-1",
                &[
                    ev(SignalKind::Saturation, State::Green, "20% full", 1),
                    ev(SignalKind::Constraint, State::Red, "violated c1", 2),
                    ev(SignalKind::GoalAlignment, State::Amber, "drifting", 3),
                ],
            )
            .unwrap();
        let flags = store.recent_flags("sess-1", 10).unwrap();
        // Only the two non-green events, newest (highest id) first.
        assert_eq!(flags.len(), 2);
        assert_eq!(flags[0].signal, "goal_alignment");
        assert_eq!(flags[0].state, "amber");
        assert_eq!(flags[0].turn_index, Some(3));
        assert_eq!(flags[1].signal, "constraint");
        assert_eq!(flags[1].detail, "violated c1");
        // Limit is honored.
        assert_eq!(store.recent_flags("sess-1", 1).unwrap().len(), 1);
    }

    #[test]
    fn events_round_trip() {
        let mut store = Store::open_in_memory().unwrap();
        store.save_conversation(&sample()).unwrap();
        let events = vec![SignalEvent::new(
            drifterr_engine::signals::SignalKind::Constraint,
            State::Red,
            Evidence {
                detail: "violated".into(),
                turn_index: Some(1),
                constraint_id: Some("c1".into()),
                span: Some(".js".into()),
            },
        )];
        store.record_events("sess-1", &events).unwrap();
        let back = store.load_events("sess-1").unwrap();
        assert_eq!(events, back);
    }
}
