//! Database schema definitions and the validate-and-seed helper for
//! lookup tables.

/// The on-disk schema version this build understands. Stored in
/// `PRAGMA user_version`. A mismatch on `open()` is a hard error.
pub const USER_VERSION: i64 = 1;

/// Idempotent CREATE-TABLE statements run on every `open()`. Includes
/// the lookup tables, conversation tables, and indices.
pub const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS speakers (
    id    INTEGER PRIMARY KEY,
    name  TEXT NOT NULL UNIQUE
);

CREATE TABLE IF NOT EXISTS pedagogical_intents (
    id    INTEGER PRIMARY KEY,
    name  TEXT NOT NULL UNIQUE
);

CREATE TABLE IF NOT EXISTS concepts (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    concept_id  TEXT NOT NULL UNIQUE
);

CREATE TABLE IF NOT EXISTS sessions (
    id          TEXT PRIMARY KEY,
    learner_id  TEXT NOT NULL,
    started_at  TEXT NOT NULL,
    ended_at    TEXT
);
CREATE INDEX IF NOT EXISTS idx_sessions_learner
    ON sessions(learner_id, started_at);

CREATE TABLE IF NOT EXISTS turns (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id  TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    turn_index  INTEGER NOT NULL,
    speaker_id  INTEGER NOT NULL REFERENCES speakers(id),
    text        TEXT NOT NULL,
    timestamp   TEXT NOT NULL,
    intent_id   INTEGER REFERENCES pedagogical_intents(id),
    UNIQUE(session_id, turn_index)
);
CREATE INDEX IF NOT EXISTS idx_turns_session
    ON turns(session_id, turn_index);

CREATE TABLE IF NOT EXISTS turn_concepts (
    turn_id     INTEGER NOT NULL REFERENCES turns(id) ON DELETE CASCADE,
    concept_id  INTEGER NOT NULL REFERENCES concepts(id),
    PRIMARY KEY(turn_id, concept_id)
);
CREATE INDEX IF NOT EXISTS idx_turn_concepts_concept
    ON turn_concepts(concept_id);
"#;
