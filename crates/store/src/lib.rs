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
    fn conversation_round_trip() {
        let mut store = Store::open_in_memory().unwrap();
        let conv = sample();
        store.save_conversation(&conv).unwrap();
        let back = store.load_conversation("sess-1").unwrap();
        assert_eq!(conv, back);
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
