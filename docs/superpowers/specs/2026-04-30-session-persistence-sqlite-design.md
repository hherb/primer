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

```sql
CREATE TABLE IF NOT EXISTS sessions (
    id          TEXT PRIMARY KEY,           -- SessionId UUID as TEXT
    learner_id  TEXT NOT NULL,
    started_at  TEXT NOT NULL,               -- RFC 3339 / ISO 8601
    ended_at    TEXT
);
CREATE INDEX IF NOT EXISTS idx_sessions_learner
    ON sessions(learner_id, started_at);

CREATE TABLE IF NOT EXISTS turns (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id  TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    turn_index  INTEGER NOT NULL,
    speaker     TEXT NOT NULL CHECK(speaker IN ('child','primer')),
    text        TEXT NOT NULL,
    timestamp   TEXT NOT NULL,
    intent      TEXT,                         -- nullable for child turns
    UNIQUE(session_id, turn_index)
);
CREATE INDEX IF NOT EXISTS idx_turns_session
    ON turns(session_id, turn_index);

CREATE TABLE IF NOT EXISTS turn_concepts (
    turn_id     INTEGER NOT NULL REFERENCES turns(id) ON DELETE CASCADE,
    concept_id  TEXT NOT NULL,
    PRIMARY KEY(turn_id, concept_id)
);
CREATE INDEX IF NOT EXISTS idx_turn_concepts_concept
    ON turn_concepts(concept_id);

PRAGMA user_version = 1;
```

The `intent` column stores the `PedagogicalIntent` enum as a string. A pair of explicit `intent_to_str` / `str_to_intent` functions lives next to the impl, using a `match` over every variant — **not** the `Debug` derive, since `Debug`'s output is not part of `primer-core`'s contract and could change. The strings happen to look like the variant names (`"SocraticQuestion"`, etc.), but the mapping is owned by `primer-storage`. Acceptable risk: adding a new variant to `PedagogicalIntent` without updating the mapping is caught by an exhaustive `match` — the crate won't compile until the mapping is extended.

### Future-proofing — additive tables

These are not built in this PR, but the schema is shaped so they slot in cleanly:

- `turn_embeddings(turn_id INTEGER PRIMARY KEY REFERENCES turns(id), model TEXT, embedding BLOB)` — for sqlite-vec / sqlite-vss attached later.
- `concept_relations(from_concept TEXT, to_concept TEXT, kind TEXT, weight REAL)` — for the simple-graph "topics that need addressing" use case.
- `learner_concepts(learner_id, concept_id, depth, confidence, encounter_count, last_encountered, …)` — Phase 0.3 learner-model persistence.

Each is purely additive and bumps `user_version` to `2`, `3`, etc. as needed. None require touching the three tables above.

## Save semantics

`save_session(&Session)` runs in a single SQLite transaction:

1. `INSERT OR REPLACE INTO sessions (...)` — upserts session metadata.
2. `DELETE FROM turns WHERE session_id = ?` — clears existing turns.
3. For each turn in `session.turns` (in order), `INSERT INTO turns (...)` and then for each `concept_id` in `turn.concepts`, `INSERT INTO turn_concepts (...)`.
4. Commit.

`ON DELETE CASCADE` on `turn_concepts` handles cleanup automatically.

Cost: O(n) writes per save, n = turn count. At ≤100 turns/session, sub-millisecond. If profiling later shows this matters, switch to an append-only `record_turn` API path; the trait can grow the method without breaking callers.

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

1. **Schema bootstrap** — opening a fresh DB sets `user_version = 1`, creates all three tables, foreign keys enabled.
2. **Round-trip** — build a `Session` with one Child turn (text + concepts), one Primer turn (text + intent + concepts) → `save_session` → query the DB directly via raw SQL → all fields equal, including timestamps preserved as RFC 3339 and concept set per turn.
3. **Idempotent re-save** — call `save_session` twice with the same `Session` → row counts unchanged, content unchanged.
4. **Append-a-turn** — save → mutate in-memory `session.turns.push(...)` → save again → second turn appears with correct `turn_index`, first turn preserved.
5. **Concept persistence** — a turn with `concepts: ["gravity", "mass"]` round-trips both rows in `turn_concepts`.
6. **Empty session** — saves with zero turn rows, no error.
7. **Cascade** — `DELETE FROM sessions WHERE id = ?` removes its turns and turn-concept rows. Sanity check on the schema, not the API surface.
8. **Schema-version mismatch** — open a DB pre-set to `user_version = 99` → `Storage` error mentioning the version.

### `primer-pedagogy::dialogue_manager::tests`

One new test: build a `DialogueManager` with `Some(&store)` (in-memory `SqliteSessionStore`), run a scripted conversation that completes successfully, query the DB to confirm both child and Primer turns landed with correct `turn_index` and `intent`. Plus one test for the mid-stream-error path: confirm the child turn is persisted even though the call returned `Err`.

### Manual REPL check

`cargo run --bin primer -- --session-db /tmp/primer-test.db --backend stub` — have a few exchanges, quit, re-open the file with `sqlite3 /tmp/primer-test.db` and inspect rows. Listed in the verification checklist; not blocking automated CI.

## Risk register

- **Save-on-error masking inference errors.** Mitigated by `tracing::warn!`-only on save failure, and by saving only after the child turn is recorded (never replacing inference's `Err` with a save's `Err`).
- **Schema drift breaking existing DBs.** `PRAGMA user_version` gate makes corruption explicit, not silent. Future additive tables only — no destructive migrations until / unless the schema changes meaningfully.
- **`PedagogicalIntent` variant rename.** Enum-as-string is fragile. Acceptable for v1 because the variant set is short and load-bearing; renames will be deliberate. If churn becomes a concern, switch to integer codes.
- **`turn.concepts` is currently always `vec![]` for child turns.** This PR persists what's there. Phase 0.3 concept-extraction will start populating it; no schema change needed.
- **Multi-process access.** SQLite supports it, but we're single-process today. WAL mode is **not** enabled in this PR; default rollback-journal mode is fine for one writer. Revisit when a parent-dashboard process appears.

## Acceptance checklist

- [ ] `primer-storage` crate compiles, is in the workspace, has `rusqlite` (with `bundled` feature, matching `primer-knowledge`).
- [ ] `SessionStore` trait added to `primer-core::storage`, re-exported from the lib root.
- [ ] `SqliteSessionStore::open(path)` creates schema, sets `PRAGMA foreign_keys = ON`, asserts/sets `user_version = 1`.
- [ ] `save_session` is transactional, idempotent, and persists all turns + concepts.
- [ ] `--session-db` flag added to `primer-cli`, defaulting to `:memory:`.
- [ ] `DialogueManager::new` takes an optional `&dyn SessionStore`; `respond_to_streaming` saves on both Ok and Err paths; `open_session` saves the greeting.
- [ ] Save failures `tracing::warn!`-log without propagating.
- [ ] All eight unit tests in `primer-storage` pass.
- [ ] Both new `dialogue_manager` integration tests pass.
- [ ] `cargo test --workspace` clean (now ~52 tests; was 42).
- [ ] `cargo clippy --workspace --all-targets` clean except the pre-existing `StubBackend::new` warning.
- [ ] Manual REPL check: data round-trips, file is inspectable with `sqlite3`.
- [ ] `ROADMAP.md` Phase 0.1 "conversation persistence" bullet checked off.
- [ ] `CLAUDE.md` updated: new crate in the dependency graph, `--session-db` flag listed in commands, `Mutex<Connection>` pattern noted.
