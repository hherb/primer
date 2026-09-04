//! Database schema definitions and the validate-and-seed helper for
//! lookup tables.
//!
//! This module owns the two pieces that describe "what shape is the
//! database": [`USER_VERSION`] (which version this build understands)
//! and [`SCHEMA_SQL`] (the v1 baseline DDL). Everything that *changes*
//! that shape lives in the [`migrations`] submodule — one file per
//! version step — and the closed-vocabulary lookup-table validation
//! lives in [`lookup`].

mod lookup;
mod migrations;

pub use lookup::validate_and_seed_lookup;
pub use migrations::apply_v2_migrations;
// Deliberately a glob, not an explicit name list: `mod migrations` is
// private to `schema`, so the open path can only reach a migration
// through this re-export. A glob means adding `vN.rs` needs no edit
// here — which is what keeps the "adding a version" checklist in
// `migrations/mod.rs` true. Spelling the names out again would add a
// second edit site that a new version can silently miss (the resulting
// E0425 in `store/mod.rs` points at `apply_v2_migrations`, not here).
pub(crate) use migrations::*;

/// The on-disk schema version this build understands. Stored in
/// `PRAGMA user_version`. A mismatch on `open()` is a hard error.
///
/// Bumped to 2 when we added the rolling-summary fields on `sessions`
/// and the `turn_text_fts` virtual table for searchable session memory.
/// `open()` migrates v1 databases in place; the migration is purely
/// additive (column adds + new objects), so existing data is preserved.
///
/// Bumped to 3 when we added the `engagement_states`, `classifiers`,
/// and `turn_classifications` tables that back the engagement-classifier
/// feature (Phase 0.3).
///
/// Bumped to 4 when we added the `understanding_depths` lookup table
/// plus `learners` and `learner_concepts` tables that back learner-model
/// SQLite persistence (Phase 0.3). Schema-only — adoption of an existing
/// session's `learner_id` happens at the CLI layer, not in this migration.
///
/// Bumped to 5 when we added the `comprehension_classifiers` and
/// `turn_comprehensions` tables that back the per-concept
/// comprehension-classifier feature (Phase 0.3).
///
/// Bumped to 6 when we added the `learners.locale` column (BCP-47 short
/// pack id, default `'en'`) and the `concepts.concept_language_tag`
/// column (default `'en'`) that back the i18n / multi-locale prompt-pack
/// architecture (Phase 0.1 i18n). Schema-only — adopting a non-default
/// locale for an existing learner is the CLI's responsibility.
///
/// Bumped to 7 when we added the `learner_concepts.box_level` column
/// (INTEGER NOT NULL DEFAULT 0) backing the spaced-repetition
/// vocabulary feature (Phase 0.3). Existing rows default to box 0 — no
/// backfill needed at this stage (Phase 0.3 has no field-deployed users).
///
/// Bumped to 8 when we added the `embedding_models` registry plus the
/// `embeddings_turns` table (one little-endian f32 BLOB per turn) that
/// back hybrid long-term-memory retrieval (Phase 0.2.5).
/// `embedding_models` mirrors the registry in `primer-knowledge`, so a
/// DB re-opened under a different embedder is a detectable hard error
/// rather than a silent retrieval-quality regression.
pub const USER_VERSION: i64 = 8;

/// Idempotent CREATE statements for the base (v1) schema. Run on every
/// `open()`. v2-specific objects are added by `apply_v2_migrations`.
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
    id    INTEGER PRIMARY KEY AUTOINCREMENT,
    name  TEXT NOT NULL UNIQUE
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

#[cfg(test)]
mod v4_tests;
