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
  -- 0/1, defaulting to 1 so rows written before this column existed keep their meaning.
  -- False = used_tokens is a lower bound, not live occupancy (transcript spans a
  -- compaction). See ContextState::occupancy_known.
  occupancy_known INTEGER NOT NULL DEFAULT 1,
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
  -- 1 when Drifterr inferred this from a project rules file rather than the user
  -- stating it. A proposed constraint caps at AMBER until confirmed, so an
  -- importer mistake can never produce a red alert. See Constraint::proposed.
  proposed   INTEGER NOT NULL DEFAULT 0,
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

-- Re-anchor events. Recorded so the weekly report can answer "did re-anchoring
-- actually help?" with a count instead of a feeling — the outcome itself is judged
-- live in the proxy (see ReanchorMark) and written back here once decided.
CREATE TABLE IF NOT EXISTS reanchors (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  session_id TEXT NOT NULL,
  signal     TEXT NOT NULL,      -- the cause it was meant to fix
  constraint_id TEXT,            -- when the cause was a specific constraint
  ts         INTEGER NOT NULL,
  -- NULL = still unknown / too early to say, 1 = the cause stayed quiet,
  -- 0 = the same cause came back. Deliberately tri-state: "we don't know yet" is
  -- a real answer and must not be reported as success.
  held       INTEGER
);

-- Install-scoped key/value metadata. Deliberately tiny and opaque: this holds
-- app facts (e.g. when the local Pro trial started), never chat content.
CREATE TABLE IF NOT EXISTS app_meta (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_turns_session ON turns(session_id, idx);
CREATE INDEX IF NOT EXISTS idx_events_session ON signal_events(session_id);
