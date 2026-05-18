//! Internal `store` module: `SqliteSessionStore` plus the trait impls
//! and helpers that operate on it.
//!
//! See the crate-level docs in `lib.rs` for the locking and error
//! conventions. Sub-modules under `store::*` split the impl by axis
//! (`session_save`, `session_load`, `session_search`, `learner`) plus
//! pure helpers (`conv`, `fts`, `embeddings`).

mod conv;
mod embeddings;
mod fts;
mod learner;
mod session;
mod session_load;
mod session_save;
mod session_search;

use std::cell::Cell;
use std::path::Path;
use std::sync::Mutex;

use primer_core::error::{PrimerError, Result};
use rusqlite::Connection;

thread_local! {
    /// Per-OS-thread counter incremented by every
    /// `SqliteSessionStore::open_for_locale` call.
    ///
    /// Visible via [`session_store_open_count`]; used by the GUI's
    /// `resume_session` test to pin the one-open invariant (issue #86).
    /// Thread-local — not a process-wide atomic — because `cargo test`
    /// runs tests in parallel across OS threads and a global counter
    /// would race. Each `#[tokio::test]` runs on its own OS thread with
    /// a default `current_thread` runtime, so all opens within one
    /// test are observed on the same thread.
    static SESSION_STORE_OPEN_COUNT: Cell<usize> = const { Cell::new(0) };
}

/// Read the calling thread's session-store open counter. Tests
/// snapshot this before a flow, then assert the delta after; production
/// code does not consult it.
pub fn session_store_open_count() -> usize {
    SESSION_STORE_OPEN_COUNT.with(|c| c.get())
}

/// SQLite-backed session store.
///
/// Each store is scoped to a single `Locale`. The application invariant
/// is one learner per DB file, and one learner has one locale, so the
/// store's locale matches the learner's. The locale is used as the
/// `concept_language_tag` value when concepts are first inserted into
/// the shared `concepts` table — `INSERT OR IGNORE` semantics mean the
/// first locale to introduce a concept name owns the tag forever.
#[derive(Debug)]
pub struct SqliteSessionStore {
    conn: Mutex<Connection>,
    locale: primer_core::i18n::Locale,
}

impl SqliteSessionStore {
    /// Open (or create) a session store at `path`, defaulting to the
    /// English locale. Back-compat shim for callers that pre-date the
    /// locale-aware API.
    #[deprecated(
        since = "0.1.0",
        note = "use open_for_locale to make the locale explicit; the shim defaults to English and would silently mis-tag concepts inserted into a non-English DB"
    )]
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_for_locale(path, primer_core::i18n::Locale::default())
    }

    /// Open (or create) a session store at `path` scoped to `locale`.
    /// Use `:memory:` for an in-memory database.
    ///
    /// Creates the schema if missing, sets `PRAGMA foreign_keys = ON`,
    /// asserts/sets `PRAGMA user_version`, and applies v2 through v6
    /// migrations to bring older DBs up to date. The migrations are
    /// idempotent — safe to run on a fresh DB or any pre-v6 DB. A version
    /// newer than this build understands is a hard error rather than a
    /// silent downgrade.
    pub fn open_for_locale(path: &Path, locale: primer_core::i18n::Locale) -> Result<Self> {
        SESSION_STORE_OPEN_COUNT.with(|c| c.set(c.get() + 1));
        let conn = Connection::open(path)
            .map_err(|e| PrimerError::Storage(format!("open failed: {e}")))?;

        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .map_err(|e| PrimerError::Storage(format!("PRAGMA foreign_keys failed: {e}")))?;

        // Read existing user_version. A fresh DB returns 0; v1 DBs from
        // before the rolling-summary work return 1; current builds stamp 2.
        let existing_version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(|e| PrimerError::Storage(format!("read user_version failed: {e}")))?;

        if existing_version > crate::schema::USER_VERSION {
            return Err(PrimerError::Storage(format!(
                "incompatible schema version: file is at user_version={existing_version}, this build expects {}",
                crate::schema::USER_VERSION
            )));
        }

        conn.execute_batch(crate::schema::SCHEMA_SQL)
            .map_err(|e| PrimerError::Storage(format!("schema creation failed: {e}")))?;

        // v2 migrations: idempotent on every open. Adds summary columns
        // and the FTS5 turn-text index if not already present.
        crate::schema::apply_v2_migrations(&conn)?;

        // v3 migrations: idempotent on every open. Adds engagement_states,
        // classifiers, and turn_classifications tables.
        crate::schema::apply_v3_migrations(&conn)?;

        // v4 migrations: idempotent on every open. Adds understanding_depths,
        // learners, and learner_concepts tables (schema-only — adoption of
        // existing-session learner_id is the CLI's job).
        crate::schema::apply_v4_migrations(&conn)?;

        // v5 migrations: idempotent on every open. Adds
        // comprehension_classifiers and turn_comprehensions tables for
        // per-concept comprehension assessments.
        crate::schema::apply_v5_migrations(&conn)?;

        // v6 migrations: idempotent on every open. Adds
        // learners.locale and concepts.concept_language_tag columns
        // (default 'en' for pre-v6 rows) for the i18n architecture.
        crate::schema::apply_v6_migrations(&conn)?;

        // v7 migrations: idempotent on every open. Adds
        // learner_concepts.box_level (default 0 for pre-v7 rows) for
        // the Leitner-box spaced-repetition vocabulary feature.
        crate::schema::apply_v7_migrations(&conn)?;

        // v8 migrations: idempotent on every open. Adds
        // embedding_models + embeddings_turns for hybrid long-term-
        // memory retrieval (Phase 0.2.5).
        crate::schema::apply_v8_migrations(&conn)?;

        if existing_version != crate::schema::USER_VERSION {
            conn.execute_batch(&format!(
                "PRAGMA user_version = {};",
                crate::schema::USER_VERSION
            ))
            .map_err(|e| PrimerError::Storage(format!("set user_version failed: {e}")))?;
        }

        // Validate-and-seed the lookup tables. Borrows the connection
        // directly; no transaction needed because the writes are
        // idempotent INSERTs.
        let speakers = crate::catalog::expected_speakers();
        let intents = crate::catalog::expected_intents();
        let engagement_states = crate::catalog::expected_engagement_states();
        let understanding_depths = crate::catalog::expected_understanding_depths();
        crate::schema::validate_and_seed_lookup(&conn, "speakers", &speakers)?;
        crate::schema::validate_and_seed_lookup(&conn, "pedagogical_intents", &intents)?;
        crate::schema::validate_and_seed_lookup(&conn, "engagement_states", &engagement_states)?;
        crate::schema::validate_and_seed_lookup(
            &conn,
            "understanding_depths",
            &understanding_depths,
        )?;

        Ok(Self {
            conn: Mutex::new(conn),
            locale,
        })
    }

    /// Locale this store is scoped to. Used as the
    /// `concept_language_tag` value when concepts are first inserted.
    pub fn locale(&self) -> primer_core::i18n::Locale {
        self.locale
    }

    /// Re-tag the store's locale without re-opening the SQLite file.
    ///
    /// Used by the GUI's resume path (issue #86): the store is first
    /// opened under cfg's locale, the persisted learner row is read to
    /// determine the effective locale, and — if they differ — this
    /// updates the locale field so subsequent writes (notably
    /// `concept_language_tag` inserts) use the persisted value. The
    /// SQLite-file schema is locale-neutral on this side (one
    /// `concepts` table across all locales, tagged via a column), so
    /// the connection itself doesn't need to be re-opened.
    ///
    /// Distinct from the KB side, where each locale gets its own
    /// `passages_<pack>` tables and re-opening is mandatory.
    pub fn set_locale(&mut self, locale: primer_core::i18n::Locale) {
        self.locale = locale;
    }
}

#[cfg(test)]
mod tests {
    mod learner_tests;
    mod session_tests;

    use super::*;
    use primer_core::i18n::Locale;
    use tempfile::tempdir;

    #[test]
    fn open_for_locale_increments_counter() {
        let dir = tempdir().unwrap();
        let before = session_store_open_count();
        let _store =
            SqliteSessionStore::open_for_locale(&dir.path().join("a.db"), Locale::English).unwrap();
        assert_eq!(
            session_store_open_count() - before,
            1,
            "one increment per open"
        );
        let _store2 =
            SqliteSessionStore::open_for_locale(&dir.path().join("b.db"), Locale::German).unwrap();
        assert_eq!(
            session_store_open_count() - before,
            2,
            "two opens visible in counter"
        );
    }

    #[test]
    fn set_locale_swaps_in_place_without_reopening() {
        let dir = tempdir().unwrap();
        let mut store =
            SqliteSessionStore::open_for_locale(&dir.path().join("a.db"), Locale::English).unwrap();
        assert_eq!(store.locale(), Locale::English);
        let before = session_store_open_count();
        store.set_locale(Locale::German);
        assert_eq!(store.locale(), Locale::German);
        assert_eq!(
            session_store_open_count() - before,
            0,
            "set_locale must not re-open"
        );
    }
}
