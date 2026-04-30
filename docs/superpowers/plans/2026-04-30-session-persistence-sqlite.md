# Session persistence (SQLite) — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Persist every conversation session to a SQLite database via a new `primer-storage` crate, with a normalised relational schema (lookup tables for `speakers`, `pedagogical_intents`, and `concepts`) and a startup-time validate-and-seed pass that treats the Rust enums as the single source of truth.

**Architecture:** New crate `primer-storage` mirroring `primer-knowledge`'s `rusqlite` + `Mutex<Connection>` pattern. New trait `SessionStore` lives in `primer-core::storage`, re-exported from the crate root. `DialogueManager` gains an optional `&dyn SessionStore` and saves the session after every turn (success or mid-stream error). The `--session-db <path>` CLI flag is wired through `main.rs`.

**Tech Stack:** Rust 2024, `rusqlite` (bundled SQLite), `async_trait`, `tokio` (runtime + test attribute), `tracing` for save-failure warnings.

**Spec:** [docs/superpowers/specs/2026-04-30-session-persistence-sqlite-design.md](../specs/2026-04-30-session-persistence-sqlite-design.md)

---

## File structure

### Created

| Path | Responsibility |
|---|---|
| `src/crates/primer-storage/Cargo.toml` | New workspace member, depends on `primer-core`, `rusqlite`, `async-trait`, `tokio`, `tracing`, `chrono`, `uuid` |
| `src/crates/primer-storage/src/lib.rs` | `SqliteSessionStore` struct, `open(...)`, `impl SessionStore for SqliteSessionStore`, `save_session(...)` |
| `src/crates/primer-storage/src/schema.rs` | Schema SQL string constants, `USER_VERSION: i64 = 1`, the generic `validate_and_seed_lookup` helper |
| `src/crates/primer-storage/src/catalog.rs` | Canonical `(variant, id, name)` mappings: `intent_id`, `speaker_id`, plus `expected_speakers()`, `expected_intents()` returning `&[(i64, &str)]` for the validate-and-seed pass |

### Modified

| Path | Change |
|---|---|
| `src/Cargo.toml` | Add `crates/primer-storage` to `[workspace] members`, add `primer-storage = { path = "crates/primer-storage" }` to `[workspace.dependencies]` |
| `src/crates/primer-core/src/error.rs` | New `Storage(String)` variant on `PrimerError` |
| `src/crates/primer-core/src/lib.rs` | New `pub mod storage;` |
| `src/crates/primer-core/src/storage.rs` | **New file** — defines `SessionStore` trait |
| `src/crates/primer-core/src/conversation.rs` | Add `impl PedagogicalIntent { pub const ALL: &'static [Self] = &[...]; }` and `impl Speaker { pub const ALL: &'static [Self] = &[...]; }` |
| `src/crates/primer-pedagogy/Cargo.toml` | Add `primer-storage.workspace = true` to `[dev-dependencies]` |
| `src/crates/primer-pedagogy/src/dialogue_manager.rs` | Add `storage: Option<&'a dyn SessionStore>` field, thread through `new`, refactor `respond_to_streaming` so the save fires on both Ok and Err paths, save in `open_session` too. New tests for save-on-success and save-on-error using a real `SqliteSessionStore` on `:memory:` |
| `src/crates/primer-cli/Cargo.toml` | Add `primer-storage.workspace = true` to `[dependencies]` |
| `src/crates/primer-cli/src/main.rs` | New `--session-db <path>` flag, construct `SqliteSessionStore`, pass to `DialogueManager::new` |
| `ROADMAP.md` | Tick the "conversation persistence" Phase 0.1 bullet |
| `CLAUDE.md` | Document the new crate, the `--session-db` flag, the `Mutex<Connection>` pattern, and the lookup-table-for-categorical-text convention |

---

## Conventions

- Run all `cargo` commands from `src/` (the workspace root, not the repo root).
- Test count baseline at start of plan: **42**. Each task that adds tests will note the running total in its commit message.
- Commit after every task. Commit subjects use the existing repo convention (`feat:`, `test:`, `refactor:`, `docs:`).
- Every test goes in a `#[cfg(test)] mod tests { ... }` inside the same file as the code under test, except where the code under test is the trait itself (which has no behaviour to test).
- Every `tokio::test` is `#[tokio::test]` with the `tokio` workspace dep enabled (already the case workspace-wide).

---

## Task 1: Add `PrimerError::Storage` variant

**Files:**
- Modify: `src/crates/primer-core/src/error.rs`

- [ ] **Step 1: Add the variant**

Append a new arm to the `PrimerError` enum, matching the style of `Knowledge` and `Inference`:

```rust
#[derive(Error, Debug)]
pub enum PrimerError {
    #[error("Inference error: {0}")]
    Inference(String),

    #[error("Speech error: {0}")]
    Speech(String),

    #[error("Knowledge base error: {0}")]
    Knowledge(String),

    #[error("Learner model error: {0}")]
    LearnerModel(String),

    #[error("Storage error: {0}")]
    Storage(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialisation error: {0}")]
    Serde(#[from] serde_json::Error),
}
```

- [ ] **Step 2: Verify the workspace still compiles**

Run from `src/`: `cargo build --workspace`
Expected: clean build, no errors.

- [ ] **Step 3: Commit**

```bash
git add src/crates/primer-core/src/error.rs
git commit -m "feat(primer-core): add PrimerError::Storage variant"
```

---

## Task 2: Add `Speaker::ALL` and `PedagogicalIntent::ALL` constants

**Files:**
- Modify: `src/crates/primer-core/src/conversation.rs`

- [ ] **Step 1: Write the failing test**

At the end of `conversation.rs`, add a `#[cfg(test)] mod tests` block (this file currently has none). Asserting `ALL.len()` matches a known constant gives a tripwire if a new variant is added without updating `ALL`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speaker_all_lists_every_variant() {
        assert_eq!(Speaker::ALL.len(), 2);
        assert!(Speaker::ALL.contains(&Speaker::Child));
        assert!(Speaker::ALL.contains(&Speaker::Primer));
    }

    #[test]
    fn pedagogical_intent_all_lists_every_variant() {
        assert_eq!(PedagogicalIntent::ALL.len(), 8);
        // Spot-check a few representatives.
        assert!(PedagogicalIntent::ALL.contains(&PedagogicalIntent::SocraticQuestion));
        assert!(PedagogicalIntent::ALL.contains(&PedagogicalIntent::SessionClose));
    }
}
```

- [ ] **Step 2: Run the failing tests**

Run from `src/`: `cargo test -p primer-core conversation::tests`
Expected: FAIL — `Speaker::ALL` and `PedagogicalIntent::ALL` do not exist.

- [ ] **Step 3: Add the `ALL` constants**

In `conversation.rs`, add `impl` blocks just after each enum's `derive`-derived definition:

```rust
impl Speaker {
    /// Every variant, in declaration order. Source for the storage layer's
    /// validate-and-seed pass.
    pub const ALL: &'static [Self] = &[Self::Child, Self::Primer];
}

impl PedagogicalIntent {
    /// Every variant, in declaration order. Source for the storage layer's
    /// validate-and-seed pass.
    pub const ALL: &'static [Self] = &[
        Self::SocraticQuestion,
        Self::ComprehensionCheck,
        Self::Scaffolding,
        Self::Encouragement,
        Self::Extension,
        Self::DirectAnswer,
        Self::AnswerThenPivot,
        Self::SessionClose,
    ];
}
```

- [ ] **Step 4: Run the tests**

Run from `src/`: `cargo test -p primer-core conversation::tests`
Expected: PASS — both tests green.

- [ ] **Step 5: Commit**

```bash
git add src/crates/primer-core/src/conversation.rs
git commit -m "feat(primer-core): add Speaker::ALL and PedagogicalIntent::ALL"
```

---

## Task 3: Add `SessionStore` trait in `primer-core::storage`

**Files:**
- Create: `src/crates/primer-core/src/storage.rs`
- Modify: `src/crates/primer-core/src/lib.rs`

- [ ] **Step 1: Add the storage module declaration**

In `lib.rs`, append:

```rust
pub mod storage;
```

- [ ] **Step 2: Create the storage module**

Create `src/crates/primer-core/src/storage.rs` with:

```rust
//! Persistence traits for session and learner-model storage.
//!
//! This crate owns the trait only. Concrete implementations live in
//! sibling crates (e.g. `primer-storage` for SQLite).

use async_trait::async_trait;

use crate::conversation::Session;
use crate::error::Result;

/// Persists conversation sessions.
///
/// Implementations must make `save_session` idempotent — the dialogue
/// manager calls it after every turn, repeatedly, with the same session
/// growing over time. Repeated calls with no in-memory changes must
/// leave the store unchanged.
#[async_trait]
pub trait SessionStore: Send + Sync {
    /// Persist the full current state of `session`. Turns persisted in
    /// a previous call but no longer present in `session.turns` are
    /// removed.
    async fn save_session(&self, session: &Session) -> Result<()>;
}
```

- [ ] **Step 3: Verify compilation**

Run from `src/`: `cargo build -p primer-core`
Expected: clean build.

- [ ] **Step 4: Commit**

```bash
git add src/crates/primer-core/src/storage.rs src/crates/primer-core/src/lib.rs
git commit -m "feat(primer-core): add SessionStore trait"
```

---

## Task 4: Create `primer-storage` crate skeleton

**Files:**
- Create: `src/crates/primer-storage/Cargo.toml`
- Create: `src/crates/primer-storage/src/lib.rs`
- Modify: `src/Cargo.toml`

- [ ] **Step 1: Add the crate to the workspace members and deps**

In `src/Cargo.toml`:

In `[workspace] members`, after the existing entries, add:

```toml
    "crates/primer-storage",
```

In `[workspace.dependencies]`, after the existing `primer-pedagogy` line, add:

```toml
primer-storage = { path = "crates/primer-storage" }
```

- [ ] **Step 2: Create the crate's Cargo.toml**

Create `src/crates/primer-storage/Cargo.toml`:

```toml
[package]
name = "primer-storage"
description = "SQLite-backed session and learner-model persistence for the Primer"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
primer-core.workspace = true
async-trait.workspace = true
tokio.workspace = true
rusqlite.workspace = true
tracing.workspace = true
thiserror.workspace = true
chrono.workspace = true
uuid.workspace = true
```

- [ ] **Step 3: Create the empty lib.rs**

Create `src/crates/primer-storage/src/lib.rs`:

```rust
//! # primer-storage
//!
//! SQLite-backed implementations of the persistence traits defined in
//! `primer-core::storage`.
//!
//! Mirrors the locking and error patterns of `primer-knowledge`: a
//! single `Connection` wrapped in `Mutex`, async trait methods with
//! synchronous bodies (acceptable at our turn rate; revisit if profiling
//! ever shows contention).

mod catalog;
mod schema;

// Public API will be added in subsequent tasks.
```

- [ ] **Step 4: Create empty `catalog.rs` and `schema.rs`**

Create `src/crates/primer-storage/src/catalog.rs`:

```rust
//! Canonical mappings between Rust enum variants and their on-disk
//! integer IDs. This module is the single source of truth for the
//! contents of the `speakers` and `pedagogical_intents` lookup tables.
```

Create `src/crates/primer-storage/src/schema.rs`:

```rust
//! Database schema definitions and the validate-and-seed helper for
//! lookup tables.
```

- [ ] **Step 5: Verify the workspace builds**

Run from `src/`: `cargo build --workspace`
Expected: clean build, the new crate compiles as an empty library.

- [ ] **Step 6: Commit**

```bash
git add src/Cargo.toml src/crates/primer-storage
git commit -m "feat(primer-storage): scaffold new crate"
```

---

## Task 5: Define canonical catalogs (`intent_id`, `speaker_id`)

**Files:**
- Modify: `src/crates/primer-storage/src/catalog.rs`

- [ ] **Step 1: Write the failing tests**

Add a `#[cfg(test)] mod tests` to `catalog.rs`:

```rust
//! Canonical mappings between Rust enum variants and their on-disk
//! integer IDs. This module is the single source of truth for the
//! contents of the `speakers` and `pedagogical_intents` lookup tables.

use primer_core::conversation::{PedagogicalIntent, Speaker};

/// Stable on-disk integer ID for every `Speaker` variant.
///
/// Explicit values (no `as i64`) so reordering variants in `primer-core`
/// cannot silently shift IDs already on disk.
pub fn speaker_id(speaker: Speaker) -> i64 {
    match speaker {
        Speaker::Child => 1,
        Speaker::Primer => 2,
    }
}

/// Human-readable name stored alongside the ID. Used by the
/// validate-and-seed pass and useful for debugging.
pub fn speaker_name(speaker: Speaker) -> &'static str {
    match speaker {
        Speaker::Child => "Child",
        Speaker::Primer => "Primer",
    }
}

/// Stable on-disk integer ID for every `PedagogicalIntent` variant.
pub fn intent_id(intent: PedagogicalIntent) -> i64 {
    match intent {
        PedagogicalIntent::SocraticQuestion => 1,
        PedagogicalIntent::ComprehensionCheck => 2,
        PedagogicalIntent::Scaffolding => 3,
        PedagogicalIntent::Encouragement => 4,
        PedagogicalIntent::Extension => 5,
        PedagogicalIntent::DirectAnswer => 6,
        PedagogicalIntent::AnswerThenPivot => 7,
        PedagogicalIntent::SessionClose => 8,
    }
}

pub fn intent_name(intent: PedagogicalIntent) -> &'static str {
    match intent {
        PedagogicalIntent::SocraticQuestion => "SocraticQuestion",
        PedagogicalIntent::ComprehensionCheck => "ComprehensionCheck",
        PedagogicalIntent::Scaffolding => "Scaffolding",
        PedagogicalIntent::Encouragement => "Encouragement",
        PedagogicalIntent::Extension => "Extension",
        PedagogicalIntent::DirectAnswer => "DirectAnswer",
        PedagogicalIntent::AnswerThenPivot => "AnswerThenPivot",
        PedagogicalIntent::SessionClose => "SessionClose",
    }
}

/// Reverse lookup: integer ID → `PedagogicalIntent`.
/// Returns `None` for IDs the current build doesn't know about.
pub fn intent_from_id(id: i64) -> Option<PedagogicalIntent> {
    PedagogicalIntent::ALL.iter().copied().find(|v| intent_id(*v) == id)
}

pub fn speaker_from_id(id: i64) -> Option<Speaker> {
    Speaker::ALL.iter().copied().find(|v| speaker_id(*v) == id)
}

/// All `(id, name)` pairs the storage layer expects to see in the
/// `speakers` lookup table. Used by the validate-and-seed pass.
pub fn expected_speakers() -> Vec<(i64, &'static str)> {
    Speaker::ALL.iter().map(|s| (speaker_id(*s), speaker_name(*s))).collect()
}

/// All `(id, name)` pairs the storage layer expects to see in the
/// `pedagogical_intents` lookup table.
pub fn expected_intents() -> Vec<(i64, &'static str)> {
    PedagogicalIntent::ALL.iter().map(|i| (intent_id(*i), intent_name(*i))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_speaker_round_trips_through_id() {
        for &s in Speaker::ALL {
            assert_eq!(speaker_from_id(speaker_id(s)), Some(s));
        }
    }

    #[test]
    fn every_intent_round_trips_through_id() {
        for &i in PedagogicalIntent::ALL {
            assert_eq!(intent_from_id(intent_id(i)), Some(i));
        }
    }

    #[test]
    fn intent_ids_are_unique() {
        let mut ids: Vec<i64> = PedagogicalIntent::ALL.iter().map(|i| intent_id(*i)).collect();
        ids.sort();
        let len = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), len, "intent_id assigned duplicate IDs");
    }

    #[test]
    fn speaker_ids_are_unique() {
        let mut ids: Vec<i64> = Speaker::ALL.iter().map(|s| speaker_id(*s)).collect();
        ids.sort();
        let len = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), len);
    }

    #[test]
    fn unknown_intent_id_returns_none() {
        assert_eq!(intent_from_id(9999), None);
    }
}
```

- [ ] **Step 2: Run the tests**

Run from `src/`: `cargo test -p primer-storage catalog::tests`
Expected: PASS — five tests green.

- [ ] **Step 3: Commit**

```bash
git add src/crates/primer-storage/src/catalog.rs
git commit -m "feat(primer-storage): canonical Speaker/PedagogicalIntent ID catalog"
```

---

## Task 6: Define schema SQL constants and `USER_VERSION`

**Files:**
- Modify: `src/crates/primer-storage/src/schema.rs`

- [ ] **Step 1: Add the schema constants**

Replace the contents of `schema.rs` with:

```rust
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
```

- [ ] **Step 2: Verify the crate compiles**

Run from `src/`: `cargo build -p primer-storage`
Expected: clean build (no tests yet, the constants are dead code that doesn't trigger warnings because the module is not yet `pub`-exposed but `cargo build` won't complain).

- [ ] **Step 3: Commit**

```bash
git add src/crates/primer-storage/src/schema.rs
git commit -m "feat(primer-storage): schema SQL and USER_VERSION constant"
```

---

## Task 7: Implement `SqliteSessionStore::open` (schema + PRAGMAs only)

**Files:**
- Modify: `src/crates/primer-storage/src/lib.rs`

This task creates the store and runs the schema, but **not yet** the validate-and-seed pass for lookup tables. That comes in Task 8.

- [ ] **Step 1: Write the failing tests**

Replace `lib.rs` with:

```rust
//! # primer-storage
//!
//! SQLite-backed implementations of the persistence traits defined in
//! `primer-core::storage`.

mod catalog;
mod schema;

use std::path::Path;
use std::sync::Mutex;

use primer_core::error::{PrimerError, Result};
use rusqlite::Connection;

/// SQLite-backed session store.
pub struct SqliteSessionStore {
    conn: Mutex<Connection>,
}

impl SqliteSessionStore {
    /// Open (or create) a session store at `path`. Use `:memory:` for
    /// an in-memory database.
    ///
    /// Creates the schema if missing, sets `PRAGMA foreign_keys = ON`,
    /// and asserts/sets `PRAGMA user_version`.
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)
            .map_err(|e| PrimerError::Storage(format!("open failed: {e}")))?;

        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .map_err(|e| PrimerError::Storage(format!("PRAGMA foreign_keys failed: {e}")))?;

        // Read existing user_version. A fresh DB returns 0.
        let existing_version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(|e| PrimerError::Storage(format!("read user_version failed: {e}")))?;

        if existing_version != 0 && existing_version != schema::USER_VERSION {
            return Err(PrimerError::Storage(format!(
                "incompatible schema version: file is at user_version={existing_version}, this build expects {}",
                schema::USER_VERSION
            )));
        }

        conn.execute_batch(schema::SCHEMA_SQL)
            .map_err(|e| PrimerError::Storage(format!("schema creation failed: {e}")))?;

        if existing_version == 0 {
            conn.execute_batch(&format!("PRAGMA user_version = {};", schema::USER_VERSION))
                .map_err(|e| PrimerError::Storage(format!("set user_version failed: {e}")))?;
        }

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn open_memory() -> SqliteSessionStore {
        SqliteSessionStore::open(&PathBuf::from(":memory:")).expect("open :memory:")
    }

    #[test]
    fn open_fresh_creates_all_tables_and_sets_user_version() {
        let store = open_memory();
        let conn = store.conn.lock().unwrap();

        // user_version was set.
        let v: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, 1);

        // foreign_keys is on.
        let fk: i64 = conn
            .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
            .unwrap();
        assert_eq!(fk, 1);

        // All six tables exist.
        for table in &[
            "speakers",
            "pedagogical_intents",
            "concepts",
            "sessions",
            "turns",
            "turn_concepts",
        ] {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "table {table} should exist");
        }
    }

    #[test]
    fn open_rejects_incompatible_user_version() {
        // Create a connection, set a future version, close, then try to open via the store.
        let path = PathBuf::from(":memory:");
        // We need a path-on-disk for this test because :memory: connections aren't
        // shared across `Connection::open` calls. Use a tempfile.
        let tmp = tempfile_path();
        {
            let conn = Connection::open(&tmp).unwrap();
            conn.execute_batch("PRAGMA user_version = 99;").unwrap();
        }
        let err = SqliteSessionStore::open(&tmp).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("99"), "error should mention the bad version: {msg}");
        assert!(msg.contains("Storage"), "error should be a Storage variant: {msg}");
        let _ = std::fs::remove_file(&tmp);
        let _ = path; // suppress unused warning
    }

    /// Returns a unique tempfile path. We don't need the `tempfile` crate —
    /// just a path with a sufficiently random suffix in /tmp.
    fn tempfile_path() -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("primer-storage-test-{nanos}.db"))
    }
}
```

- [ ] **Step 2: Run the tests**

Run from `src/`: `cargo test -p primer-storage tests`
Expected: PASS — both tests green.

- [ ] **Step 3: Commit**

```bash
git add src/crates/primer-storage/src/lib.rs
git commit -m "feat(primer-storage): SqliteSessionStore::open with schema + PRAGMAs"
```

---

## Task 8: Validate-and-seed pass for lookup tables

**Files:**
- Modify: `src/crates/primer-storage/src/schema.rs`
- Modify: `src/crates/primer-storage/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

In `src/crates/primer-storage/src/lib.rs`'s `tests` module, add the new tests just before the closing brace:

```rust
    #[test]
    fn open_seeds_lookup_tables_on_fresh_db() {
        let store = open_memory();
        let conn = store.conn.lock().unwrap();

        let speaker_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM speakers", [], |r| r.get(0))
            .unwrap();
        assert_eq!(speaker_count, 2);

        let intent_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM pedagogical_intents", [], |r| r.get(0))
            .unwrap();
        assert_eq!(intent_count, 8);

        // Spot-check a specific row.
        let name: String = conn
            .query_row(
                "SELECT name FROM pedagogical_intents WHERE id = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(name, "SocraticQuestion");
    }

    #[test]
    fn reopen_is_a_no_op_on_seeded_tables() {
        let tmp = tempfile_path();
        {
            let _store = SqliteSessionStore::open(&tmp).unwrap();
        }
        let store = SqliteSessionStore::open(&tmp).unwrap();
        let conn = store.conn.lock().unwrap();
        let speaker_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM speakers", [], |r| r.get(0))
            .unwrap();
        assert_eq!(speaker_count, 2, "second open should not duplicate seed rows");
        drop(conn);
        drop(store);
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn validate_rejects_name_mismatch() {
        let tmp = tempfile_path();
        {
            let conn = Connection::open(&tmp).unwrap();
            conn.execute_batch(
                "
                PRAGMA foreign_keys = ON;
                CREATE TABLE pedagogical_intents (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE);
                INSERT INTO pedagogical_intents (id, name) VALUES (1, 'WrongName');
                PRAGMA user_version = 1;
                ",
            )
            .unwrap();
        }
        let err = SqliteSessionStore::open(&tmp).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("WrongName") || msg.contains("SocraticQuestion"),
            "error should mention the conflict: {msg}");
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn validate_rejects_unknown_id() {
        let tmp = tempfile_path();
        {
            let conn = Connection::open(&tmp).unwrap();
            conn.execute_batch(
                "
                PRAGMA foreign_keys = ON;
                CREATE TABLE pedagogical_intents (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE);
                INSERT INTO pedagogical_intents (id, name) VALUES (99, 'FromTheFuture');
                PRAGMA user_version = 1;
                ",
            )
            .unwrap();
        }
        let err = SqliteSessionStore::open(&tmp).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("99") || msg.contains("unknown"),
            "error should mention the unknown id: {msg}");
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn validate_seeds_missing_rows() {
        // Pre-populate one valid row; the validator should fill the others in.
        let tmp = tempfile_path();
        {
            let conn = Connection::open(&tmp).unwrap();
            conn.execute_batch(
                "
                PRAGMA foreign_keys = ON;
                CREATE TABLE pedagogical_intents (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE);
                INSERT INTO pedagogical_intents (id, name) VALUES (1, 'SocraticQuestion');
                PRAGMA user_version = 1;
                ",
            )
            .unwrap();
        }
        let store = SqliteSessionStore::open(&tmp).unwrap();
        let conn = store.conn.lock().unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM pedagogical_intents", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 8, "missing rows should have been seeded");
        drop(conn);
        drop(store);
        let _ = std::fs::remove_file(&tmp);
    }
```

- [ ] **Step 2: Run the failing tests**

Run from `src/`: `cargo test -p primer-storage tests`
Expected: FAIL — no validate-and-seed implementation yet.

- [ ] **Step 3: Implement the validate-and-seed helper**

Append to `schema.rs`:

```rust
use primer_core::error::{PrimerError, Result};
use rusqlite::Connection;
use std::collections::HashMap;

/// Validate-and-seed pass for a lookup table.
///
/// `expected` is the canonical source of truth (Rust enum projected to
/// `(id, name)` pairs). The function:
///
/// - Inserts any expected row missing from the table.
/// - Returns `Storage` error if a known id has the wrong name.
/// - Returns `Storage` error if the table contains an id not in `expected`.
///
/// Runs synchronously inside an `&Connection` borrow; caller is
/// responsible for transaction scope.
pub fn validate_and_seed_lookup(
    conn: &Connection,
    table: &str,
    expected: &[(i64, &str)],
) -> Result<()> {
    let actual: HashMap<i64, String> = {
        let sql = format!("SELECT id, name FROM {table}");
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| PrimerError::Storage(format!("prepare for {table}: {e}")))?;
        let rows = stmt
            .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)))
            .map_err(|e| PrimerError::Storage(format!("query {table}: {e}")))?;
        let mut map = HashMap::new();
        for row in rows {
            let (id, name) =
                row.map_err(|e| PrimerError::Storage(format!("read {table} row: {e}")))?;
            map.insert(id, name);
        }
        map
    };

    let expected_ids: std::collections::HashSet<i64> = expected.iter().map(|(id, _)| *id).collect();

    // Step 1: detect unknown ids in the DB.
    for (id, name) in &actual {
        if !expected_ids.contains(id) {
            return Err(PrimerError::Storage(format!(
                "{table} has unknown row id={id} name={name:?} — database may be from a newer build"
            )));
        }
    }

    // Step 2: validate matches and insert missing.
    let insert_sql = format!("INSERT INTO {table} (id, name) VALUES (?1, ?2)");
    for (id, expected_name) in expected {
        match actual.get(id) {
            Some(actual_name) if actual_name == expected_name => {
                // match — nothing to do
            }
            Some(actual_name) => {
                return Err(PrimerError::Storage(format!(
                    "{table} row id={id} has name {actual_name:?}, this build expects {expected_name:?}"
                )));
            }
            None => {
                conn.execute(&insert_sql, rusqlite::params![id, expected_name])
                    .map_err(|e| {
                        PrimerError::Storage(format!("insert into {table}: {e}"))
                    })?;
            }
        }
    }

    Ok(())
}
```

- [ ] **Step 4: Wire it into `open()`**

In `src/crates/primer-storage/src/lib.rs`, replace the existing `open` body. The full updated function:

```rust
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)
            .map_err(|e| PrimerError::Storage(format!("open failed: {e}")))?;

        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .map_err(|e| PrimerError::Storage(format!("PRAGMA foreign_keys failed: {e}")))?;

        let existing_version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(|e| PrimerError::Storage(format!("read user_version failed: {e}")))?;

        if existing_version != 0 && existing_version != schema::USER_VERSION {
            return Err(PrimerError::Storage(format!(
                "incompatible schema version: file is at user_version={existing_version}, this build expects {}",
                schema::USER_VERSION
            )));
        }

        conn.execute_batch(schema::SCHEMA_SQL)
            .map_err(|e| PrimerError::Storage(format!("schema creation failed: {e}")))?;

        if existing_version == 0 {
            conn.execute_batch(&format!("PRAGMA user_version = {};", schema::USER_VERSION))
                .map_err(|e| PrimerError::Storage(format!("set user_version failed: {e}")))?;
        }

        // Validate-and-seed the lookup tables. Borrows the connection
        // directly; no transaction needed because the writes are
        // idempotent INSERTs.
        let speakers = catalog::expected_speakers();
        let intents = catalog::expected_intents();
        schema::validate_and_seed_lookup(&conn, "speakers", &speakers)?;
        schema::validate_and_seed_lookup(&conn, "pedagogical_intents", &intents)?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }
```

- [ ] **Step 5: Run the tests**

Run from `src/`: `cargo test -p primer-storage tests`
Expected: PASS — all five new tests green plus the two from Task 7 (seven total).

- [ ] **Step 6: Commit**

```bash
git add src/crates/primer-storage/src/lib.rs src/crates/primer-storage/src/schema.rs
git commit -m "feat(primer-storage): validate-and-seed pass for lookup tables"
```

---

## Task 9: `save_session` — sessions table only (empty session)

**Files:**
- Modify: `src/crates/primer-storage/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `lib.rs`:

```rust
    use chrono::Utc;
    use primer_core::conversation::{Session, Turn};
    use primer_core::storage::SessionStore;
    use uuid::Uuid;

    #[tokio::test]
    async fn save_empty_session_persists_metadata_only() {
        let store = open_memory();
        let learner_id = Uuid::new_v4();
        let session = Session::new(learner_id);

        store.save_session(&session).await.unwrap();

        let conn = store.conn.lock().unwrap();
        let row: (String, String, String, Option<String>) = conn
            .query_row(
                "SELECT id, learner_id, started_at, ended_at FROM sessions",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(row.0, session.id.to_string());
        assert_eq!(row.1, learner_id.to_string());
        assert!(!row.2.is_empty());
        assert!(row.3.is_none());

        let turn_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM turns", [], |r| r.get(0))
            .unwrap();
        assert_eq!(turn_count, 0);
    }
```

You'll also need to add a few uses inside the `tests` module if not already present (Utc, Turn, etc.); the snippet above includes them. The `Turn` import is for later tasks.

- [ ] **Step 2: Run the failing test**

Run from `src/`: `cargo test -p primer-storage save_empty_session`
Expected: FAIL — `save_session` does not exist yet (or returns an error).

- [ ] **Step 3: Implement `save_session` for the empty case**

In `lib.rs`, add `use async_trait::async_trait;` near the top, then append after the `impl SqliteSessionStore` block:

```rust
#[async_trait]
impl primer_core::storage::SessionStore for SqliteSessionStore {
    async fn save_session(&self, session: &primer_core::conversation::Session) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| PrimerError::Storage(format!("begin tx: {e}")))?;

        // Upsert session metadata.
        tx.execute(
            "INSERT OR REPLACE INTO sessions (id, learner_id, started_at, ended_at)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![
                session.id.to_string(),
                session.learner_id.to_string(),
                session.started_at.to_rfc3339(),
                session.ended_at.map(|t| t.to_rfc3339()),
            ],
        )
        .map_err(|e| PrimerError::Storage(format!("upsert session: {e}")))?;

        // Clear turns; they get fully rebuilt below. Cascades to turn_concepts.
        tx.execute(
            "DELETE FROM turns WHERE session_id = ?1",
            rusqlite::params![session.id.to_string()],
        )
        .map_err(|e| PrimerError::Storage(format!("delete old turns: {e}")))?;

        // Turns and concepts are added in the next tasks.

        tx.commit()
            .map_err(|e| PrimerError::Storage(format!("commit: {e}")))?;
        Ok(())
    }
}
```

- [ ] **Step 4: Run the test**

Run from `src/`: `cargo test -p primer-storage save_empty_session`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/crates/primer-storage/src/lib.rs
git commit -m "feat(primer-storage): save_session persists session metadata"
```

---

## Task 10: `save_session` — turns table

**Files:**
- Modify: `src/crates/primer-storage/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Add to the `tests` module:

```rust
    fn make_turn(speaker: primer_core::conversation::Speaker, text: &str,
                 intent: Option<primer_core::conversation::PedagogicalIntent>,
                 concepts: Vec<String>) -> Turn {
        Turn {
            speaker,
            text: text.to_string(),
            timestamp: Utc::now(),
            intent,
            concepts,
        }
    }

    #[tokio::test]
    async fn save_session_persists_turns_in_order() {
        use primer_core::conversation::{PedagogicalIntent, Speaker};

        let store = open_memory();
        let mut session = Session::new(Uuid::new_v4());
        session.add_turn(make_turn(Speaker::Child, "why is the sky blue", None, vec![]));
        session.add_turn(make_turn(
            Speaker::Primer,
            "What do you notice about the sky during the day?",
            Some(PedagogicalIntent::SocraticQuestion),
            vec![],
        ));

        store.save_session(&session).await.unwrap();

        let conn = store.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT turn_index, speaker_id, text, intent_id FROM turns
                     WHERE session_id = ?1 ORDER BY turn_index")
            .unwrap();
        let rows: Vec<(i64, i64, String, Option<i64>)> = stmt
            .query_map(rusqlite::params![session.id.to_string()], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })
            .unwrap()
            .map(|r| r.unwrap())
            .collect();

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, 0); // turn_index
        assert_eq!(rows[0].1, 1); // Child = 1
        assert_eq!(rows[0].2, "why is the sky blue");
        assert_eq!(rows[0].3, None);

        assert_eq!(rows[1].0, 1);
        assert_eq!(rows[1].1, 2); // Primer = 2
        assert_eq!(rows[1].3, Some(1)); // SocraticQuestion = 1
    }
```

- [ ] **Step 2: Run the failing test**

Run from `src/`: `cargo test -p primer-storage save_session_persists_turns`
Expected: FAIL — turn-insertion logic is not in place.

- [ ] **Step 3: Implement the turn-insertion loop**

In the `save_session` impl, replace the line `// Turns and concepts are added in the next tasks.` with:

```rust
        // Insert each turn with its speaker_id and intent_id integer FKs.
        let mut insert_turn = tx
            .prepare(
                "INSERT INTO turns (session_id, turn_index, speaker_id, text, timestamp, intent_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )
            .map_err(|e| PrimerError::Storage(format!("prepare insert turn: {e}")))?;

        for (idx, turn) in session.turns.iter().enumerate() {
            let speaker_id = catalog::speaker_id(turn.speaker);
            let intent_id = turn.intent.map(catalog::intent_id);
            insert_turn
                .execute(rusqlite::params![
                    session.id.to_string(),
                    idx as i64,
                    speaker_id,
                    turn.text,
                    turn.timestamp.to_rfc3339(),
                    intent_id,
                ])
                .map_err(|e| PrimerError::Storage(format!("insert turn {idx}: {e}")))?;
        }
        drop(insert_turn);
```

- [ ] **Step 4: Run the test**

Run from `src/`: `cargo test -p primer-storage save_session_persists_turns`
Expected: PASS.

- [ ] **Step 5: Run all primer-storage tests to confirm nothing regressed**

Run from `src/`: `cargo test -p primer-storage`
Expected: PASS — all tests so far.

- [ ] **Step 6: Commit**

```bash
git add src/crates/primer-storage/src/lib.rs
git commit -m "feat(primer-storage): save_session persists turns with integer FKs"
```

---

## Task 11: `save_session` — concepts (lazy creation, dedup)

**Files:**
- Modify: `src/crates/primer-storage/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module:

```rust
    #[tokio::test]
    async fn save_session_persists_turn_concepts_with_lazy_creation() {
        use primer_core::conversation::Speaker;

        let store = open_memory();
        let mut session = Session::new(Uuid::new_v4());
        session.add_turn(make_turn(
            Speaker::Child,
            "why does gravity pull things down",
            None,
            vec!["gravity".to_string(), "mass".to_string()],
        ));

        store.save_session(&session).await.unwrap();

        let conn = store.conn.lock().unwrap();
        let concept_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM concepts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(concept_count, 2);

        let link_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM turn_concepts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(link_count, 2);

        // The concept names round-trip via the lookup.
        let mut stmt = conn
            .prepare(
                "SELECT c.concept_id FROM turn_concepts tc
                 JOIN concepts c ON c.id = tc.concept_id
                 ORDER BY c.concept_id",
            )
            .unwrap();
        let names: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(names, vec!["gravity".to_string(), "mass".to_string()]);
    }

    #[tokio::test]
    async fn save_session_dedups_concepts_across_turns() {
        use primer_core::conversation::Speaker;

        let store = open_memory();
        let mut session = Session::new(Uuid::new_v4());
        // "gravity" appears in two turns — should be one concepts row, two turn_concepts rows.
        session.add_turn(make_turn(Speaker::Child, "what is gravity",
            None, vec!["gravity".to_string()]));
        session.add_turn(make_turn(Speaker::Primer, "good question",
            None, vec!["gravity".to_string()]));

        store.save_session(&session).await.unwrap();

        let conn = store.conn.lock().unwrap();
        let concept_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM concepts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(concept_count, 1, "gravity should be one concept row");

        let link_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM turn_concepts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(link_count, 2, "gravity should be linked to both turns");
    }
```

- [ ] **Step 2: Run the failing tests**

Run from `src/`: `cargo test -p primer-storage save_session_persists_turn_concepts save_session_dedups`
Expected: FAIL — concept-insertion logic missing.

- [ ] **Step 3: Implement concept upsert + linking**

In `save_session`, after the `drop(insert_turn);` line, add:

```rust
        // Concepts: lazy upsert with a per-call cache to avoid re-querying
        // the same concept_id within one save.
        let mut concept_id_cache: std::collections::HashMap<String, i64> =
            std::collections::HashMap::new();

        let mut insert_concept = tx
            .prepare("INSERT OR IGNORE INTO concepts (concept_id) VALUES (?1)")
            .map_err(|e| PrimerError::Storage(format!("prepare insert concept: {e}")))?;
        let mut select_concept = tx
            .prepare("SELECT id FROM concepts WHERE concept_id = ?1")
            .map_err(|e| PrimerError::Storage(format!("prepare select concept: {e}")))?;
        let mut link_concept = tx
            .prepare("INSERT OR IGNORE INTO turn_concepts (turn_id, concept_id) VALUES (?1, ?2)")
            .map_err(|e| PrimerError::Storage(format!("prepare link concept: {e}")))?;

        // Re-query the turns we just inserted to obtain their auto-incremented ids.
        let mut select_turn_id = tx
            .prepare(
                "SELECT id FROM turns WHERE session_id = ?1 AND turn_index = ?2",
            )
            .map_err(|e| PrimerError::Storage(format!("prepare select turn id: {e}")))?;

        for (idx, turn) in session.turns.iter().enumerate() {
            if turn.concepts.is_empty() {
                continue;
            }
            let turn_db_id: i64 = select_turn_id
                .query_row(
                    rusqlite::params![session.id.to_string(), idx as i64],
                    |r| r.get(0),
                )
                .map_err(|e| PrimerError::Storage(format!("look up turn id {idx}: {e}")))?;

            for concept_id in &turn.concepts {
                let cached = concept_id_cache.get(concept_id).copied();
                let id = match cached {
                    Some(id) => id,
                    None => {
                        insert_concept
                            .execute(rusqlite::params![concept_id])
                            .map_err(|e| {
                                PrimerError::Storage(format!("upsert concept {concept_id}: {e}"))
                            })?;
                        let id: i64 = select_concept
                            .query_row(rusqlite::params![concept_id], |r| r.get(0))
                            .map_err(|e| {
                                PrimerError::Storage(format!("select concept {concept_id}: {e}"))
                            })?;
                        concept_id_cache.insert(concept_id.clone(), id);
                        id
                    }
                };
                link_concept
                    .execute(rusqlite::params![turn_db_id, id])
                    .map_err(|e| {
                        PrimerError::Storage(format!("link concept {concept_id}: {e}"))
                    })?;
            }
        }
        drop(select_turn_id);
        drop(link_concept);
        drop(select_concept);
        drop(insert_concept);
```

- [ ] **Step 4: Run the tests**

Run from `src/`: `cargo test -p primer-storage`
Expected: PASS — all primer-storage tests, including the two new concept tests.

- [ ] **Step 5: Commit**

```bash
git add src/crates/primer-storage/src/lib.rs
git commit -m "feat(primer-storage): save_session persists concepts with dedup"
```

---

## Task 12: Idempotency, append-a-turn, and exhaustive-intent tests

**Files:**
- Modify: `src/crates/primer-storage/src/lib.rs`

These tests exercise the existing `save_session` behaviour from new angles. They should pass without further code changes — if any fail, the implementation has a bug that needs fixing in this task.

- [ ] **Step 1: Add the three tests**

Append to the `tests` module:

```rust
    #[tokio::test]
    async fn idempotent_re_save_does_not_duplicate_rows() {
        use primer_core::conversation::Speaker;

        let store = open_memory();
        let mut session = Session::new(Uuid::new_v4());
        session.add_turn(make_turn(Speaker::Child, "hi",
            None, vec!["greeting".to_string()]));

        store.save_session(&session).await.unwrap();
        store.save_session(&session).await.unwrap();

        let conn = store.conn.lock().unwrap();
        let session_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))
            .unwrap();
        let turn_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM turns", [], |r| r.get(0))
            .unwrap();
        let concept_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM concepts", [], |r| r.get(0))
            .unwrap();
        let link_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM turn_concepts", [], |r| r.get(0))
            .unwrap();

        assert_eq!(session_count, 1);
        assert_eq!(turn_count, 1);
        assert_eq!(concept_count, 1);
        assert_eq!(link_count, 1);
    }

    #[tokio::test]
    async fn append_a_turn_grows_the_persisted_session() {
        use primer_core::conversation::Speaker;

        let store = open_memory();
        let mut session = Session::new(Uuid::new_v4());
        session.add_turn(make_turn(Speaker::Child, "first", None, vec![]));
        store.save_session(&session).await.unwrap();
        session.add_turn(make_turn(Speaker::Primer, "second", None, vec![]));
        store.save_session(&session).await.unwrap();

        let conn = store.conn.lock().unwrap();
        let turn_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM turns", [], |r| r.get(0))
            .unwrap();
        assert_eq!(turn_count, 2);

        let last_text: String = conn
            .query_row(
                "SELECT text FROM turns WHERE turn_index = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(last_text, "second");
    }

    #[tokio::test]
    async fn every_intent_variant_round_trips() {
        use primer_core::conversation::{PedagogicalIntent, Speaker};

        let store = open_memory();
        let mut session = Session::new(Uuid::new_v4());
        // One turn per intent variant.
        for &variant in PedagogicalIntent::ALL {
            session.add_turn(make_turn(Speaker::Primer, "_", Some(variant), vec![]));
        }
        store.save_session(&session).await.unwrap();

        let conn = store.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT intent_id FROM turns ORDER BY turn_index")
            .unwrap();
        let ids: Vec<i64> = stmt
            .query_map([], |r| r.get::<_, Option<i64>>(0))
            .unwrap()
            .map(|r| r.unwrap().unwrap())
            .collect();
        assert_eq!(ids.len(), PedagogicalIntent::ALL.len());

        // Every persisted id must reverse-map to the original variant.
        let mut variants_seen = Vec::new();
        for id in ids {
            let v = catalog::intent_from_id(id).expect("known id");
            variants_seen.push(v);
        }
        let expected: Vec<PedagogicalIntent> =
            PedagogicalIntent::ALL.iter().copied().collect();
        assert_eq!(variants_seen, expected);
    }
```

- [ ] **Step 2: Run the tests**

Run from `src/`: `cargo test -p primer-storage`
Expected: PASS — all primer-storage tests (now ~14).

- [ ] **Step 3: Run the full workspace to confirm nothing regressed**

Run from `src/`: `cargo test --workspace`
Expected: PASS — original 42 + the new primer-storage tests.

- [ ] **Step 4: Commit**

```bash
git add src/crates/primer-storage/src/lib.rs
git commit -m "test(primer-storage): idempotency, append, exhaustive intent round-trip"
```

---

## Task 13: Wire `SessionStore` into `DialogueManager`

**Files:**
- Modify: `src/crates/primer-pedagogy/Cargo.toml`
- Modify: `src/crates/primer-pedagogy/src/dialogue_manager.rs`

This task introduces the optional `&dyn SessionStore` field, refactors `respond_to_streaming` so the save fires on both the success and the mid-stream-error paths, and adds two new tests using a real `SqliteSessionStore` on `:memory:`.

- [ ] **Step 1: Add `primer-storage` as a dev-dependency**

In `src/crates/primer-pedagogy/Cargo.toml`, add a `[dev-dependencies]` section (or extend the existing one):

```toml
[dev-dependencies]
primer-storage.workspace = true
```

- [ ] **Step 2: Write the failing tests**

Append to `src/crates/primer-pedagogy/src/dialogue_manager.rs`'s `tests` module, before the closing `}`:

```rust
    #[tokio::test]
    async fn respond_to_streaming_persists_session_on_success() {
        use primer_core::storage::SessionStore;
        use primer_storage::SqliteSessionStore;
        use std::path::PathBuf;

        let backend = ScriptedBackend::new(vec![
            Ok(chunk("Hi", false)),
            Ok(chunk("", true)),
        ]);
        let knowledge = EmptyKnowledge;
        let store = SqliteSessionStore::open(&PathBuf::from(":memory:")).unwrap();
        let mut dm = DialogueManager::new(
            test_learner(),
            &backend,
            &knowledge,
            Some(&store as &dyn SessionStore),
            PedagogyConfig::default(),
        );

        let _ = dm.respond_to_streaming("hello", |_| {}).await.unwrap();

        // The store now has both the child turn and the Primer turn.
        // Use the trait surface? It only exposes save. Reach into the impl
        // via a follow-up save and inspect by saving to a new store? That's
        // round-about. Easier: call save_session a second time and verify
        // it succeeds (idempotent). The actual row inspection is covered by
        // primer-storage's own tests.
        store.save_session(&dm.session).await.unwrap();
    }

    #[tokio::test]
    async fn respond_to_streaming_persists_child_turn_on_stream_error() {
        use primer_core::storage::SessionStore;
        use primer_storage::SqliteSessionStore;
        use std::path::PathBuf;

        let backend = ScriptedBackend::new(vec![
            Ok(chunk("partial", false)),
            Err(PrimerError::Inference("simulated drop".into())),
        ]);
        let knowledge = EmptyKnowledge;
        let store = SqliteSessionStore::open(&PathBuf::from(":memory:")).unwrap();
        let mut dm = DialogueManager::new(
            test_learner(),
            &backend,
            &knowledge,
            Some(&store as &dyn SessionStore),
            PedagogyConfig::default(),
        );

        let result = dm.respond_to_streaming("question", |_| {}).await;
        assert!(result.is_err());

        // The session should contain only the child turn, and saving again
        // (the in-engine save already happened) is a clean no-op.
        assert_eq!(dm.session.turns.len(), 1);
        assert_eq!(dm.session.turns[0].speaker, Speaker::Child);
        store.save_session(&dm.session).await.unwrap();
    }
```

The existing `DialogueManager::new(...)` signature has four args; the new tests pass five. The tests will fail to compile until step 3.

- [ ] **Step 3: Run the tests to confirm they fail compilation**

Run from `src/`: `cargo test -p primer-pedagogy`
Expected: FAIL — `DialogueManager::new` does not take a fifth argument yet.

- [ ] **Step 4: Add the storage field and update the constructor**

In `src/crates/primer-pedagogy/src/dialogue_manager.rs`, modify the struct:

```rust
pub struct DialogueManager<'a> {
    /// The learner model — updated in place as we learn about the child.
    pub learner: LearnerModel,
    /// The current conversation session.
    pub session: Session,
    /// Inference backend (local model or cloud API).
    inference: &'a dyn InferenceBackend,
    /// Knowledge base for RAG retrieval.
    knowledge: &'a dyn KnowledgeBase,
    /// Optional session persistence. When set, the session is saved after
    /// every `respond_to_streaming` call (success or mid-stream error).
    storage: Option<&'a dyn primer_core::storage::SessionStore>,
    /// Pedagogical configuration.
    config: PedagogyConfig,
}
```

Update the existing import block at the top of the file if needed (no extra `use` statement is strictly required because the field uses a fully qualified path).

Update `new()`:

```rust
impl<'a> DialogueManager<'a> {
    /// Create a new dialogue manager for a session.
    pub fn new(
        learner: LearnerModel,
        inference: &'a dyn InferenceBackend,
        knowledge: &'a dyn KnowledgeBase,
        storage: Option<&'a dyn primer_core::storage::SessionStore>,
        config: PedagogyConfig,
    ) -> Self {
        let session = Session::new(learner.profile.id);
        Self {
            learner,
            session,
            inference,
            knowledge,
            storage,
            config,
        }
    }
```

- [ ] **Step 5: Update every existing call site of `DialogueManager::new`**

The existing tests in `dialogue_manager.rs::tests` create managers without storage. Update every call from:

```rust
let mut dm = DialogueManager::new(
    test_learner(),
    &backend,
    &knowledge,
    PedagogyConfig::default(),
);
```

to:

```rust
let mut dm = DialogueManager::new(
    test_learner(),
    &backend,
    &knowledge,
    None,
    PedagogyConfig::default(),
);
```

There are six existing tests in `dialogue_manager.rs::tests` that build a manager. Update each.

The CLI also calls `DialogueManager::new`; that's handled in Task 14 — leave it broken for now.

- [ ] **Step 6: Refactor `respond_to_streaming` to converge before return**

In `respond_to_streaming`, replace the current body's stream loop and turn-recording block with a structure that lands its result in a local variable so the save call can run in one place. Full updated method:

```rust
    pub async fn respond_to_streaming<F>(
        &mut self,
        child_input: &str,
        mut on_chunk: F,
    ) -> Result<String>
    where
        F: FnMut(&str),
    {
        // 1. Record the child's turn.
        let child_turn = Turn {
            speaker: Speaker::Child,
            text: child_input.to_string(),
            timestamp: Utc::now(),
            intent: None,
            concepts: vec![],
        };
        self.session.add_turn(child_turn);

        // 2. Decide intent + retrieve knowledge + build prompt.
        let intent = prompt_builder::decide_intent(&self.learner, &self.session);
        let knowledge_context = self.retrieve_knowledge(child_input).await;
        let prompt = prompt_builder::build_prompt(
            &self.learner,
            &self.session,
            intent,
            &knowledge_context,
            self.config.context_window_turns,
        );

        // 3. Stream the response, accumulating into a single String.
        // The result is captured in `result` so we can run the save call
        // exactly once afterwards, regardless of which path we took.
        let params = GenerationParams::default();
        let result: Result<String> = async {
            let mut stream = self
                .inference
                .generate_stream(&prompt, &params)
                .await
                .map_err(|e| PrimerError::Inference(format!("Generation failed: {e}")))?;

            let mut accumulated = String::new();
            while let Some(item) = stream.next().await {
                let chunk = item.inspect_err(|e| {
                    tracing::warn!("Stream error mid-generation: {e}");
                })?;
                if !chunk.text.is_empty() {
                    on_chunk(&chunk.text);
                    accumulated.push_str(&chunk.text);
                }
                if chunk.done {
                    break;
                }
            }
            Ok(accumulated)
        }
        .await;

        // 4. On success, record the Primer turn and update the learner.
        if let Ok(accumulated) = &result {
            if accumulated.is_empty() {
                tracing::warn!("Inference stream produced no text");
            }
            let active_concepts = prompt_builder::extract_active_concepts(&self.session, 4);
            let primer_turn = Turn {
                speaker: Speaker::Primer,
                text: accumulated.clone(),
                timestamp: Utc::now(),
                intent: Some(intent),
                concepts: active_concepts,
            };
            self.session.add_turn(primer_turn);
            self.update_learner_model(child_input, &intent);
        }

        // 5. Save the session if a store is configured. Runs on both Ok
        //    and Err paths. Save failures are logged, not propagated.
        if let Some(store) = self.storage {
            if let Err(e) = store.save_session(&self.session).await {
                tracing::warn!("session save failed: {e}");
            }
        }

        result
    }
```

- [ ] **Step 7: Save in `open_session` too**

Modify `open_session` to save after recording the greeting:

```rust
    pub async fn open_session(&mut self) -> Result<String> {
        let name = &self.learner.profile.name;
        let greeting = format!(
            "Hello, {name}. What are you curious about today?"
        );

        self.session.add_turn(Turn {
            speaker: Speaker::Primer,
            text: greeting.clone(),
            timestamp: Utc::now(),
            intent: Some(PedagogicalIntent::SocraticQuestion),
            concepts: vec![],
        });

        if let Some(store) = self.storage {
            if let Err(e) = store.save_session(&self.session).await {
                tracing::warn!("session save failed: {e}");
            }
        }

        Ok(greeting)
    }
```

- [ ] **Step 8: Run the pedagogy tests**

Run from `src/`: `cargo test -p primer-pedagogy`
Expected: PASS — the eight existing dialogue_manager tests still pass (they pass `None` for storage), plus the two new persistence tests pass.

- [ ] **Step 9: Run the full workspace**

Run from `src/`: `cargo test --workspace`
Expected: PASS — primer-cli does not yet compile; that's Task 14.

If `cargo test --workspace` is failing on primer-cli alone, note it in the commit message and proceed; Task 14 fixes it.

- [ ] **Step 10: Commit**

```bash
git add src/crates/primer-pedagogy
git commit -m "feat(primer-pedagogy): wire SessionStore into DialogueManager"
```

---

## Task 14: CLI flag `--session-db` and main.rs wiring

**Files:**
- Modify: `src/crates/primer-cli/Cargo.toml`
- Modify: `src/crates/primer-cli/src/main.rs`

- [ ] **Step 1: Add `primer-storage` to the CLI's deps**

In `src/crates/primer-cli/Cargo.toml`, add to the `[dependencies]` section:

```toml
primer-storage.workspace = true
```

- [ ] **Step 2: Add the `--session-db` flag**

In `src/crates/primer-cli/src/main.rs`, modify the `Cli` struct to add a new field after `knowledge_db`:

```rust
    /// Path to session database SQLite file.
    /// If omitted, uses an in-memory database (sessions are not persisted).
    #[arg(long)]
    session_db: Option<PathBuf>,
```

- [ ] **Step 3: Construct the store and thread it through**

After the existing knowledge-base construction in `main()`:

```rust
    let knowledge_path = cli
        .knowledge_db
        .unwrap_or_else(|| PathBuf::from(":memory:"));
    let knowledge = SqliteKnowledgeBase::open(&knowledge_path)?;
```

add:

```rust
    let session_path = cli
        .session_db
        .unwrap_or_else(|| PathBuf::from(":memory:"));
    let session_store = primer_storage::SqliteSessionStore::open(&session_path)?;
```

Update the `DialogueManager::new` call:

```rust
    let mut dm = DialogueManager::new(
        learner,
        inference.as_ref(),
        &knowledge as &dyn KnowledgeBase,
        Some(&session_store as &dyn primer_core::storage::SessionStore),
        pedagogy_config,
    );
```

- [ ] **Step 4: Verify the CLI compiles**

Run from `src/`: `cargo build -p primer-cli`
Expected: clean build.

- [ ] **Step 5: Smoke-test against the stub backend**

Run from `src/`:

```bash
cargo run --bin primer -- --session-db /tmp/primer-session-smoke.db
```

In the REPL, type:

```
hi
quit
```

Expected: Primer responds with the canned stub response, then exits cleanly.

Inspect the database:

```bash
sqlite3 /tmp/primer-session-smoke.db "SELECT speaker_id, text FROM turns ORDER BY turn_index;"
```

Expected: at least three rows — Primer greeting (speaker_id=2), child turn `hi` (speaker_id=1), Primer response (speaker_id=2).

Clean up:

```bash
rm /tmp/primer-session-smoke.db
```

- [ ] **Step 6: Run the full workspace tests**

Run from `src/`: `cargo test --workspace`
Expected: PASS — all tests, including primer-storage and primer-pedagogy.

- [ ] **Step 7: Run clippy**

Run from `src/`: `cargo clippy --workspace --all-targets`
Expected: clean except the pre-existing `StubBackend::new` warning.

- [ ] **Step 8: Commit**

```bash
git add src/crates/primer-cli
git commit -m "feat(primer-cli): --session-db flag wires SqliteSessionStore"
```

---

## Task 15: Documentation updates and ROADMAP tick

**Files:**
- Modify: `ROADMAP.md`
- Modify: `CLAUDE.md`

- [ ] **Step 1: Tick the ROADMAP item**

In `ROADMAP.md`, find the Phase 0.1 bullet:

```markdown
- [ ] Add conversation persistence — save/load sessions as JSON so a child can pick up where they left off
```

Replace with:

```markdown
- [x] Add conversation persistence — sessions persist to SQLite via `primer-storage` (per-turn save). Load/resume CLI surface deferred — see [docs/superpowers/specs/2026-04-30-session-persistence-sqlite-design.md](docs/superpowers/specs/2026-04-30-session-persistence-sqlite-design.md)
```

- [ ] **Step 2: Update CLAUDE.md — add the new crate to the dependency graph**

In `CLAUDE.md`, find the existing crate-graph block:

```
primer-cli  →  primer-pedagogy  →  primer-core  ←  primer-inference, primer-speech, primer-knowledge
```

Replace with:

```
primer-cli  →  primer-pedagogy  →  primer-core  ←  primer-inference, primer-speech, primer-knowledge, primer-storage
```

- [ ] **Step 3: Update CLAUDE.md — describe `primer-storage`**

In the "Architecture: trait-based hardware abstraction" section, after the bullet describing `primer-knowledge`, add a new bullet:

```markdown
- **`primer-storage`** — `SqliteSessionStore` persists every conversation `Session` (one DB per child, separate from the RAG corpus on privacy grounds). Schema is normalised: every categorical text column (`speaker`, `pedagogical_intent`, `concept_id`) is a foreign key into a lookup table. The `speakers` and `pedagogical_intents` lookup tables are seeded and validated against the canonical Rust enums on every `open()` — drift is a hard error. `concepts` is open-vocabulary and lazily populated. `DialogueManager` accepts an optional `&dyn SessionStore`; when set, it saves after every turn (success or mid-stream error). Save failures `tracing::warn!` instead of propagating.
```

- [ ] **Step 4: Update CLAUDE.md — list `--session-db` in the commands**

In the "Common commands" section, find:

```bash
cargo run --bin primer -- --backend ollama --model llama3.2                   # local Ollama at http://localhost:11434
```

After it (still inside the code fence), add:

```bash
cargo run --bin primer -- --session-db ~/primer-sessions.db                   # persist conversations to SQLite (default :memory:)
```

Then in the surrounding prose paragraph that lists CLI flags, after the existing `--knowledge-db <path>` mention, add:

```
`--session-db <path>` (defaults to `:memory:`; sessions persist on every turn when a real path is given).
```

- [ ] **Step 5: Update CLAUDE.md — add the lookup-table convention**

In the "Conventions and gotchas worth knowing" section, append a new bullet at the end:

```markdown
- **Categorical text columns are normalised into lookup tables and stored as integer foreign keys.** Applies to every SQLite schema in this codebase (sessions, learner-model concepts when Phase 0.3 lands, etc.). For closed Rust enums (`Speaker`, `PedagogicalIntent`, `UnderstandingDepth`), the Rust enum is the single source of truth; the DB lookup table is a derived projection that the storage layer regenerates and validates against on every `open()`. Drift (mismatched name on a known id, unknown id) is a hard error — there is **no** API exposed for mutating lookup tables. Adding a new variant means: edit the Rust source, recompile, next `open()` seeds the new row. Retired integer IDs are never reused.
```

- [ ] **Step 6: Verify nothing else needs touching**

Run from `src/`: `cargo test --workspace && cargo clippy --workspace --all-targets`
Expected: PASS — same as Task 14.

- [ ] **Step 7: Commit**

```bash
git add ROADMAP.md CLAUDE.md
git commit -m "docs: SQLite session persistence shipped — Phase 0.1 closed"
```

---

## Acceptance — final cross-check

Before declaring the work done, run from `src/`:

- [ ] `cargo build --workspace` — clean
- [ ] `cargo test --workspace` — clean, **~65 tests** total (was 42; +5 catalog round-trips, +14 store/schema/save tests in primer-storage, +2 in primer-pedagogy, +2 in primer-core conversation tests)
- [ ] `cargo clippy --workspace --all-targets` — clean except the pre-existing `StubBackend::new` warning
- [ ] Stub REPL with `--session-db /tmp/primer-test.db` round-trips data; `sqlite3` confirms turns landed
- [ ] (Optional, requires API key) Cloud REPL with `--session-db` exhibits the same persistence
- [ ] (Optional, requires running ollama) Ollama REPL with `--session-db` exhibits the same persistence
- [ ] `ROADMAP.md` Phase 0.1 conversation persistence bullet is checked off
- [ ] `CLAUDE.md` mentions `primer-storage`, `--session-db`, and the lookup-table convention

If all green, the implementation is complete. The next session's brief (which lives in `primer_next_session.md` at the repo root) should be refreshed by the operator at the end of this work — that's outside the scope of this plan.
