# Learner-model SQLite persistence — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Persist `LearnerModel` across sessions via a new `LearnerStore` trait + v3 → v4 SQLite migration, and close the documented in-memory-vs-on-disk `learner_id` divergence by adopting an existing session's `learner_id` on first-run after upgrade.

**Architecture:** Schema-additive migration (`apply_v4_migrations` follows the v3 template — idempotent, transactional, schema-only). The migration creates `learners`, `learner_concepts`, and the `understanding_depths` lookup. Adoption of orphan sessions is a CLI-level concern mediated by one new `SessionStore::most_recent_session_learner_id()` method. `LearnerStore` is a new trait alongside `SessionStore` in `primer-core::storage`; both are implemented by the same `SqliteSessionStore` struct.

**Tech Stack:** Rust 2024, `rusqlite` (bundled SQLite), `async-trait`, `serde_json` for the three `Vec<String>` JSON-in-TEXT fields, `tracing` for warnings.

**Spec:** [docs/superpowers/specs/2026-05-02-learner-model-persistence-design.md](docs/superpowers/specs/2026-05-02-learner-model-persistence-design.md)

---

## File structure

All changes are additive within existing files; no new files are created.

**Modified:**
- `src/crates/primer-core/src/learner.rs` — add `UnderstandingDepth::ALL` constant and one test.
- `src/crates/primer-core/src/storage.rs` — add `LearnerStore` trait; add `most_recent_session_learner_id` to `SessionStore`.
- `src/crates/primer-core/src/lib.rs` — re-export `LearnerStore` (mirror existing `SessionStore` re-export).
- `src/crates/primer-storage/Cargo.toml` — add `serde_json` workspace dep.
- `src/crates/primer-storage/src/schema.rs` — add v4 schema constants and `apply_v4_migrations`; bump `USER_VERSION` to 4.
- `src/crates/primer-storage/src/catalog.rs` — add `understanding_depth_*` functions and `expected_understanding_depths`.
- `src/crates/primer-storage/src/lib.rs` — wire v4 migration + `validate_and_seed_lookup` for `understanding_depths` into `open()`; implement `LearnerStore` and `most_recent_session_learner_id` on `SqliteSessionStore`.
- `src/crates/primer-pedagogy/src/dialogue_manager.rs` — add `learner_store: Option<Arc<dyn LearnerStore>>` field; save at the five save points.
- `src/crates/primer-cli/src/main.rs` — add `create_learner_with_id` helper; add adoption flow; tests for birthday + name mismatch.
- `CLAUDE.md` — bump schema mention to v4; mention `LearnerStore`; remove the "in-memory `LearnerModel` vs `Session.learner_id` divergence" gotcha.
- `ROADMAP.md` — tick the Phase 0.3 "learner model persistence (SQLite)" bullet.

All tests live inline as `#[cfg(test)] mod tests` blocks alongside the code they test (matching existing convention).

**Branching:** before starting Task 1, `git switch -c feature/learner-model-persistence` from `main` (which carries the spec commit `4e68e16`). Don't push until the user asks.

---

## Task 1 — `UnderstandingDepth::ALL` constant

**Files:**
- Modify: `src/crates/primer-core/src/learner.rs`

The catalog functions in Task 2 will iterate `UnderstandingDepth::ALL`. This task adds it. Mirror the existing `EngagementState::ALL` pattern at [learner.rs:155-179](src/crates/primer-core/src/learner.rs#L155).

- [ ] **Step 1: Write the failing test**

Append to the existing `#[cfg(test)] mod tests` block at the bottom of `learner.rs` (the one with `engagement_state_all_lists_every_variant`):

```rust
#[test]
fn understanding_depth_all_lists_every_variant() {
    assert_eq!(UnderstandingDepth::ALL.len(), 6);
    assert!(UnderstandingDepth::ALL.contains(&UnderstandingDepth::Unknown));
    assert!(UnderstandingDepth::ALL.contains(&UnderstandingDepth::Aware));
    assert!(UnderstandingDepth::ALL.contains(&UnderstandingDepth::Recall));
    assert!(UnderstandingDepth::ALL.contains(&UnderstandingDepth::Comprehension));
    assert!(UnderstandingDepth::ALL.contains(&UnderstandingDepth::Application));
    assert!(UnderstandingDepth::ALL.contains(&UnderstandingDepth::Analysis));
}
```

- [ ] **Step 2: Run test to verify it fails**

From `src/`:

```bash
cargo test -p primer-core understanding_depth_all_lists_every_variant
```

Expected: FAIL with `error[E0599]: no associated item named ALL found for enum UnderstandingDepth`.

- [ ] **Step 3: Add the `ALL` constant**

Add an `impl UnderstandingDepth` block right after the `UnderstandingDepth` enum definition (currently at [learner.rs:48-62](src/crates/primer-core/src/learner.rs#L48)):

```rust
impl UnderstandingDepth {
    pub const ALL: &'static [Self] = &[
        Self::Unknown,
        Self::Aware,
        Self::Recall,
        Self::Comprehension,
        Self::Application,
        Self::Analysis,
    ];
}
```

- [ ] **Step 4: Run test to verify it passes**

```bash
cargo test -p primer-core understanding_depth_all_lists_every_variant
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/crates/primer-core/src/learner.rs
git commit -m "$(cat <<'EOF'
feat(core): add UnderstandingDepth::ALL constant

Used by primer-storage::catalog to project the closed enum onto its
on-disk integer IDs. Mirrors the existing EngagementState::ALL pattern.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2 — Catalog functions for `understanding_depths`

**Files:**
- Modify: `src/crates/primer-storage/src/catalog.rs`

Mirror the existing `engagement_state_*` block at [catalog.rs:90-134](src/crates/primer-storage/src/catalog.rs#L90).

- [ ] **Step 1: Write the failing test**

Append to the existing `#[cfg(test)] mod engagement_state_tests` block (or add a new `mod understanding_depth_tests` module right after it):

```rust
#[cfg(test)]
mod understanding_depth_tests {
    use super::*;
    use primer_core::learner::UnderstandingDepth;

    #[test]
    fn understanding_depth_id_is_stable_for_every_variant() {
        assert_eq!(understanding_depth_id(UnderstandingDepth::Unknown), 1);
        assert_eq!(understanding_depth_id(UnderstandingDepth::Aware), 2);
        assert_eq!(understanding_depth_id(UnderstandingDepth::Recall), 3);
        assert_eq!(understanding_depth_id(UnderstandingDepth::Comprehension), 4);
        assert_eq!(understanding_depth_id(UnderstandingDepth::Application), 5);
        assert_eq!(understanding_depth_id(UnderstandingDepth::Analysis), 6);
    }

    #[test]
    fn understanding_depth_from_id_is_inverse_of_id() {
        for d in UnderstandingDepth::ALL {
            let id = understanding_depth_id(*d);
            assert_eq!(understanding_depth_from_id(id), Some(*d));
        }
    }

    #[test]
    fn understanding_depth_from_id_returns_none_on_unknown_id() {
        assert!(understanding_depth_from_id(999).is_none());
    }

    #[test]
    fn expected_understanding_depths_covers_all_variants() {
        let pairs = expected_understanding_depths();
        assert_eq!(pairs.len(), UnderstandingDepth::ALL.len());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p primer-storage understanding_depth_id_is_stable
```

Expected: FAIL with `error[E0425]: cannot find function understanding_depth_id`.

- [ ] **Step 3: Add the catalog functions**

Add to `catalog.rs`, right after the `expected_engagement_states()` function (around line 134):

```rust
/// Stable on-disk integer ID for every `UnderstandingDepth` variant.
///
/// Explicit match arms so reordering variants in `primer-core`
/// cannot silently shift IDs already on disk.
pub fn understanding_depth_id(depth: UnderstandingDepth) -> i64 {
    match depth {
        UnderstandingDepth::Unknown       => 1,
        UnderstandingDepth::Aware         => 2,
        UnderstandingDepth::Recall        => 3,
        UnderstandingDepth::Comprehension => 4,
        UnderstandingDepth::Application   => 5,
        UnderstandingDepth::Analysis      => 6,
    }
}

/// Human-readable name stored alongside the ID in the
/// `understanding_depths` lookup table.
pub fn understanding_depth_name(depth: UnderstandingDepth) -> &'static str {
    match depth {
        UnderstandingDepth::Unknown       => "Unknown",
        UnderstandingDepth::Aware         => "Aware",
        UnderstandingDepth::Recall        => "Recall",
        UnderstandingDepth::Comprehension => "Comprehension",
        UnderstandingDepth::Application   => "Application",
        UnderstandingDepth::Analysis      => "Analysis",
    }
}

/// Reverse lookup: integer ID → `UnderstandingDepth`. Returns `None`
/// for IDs the current build doesn't know about.
pub fn understanding_depth_from_id(id: i64) -> Option<UnderstandingDepth> {
    UnderstandingDepth::ALL
        .iter()
        .copied()
        .find(|v| understanding_depth_id(*v) == id)
}

/// All `(id, name)` pairs the storage layer expects to see in the
/// `understanding_depths` lookup table. Used by the validate-and-seed pass.
pub fn expected_understanding_depths() -> Vec<(i64, &'static str)> {
    UnderstandingDepth::ALL
        .iter()
        .map(|d| (understanding_depth_id(*d), understanding_depth_name(*d)))
        .collect()
}
```

Add `UnderstandingDepth` to the `use primer_core::learner::EngagementState;` import at the top of `catalog.rs`:

```rust
use primer_core::learner::{EngagementState, UnderstandingDepth};
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test -p primer-storage understanding_depth
```

Expected: PASS (all 4 tests in the new module).

- [ ] **Step 5: Run the whole crate's tests to confirm no regressions**

```bash
cargo test -p primer-storage
```

Expected: previous test count (60) + 4 new = 64 passing.

- [ ] **Step 6: Commit**

```bash
git add src/crates/primer-storage/src/catalog.rs
git commit -m "$(cat <<'EOF'
feat(storage): catalog functions for understanding_depths lookup

Adds understanding_depth_id / _name / _from_id / expected_understanding_depths
mirroring the engagement_state_* pattern. Will be wired into the v4
schema's validate-and-seed pass next.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3 — v4 schema constants + `apply_v4_migrations`

**Files:**
- Modify: `src/crates/primer-storage/src/schema.rs`

Add the v4 schema strings and the `apply_v4_migrations` function. Bump `USER_VERSION` to 4. The migration is schema-only; it does NOT insert into `learners` (per spec).

- [ ] **Step 1: Write the failing tests**

Append to the bottom of `schema.rs` (or create a new `#[cfg(test)] mod v4_tests` block at the end of the file):

```rust
#[cfg(test)]
mod v4_tests {
    use super::*;
    use crate::catalog::expected_understanding_depths;
    use rusqlite::Connection;

    fn fresh_v3_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(SCHEMA_SQL).unwrap();
        apply_v2_migrations(&conn).unwrap();
        apply_v3_migrations(&conn).unwrap();
        conn
    }

    fn table_exists(conn: &Connection, name: &str) -> bool {
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                rusqlite::params![name],
                |r| r.get(0),
            )
            .unwrap();
        count > 0
    }

    #[test]
    fn apply_v4_migrations_creates_three_tables() {
        let conn = fresh_v3_conn();
        apply_v4_migrations(&conn).unwrap();
        assert!(table_exists(&conn, "understanding_depths"));
        assert!(table_exists(&conn, "learners"));
        assert!(table_exists(&conn, "learner_concepts"));
    }

    #[test]
    fn apply_v4_migrations_is_idempotent() {
        let conn = fresh_v3_conn();
        apply_v4_migrations(&conn).unwrap();
        apply_v4_migrations(&conn).unwrap(); // second call must not fail or duplicate
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='learners'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn apply_v4_migrations_does_not_insert_learners_row() {
        let conn = fresh_v3_conn();
        apply_v4_migrations(&conn).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM learners", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0, "v4 migration must not insert any learners rows");
    }

    #[test]
    fn understanding_depths_is_seeded_after_v4_with_validate_and_seed() {
        let conn = fresh_v3_conn();
        apply_v4_migrations(&conn).unwrap();
        validate_and_seed_lookup(
            &conn,
            "understanding_depths",
            &expected_understanding_depths(),
        )
        .unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM understanding_depths", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 6);
    }

    #[test]
    fn user_version_constant_is_four() {
        assert_eq!(USER_VERSION, 4);
    }

    #[test]
    fn apply_v4_migrations_rolls_back_on_failure() {
        // Fault injection: drop `engagement_states` so the v4 transaction's
        // FK references in `learners.current_engagement_state_id` can still
        // be satisfied, but break the migration by pre-creating an
        // `understanding_depths` table with a conflicting schema. The
        // CREATE TABLE IF NOT EXISTS would no-op, but a CREATE INDEX with
        // a name collision against an existing object will fail.
        //
        // Simpler approach: pre-create a TABLE named `learner_concepts`
        // with a different shape so CREATE TABLE IF NOT EXISTS skips it,
        // then verify that's fine. Reverse: pre-create an unrelated TABLE
        // with the same name as the v4 index, forcing the CREATE INDEX
        // step to fail.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(SCHEMA_SQL).unwrap();
        apply_v2_migrations(&conn).unwrap();
        apply_v3_migrations(&conn).unwrap();
        // Pre-create a TABLE colliding with the index name.
        conn.execute_batch(
            "CREATE TABLE idx_learner_concepts_learner (id INTEGER PRIMARY KEY);",
        )
        .unwrap();

        let result = apply_v4_migrations(&conn);
        assert!(result.is_err(), "expected migration to fail");

        // The transaction must have rolled back: `learners` and
        // `learner_concepts` and `understanding_depths` must not exist.
        assert!(!table_exists(&conn, "learners"));
        assert!(!table_exists(&conn, "learner_concepts"));
        assert!(!table_exists(&conn, "understanding_depths"));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p primer-storage v4_tests
```

Expected: FAIL — `apply_v4_migrations` is undefined and `USER_VERSION` is 3.

- [ ] **Step 3: Bump `USER_VERSION` and add v4 schema constants + migration function**

In `schema.rs`, change line 20:

```rust
pub const USER_VERSION: i64 = 4;
```

Update the doc-comment to mention v4:

```rust
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
pub const USER_VERSION: i64 = 4;
```

Add v4 schema strings + migration function at the end of `schema.rs`, right after `apply_v3_migrations`:

```rust
// ─── v4 schema strings ──────────────────────────────────────────────────────

const CREATE_UNDERSTANDING_DEPTHS_TABLE: &str = "
    CREATE TABLE IF NOT EXISTS understanding_depths (
        id   INTEGER PRIMARY KEY,
        name TEXT NOT NULL UNIQUE
    )
";

const CREATE_LEARNERS_TABLE: &str = "
    CREATE TABLE IF NOT EXISTS learners (
        id                          TEXT PRIMARY KEY,
        name                        TEXT NOT NULL,
        age                         INTEGER NOT NULL,
        languages                   TEXT NOT NULL,
        created_at                  TEXT NOT NULL,
        last_active                 TEXT NOT NULL,
        pref_narrative              REAL NOT NULL,
        pref_socratic               REAL NOT NULL,
        pref_visual                 REAL NOT NULL,
        pref_kinesthetic            REAL NOT NULL,
        typical_session_minutes     REAL NOT NULL,
        high_engagement_topics      TEXT NOT NULL,
        early_disengagement_secs    INTEGER NOT NULL,
        current_engagement_state_id INTEGER NOT NULL REFERENCES engagement_states(id)
    )
";

const CREATE_LEARNER_CONCEPTS_TABLE: &str = "
    CREATE TABLE IF NOT EXISTS learner_concepts (
        learner_id        TEXT NOT NULL REFERENCES learners(id) ON DELETE CASCADE,
        concept_id        INTEGER NOT NULL REFERENCES concepts(id),
        depth_id          INTEGER NOT NULL REFERENCES understanding_depths(id),
        confidence        REAL NOT NULL,
        encounter_count   INTEGER NOT NULL,
        last_encountered  TEXT,
        notes             TEXT NOT NULL DEFAULT '[]',
        PRIMARY KEY (learner_id, concept_id)
    )
";

const CREATE_LEARNER_CONCEPTS_INDEX: &str = "
    CREATE INDEX IF NOT EXISTS idx_learner_concepts_learner
        ON learner_concepts(learner_id)
";

/// Apply v4 migrations idempotently. Safe to run on a fresh DB (after
/// v3 objects exist), on a v3 DB being upgraded, and on a v4 DB being
/// re-opened.
///
/// All steps run inside a single transaction so a partial failure rolls
/// back to the pre-migration state.
///
/// v4 adds:
/// - `understanding_depths` lookup table (seeded by the validate pass
///   in `open()` after this migration runs).
/// - `learners` table — one row per learner DB file (application-level
///   invariant), holds profile + preferences + engagement snapshot.
/// - `learner_concepts` junction table for per-learner concept-mastery
///   state, FK'd into `learners`, `concepts`, and `understanding_depths`.
/// - `idx_learner_concepts_learner` index on the junction table.
///
/// Schema-only. Adopting an existing session's `learner_id` for the new
/// learners row is the CLI's responsibility — `apply_v4_migrations` runs
/// without CLI flag access and cannot populate `name` / `age`.
pub(crate) fn apply_v4_migrations(conn: &Connection) -> Result<()> {
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| PrimerError::Storage(format!("v4 migration: failed to begin tx: {e}")))?;
    tx.execute(CREATE_UNDERSTANDING_DEPTHS_TABLE, [])
        .map_err(|e| PrimerError::Storage(format!("v4 migration: understanding_depths: {e}")))?;
    tx.execute(CREATE_LEARNERS_TABLE, [])
        .map_err(|e| PrimerError::Storage(format!("v4 migration: learners: {e}")))?;
    tx.execute(CREATE_LEARNER_CONCEPTS_TABLE, [])
        .map_err(|e| PrimerError::Storage(format!("v4 migration: learner_concepts: {e}")))?;
    tx.execute(CREATE_LEARNER_CONCEPTS_INDEX, [])
        .map_err(|e| PrimerError::Storage(format!("v4 migration: index: {e}")))?;
    tx.commit()
        .map_err(|e| PrimerError::Storage(format!("v4 migration: commit: {e}")))?;
    Ok(())
}
```

- [ ] **Step 4: Run the new tests to verify they pass**

```bash
cargo test -p primer-storage v4_tests
```

Expected: PASS (all 6 tests).

- [ ] **Step 5: Run the whole storage crate**

```bash
cargo test -p primer-storage
```

Expected: every previous test still passes; the rollback test for v2 and v3 still works. The v3 idempotency tests may need updating because of the bumped `USER_VERSION` — see Step 6 if they fail.

- [ ] **Step 6: Fix any v3-era tests that asserted `USER_VERSION == 3`**

Search for `assert_eq!(.*USER_VERSION.*3)` patterns:

```bash
grep -rn "USER_VERSION" src/crates/primer-storage/src/
```

Update any tests that hard-coded `3` to instead read `schema::USER_VERSION` (or update the literal to `4` if it's a hard-coded check). Most are already written as `assert_eq!(v, schema::USER_VERSION)` per the patterns at lib.rs:657 and lib.rs:723 — those should keep working.

After fixing, re-run `cargo test -p primer-storage` until clean.

- [ ] **Step 7: Commit**

```bash
git add src/crates/primer-storage/src/schema.rs
git commit -m "$(cat <<'EOF'
feat(storage): v4 schema migration adds learners, learner_concepts, understanding_depths

Schema-only — adoption of an existing session's learner_id into a new
learners row is the CLI's responsibility (apply_v4_migrations runs inside
SqliteSessionStore::open() with no CLI flag access). Migration is
idempotent, transactional, and rolls back on partial failure.

Bumps USER_VERSION 3 → 4.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4 — Wire v4 migration into `SqliteSessionStore::open()`

**Files:**
- Modify: `src/crates/primer-storage/src/lib.rs`

The `open()` function already wires v2 and v3 migrations + their lookup-table seeding. Extend the same pattern for v4.

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` in `lib.rs` (or wherever the existing `open_*` tests live):

```rust
#[tokio::test]
async fn open_fresh_db_creates_v4_schema_with_understanding_depths_seeded() {
    use rusqlite::Connection;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fresh.db");
    let _store = SqliteSessionStore::open(&path).unwrap();

    // Inspect the file directly via a raw rusqlite connection.
    let conn = Connection::open(&path).unwrap();
    let v: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v, schema::USER_VERSION);

    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM understanding_depths", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 6, "expected all 6 UnderstandingDepth variants seeded");

    let learners: i64 = conn
        .query_row("SELECT COUNT(*) FROM learners", [], |r| r.get(0))
        .unwrap();
    assert_eq!(learners, 0, "open() must not insert any learners row");
}

#[tokio::test]
async fn fk_enforcement_rejects_unknown_depth_id_in_learner_concepts() {
    // Proves PRAGMA foreign_keys = ON covers the v4 tables too.
    // We cannot exercise this through save_learner (which only ever uses
    // valid depth_ids from the catalog), so reach for the underlying
    // connection and try to insert a row with depth_id = 99.
    let store = SqliteSessionStore::open(std::path::Path::new(":memory:")).unwrap();

    // Pre-seed: a learners row + a concepts row, so the only FK that can
    // fail is depth_id.
    {
        let conn = store.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO learners (
                 id, name, age, languages, created_at, last_active,
                 pref_narrative, pref_socratic, pref_visual, pref_kinesthetic,
                 typical_session_minutes, high_engagement_topics,
                 early_disengagement_secs, current_engagement_state_id
             ) VALUES (?1, 'Test', 8, '[]', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z',
                       0.5, 0.5, 0.5, 0.5, 20.0, '[]', 300, 1)",
            rusqlite::params!["00000000-0000-0000-0000-000000000001"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO concepts (name) VALUES ('test:concept')",
            [],
        )
        .unwrap();

        let result = conn.execute(
            "INSERT INTO learner_concepts (
                 learner_id, concept_id, depth_id, confidence,
                 encounter_count, last_encountered, notes
             ) VALUES ('00000000-0000-0000-0000-000000000001',
                       (SELECT id FROM concepts WHERE name = 'test:concept'),
                       99, 0.5, 1, NULL, '[]')",
            [],
        );
        assert!(result.is_err(), "expected FK constraint failure on depth_id = 99");
    }
}
```

(Note: `tempfile` may need adding as a `dev-dependencies` line in `primer-storage/Cargo.toml`. Check with `grep tempfile src/crates/primer-storage/Cargo.toml`. If missing, add `tempfile.workspace = true` under `[dev-dependencies]` and ensure it's pinned in `src/Cargo.toml` workspace deps.)

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p primer-storage open_fresh_db_creates_v4_schema_with_understanding_depths_seeded
```

Expected: FAIL — `apply_v4_migrations` not called from `open()`, `understanding_depths` table does not exist or is unseeded.

- [ ] **Step 3: Wire v4 into `open()`**

In `lib.rs`, locate the `open()` method (around line 48). After `schema::apply_v3_migrations(&conn)?;`, add:

```rust
        // v4 migrations: idempotent on every open. Adds understanding_depths,
        // learners, and learner_concepts tables (schema-only — adoption of
        // existing-session learner_id is the CLI's job).
        schema::apply_v4_migrations(&conn)?;
```

After the existing two `validate_and_seed_lookup` calls for `speakers` / `pedagogical_intents` / `engagement_states`, add a fourth:

```rust
        let understanding_depths = catalog::expected_understanding_depths();
        schema::validate_and_seed_lookup(&conn, "understanding_depths", &understanding_depths)?;
```

The complete updated `open()` body around the migration block should look like:

```rust
        conn.execute_batch(schema::SCHEMA_SQL)
            .map_err(|e| PrimerError::Storage(format!("schema creation failed: {e}")))?;

        // v2 migrations: idempotent on every open. Adds summary columns
        // and the FTS5 turn-text index if not already present.
        schema::apply_v2_migrations(&conn)?;

        // v3 migrations: idempotent on every open. Adds engagement_states,
        // classifiers, and turn_classifications tables.
        schema::apply_v3_migrations(&conn)?;

        // v4 migrations: idempotent on every open. Adds understanding_depths,
        // learners, and learner_concepts tables (schema-only — adoption of
        // existing-session learner_id is the CLI's job).
        schema::apply_v4_migrations(&conn)?;

        if existing_version != schema::USER_VERSION {
            conn.execute_batch(&format!("PRAGMA user_version = {};", schema::USER_VERSION))
                .map_err(|e| PrimerError::Storage(format!("set user_version failed: {e}")))?;
        }

        // Validate-and-seed the lookup tables. Borrows the connection
        // directly; no transaction needed because the writes are
        // idempotent INSERTs.
        let speakers = catalog::expected_speakers();
        let intents = catalog::expected_intents();
        let engagement_states = catalog::expected_engagement_states();
        let understanding_depths = catalog::expected_understanding_depths();
        schema::validate_and_seed_lookup(&conn, "speakers", &speakers)?;
        schema::validate_and_seed_lookup(&conn, "pedagogical_intents", &intents)?;
        schema::validate_and_seed_lookup(&conn, "engagement_states", &engagement_states)?;
        schema::validate_and_seed_lookup(&conn, "understanding_depths", &understanding_depths)?;
```

- [ ] **Step 4: Make `apply_v4_migrations` visible to the test in lib.rs**

Currently `apply_v4_migrations` is `pub(crate)`. That stays — `open()` is in the same crate. No change needed.

- [ ] **Step 5: Run the test**

```bash
cargo test -p primer-storage open_fresh_db_creates_v4_schema_with_understanding_depths_seeded
```

Expected: PASS.

- [ ] **Step 6: Run the full crate to catch regressions**

```bash
cargo test -p primer-storage
```

Expected: all previous tests pass + 1 new test passes.

- [ ] **Step 7: Commit**

```bash
git add src/crates/primer-storage/src/lib.rs src/crates/primer-storage/Cargo.toml src/Cargo.toml
git commit -m "$(cat <<'EOF'
feat(storage): wire v4 migration + understanding_depths seed into open()

open() now applies v2 → v3 → v4 migrations idempotently and seeds the
understanding_depths lookup alongside speakers / pedagogical_intents /
engagement_states. Schema is now at v4 by default.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5 — `SessionStore::most_recent_session_learner_id`

**Files:**
- Modify: `src/crates/primer-core/src/storage.rs`
- Modify: `src/crates/primer-storage/src/lib.rs`

Add the new trait method + implement it. The trait method addition will fail compilation until the impl exists, so do them in the same task.

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` in `primer-storage/src/lib.rs`:

```rust
#[tokio::test]
async fn most_recent_session_learner_id_returns_none_on_empty_db() {
    let store = SqliteSessionStore::open(std::path::Path::new(":memory:")).unwrap();
    let result = store.most_recent_session_learner_id().await.unwrap();
    assert_eq!(result, None);
}

#[tokio::test]
async fn most_recent_session_learner_id_returns_single_existing_learner_id() {
    use primer_core::conversation::Session;

    let store = SqliteSessionStore::open(std::path::Path::new(":memory:")).unwrap();
    let learner_id = uuid::Uuid::new_v4();
    let session = Session::new(learner_id);
    store.save_session(&session).await.unwrap();

    let result = store.most_recent_session_learner_id().await.unwrap();
    assert_eq!(result, Some(learner_id));
}

#[tokio::test]
async fn most_recent_session_learner_id_picks_most_recent_when_multiple_distinct() {
    use chrono::{Duration, Utc};
    use primer_core::conversation::Session;

    let store = SqliteSessionStore::open(std::path::Path::new(":memory:")).unwrap();
    let older_learner = uuid::Uuid::new_v4();
    let newer_learner = uuid::Uuid::new_v4();

    let mut older = Session::new(older_learner);
    older.started_at = Utc::now() - Duration::hours(2);
    let mut newer = Session::new(newer_learner);
    newer.started_at = Utc::now();

    store.save_session(&older).await.unwrap();
    store.save_session(&newer).await.unwrap();

    let result = store.most_recent_session_learner_id().await.unwrap();
    assert_eq!(result, Some(newer_learner));
}
```

- [ ] **Step 2: Run tests to verify they fail (compilation error)**

```bash
cargo test -p primer-storage most_recent_session_learner_id
```

Expected: FAIL — `no method named most_recent_session_learner_id found for SqliteSessionStore`.

- [ ] **Step 3: Add the trait method**

In `src/crates/primer-core/src/storage.rs`, add the method to the `SessionStore` trait (after `load_recent_assessments`, the last existing method):

```rust
    /// Return the `learner_id` of the most-recent session in this DB,
    /// if any. Used by the CLI on first-run after a v3 → v4 upgrade to
    /// adopt the existing session-id as the new learner's persistent
    /// UUID, eliminating the otherwise-orphan-session class.
    ///
    /// Returns `Ok(None)` for a DB with no sessions. Reserves `Err` for
    /// genuine I/O / decoding failures.
    async fn most_recent_session_learner_id(&self) -> Result<Option<Uuid>>;
```

`Uuid` is already imported at the top of the file.

- [ ] **Step 4: Implement on `SqliteSessionStore`**

In `src/crates/primer-storage/src/lib.rs`, inside `impl SessionStore for SqliteSessionStore { ... }`, add the new method. Place it after `load_recent_assessments` (the last existing method):

```rust
    async fn most_recent_session_learner_id(&self) -> Result<Option<Uuid>> {
        let conn = self.conn.lock().unwrap();
        let row: Option<String> = conn
            .query_row(
                "SELECT learner_id FROM sessions ORDER BY started_at DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| PrimerError::Storage(format!("most_recent_session_learner_id: {e}")))?;

        match row {
            None => Ok(None),
            Some(s) => {
                let uuid = Uuid::parse_str(&s).map_err(|e| {
                    PrimerError::Storage(format!("parse learner_id {s}: {e}"))
                })?;
                Ok(Some(uuid))
            }
        }
    }
```

- [ ] **Step 5: Run tests to verify they pass**

```bash
cargo test -p primer-storage most_recent_session_learner_id
```

Expected: PASS (all 3 tests).

- [ ] **Step 6: Run workspace to catch regressions**

```bash
cargo test --workspace
```

Expected: all tests pass. Any other `impl SessionStore for ...` blocks (e.g. test mocks in `primer-pedagogy/src/dialogue_manager.rs`) need the new method added — see Step 7.

- [ ] **Step 7: Update test mocks for `SessionStore`**

`grep -rn "impl SessionStore" src/crates/` to find mock implementations. The session-store spy in `dialogue_manager.rs` (line 671 area) implements `SessionStore`; add the new method to it:

```rust
        async fn most_recent_session_learner_id(&self) -> Result<Option<uuid::Uuid>> {
            Ok(None)
        }
```

(Test spies don't need real implementations; returning `None` is the correct stub for tests that don't exercise this path.)

Re-run `cargo test --workspace` until clean.

- [ ] **Step 8: Commit**

```bash
git add src/crates/primer-core/src/storage.rs src/crates/primer-storage/src/lib.rs src/crates/primer-pedagogy/src/dialogue_manager.rs
git commit -m "$(cat <<'EOF'
feat(core,storage): SessionStore::most_recent_session_learner_id

Returns the learner_id of the most-recent session in the DB, if any.
Used by the CLI on first-run after a v3 → v4 upgrade to adopt the
existing session-id as the persistent learner UUID, eliminating the
otherwise-orphan-session class.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 6 — `LearnerStore` trait + `serde_json` dep

**Files:**
- Modify: `src/crates/primer-core/src/storage.rs`
- Modify: `src/crates/primer-core/src/lib.rs`
- Modify: `src/crates/primer-storage/Cargo.toml`

Adds the trait declaration only. Implementation comes in Tasks 7 and 8.

- [ ] **Step 1: Add `serde_json` dep to `primer-storage`**

`serde_json` is already pinned in workspace deps at `src/Cargo.toml:28` (`serde_json = "1"`). Add to `src/crates/primer-storage/Cargo.toml` under `[dependencies]`:

```toml
serde_json.workspace = true
```

The full `[dependencies]` block becomes:

```toml
[dependencies]
primer-core.workspace = true
async-trait.workspace = true
tokio.workspace = true
rusqlite.workspace = true
tracing.workspace = true
thiserror.workspace = true
chrono.workspace = true
uuid.workspace = true
serde_json.workspace = true
```

- [ ] **Step 2: Add the `LearnerStore` trait**

In `src/crates/primer-core/src/storage.rs`, add to the existing imports (top of file):

```rust
use crate::learner::LearnerModel;
```

After the existing `SessionStore` trait, add:

```rust
/// Persists the per-child `LearnerModel` to disk.
///
/// One implementation lives per DB file (the application invariant: at
/// most one `learners` row per file). `load_learner` returns `Ok(None)`
/// if the file has never had a learner row created — first-run signal.
///
/// `save_learner` is monotonic on concepts: it upserts every concept
/// in `learner.concepts` but never deletes `learner_concepts` rows.
/// Concept state is monotonic across a child's lifetime — knowing-then-
/// not-knowing is a separate event ("forgetting") that should be an
/// explicit operation, not a side effect.
#[async_trait]
pub trait LearnerStore: Send + Sync {
    /// Look up the (single) learner row in this DB. Returns Ok(None) if
    /// the file has never had a learner row created.
    async fn load_learner(&self) -> Result<Option<LearnerModel>>;

    /// Persist the full current state of `learner`. Idempotent at the
    /// row level: the `learners` row is upserted (`INSERT … ON CONFLICT
    /// DO UPDATE`), and `learner_concepts` rows are upserted by
    /// `(learner_id, concept_id)` PRIMARY KEY. Concepts dropped from
    /// `learner.concepts` are NOT removed from the DB — concept state is
    /// monotonic across a child's lifetime.
    async fn save_learner(&self, learner: &LearnerModel) -> Result<()>;
}
```

- [ ] **Step 3: Re-export from `primer-core::lib`**

In `src/crates/primer-core/src/lib.rs`, find the existing `pub use crate::storage::SessionStore;` (or similar) and add alongside:

```rust
pub use crate::storage::{LearnerStore, SessionStore};
```

(If the existing line is `pub use crate::storage::SessionStore;`, replace it with the combined import above. If `SessionStore` is exported some other way, match that pattern.)

- [ ] **Step 4: Confirm compilation**

```bash
cargo build -p primer-core
```

Expected: clean build. The trait has no implementations yet, but the declaration is valid.

- [ ] **Step 5: Confirm workspace still compiles**

```bash
cargo build --workspace
```

Expected: clean. No code yet uses `LearnerStore`.

- [ ] **Step 6: Commit**

```bash
git add src/crates/primer-core/src/storage.rs src/crates/primer-core/src/lib.rs src/crates/primer-storage/Cargo.toml
git commit -m "$(cat <<'EOF'
feat(core): LearnerStore trait + serde_json dep on primer-storage

Trait declaration only; implementation lands in the next two commits.
serde_json is needed for the JSON-in-TEXT encoding of three Vec<String>
fields (learners.languages, learners.high_engagement_topics,
learner_concepts.notes).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 7 — Implement `LearnerStore::save_learner`

**Files:**
- Modify: `src/crates/primer-storage/src/lib.rs`

The implementation upserts the `learners` row and every `learner_concepts` row in a single transaction. Concept names are looked up against the existing `concepts(id, name)` table (lazy-create with `INSERT OR IGNORE`).

- [ ] **Step 1: Write the failing tests**

Add to `#[cfg(test)] mod tests` in `lib.rs` (a new dedicated `mod learner_store_tests` if it makes structure cleaner):

```rust
#[cfg(test)]
mod learner_store_tests {
    use super::*;
    use chrono::Utc;
    use primer_core::learner::{
        ConceptState, EngagementState, LearnerModel, LearnerProfile, LearningPreferences,
        UnderstandingDepth,
    };
    use primer_core::storage::LearnerStore;
    use std::time::Duration;
    use uuid::Uuid;

    fn open_in_mem() -> SqliteSessionStore {
        SqliteSessionStore::open(std::path::Path::new(":memory:")).unwrap()
    }

    fn sample_learner() -> LearnerModel {
        LearnerModel {
            profile: LearnerProfile {
                id: Uuid::new_v4(),
                name: "Binti".into(),
                age: 8,
                languages: vec!["en".into(), "fr".into()],
                created_at: Utc::now(),
                last_active: Utc::now(),
            },
            concepts: vec![
                ConceptState {
                    concept_id: "physics:gravity".into(),
                    depth: UnderstandingDepth::Comprehension,
                    confidence: 0.7,
                    encounter_count: 3,
                    last_encountered: Some(Utc::now()),
                    notes: vec!["mass vs weight confusion".into()],
                },
                ConceptState {
                    concept_id: "biology:photosynthesis".into(),
                    depth: UnderstandingDepth::Aware,
                    confidence: 0.4,
                    encounter_count: 1,
                    last_encountered: None,
                    notes: vec![],
                },
            ],
            preferences: LearningPreferences {
                narrative: 0.8,
                socratic: 0.7,
                visual: 0.5,
                kinesthetic: 0.3,
                typical_session_minutes: 25.0,
                high_engagement_topics: vec!["dinosaurs".into(), "space".into()],
                early_disengagement_threshold: Duration::from_secs(420),
            },
            current_engagement: EngagementState::Engaged,
            recent_assessments: vec![],
        }
    }

    #[tokio::test]
    async fn save_learner_writes_one_row_to_learners_table() {
        let store = open_in_mem();
        store.save_learner(&sample_learner()).await.unwrap();

        let conn = store.conn.lock().unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM learners", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn save_learner_writes_one_learner_concepts_row_per_concept() {
        let store = open_in_mem();
        store.save_learner(&sample_learner()).await.unwrap();

        let conn = store.conn.lock().unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM learner_concepts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn save_learner_is_idempotent_on_repeat_calls() {
        let store = open_in_mem();
        let l = sample_learner();
        store.save_learner(&l).await.unwrap();
        store.save_learner(&l).await.unwrap();

        let conn = store.conn.lock().unwrap();
        let learners: i64 = conn
            .query_row("SELECT COUNT(*) FROM learners", [], |r| r.get(0))
            .unwrap();
        let learner_concepts: i64 = conn
            .query_row("SELECT COUNT(*) FROM learner_concepts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(learners, 1);
        assert_eq!(learner_concepts, 2);
    }

    #[tokio::test]
    async fn save_learner_updates_concept_in_place() {
        let store = open_in_mem();
        let mut l = sample_learner();
        store.save_learner(&l).await.unwrap();

        // Mutate the first concept's encounter_count and depth.
        l.concepts[0].encounter_count = 7;
        l.concepts[0].depth = UnderstandingDepth::Application;
        store.save_learner(&l).await.unwrap();

        let conn = store.conn.lock().unwrap();
        let (count, depth_id): (i64, i64) = conn
            .query_row(
                "SELECT lc.encounter_count, lc.depth_id
                 FROM learner_concepts lc
                 JOIN concepts c ON c.id = lc.concept_id
                 WHERE c.name = 'physics:gravity'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(count, 7);
        assert_eq!(depth_id, crate::catalog::understanding_depth_id(UnderstandingDepth::Application));

        // Still only two rows total — no duplicate.
        let total: i64 = conn
            .query_row("SELECT COUNT(*) FROM learner_concepts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(total, 2);
    }

    #[tokio::test]
    async fn save_learner_is_monotonic_on_concepts() {
        let store = open_in_mem();
        let mut l = sample_learner();
        store.save_learner(&l).await.unwrap();

        // Drop one concept from the in-memory Vec.
        l.concepts.truncate(1);
        store.save_learner(&l).await.unwrap();

        let conn = store.conn.lock().unwrap();
        let total: i64 = conn
            .query_row("SELECT COUNT(*) FROM learner_concepts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(total, 2, "dropped concept must remain on disk");
    }

    #[tokio::test]
    async fn save_learner_persists_every_understanding_depth_variant() {
        let store = open_in_mem();
        let mut l = sample_learner();
        l.concepts.clear();
        for d in UnderstandingDepth::ALL {
            l.concepts.push(ConceptState {
                concept_id: format!("test:{}", crate::catalog::understanding_depth_name(*d)),
                depth: *d,
                confidence: 0.5,
                encounter_count: 1,
                last_encountered: None,
                notes: vec![],
            });
        }
        store.save_learner(&l).await.unwrap();

        let conn = store.conn.lock().unwrap();
        let total: i64 = conn
            .query_row("SELECT COUNT(*) FROM learner_concepts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(total as usize, UnderstandingDepth::ALL.len());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p primer-storage learner_store_tests
```

Expected: FAIL — `LearnerStore` not implemented for `SqliteSessionStore`.

- [ ] **Step 3: Implement `LearnerStore::save_learner`**

In `src/crates/primer-storage/src/lib.rs`, add after the `impl SessionStore for SqliteSessionStore { ... }` block:

```rust
#[async_trait]
impl primer_core::storage::LearnerStore for SqliteSessionStore {
    async fn save_learner(&self, learner: &primer_core::learner::LearnerModel) -> Result<()> {
        use primer_core::learner::ConceptState;

        let mut conn = self.conn.lock().unwrap();
        let tx = conn
            .transaction()
            .map_err(|e| PrimerError::Storage(format!("save_learner begin tx: {e}")))?;

        // 1. Upsert the learners row. Use proper UPSERT (ON CONFLICT DO
        // UPDATE) so we do NOT cascade-wipe learner_concepts via the FK.
        // INSERT OR REPLACE would do exactly that — see the save_session
        // notes for the same footgun.
        let languages_json = serde_json::to_string(&learner.profile.languages)
            .map_err(|e| PrimerError::Storage(format!("encode languages: {e}")))?;
        let topics_json = serde_json::to_string(&learner.preferences.high_engagement_topics)
            .map_err(|e| PrimerError::Storage(format!("encode high_engagement_topics: {e}")))?;
        let early_secs = learner.preferences.early_disengagement_threshold.as_secs() as i64;
        let engagement_state_id = catalog::engagement_state_id(learner.current_engagement);

        tx.execute(
            "INSERT INTO learners (
                 id, name, age, languages, created_at, last_active,
                 pref_narrative, pref_socratic, pref_visual, pref_kinesthetic,
                 typical_session_minutes, high_engagement_topics,
                 early_disengagement_secs, current_engagement_state_id
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
             ON CONFLICT(id) DO UPDATE SET
                 name = excluded.name,
                 age = excluded.age,
                 languages = excluded.languages,
                 last_active = excluded.last_active,
                 pref_narrative = excluded.pref_narrative,
                 pref_socratic = excluded.pref_socratic,
                 pref_visual = excluded.pref_visual,
                 pref_kinesthetic = excluded.pref_kinesthetic,
                 typical_session_minutes = excluded.typical_session_minutes,
                 high_engagement_topics = excluded.high_engagement_topics,
                 early_disengagement_secs = excluded.early_disengagement_secs,
                 current_engagement_state_id = excluded.current_engagement_state_id",
            rusqlite::params![
                learner.profile.id.to_string(),
                learner.profile.name,
                learner.profile.age as i64,
                languages_json,
                learner.profile.created_at.to_rfc3339(),
                learner.profile.last_active.to_rfc3339(),
                learner.preferences.narrative as f64,
                learner.preferences.socratic as f64,
                learner.preferences.visual as f64,
                learner.preferences.kinesthetic as f64,
                learner.preferences.typical_session_minutes as f64,
                topics_json,
                early_secs,
                engagement_state_id,
            ],
        )
        .map_err(|e| PrimerError::Storage(format!("upsert learner: {e}")))?;

        // 2. For each concept, ensure the concepts row exists and upsert
        //    learner_concepts.
        if !learner.concepts.is_empty() {
            let mut insert_concept = tx
                .prepare("INSERT OR IGNORE INTO concepts (name) VALUES (?1)")
                .map_err(|e| PrimerError::Storage(format!("prepare insert concept: {e}")))?;
            let mut select_concept = tx
                .prepare("SELECT id FROM concepts WHERE name = ?1")
                .map_err(|e| PrimerError::Storage(format!("prepare select concept: {e}")))?;
            let mut upsert_lc = tx
                .prepare(
                    "INSERT INTO learner_concepts (
                         learner_id, concept_id, depth_id, confidence,
                         encounter_count, last_encountered, notes
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                     ON CONFLICT(learner_id, concept_id) DO UPDATE SET
                         depth_id = excluded.depth_id,
                         confidence = excluded.confidence,
                         encounter_count = excluded.encounter_count,
                         last_encountered = excluded.last_encountered,
                         notes = excluded.notes",
                )
                .map_err(|e| PrimerError::Storage(format!("prepare upsert lc: {e}")))?;

            // Per-call cache to skip re-querying concepts within one save.
            let mut concept_id_cache: std::collections::HashMap<String, i64> =
                std::collections::HashMap::new();

            for concept in &learner.concepts {
                let cid = match concept_id_cache.get(&concept.concept_id).copied() {
                    Some(id) => id,
                    None => {
                        insert_concept
                            .execute(rusqlite::params![concept.concept_id])
                            .map_err(|e| {
                                PrimerError::Storage(format!(
                                    "upsert concept {}: {e}",
                                    concept.concept_id
                                ))
                            })?;
                        let id: i64 = select_concept
                            .query_row(rusqlite::params![concept.concept_id], |r| r.get(0))
                            .map_err(|e| {
                                PrimerError::Storage(format!(
                                    "select concept {}: {e}",
                                    concept.concept_id
                                ))
                            })?;
                        concept_id_cache.insert(concept.concept_id.clone(), id);
                        id
                    }
                };

                let notes_json = serde_json::to_string(&concept.notes)
                    .map_err(|e| PrimerError::Storage(format!("encode notes: {e}")))?;
                let last_encountered = concept.last_encountered.map(|t| t.to_rfc3339());

                upsert_lc
                    .execute(rusqlite::params![
                        learner.profile.id.to_string(),
                        cid,
                        catalog::understanding_depth_id(concept.depth),
                        concept.confidence as f64,
                        concept.encounter_count as i64,
                        last_encountered,
                        notes_json,
                    ])
                    .map_err(|e| PrimerError::Storage(format!("upsert learner_concept: {e}")))?;
            }

            drop(upsert_lc);
            drop(select_concept);
            drop(insert_concept);
        }

        tx.commit()
            .map_err(|e| PrimerError::Storage(format!("save_learner commit: {e}")))?;
        Ok(())
    }

    async fn load_learner(&self) -> Result<Option<primer_core::learner::LearnerModel>> {
        // Stub for now — real impl in Task 8. Returning None lets save_learner
        // tests pass without depending on load_learner.
        Ok(None)
    }
}
```

(Note the stub `load_learner`. It compiles, the trait method exists, and Task 7's tests don't exercise it. Task 8 replaces the body.)

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test -p primer-storage learner_store_tests
```

Expected: PASS — all 6 tests.

- [ ] **Step 5: Run workspace to confirm no regressions**

```bash
cargo test --workspace
```

Expected: all previous tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/crates/primer-storage/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(storage): implement LearnerStore::save_learner

Single-transaction upsert of the learners row + every learner_concepts
row. Uses proper UPSERT (ON CONFLICT DO UPDATE) on learners.id to avoid
cascade-wiping learner_concepts via the FK. Concepts are monotonic —
dropping a concept from the in-memory Vec does not delete the row.

load_learner is a stub returning Ok(None); real impl in the next commit.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 8 — Implement `LearnerStore::load_learner`

**Files:**
- Modify: `src/crates/primer-storage/src/lib.rs`

Replace the stub with a real implementation. Three SELECTs: learners row, learner_concepts JOIN concepts, decode JSON arrays.

- [ ] **Step 1: Write the failing tests**

Add to `mod learner_store_tests`:

```rust
    #[tokio::test]
    async fn load_learner_returns_none_for_empty_db() {
        let store = open_in_mem();
        let result = store.load_learner().await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn save_then_load_round_trips_every_field() {
        let store = open_in_mem();
        let original = sample_learner();
        store.save_learner(&original).await.unwrap();

        let loaded = store.load_learner().await.unwrap().expect("learner row");

        // Profile.
        assert_eq!(loaded.profile.id, original.profile.id);
        assert_eq!(loaded.profile.name, original.profile.name);
        assert_eq!(loaded.profile.age, original.profile.age);
        assert_eq!(loaded.profile.languages, original.profile.languages);
        // Timestamps round-trip via RFC 3339; allow sub-second equality.
        assert_eq!(
            loaded.profile.created_at.timestamp(),
            original.profile.created_at.timestamp()
        );
        assert_eq!(
            loaded.profile.last_active.timestamp(),
            original.profile.last_active.timestamp()
        );

        // Preferences.
        assert!((loaded.preferences.narrative - original.preferences.narrative).abs() < 1e-6);
        assert!((loaded.preferences.socratic - original.preferences.socratic).abs() < 1e-6);
        assert!((loaded.preferences.visual - original.preferences.visual).abs() < 1e-6);
        assert!((loaded.preferences.kinesthetic - original.preferences.kinesthetic).abs() < 1e-6);
        assert_eq!(
            loaded.preferences.high_engagement_topics,
            original.preferences.high_engagement_topics
        );
        assert_eq!(
            loaded.preferences.early_disengagement_threshold,
            original.preferences.early_disengagement_threshold
        );

        // Engagement snapshot.
        assert_eq!(loaded.current_engagement, original.current_engagement);

        // Concepts — match by concept_id (order is not guaranteed by SELECT).
        assert_eq!(loaded.concepts.len(), original.concepts.len());
        for original_c in &original.concepts {
            let loaded_c = loaded
                .concepts
                .iter()
                .find(|c| c.concept_id == original_c.concept_id)
                .expect("concept present");
            assert_eq!(loaded_c.depth, original_c.depth);
            assert!((loaded_c.confidence - original_c.confidence).abs() < 1e-6);
            assert_eq!(loaded_c.encounter_count, original_c.encounter_count);
            assert_eq!(loaded_c.notes, original_c.notes);
            assert_eq!(
                loaded_c.last_encountered.map(|t| t.timestamp()),
                original_c.last_encountered.map(|t| t.timestamp()),
            );
        }

        // recent_assessments is rehydrated separately from
        // turn_classifications and is not part of the round-trip.
        assert!(loaded.recent_assessments.is_empty());
    }
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p primer-storage save_then_load_round_trips_every_field
```

Expected: FAIL — `load_learner` returns `Ok(None)` (stub).

- [ ] **Step 3: Implement `load_learner`**

Replace the stub `load_learner` body in `lib.rs` with:

```rust
    async fn load_learner(&self) -> Result<Option<primer_core::learner::LearnerModel>> {
        use primer_core::learner::{
            ConceptState, EngagementState, LearnerModel, LearnerProfile, LearningPreferences,
        };
        use std::time::Duration;

        let conn = self.conn.lock().unwrap();

        // Step 1: the learners row.
        type Row = (
            String, String, i64, String, String, String,
            f64, f64, f64, f64, f64, String, i64, i64,
        );
        let row: Option<Row> = conn
            .query_row(
                "SELECT id, name, age, languages, created_at, last_active,
                        pref_narrative, pref_socratic, pref_visual, pref_kinesthetic,
                        typical_session_minutes, high_engagement_topics,
                        early_disengagement_secs, current_engagement_state_id
                 FROM learners LIMIT 1",
                [],
                |r| {
                    Ok((
                        r.get(0)?,  r.get(1)?,  r.get(2)?,  r.get(3)?,
                        r.get(4)?,  r.get(5)?,  r.get(6)?,  r.get(7)?,
                        r.get(8)?,  r.get(9)?,  r.get(10)?, r.get(11)?,
                        r.get(12)?, r.get(13)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| PrimerError::Storage(format!("load_learner select: {e}")))?;

        let Some((
            id_str, name, age, languages_json, created_str, last_active_str,
            pref_narrative, pref_socratic, pref_visual, pref_kinesthetic,
            typical_session_minutes, topics_json, early_secs, engagement_state_id,
        )) = row
        else {
            return Ok(None);
        };

        let id = Uuid::parse_str(&id_str)
            .map_err(|e| PrimerError::Storage(format!("parse learner id {id_str}: {e}")))?;
        let languages: Vec<String> = serde_json::from_str(&languages_json)
            .map_err(|e| PrimerError::Storage(format!("decode languages: {e}")))?;
        let high_engagement_topics: Vec<String> = serde_json::from_str(&topics_json)
            .map_err(|e| PrimerError::Storage(format!("decode high_engagement_topics: {e}")))?;
        let created_at = parse_rfc3339(&created_str, "learners.created_at")?;
        let last_active = parse_rfc3339(&last_active_str, "learners.last_active")?;
        let current_engagement = catalog::engagement_state_from_id(engagement_state_id)
            .ok_or_else(|| {
                PrimerError::Storage(format!(
                    "unknown engagement_state_id {engagement_state_id} on learners row"
                ))
            })?;

        let profile = LearnerProfile {
            id,
            name,
            age: age as u8,
            languages,
            created_at,
            last_active,
        };
        let preferences = LearningPreferences {
            narrative: pref_narrative as f32,
            socratic: pref_socratic as f32,
            visual: pref_visual as f32,
            kinesthetic: pref_kinesthetic as f32,
            typical_session_minutes: typical_session_minutes as f32,
            high_engagement_topics,
            early_disengagement_threshold: Duration::from_secs(early_secs.max(0) as u64),
        };

        // Step 2: every learner_concepts row, joined to concepts for the
        // string concept_id.
        let mut stmt = conn
            .prepare(
                "SELECT c.name, lc.depth_id, lc.confidence, lc.encounter_count,
                        lc.last_encountered, lc.notes
                 FROM learner_concepts lc
                 JOIN concepts c ON c.id = lc.concept_id
                 WHERE lc.learner_id = ?1",
            )
            .map_err(|e| PrimerError::Storage(format!("prepare load_learner concepts: {e}")))?;
        let rows = stmt
            .query_map(rusqlite::params![id.to_string()], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, f64>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, String>(5)?,
                ))
            })
            .map_err(|e| PrimerError::Storage(format!("query learner_concepts: {e}")))?;

        let mut concepts: Vec<ConceptState> = Vec::new();
        for row in rows {
            let (concept_name, depth_id, confidence, encounter_count, last_encountered_opt, notes_json) =
                row.map_err(|e| PrimerError::Storage(format!("read learner_concepts: {e}")))?;
            let depth =
                catalog::understanding_depth_from_id(depth_id).ok_or_else(|| {
                    PrimerError::Storage(format!("unknown depth_id {depth_id}"))
                })?;
            let last_encountered = last_encountered_opt
                .as_deref()
                .map(|s| parse_rfc3339(s, "learner_concepts.last_encountered"))
                .transpose()?;
            let notes: Vec<String> = serde_json::from_str(&notes_json)
                .map_err(|e| PrimerError::Storage(format!("decode notes: {e}")))?;
            concepts.push(ConceptState {
                concept_id: concept_name,
                depth,
                confidence: confidence as f32,
                encounter_count: encounter_count as u32,
                last_encountered,
                notes,
            });
        }

        Ok(Some(LearnerModel {
            profile,
            concepts,
            preferences,
            current_engagement,
            recent_assessments: vec![],
        }))
    }
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test -p primer-storage learner_store_tests
```

Expected: PASS — both new tests + the 6 from Task 7.

- [ ] **Step 5: Run workspace to confirm**

```bash
cargo test --workspace
```

Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add src/crates/primer-storage/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(storage): implement LearnerStore::load_learner

Three SELECTs (one per logical block: learners row, learner_concepts
JOIN concepts, JSON-decode the three Vec<String> fields). Returns
Ok(None) on an empty learners table. recent_assessments is left empty
— rehydrated separately by DialogueManager from turn_classifications
on resume.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 9 — Wire `LearnerStore` into `DialogueManager`

**Files:**
- Modify: `src/crates/primer-pedagogy/src/dialogue_manager.rs`

Add the field, update `new()`, and add `save_learner` calls at the five save points: `open_session`, `resume_session`, after each Primer turn, on mid-stream error, on `close_session`.

- [ ] **Step 1: Write the failing test**

Add to the existing `#[cfg(test)] mod tests` block in `dialogue_manager.rs`:

```rust
    #[tokio::test]
    async fn end_to_end_save_learner_after_open_and_one_turn() {
        // Build a manager with both Some(SessionStore) and Some(LearnerStore).
        // Run a single turn and verify the learners row was upserted with
        // a refreshed last_active.
        use primer_core::storage::LearnerStore;
        use primer_storage::SqliteSessionStore;

        let store = Arc::new(SqliteSessionStore::open(std::path::Path::new(":memory:")).unwrap());
        let learner = test_learner();
        let backend = StubBackend::new();
        let knowledge = primer_knowledge::SqliteKnowledgeBase::open(":memory:").unwrap();

        // Pre-save the learner so the DB has a row to UPDATE rather than INSERT.
        store.save_learner(&learner).await.unwrap();

        let mut dm = DialogueManager::new(
            learner,
            &backend,
            &knowledge,
            Some(Arc::clone(&store) as Arc<dyn SessionStore>),
            Some(Arc::clone(&store) as Arc<dyn LearnerStore>),
            None,
            PedagogyConfig::default(),
        );
        let _greeting = dm.open_session().await.unwrap();
        let _reply = dm.respond_to("hello").await.unwrap();

        // load_learner should return the persisted row.
        let loaded = store.load_learner().await.unwrap().expect("learner row");
        assert_eq!(loaded.profile.id, dm.learner.profile.id);
    }

    #[tokio::test]
    async fn divergence_bug_closed_via_cli_startup_flow() {
        // Fixture: a fresh DB seeded with a session under UUID U1, no
        // learners row yet (simulates the v3 → v4 upgrade-on-first-open).
        // Then run the CLI's first-run startup flow:
        //   load_learner() == None
        //   most_recent_session_learner_id() == Some(U1)
        //   mint LearnerModel with id=U1, save_learner(...)
        // Assert the resulting LearnerModel.profile.id == U1.
        use primer_core::conversation::Session;
        use primer_core::storage::{LearnerStore, SessionStore};
        use primer_storage::SqliteSessionStore;

        let store = Arc::new(SqliteSessionStore::open(std::path::Path::new(":memory:")).unwrap());
        let u1 = uuid::Uuid::new_v4();
        let s = Session::new(u1);
        store.save_session(&s).await.unwrap();

        // Simulate the CLI startup flow.
        let load_result = store.load_learner().await.unwrap();
        assert!(load_result.is_none(), "no learner row yet");

        let adopted = store
            .most_recent_session_learner_id()
            .await
            .unwrap()
            .expect("session exists");
        assert_eq!(adopted, u1);

        let mut adopted_learner = test_learner();
        adopted_learner.profile.id = adopted;
        store.save_learner(&adopted_learner).await.unwrap();

        // Now construct a DialogueManager with the adopted learner and
        // confirm Session.learner_id matches.
        let backend = StubBackend::new();
        let knowledge = primer_knowledge::SqliteKnowledgeBase::open(":memory:").unwrap();
        let mut dm = DialogueManager::new(
            adopted_learner,
            &backend,
            &knowledge,
            Some(Arc::clone(&store) as Arc<dyn SessionStore>),
            Some(Arc::clone(&store) as Arc<dyn LearnerStore>),
            None,
            PedagogyConfig::default(),
        );
        let _ = dm.open_session().await.unwrap();
        assert_eq!(dm.session.learner_id, dm.learner.profile.id);
        assert_eq!(dm.session.learner_id, u1);
    }
```

(`primer-pedagogy/Cargo.toml` may need `primer-storage` added under `[dev-dependencies]` to reference `SqliteSessionStore` from the test. Check with `grep primer-storage src/crates/primer-pedagogy/Cargo.toml`. If missing, add `primer-storage.workspace = true` under `[dev-dependencies]`.)

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p primer-pedagogy end_to_end_save_learner_after_open_and_one_turn
```

Expected: FAIL — `DialogueManager::new` signature does not accept a 5th positional arg.

- [ ] **Step 3: Add `learner_store` field + update `new`**

In `dialogue_manager.rs`, find the `pub struct DialogueManager<'a> { ... }` definition and add:

```rust
    pub learner: LearnerModel,
    pub session: Session,
    inference: &'a dyn InferenceBackend,
    knowledge: &'a dyn KnowledgeBase,
    storage: Option<Arc<dyn SessionStore>>,
    learner_store: Option<Arc<dyn LearnerStore>>,        // new
    classifier: Option<Arc<dyn EngagementClassifier>>,
    config: PedagogyConfig,
    // ...remaining fields unchanged...
```

(Add `use primer_core::storage::LearnerStore;` to the imports if not already present.)

Update `pub fn new(...)`:

```rust
    pub fn new(
        learner: LearnerModel,
        inference: &'a dyn InferenceBackend,
        knowledge: &'a dyn KnowledgeBase,
        storage: Option<Arc<dyn SessionStore>>,
        learner_store: Option<Arc<dyn LearnerStore>>,    // new
        classifier: Option<Arc<dyn EngagementClassifier>>,
        config: PedagogyConfig,
    ) -> Self {
        let session = Session::new(learner.profile.id);
        Self {
            learner,
            session,
            inference,
            knowledge,
            storage,
            learner_store,   // new
            classifier,
            config,
            // ...remaining field initialisers unchanged...
        }
    }
```

- [ ] **Step 4: Add `save_learner` calls at the five save points**

In `dialogue_manager.rs`, locate each `save_session` call and add a paired `save_learner` immediately after.

**(a) `open_session` — after `save_session` at line ~144:**

```rust
        if let Some(ref store) = self.storage {
            if let Err(e) = store.save_session(&self.session).await {
                tracing::warn!("session save failed: {e}");
            }
        }
        if let Some(ref ls) = self.learner_store {
            if let Err(e) = ls.save_learner(&self.learner).await {
                tracing::warn!("learner save failed (open_session): {e}");
            }
        }
```

**(b) `resume_session` — after `save_session` at line ~192:**

```rust
        if let Some(ref store) = self.storage {
            if let Err(e) = store.save_session(&self.session).await {
                tracing::warn!("session save failed: {e}");
            }
        }
        if let Some(ref ls) = self.learner_store {
            if let Err(e) = ls.save_learner(&self.learner).await {
                tracing::warn!("learner save failed (resume_session): {e}");
            }
        }
```

**(c) After every Primer turn (covers ok + mid-stream error) — after `save_session` at line ~308:**

```rust
        // 5. Save the session if a store is configured. Runs on both Ok
        //    and Err paths. Save failures are logged, not propagated.
        if let Some(ref store) = self.storage {
            if let Err(e) = store.save_session(&self.session).await {
                tracing::warn!("session save failed: {e}");
            }
        }
        if let Some(ref ls) = self.learner_store {
            if let Err(e) = ls.save_learner(&self.learner).await {
                tracing::warn!("learner save failed (per-turn): {e}");
            }
        }
```

**(d) `close_session` — after `save_session` at line ~429:**

```rust
        if let Some(ref store) = self.storage {
            if let Err(e) = store.save_session(&self.session).await {
                tracing::warn!("session save failed: {e}");
            }
        }
        if let Some(ref ls) = self.learner_store {
            if let Err(e) = ls.save_learner(&self.learner).await {
                tracing::warn!("learner save failed (close_session): {e}");
            }
        }
```

(Note: the per-turn site at point (c) already covers the mid-stream-error case since it runs on both `Ok` and `Err` branches of `result`. There are four physical save sites total, not five.)

- [ ] **Step 5: Update existing test call sites of `DialogueManager::new`**

Many existing tests call `DialogueManager::new(...)` with the old signature. Find them all:

```bash
grep -rn "DialogueManager::new" src/crates/
```

For each call, insert `None` (or `Some(Arc::new(...))`) for the new `learner_store` argument in position 5. Most existing tests should pass `None` — the test isn't exercising learner persistence.

Example transformation (before):

```rust
let mut dm = DialogueManager::new(
    learner,
    &backend,
    &knowledge,
    None,
    None,
    PedagogyConfig::default(),
);
```

After:

```rust
let mut dm = DialogueManager::new(
    learner,
    &backend,
    &knowledge,
    None,    // storage
    None,    // learner_store  (new)
    None,    // classifier
    PedagogyConfig::default(),
);
```

Also update the same pattern in `src/crates/primer-cli/src/main.rs` at line 502 (where the production `DialogueManager` is constructed). This is wired up properly in Task 10; for now, pass `None` so things compile:

```rust
        learner,
        // …
        None,    // learner_store — wired in Task 10
        // …
```

- [ ] **Step 6: Run tests to verify they pass**

```bash
cargo test -p primer-pedagogy
```

Expected: all previous tests pass + 2 new tests pass.

- [ ] **Step 7: Run the workspace**

```bash
cargo test --workspace
```

Expected: clean.

- [ ] **Step 8: Commit**

```bash
git add src/crates/primer-pedagogy/src/dialogue_manager.rs src/crates/primer-pedagogy/Cargo.toml src/crates/primer-cli/src/main.rs
git commit -m "$(cat <<'EOF'
feat(pedagogy): wire LearnerStore into DialogueManager

DialogueManager::new now takes Option<Arc<dyn LearnerStore>>; save points
at open_session, resume_session, after every Primer turn (covers Ok and
mid-stream Err), and close_session. Save failures are warn-on-error,
matching the SessionStore pattern.

CLI passes None for the learner_store arg as a placeholder; the adoption
flow lands in the next commit.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 10 — CLI adoption flow + birthday/name-mismatch tests

**Files:**
- Modify: `src/crates/primer-cli/src/main.rs`

Add `create_learner_with_id` helper, the adoption-flow match block, and tests for the birthday + name-mismatch cases.

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)]` section of `main.rs`:

```rust
    #[tokio::test]
    async fn cli_birthday_case_updates_age_and_keeps_uuid() {
        // Save a learner with age=8, simulate startup with --age=9, verify
        // the persisted row has age=9 with the same UUID and created_at.
        use chrono::Utc;
        use primer_core::storage::{LearnerStore, SessionStore};
        use primer_storage::SqliteSessionStore;
        use std::sync::Arc;

        let store = Arc::new(SqliteSessionStore::open(std::path::Path::new(":memory:")).unwrap());
        let original_id = Uuid::new_v4();
        let original_created = Utc::now() - chrono::Duration::days(365);
        let mut original = create_learner_with_id(original_id, "Binti", 8);
        original.profile.created_at = original_created;
        store.save_learner(&original).await.unwrap();

        // Simulate startup with --age=9.
        let cli_age: u8 = 9;
        let cli_name = "Binti";

        let learner = match store.load_learner().await.unwrap() {
            Some(mut existing) => {
                existing.profile.age = cli_age;
                existing.profile.last_active = Utc::now();
                store.save_learner(&existing).await.unwrap();
                existing
            }
            None => unreachable!(),
        };

        assert_eq!(learner.profile.id, original_id, "UUID stable across launches");
        assert_eq!(learner.profile.age, 9, "age updated to CLI value");
        assert_eq!(
            learner.profile.created_at.timestamp(),
            original_created.timestamp(),
            "created_at preserved",
        );
        // Silence unused-var warning; cli_name is here to make the
        // intent of the test explicit.
        let _ = cli_name;
    }

    #[tokio::test]
    async fn cli_name_mismatch_warns_and_keeps_persisted_name() {
        // Save with name="Binti", reload with --name="Other", verify the
        // persisted name stays "Binti".
        use primer_core::storage::LearnerStore;
        use primer_storage::SqliteSessionStore;
        use std::sync::Arc;

        let store = Arc::new(SqliteSessionStore::open(std::path::Path::new(":memory:")).unwrap());
        let original = create_learner_with_id(Uuid::new_v4(), "Binti", 8);
        store.save_learner(&original).await.unwrap();

        // Simulate startup with --name=Other.
        let cli_name = "Other";
        let cli_age: u8 = 8;

        let learner = match store.load_learner().await.unwrap() {
            Some(mut existing) => {
                if existing.profile.name != cli_name {
                    // Production code emits a tracing::warn! here.
                    // The test asserts the PERSISTED name is unchanged
                    // — it does not assert the warning was emitted (that
                    // would require hooking a tracing subscriber and is
                    // covered separately if needed).
                }
                existing.profile.age = cli_age;
                existing.profile.last_active = chrono::Utc::now();
                store.save_learner(&existing).await.unwrap();
                existing
            }
            None => unreachable!(),
        };

        assert_eq!(learner.profile.name, "Binti", "persisted name wins over CLI");
    }
```

(`primer-cli/Cargo.toml` may need `primer-storage` added under `[dev-dependencies]`. Check with `grep primer-storage src/crates/primer-cli/Cargo.toml`. If missing, add `primer-storage.workspace = true`.)

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p primer-cli cli_birthday_case_updates_age_and_keeps_uuid
```

Expected: FAIL — `create_learner_with_id` is undefined.

- [ ] **Step 3: Add `create_learner_with_id` helper**

In `main.rs`, replace the existing `create_learner` function (around line 299) with two functions:

```rust
fn create_learner(name: &str, age: u8) -> LearnerModel {
    create_learner_with_id(Uuid::new_v4(), name, age)
}

fn create_learner_with_id(id: Uuid, name: &str, age: u8) -> LearnerModel {
    LearnerModel {
        profile: LearnerProfile {
            id,
            name: name.to_string(),
            age,
            languages: vec!["en".to_string()],
            created_at: Utc::now(),
            last_active: Utc::now(),
        },
        concepts: vec![],
        preferences: LearningPreferences::default(),
        current_engagement: EngagementState::Engaged,
        recent_assessments: vec![],
    }
}
```

- [ ] **Step 4: Replace the `create_learner(...)` call site with the adoption flow**

Find the line in `main` where the production code constructs the learner (currently around line 467: `let learner = create_learner(&cli.name, cli.age);`). Replace it with the full adoption flow:

```rust
    let learner = match store.load_learner().await {
        Ok(Some(mut existing)) => {
            if existing.profile.name != cli.name {
                tracing::warn!(
                    "CLI --name {:?} differs from persisted learner name {:?}; using persisted",
                    cli.name,
                    existing.profile.name
                );
            }
            existing.profile.age = cli.age;
            existing.profile.last_active = Utc::now();
            if let Err(e) = store.save_learner(&existing).await {
                tracing::warn!("save_learner on startup failed: {e}");
            }
            existing
        }
        Ok(None) => {
            // First run on this file. Two sub-cases:
            //   1. Truly fresh DB → mint a new UUID.
            //   2. v3 DB with sessions but no learners row → adopt the
            //      most-recent session's learner_id so existing sessions
            //      are not orphaned.
            let id = match store.most_recent_session_learner_id().await {
                Ok(Some(uuid)) => {
                    tracing::info!("adopted learner_id {uuid} from existing sessions");
                    uuid
                }
                Ok(None) => Uuid::new_v4(),
                Err(e) => {
                    tracing::warn!("most_recent_session_learner_id failed: {e}; minting fresh UUID");
                    Uuid::new_v4()
                }
            };
            let fresh = create_learner_with_id(id, &cli.name, cli.age);
            if let Err(e) = store.save_learner(&fresh).await {
                tracing::warn!("save_learner on startup failed: {e}");
            }
            fresh
        }
        Err(e) => {
            // load_learner failing on startup is catastrophic — the file
            // is unreadable or the schema is corrupt. Propagate.
            return Err(anyhow::anyhow!("load_learner failed on startup: {e}"));
        }
    };
```

(`store` here is the `Arc<SqliteSessionStore>` already constructed earlier in `main`; the trait imports `LearnerStore` and `SessionStore` may need adding at the top of the file: `use primer_core::storage::{LearnerStore, SessionStore};` if not already present.)

- [ ] **Step 5: Wire the `learner_store` arg into the production `DialogueManager::new` call**

Find the `DialogueManager::new(...)` call site in `main` (around line 502). Replace the placeholder `None` (added in Task 9 Step 5) with `Some(Arc::clone(&store) as Arc<dyn LearnerStore>)`:

```rust
        let mut dm = DialogueManager::new(
            learner,
            backend.as_ref(),
            knowledge.as_ref(),
            Some(Arc::clone(&store) as Arc<dyn SessionStore>),
            Some(Arc::clone(&store) as Arc<dyn LearnerStore>),
            classifier_arc,
            pedagogy_config,
        );
```

(Match the actual variable names already used at the call site; the structure may already use a slightly different binding pattern — keep it consistent.)

- [ ] **Step 6: Run tests to verify they pass**

```bash
cargo test -p primer-cli
```

Expected: previous tests pass + 2 new tests pass.

- [ ] **Step 7: Run the workspace + clippy**

```bash
cargo test --workspace
cargo clippy --workspace --all-targets
```

Expected: both clean.

- [ ] **Step 8: Manual REPL check**

```bash
cargo run --bin primer -- --name Binti --age 8 --backend stub
```

Type a message. Quit with `quit`. Then:

```bash
sqlite3 ~/.primer/binti.db 'SELECT name, age FROM learners'
```

Expected: one row with `Binti|8`.

Restart with `--age 9`, type a message, quit. Re-query — `age` should now be `9`, the learner UUID (visible via `SELECT id FROM learners`) unchanged.

If the manual check passes, clean up: `rm ~/.primer/binti.db` (don't keep test data on disk).

- [ ] **Step 9: Commit**

```bash
git add src/crates/primer-cli/src/main.rs src/crates/primer-cli/Cargo.toml
git commit -m "$(cat <<'EOF'
feat(cli): learner-model adoption flow + create_learner_with_id helper

On startup, load_learner() first; if Some, persisted UUID + name win
(--age updates, --name mismatch warns). If None, adopt the most-recent
session's learner_id (closing the orphan-session class on v3 → v4
upgrades) or mint fresh. save_learner materialises the row.

Closes the in-memory LearnerModel / Session.learner_id divergence bug
documented in CLAUDE.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 11 — Doc updates: CLAUDE.md + ROADMAP.md

**Files:**
- Modify: `CLAUDE.md`
- Modify: `ROADMAP.md`

- [ ] **Step 1: Update `CLAUDE.md`**

Find and update three sections:

**(a) The architecture section mention of schema version.** Locate the paragraph describing `primer-storage` (currently in the Architecture section). Update text that says "Schema is at version 3" / "v3 added the `turn_classifications` table" to add v4:

> Schema is at version 4: v2 added `sessions.summary` + `sessions.summary_through_turn_index` and a `turn_text_fts` FTS5 virtual table; v3 added the `turn_classifications` table for persisting every `EngagementAssessment`; v4 added the `understanding_depths` lookup plus `learners` and `learner_concepts` tables for learner-model SQLite persistence (Phase 0.3).

Find the existing list of trait method names for `SessionStore` and add `most_recent_session_learner_id` to it. Find the trait list (`SessionStore`, etc.) and add `LearnerStore` to the description (a new trait, alongside `SessionStore`, with `load_learner` + `save_learner`).

**(b) Schema-migration template.** The existing bullet about migrations mentions v2 + v3 patterns. Add a sentence noting that schema is now at v4 and the v4 migration is schema-only — adoption of an existing session's `learner_id` is the CLI's responsibility, not the migration's.

**(c) Remove the divergence gotcha.** Find this bullet under "Watch-outs" (or wherever it appears in CLAUDE.md):

> - **In-memory `LearnerModel` and `Session.learner_id` can diverge after resume.** The CLI builds the LearnerModel from `--name`/`--age` flags; the loaded session keeps its original `learner_id`. Closed by Option A (learner-model persistence).

Either delete it or replace with a closed-status note:

> - **Closed:** the in-memory-vs-on-disk `learner_id` divergence is now resolved. The CLI's adoption flow on first-run uses `SessionStore::most_recent_session_learner_id()` to inherit the existing session's UUID. See `LearnerStore` for the persistence trait.

- [ ] **Step 2: Update `ROADMAP.md`**

Find the Phase 0.3 bullet:

```markdown
- [ ] Implement learner model persistence (SQLite) so the Primer remembers what a child knows across sessions
```

Change to:

```markdown
- ✅ Implement learner model persistence (SQLite) so the Primer remembers what a child knows across sessions — landed via the `LearnerStore` trait + schema v4 migration; CLI startup adopts existing `sessions.learner_id` on first-run-after-upgrade
```

- [ ] **Step 3: Commit the docs**

```bash
git add CLAUDE.md ROADMAP.md
git commit -m "$(cat <<'EOF'
docs: update CLAUDE.md and ROADMAP.md for learner-model persistence

Schema now at v4; LearnerStore and most_recent_session_learner_id
documented; the in-memory/Session.learner_id divergence gotcha is
removed (closed). Phase 0.3 learner-persistence bullet ticked.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 4: Final verification**

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets
cargo fmt --check
```

All four must be clean. Test count should be roughly +14 to +16 from the 195 baseline.

If everything passes, the feature is complete on the branch. Push only when the user asks:

```bash
git push -u origin feature/learner-model-persistence
```

---

## Spec coverage summary

Each spec acceptance-checklist item maps to one or more tasks:

| Spec checklist item | Task(s) |
|---|---|
| `apply_v4_migrations` schema-only, idempotent + transactional, no `learners` insert | Task 3 |
| `understanding_depths` lookup + catalog functions | Tasks 1, 2, 4 (seeding) |
| `UnderstandingDepth::ALL` constant | Task 1 |
| `learners` and `learner_concepts` tables + FKs + indices | Task 3 |
| FK enforcement (PRAGMA foreign_keys covers v4 too) | Task 4 |
| `LearnerStore` trait, `load_learner` + `save_learner`, re-exported | Task 6 (trait) + Tasks 7, 8 (impls) |
| `SessionStore::most_recent_session_learner_id` | Task 5 |
| `SqliteSessionStore` implements both traits | Tasks 5, 7, 8 |
| `DialogueManager::new` takes `Option<Arc<dyn LearnerStore>>`, saves at five points | Task 9 |
| CLI adoption flow + `create_learner_with_id` | Task 10 |
| `serde_json` dep | Task 6 |
| Tests pass, clippy clean | Steps inside each task + Task 11 final |
| CLAUDE.md + ROADMAP.md updates | Task 11 |

## Self-review notes (inline fixes applied)

- **Type consistency:** `understanding_depth_id` / `_name` / `_from_id` / `expected_understanding_depths` names are consistent across Tasks 2, 3 (consumed in `apply_v4_migrations` indirectly), 4 (consumed in `validate_and_seed_lookup`), 7 (consumed in `save_learner`), and 8 (consumed in `load_learner`). `LearnerStore::save_learner` / `load_learner` and `SessionStore::most_recent_session_learner_id` are stable across Tasks 5, 6, 7, 8, 9, 10.
- **Five save points vs. four physical sites:** the spec lists five save points (open / resume / per-turn / mid-stream-error / close); the per-turn and mid-stream-error are one physical save site that runs on both `Ok` and `Err` (matching the existing `save_session` pattern). Task 9 Step 4 calls this out explicitly so the engineer doesn't add a duplicate write on the error path.
- **Test-mock impact (Task 5 Step 7):** the `SessionStore` mock in `dialogue_manager.rs` tests gets the new method; this is called out so the engineer doesn't get blindsided by a workspace test failure mid-task.
- **`primer-storage` dev-dep on `primer-pedagogy` and `primer-cli`:** Task 9 Step 1 and Task 10 Step 1 both flag possible Cargo.toml additions (`primer-storage.workspace = true` under `[dev-dependencies]`) — the engineer checks before assuming.
