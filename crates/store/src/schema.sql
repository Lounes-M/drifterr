-- Drifterr local store. 100% local, requestable. One SQLite file per install.
-- Mirrors the data model in the technical brief (Part IV §7).

CREATE TABLE IF NOT EXISTS sessions (
  id         TEXT PRIMARY KEY,
  model      TEXT NOT NULL,
  source     TEXT NOT NULL,
  goal       TEXT,
  started_at INTEGER,
  status     TEXT            -- green | amber | red
);

CREATE TABLE IF NOT EXISTS turns (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  idx        INTEGER NOT NULL,
  role       TEXT NOT NULL,
  content    TEXT NOT NULL,
  tokens     INTEGER NOT NULL,
  ts         INTEGER NOT NULL,
  UNIQUE(session_id, idx)
);

CREATE TABLE IF NOT EXISTS context_state (
  session_id  TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
  window_size INTEGER NOT NULL,
  used_tokens INTEGER NOT NULL,
  exact       INTEGER NOT NULL,   -- 0/1
  tool_calls  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS constraints (
  id         TEXT NOT NULL,
  session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  text       TEXT NOT NULL,
  type       TEXT NOT NULL,
  checkable  TEXT NOT NULL,
  active     INTEGER NOT NULL,
  rule_json  TEXT,               -- serialized Rule, NULL when inferred/none
  PRIMARY KEY (session_id, id)
);

CREATE TABLE IF NOT EXISTS decisions (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  text       TEXT NOT NULL,
  embedding  BLOB,               -- reserved for Signal 3 (later milestone)
  rejected   INTEGER NOT NULL,
  ts         INTEGER
);

CREATE TABLE IF NOT EXISTS signal_events (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  signal     TEXT NOT NULL,      -- signal kind
  state      TEXT NOT NULL,      -- green | amber | red
  evidence   TEXT NOT NULL,      -- serialized Evidence JSON
  turn_idx   INTEGER,
  ts         INTEGER
);

CREATE TABLE IF NOT EXISTS standing_orders (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  text        TEXT NOT NULL,
  embedding   BLOB,              -- reserved for dedup (later milestone)
  occurrences INTEGER NOT NULL,
  promoted    INTEGER NOT NULL
);

-- Install-scoped key/value metadata. Deliberately tiny and opaque: this holds
-- app facts (e.g. when the local Pro trial started), never chat content.
CREATE TABLE IF NOT EXISTS app_meta (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_turns_session ON turns(session_id, idx);
CREATE INDEX IF NOT EXISTS idx_events_session ON signal_events(session_id);
