# Conversation persistence — SQLite-backed `SessionStore`

**Status:** approved design, ready for implementation plan
**Phase:** 0.1 (last open item)
**Author:** brainstorming session, 2026-04-30

## Goal

Persist every conversation session to disk so children can pick up where they left off, and so the dialogue engine can later treat past sessions as a searchable memory (cross-session pattern recognition, learning-difficulty surfacing, embedding-based retrieval, simple concept-graph queries).

This PR delivers the **recording side only**. Resume, cross-session queries, and embeddings are intentionally deferred — but the schema is shaped so they land as additive tables rather than rewrites.

## Scope

**In scope**

- New crate `primer-storage` (sibling to `primer-knowledge`).
- New trait `SessionStore` in `primer-core` with one method: `save_session`.
- `SqliteSessionStore` impl backed by `rusqlite` + bundled SQLite, mirroring the locking/error patterns of `SqliteKnowledgeBase`.
- Schema for sessions, turns, and turn-concept edges, with `PRAGMA user_version = 1`.
- CLI flag `--session-db <path>` on `primer-cli`, defaulting to `:memory:` for parity with `--knowledge-db`.
- Wiring in `DialogueManager` so every call to `respond_to_streaming` saves the current session, regardless of whether the inference call returned `Ok` or `Err`.
- Unit tests for the store, plus one integration test through `DialogueManager`.

**Out of scope (deliberately deferred)**

- Resume CLI flag and any session-discovery surface (no `--resume <id>`, no auto-resume-most-recent).
- Cross-session queries, learning-difficulty surfacing.
- Embeddings (sqlite-vec / sqlite-vss).
- Concept-graph relations.
- Learner-model persistence (Phase 0.3).
- Multi-DB / multi-user shielding beyond the existing `learner_id` column.

## Key decisions

### New crate, not extension of `primer-knowledge`

Sessions and the RAG knowledge base live in **separate SQLite files**, owned by separate crates, because they sit on opposite sides of a privacy boundary:

- The knowledge corpus is **shareable**. It will be updated periodically from public sources (Wikipedia, curated curricula). Multiple devices can carry an identical copy.
- Session data is **private to one child**. Nothing about it ever travels off-device without explicit per-child, per-export parental consent (per the principle in `ROADMAP.md`).

A SQLite database is a single portable file. Keeping the two stores in different files means "delete this child's data" is `rm sessions.db`, with no risk of co-mingling shareable corpus content. It also lets a parent inspect, archive, or back up sessions independently of the corpus.

### Recording only this PR

The user pivot was explicit: "this initial version, we can just focus on recording the session." Resume is cheap to add once recording is solid, but landing both at once doubles the test surface and the CLI complexity. The schema-first design means resume becomes "add `load_session` to the trait, add `--resume <id>` to the CLI" with no schema migration.

### Save on every turn (Q2 from brainstorming)

Children pull power, drop devices, walk away. Per-turn save is more robust than graceful-exit-only or signal-handling, and the cost (one small transactional write per turn) is negligible at our scale. This decision was approved before the SQLite pivot and still applies — SQLite transactions actually simplify the atomicity story versus the temp-file-rename dance JSON would have needed.

### Save errors log, do not propagate

If the disk is full or the DB file is read-only, `save_session` failures are reported via `tracing::warn!` and the REPL continues. Trade-off: a child may keep talking to a dead saver. Mitigation: clear log line; the parent / dev can spot it. Propagating instead would mask the (more important) inference-side error path with a downstream persistence failure. Reconsider once we have a parent dashboard that can surface persistence health visibly.

## Architecture

### Crate graph (after this PR)

```
primer-cli  →  primer-pedagogy  →  primer-core  ←  primer-inference
                                                ←  primer-knowledge
                                                ←  primer-storage   (new)
                                                ←  primer-speech
```

`primer-storage` depends only on `primer-core` (trait + types), `rusqlite`, `async-trait`, `tokio` (for the test attribute), and `chrono` (for timestamp parsing). It does **not** depend on `primer-knowledge` — the two SQLite-using crates stay independent.

### Trait

In a new module `primer-core::storage`:

```rust
use async_trait::async_trait;
use crate::conversation::Session;
use crate::error::Result;

#[async_trait]
pub trait SessionStore: Send + Sync {
    /// Persist the full current state of the session.
    ///
    /// Idempotent: calling repeatedly with the same `Session` is a no-op
    /// at the row level beyond the latest turn. Turns persisted in a
    /// previous call but no longer present in `session.turns` are removed.
    async fn save_session(&self, session: &Session) -> Result<()>;
}
```

Re-exported from `primer-core::lib` so `primer-pedagogy` and `primer-cli` can depend on the trait without pulling in the impl crate.

### Implementation

`primer-storage::SqliteSessionStore`, mirroring `SqliteKnowledgeBase`:

```rust
pub struct SqliteSessionStore {
    conn: Mutex<Connection>,
}

impl SqliteSessionStore {
    pub fn open(path: &Path) -> Result<Self> { … }
}

#[async_trait]
impl SessionStore for SqliteSessionStore {
    async fn save_session(&self, session: &Session) -> Result<()> { … }
}
```

`open` creates the schema (`CREATE TABLE IF NOT EXISTS`), checks `PRAGMA user_version`, and enables `PRAGMA foreign_keys = ON`. If the existing `user_version` is non-zero and not `1`, return `PrimerError::Storage` rather than silently corrupting a future schema.

### Schema (PRAGMA user_version = 1)

**Project convention applied:** every categorical text value is normalised into a lookup table and referenced by integer foreign key — saves storage at scale, makes invalid values impossible at the schema level, and makes "what intents has this child seen?" a one-line query over a small table. This applies to `speaker`, `intent`, and `concept_id`.

```sql
-- ─── Lookup tables (categorical text → integer FK) ───────────────

CREATE TABLE IF NOT EXISTS speakers (
    id    INTEGER PRIMARY KEY,
    name  TEXT NOT NULL UNIQUE
);

CREATE TABLE IF NOT EXISTS pedagogical_intents (
    id    INTEGER PRIMARY KEY,
    name  TEXT NOT NULL UNIQUE
);

-- Concepts are an open vocabulary — populated on first encounter
-- rather than seeded. Still normalised because concept_id strings
-- repeat heavily across turns ("physics:gravity" appearing in
-- dozens of sessions).
CREATE TABLE IF NOT EXISTS concepts (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    concept_id  TEXT NOT NULL UNIQUE
);

-- ─── Conversation tables ─────────────────────────────────────────

CREATE TABLE IF NOT EXISTS sessions (
    id          TEXT PRIMARY KEY,             -- SessionId UUID as TEXT
    learner_id  TEXT NOT NULL,
    started_at  TEXT NOT NULL,                 -- RFC 3339 / ISO 8601
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
    intent_id   INTEGER REFERENCES pedagogical_intents(id),  -- nullable
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

PRAGMA user_version = 1;
```

### Lookup-table seeding and drift detection

**Single source of truth: the Rust enum.** The `speakers` and `pedagogical_intents` rows in the DB are a *derived projection* of the corresponding Rust types, regenerated and validated on every `SqliteSessionStore::open(...)` call. The crate exposes no public API for inserting, updating, or deleting rows in either lookup table — the only way to add a new variant is to change the Rust source and recompile. New categories will almost always require pedagogical-engine changes anyway, so the recompile cost is buying genuine safety, not bureaucratic friction.

Each lookup table maps to a pair of pieces of Rust data co-located with the type definition (or in a small `primer-storage` companion module if we don't want `primer-core` to know its IDs):

- A list of every variant (e.g. `PedagogicalIntent::ALL: &'static [Self]`).
- A canonical `(variant) → (id, name)` mapping via an exhaustive `match`. Explicit integer IDs (never `as i64`), since variant declaration order in Rust must not be allowed to silently shift on-disk identity.

On every `open()`, **before any reads or writes are accepted**, the storage layer runs a *validate-and-seed* pass per lookup table:

1. Read every existing `(id, name)` row from the table.
2. For each Rust variant in `ALL`:
   - Compute `(expected_id, expected_name)` via the canonical match.
   - If the DB has a row at `expected_id` with a different `name` → return `PrimerError::Storage` with a clear message ("`pedagogical_intents` row id=4 has name 'Scaffolding' but Rust expects 'Encouragement' — incompatible schema/code mismatch"). Do **not** rewrite — drift this severe means a code/data mismatch the user has to investigate.
   - If the DB has no row at `expected_id` → insert it.
3. For each row in the DB whose `id` is not in the Rust `ALL` set → return `PrimerError::Storage` ("`pedagogical_intents` has unknown row id=9 'NewIntent' — database may be from a newer build"). Don't delete: the row may be load-bearing for the newer code.

This gives:
- **Automatic seeding** on first open (empty DB → fully populated tables).
- **Drift detection** at startup (mismatched names, missing IDs, unknown IDs all surface immediately).
- **One source of truth** (Rust). The DB never authoritatively defines what categories exist; it just stores a snapshot for FK-integrity purposes.
- **Safe upgrades**: opening a current DB with current code is a no-op. Opening an older DB with current code seeds the new variant. Opening a *newer* DB with older code fails loudly.

**Adding a new variant** in future is therefore mechanical:
1. Add the Rust variant.
2. Append an entry to `ALL`.
3. Add the arm to the canonical match (with a fresh integer ID — never reuse a retired one).
4. Recompile. First `open()` against an existing DB seeds the new row.

**Renaming a variant** is mechanical too: update the Rust name, update the canonical match (keeping the integer ID stable). The validate-and-seed pass detects the name change and updates the DB row in step 2 (well — the spec above currently treats name mismatch as an error rather than a rewrite; see Risk Register for the trade-off).

**Concepts** are **not** treated this way. They're an open, runtime-populated vocabulary — there is no Rust source of truth, only "whatever `turn.concepts` contained at save time." Entries are created lazily via `INSERT OR IGNORE INTO concepts (concept_id) VALUES (?)` followed by `SELECT id FROM concepts WHERE concept_id = ?`, with a `HashMap<String, i64>` cache for the duration of the enclosing transaction.

### Future-proofing — additive tables

These are not built in this PR, but the schema is shaped so they slot in cleanly:

- `turn_embeddings(turn_id INTEGER PRIMARY KEY REFERENCES turns(id), model_id INTEGER REFERENCES embedding_models(id), embedding BLOB)` — for sqlite-vec / sqlite-vss attached later. Plus its own lookup table `embedding_models(id, name)`.
- `concept_relations(from_concept INTEGER REFERENCES concepts(id), to_concept INTEGER REFERENCES concepts(id), kind_id INTEGER REFERENCES relation_kinds(id), weight REAL)` — for the simple-graph "topics that need addressing" use case. Reuses the existing `concepts` table.
- `learner_concepts(learner_id, concept_id INTEGER REFERENCES concepts(id), depth_id INTEGER REFERENCES understanding_depths(id), confidence, encounter_count, last_encountered, …)` — Phase 0.3 learner-model persistence. Reuses `concepts`; new lookup table `understanding_depths` for the `UnderstandingDepth` enum.

Each is purely additive, follows the same lookup-table convention, and bumps `user_version` to `2`, `3`, etc. as needed. None require touching the six tables above.

## Save semantics

`save_session(&Session)` runs in a single SQLite transaction:

1. `INSERT OR REPLACE INTO sessions (...)` — upserts session metadata.
2. `DELETE FROM turns WHERE session_id = ?` — clears existing turns. `ON DELETE CASCADE` removes the corresponding `turn_concepts` rows.
3. For each turn in `session.turns` (in order):
   - Look up the speaker integer ID via the in-memory `speaker_id(...)` mapping.
   - Look up the intent integer ID (or `NULL`) via the in-memory `intent_id(...)` mapping.
   - `INSERT INTO turns (session_id, turn_index, speaker_id, text, timestamp, intent_id)`.
   - For each `concept_id` string in `turn.concepts`:
     - `INSERT OR IGNORE INTO concepts (concept_id) VALUES (?)`.
     - `SELECT id FROM concepts WHERE concept_id = ?` (cached in a `HashMap<String, i64>` for the duration of this save call to avoid re-querying within the transaction).
     - `INSERT INTO turn_concepts (turn_id, concept_id) VALUES (?, ?)`.
4. Commit.

Cost: O(n + m) writes per save, n = turn count, m = total concept references. At ≤100 turns/session and a handful of concepts per turn, sub-millisecond. If profiling later shows this matters, switch to an append-only `record_turn` API path; the trait can grow the method without breaking callers.

## DialogueManager wiring

```rust
pub struct DialogueManager<'a> {
    pub learner: LearnerModel,
    pub session: Session,
    inference: &'a dyn InferenceBackend,
    knowledge: &'a dyn KnowledgeBase,
    storage:   Option<&'a dyn SessionStore>,   // new
    config: PedagogyConfig,
}

impl<'a> DialogueManager<'a> {
    pub fn new(
        learner: LearnerModel,
        inference: &'a dyn InferenceBackend,
        knowledge: &'a dyn KnowledgeBase,
        storage: Option<&'a dyn SessionStore>,  // new
        config: PedagogyConfig,
    ) -> Self { … }
}
```

In `respond_to_streaming`, the streaming-loop body needs a small structural change: today a mid-stream `Err` propagates via `?` directly out of the function, so there's no point at which both the success and error paths converge before the return. The implementation will refactor the inner loop to land its `Result` in a local variable (or an explicit `match`), and after that the save call runs in one place — covering both branches — before propagating the result. Pseudocode:

```rust
// success path: Primer turn already recorded
// error path:   only child turn recorded
if let Some(store) = self.storage {
    if let Err(e) = store.save_session(&self.session).await {
        tracing::warn!("session save failed: {e}");
    }
}
result   // either Ok(text) or Err propagated from the stream
```

The child's input is never silently dropped from the persisted record. `open_session` likewise saves after recording the greeting turn.

## CLI

New flag in `primer-cli`:

```
--session-db <path>     SQLite path for session storage (default: :memory:)
```

Defaulting to `:memory:` matches `--knowledge-db` and means the existing "no flags" REPL behaviour is unchanged: sessions still happen, they're just discarded on exit. To get persistence the user passes a real path.

`main` constructs the `SqliteSessionStore` once per process and hands a `&dyn SessionStore` to `DialogueManager::new`. If `--session-db` is `:memory:`, the store still works (saves to an in-memory DB) — no special-case branch.

## Errors

New variant in `primer-core::error::PrimerError`:

```rust
Storage(String),
```

Wrapping at the `SqliteSessionStore` boundary so callers see one variant for both rusqlite errors and any timestamp/UUID parse failures. Pattern matches `Knowledge` and `Inference`.

## Testing

### `primer-storage::tests`

Connection-against-`:memory:` for all unit tests. Uses `tokio::test` since the trait is async.

1. **Schema bootstrap** — opening a fresh DB sets `user_version = 1`, creates all six tables (3 lookup, 3 conversation), foreign keys enabled.
2. **Lookup-table seed (fresh DB)** — `speakers` and `pedagogical_intents` are populated with every Rust variant on first open, with the IDs from the canonical match. Re-opening the same DB is a clean no-op (no duplicates, no errors).
2a. **Lookup-table drift detection — name mismatch** — pre-populate `pedagogical_intents` with `(1, 'Wrong')` then call `open()` → returns `Storage` error mentioning the id, the expected name, and the found name.
2b. **Lookup-table drift detection — unknown row** — pre-populate `pedagogical_intents` with `(99, 'FromTheFuture')` then call `open()` → returns `Storage` error mentioning the unknown id.
2c. **Lookup-table partial seed** — pre-populate with a subset of expected rows, call `open()` → the missing rows are inserted; existing matching rows are untouched; no error.
3. **Round-trip** — build a `Session` with one Child turn (text + concepts) and one Primer turn (text + intent + concepts) → `save_session` → query the DB directly via raw SQL **joined to the lookup tables** → all fields equal, including timestamps preserved as RFC 3339, intent name resolved correctly, concept set per turn.
4. **Idempotent re-save** — call `save_session` twice with the same `Session` → turn-row count and content unchanged, no duplicate rows in `concepts`, no duplicate rows in `turn_concepts`.
5. **Append-a-turn** — save → mutate in-memory `session.turns.push(...)` → save again → second turn appears with correct `turn_index`, first turn preserved.
6. **Concept normalisation** — a session that uses `"gravity"` in three different turns produces exactly one row in `concepts` and three rows in `turn_concepts`. Re-saving across sessions reuses the same `concepts.id`.
7. **Empty session** — saves with zero turn rows, no error.
8. **Cascade** — `DELETE FROM sessions WHERE id = ?` removes its turns and turn-concept rows. Concept rows themselves remain (a concept once seen survives the deletion of any one session that referenced it). Sanity check on the schema.
9. **Foreign-key enforcement** — a `turns` row with an unknown `speaker_id` or `intent_id` is rejected (proves `PRAGMA foreign_keys = ON` took effect).
10. **Schema-version mismatch** — open a DB pre-set to `user_version = 99` → `Storage` error mentioning the version.
11. **Every variant of `PedagogicalIntent` round-trips** — a single test that saves one turn per variant and reads them all back. Adding a new variant without updating `intent_id`/the seed will fail this exhaustively.

### `primer-pedagogy::dialogue_manager::tests`

One new test: build a `DialogueManager` with `Some(&store)` (in-memory `SqliteSessionStore`), run a scripted conversation that completes successfully, query the DB to confirm both child and Primer turns landed with correct `turn_index` and `intent`. Plus one test for the mid-stream-error path: confirm the child turn is persisted even though the call returned `Err`.

### Manual REPL check

`cargo run --bin primer -- --session-db /tmp/primer-test.db --backend stub` — have a few exchanges, quit, re-open the file with `sqlite3 /tmp/primer-test.db` and inspect rows. Listed in the verification checklist; not blocking automated CI.

## Risk register

- **Save-on-error masking inference errors.** Mitigated by `tracing::warn!`-only on save failure, and by saving only after the child turn is recorded (never replacing inference's `Err` with a save's `Err`).
- **Schema drift breaking existing DBs.** `PRAGMA user_version` gate makes corruption explicit, not silent. Future additive tables only — no destructive migrations until / unless the schema changes meaningfully.
- **`PedagogicalIntent` variant rename.** As specified, the validate-and-seed pass treats a name mismatch on a known ID as an *error* rather than silently rewriting the row. This is deliberately strict — it forces the developer to acknowledge that an existing DB is being upgraded — but means a rename is a two-step migration in production: ship code that accepts both old and new names for a release, then drop the old. Alternative we considered and rejected for v1: silently `UPDATE pedagogical_intents SET name = ? WHERE id = ?` to overwrite the DB to match the Rust source. Rejected because it disguises code/data mismatches that are sometimes a sign of operator error (wrong DB file, wrong build). If renames become frequent, we'll revisit.
- **Adding a new `PedagogicalIntent` variant.** Mechanical: pick a previously-unused integer ID, append to `ALL`, extend the canonical match. The exhaustive match enforces the Rust side at compile time; the validate-and-seed pass populates the DB on next `open()`. Retired variants keep their ID forever — never reuse.
- **Source-of-truth duplication.** The Rust enum is canonical; the DB is a derived projection. `primer-storage` exposes no API to mutate lookup tables, so the only way to introduce drift is to edit the DB by hand — and that's caught at next startup by the validate-and-seed pass.
- **`turn.concepts` is currently always `vec![]` for child turns.** This PR persists what's there. Phase 0.3 concept-extraction will start populating it; no schema change needed.
- **Multi-process access.** SQLite supports it, but we're single-process today. WAL mode is **not** enabled in this PR; default rollback-journal mode is fine for one writer. Revisit when a parent-dashboard process appears.

## Acceptance checklist

- [ ] `primer-storage` crate compiles, is in the workspace, has `rusqlite` (with `bundled` feature, matching `primer-knowledge`).
- [ ] `SessionStore` trait added to `primer-core::storage`, re-exported from the lib root.
- [ ] `SqliteSessionStore::open(path)` creates schema, sets `PRAGMA foreign_keys = ON`, asserts/sets `user_version = 1`, and runs the **validate-and-seed** pass for `speakers` and `pedagogical_intents`.
- [ ] Validate-and-seed: rejects name mismatches and unknown IDs with a clear `Storage` error; inserts missing rows; no-op on a fully-consistent DB.
- [ ] No public function on `primer-storage` permits inserting/updating/deleting rows in `speakers` or `pedagogical_intents` (lookup tables are derived from Rust, not editable from outside).
- [ ] `save_session` is transactional, idempotent, persists all turns + concepts, and uses integer FKs for speaker / intent / concept.
- [ ] `intent_id`, `speaker_id` mappings use explicit integer values (no `as i64`).
- [ ] `--session-db` flag added to `primer-cli`, defaulting to `:memory:`.
- [ ] `DialogueManager::new` takes an optional `&dyn SessionStore`; `respond_to_streaming` saves on both Ok and Err paths; `open_session` saves the greeting.
- [ ] Save failures `tracing::warn!`-log without propagating.
- [ ] All fourteen unit tests in `primer-storage` pass.
- [ ] Both new `dialogue_manager` integration tests pass.
- [ ] `cargo test --workspace` clean (now ~58 tests; was 42).
- [ ] `cargo clippy --workspace --all-targets` clean except the pre-existing `StubBackend::new` warning.
- [ ] Manual REPL check: data round-trips, file is inspectable with `sqlite3`.
- [ ] `ROADMAP.md` Phase 0.1 "conversation persistence" bullet checked off.
- [ ] `CLAUDE.md` updated: new crate in the dependency graph, `--session-db` flag listed in commands, `Mutex<Connection>` pattern noted, **lookup-table-for-categorical-text convention** added under "Conventions and gotchas worth knowing".
