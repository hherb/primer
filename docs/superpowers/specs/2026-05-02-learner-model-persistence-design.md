# Learner-model SQLite persistence

**Status:** approved design, ready for implementation plan
**Phase:** 0.3 (closes one of the five remaining bullets)
**Author:** brainstorming session, 2026-05-02

## Goal

Persist the `LearnerModel` to disk so a returning child carries forward their identity, concept-mastery state, learning preferences, and engagement-trajectory snapshot across sessions. Closes the documented divergence bug where the CLI builds a fresh `LearnerProfile { id: Uuid::new_v4(), … }` every run while a resumed session keeps its original `Session.learner_id` — they never agree today.

## Scope

**In scope**

- Schema v3 → v4 migration in `primer-storage`, additive only. New `learners` and `learner_concepts` tables; new `understanding_depths` lookup table. Migration creates schema and seeds the lookup; it does **not** insert any `learners` row.
- New `LearnerStore` trait in `primer-core::storage` with `load_learner` + `save_learner`. `SqliteSessionStore` implements it.
- New `SessionStore::most_recent_session_learner_id()` method to support the CLI's adoption-of-existing-sessions flow on first-run.
- `DialogueManager` wiring: `Option<Arc<dyn LearnerStore>>` on `new`, save points on open / resume / per-turn / mid-stream-error / close, warn-on-error policy.
- CLI startup: `load_learner` first; if `Some`, the persisted UUID + name win (`--age` updates persisted, `--name` mismatch logs a warning). If `None`, call `most_recent_session_learner_id()` — if `Some(uuid)`, mint a `LearnerModel` with that UUID + CLI flags (adoption case, eliminates the orphan-session class); if `None`, mint a fresh UUID. Either way, `save_learner` to materialise the row.

**Out of scope (deliberately deferred)**

- Concept extraction from child responses (separate Phase 0.3 bullet — `turn.concepts` is still always `vec![]` for child turns; the persistence path handles the empty case gracefully).
- LLM-based comprehension classifier (separate Phase 0.3 bullet).
- Per-child vocabulary tracking with spaced repetition (separate Phase 0.3 bullet — vocabulary will reuse `learner_concepts` with `concept_id = "vocab:..."` once that work lands).
- Session time tracking with break suggestions (separate Phase 0.3 bullet).
- Multi-learner-per-DB / household mode. The personal-device project memory rules this out: multi-user means separate OS logins, each with their own `$HOME` and `~/.primer/<slug>.db`.
- Forget-a-concept API. `save_learner` is monotonic on concepts. If we ever need explicit forgetting it lands as an explicit method.
- Aggregated cross-session engagement digests. The longitudinal record already exists in `turn_classifications`; aggregates compute on demand and only need persistence when they become a hot query path.

## Key decisions

### One learner per DB file, slug is the natural key

The Primer is a personal device. Multiple children share hardware via separate OS logins, not via in-app multi-user. The existing CLI layout (`~/.primer/<slug-of-name>.db`) already encodes this — the file *is* the learner's container. The schema enforces it at the application layer: there is at most one row in `learners` per DB. `load_learner()` takes no UUID argument because there is no lookup-by-id problem; the file boundary is the lookup.

This is a strict superset of any future "household mode" — relaxing the application-level invariant to allow multiple rows is non-destructive. We don't pre-build for that.

### Full-shape persistence (profile + concepts + preferences + engagement snapshot)

Every `LearnerModel` field that isn't trivially recoverable persists:

- `profile` — identity-critical, has to persist.
- `concepts` — the entire point of Phase 0.3.
- `preferences` — `early_disengagement_threshold` is already a real per-child tunable used today by `decide_intent_at`; the four learning-style scalars and `typical_session_minutes` are dead storage now but cheap to add and stable in shape.
- `current_engagement` — persists as a snapshot column on `learners`. The per-turn longitudinal record stays in `turn_classifications` (a row per turn already, FK'd through `turns → sessions → learners`). The snapshot is for resume continuity ("where did we leave off emotionally"); cross-session aggregates compute from the longitudinal table.
- `recent_assessments` — does **not** persist (already rehydrated from `turn_classifications` on resume).

### Per-turn save cadence

Same robustness pressure as session save: children pull power, drop devices, walk away. `save_learner` is called everywhere `save_session` is called — per turn, on open / resume / mid-stream error / close. Cost is negligible (one upsert on `learners` plus an upsert per concept; sub-millisecond at our scale). Loss window is at most one in-flight turn, matching the session contract.

### Save errors log, do not propagate

Same policy as `save_session`. Mismatches between in-memory state and on-disk state are surfaced via `tracing::warn!`; the REPL doesn't die because of a disk-full event during a Socratic question. The `DialogueManager` keeps running on the in-memory model — the next successful save catches up.

### Concept persistence is monotonic

`save_learner` upserts every concept in `learner.concepts` and never deletes a `learner_concepts` row. A concept dropped from the in-memory `Vec` stays on disk. Rationale: concept state is monotonic across a child's lifetime — knowing-then-not-knowing is a separate event ("forgetting") that should be an explicit operation, not a side effect. Mirrors `save_session`'s append-only-on-turns invariant.

### Vec<String> as JSON-in-TEXT (not sub-tables)

Three list-of-strings fields persist: `LearnerProfile.languages`, `LearningPreferences.high_engagement_topics`, `ConceptState.notes`. Each persists as a JSON-encoded array in a `TEXT` column. Sub-tables were considered and rejected because none of these lists are queried independently today, all are tiny (≤ a dozen entries each), and SQLite's `json_each` gives us a one-line query path the day that changes. ISO-639-1 language codes specifically are short enough that the integer-FK savings don't apply, and they carry unambiguous meaning on their own.

This is a deliberate departure from the project-wide "categorical text → integer FK" rule, which applies to *closed enums under Rust control* (Speaker, PedagogicalIntent, EngagementState, UnderstandingDepth — all of these get lookup tables). Language codes are not a closed enum; they're an external standard.

### Migration of existing per-learner DBs

The v4 migration creates schema only — it does **not** insert any `learners` row. `apply_v4_migrations` runs inside `SqliteSessionStore::open()` with no access to CLI flags, so it cannot populate `name` or `age`. The migration would otherwise have to write placeholder values that the CLI would then need to overwrite — fragile, and the sentinel-detection logic is more complex than just doing the work in one place.

Instead, **adoption is a CLI-level concern.** On startup, after `load_learner()` returns `Ok(None)`, the CLI calls a new `SessionStore::most_recent_session_learner_id()` method:

```sql
SELECT learner_id FROM sessions
ORDER BY started_at DESC
LIMIT 1;
```

If `Some(uuid)`, the CLI mints a `LearnerModel` with `id = uuid`, `name`/`age` from CLI flags, `LearningPreferences::default()`, `current_engagement = Unknown`, then calls `save_learner` to materialise the row. A `tracing::info!` line documents the adoption.

If multiple distinct `learner_id`s exist in `sessions` (pre-existing weirdness — manual edits, or `--name` swap without changing `--session-db`), the most-recent wins and a `tracing::warn!` fires once.

If `most_recent_session_learner_id()` returns `None` (truly fresh DB), the CLI mints a fresh UUID via `Uuid::new_v4()`.

Net effect: no orphan sessions after upgrade. Every existing `~/.primer/<slug>.db` ends up with a single `learners` row whose UUID is the same one its sessions already reference.

### `LearnerStore` is a separate trait, not an extension of `SessionStore`

`SessionStore` already covers four method shapes (`save_session`, `load_session`, `retrieve_session_turns`, `save_classification`, `load_recent_assessments`). Adding two more learner-shaped methods would push it toward a "everything-DB" trait that's hard to mock surgically and whose name no longer matches its surface.

`LearnerStore` is the cleaner abstraction:
- The lifecycles are different — the learner is loaded *once* on startup and saved *every turn*; the session is opened, mutated, and closed.
- The dialogue manager's reasoning about the two is independent.
- The concrete impl is unchanged: `SqliteSessionStore` implements both traits on the same struct, sharing one `Mutex<Connection>`.

Plumbing cost: one additional `Option<Arc<dyn LearnerStore>>` field on `DialogueManager`. The CLI hands the same `Arc` in both type-erased forms.

## Architecture

### Crate graph (unchanged)

```
primer-cli  →  primer-pedagogy  →  primer-core  ←  primer-inference
                                                ←  primer-knowledge
                                                ←  primer-storage
                                                ←  primer-classifier
                                                ←  primer-speech
```

`primer-storage` continues to depend only on `primer-core`, `rusqlite`, `async-trait`, `tokio`, `chrono`, `uuid`, plus `serde_json` (added — used for the JSON-in-TEXT encoding of the three `Vec<String>` fields). No new crate-graph edges.

### Trait

In `primer-core::storage`, alongside the existing `SessionStore`:

```rust
use async_trait::async_trait;
use uuid::Uuid;
use crate::error::Result;
use crate::learner::LearnerModel;

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

Re-exported from `primer-core::lib` so `primer-pedagogy` and `primer-cli` can depend on the trait without pulling in the impl crate.

The existing `SessionStore` trait gains one method to support the CLI's first-run adoption flow:

```rust
#[async_trait]
pub trait SessionStore: Send + Sync {
    // …existing methods unchanged…

    /// Return the `learner_id` of the most-recent session in this DB,
    /// if any. Used by the CLI on first-run after a v3 → v4 upgrade to
    /// adopt the existing session-id as the new learner's persistent
    /// UUID, eliminating the otherwise-orphan-session class.
    ///
    /// Returns `Ok(None)` for a DB with no sessions. Reserves `Err` for
    /// genuine I/O / decoding failures.
    async fn most_recent_session_learner_id(&self) -> Result<Option<Uuid>>;
}
```

Default impl in the trait would be tempting but is intentionally not provided — every concrete impl already touches the underlying store and the SELECT is a one-liner.

### Implementation

`SqliteSessionStore` (in `primer-storage`) implements both `SessionStore` and `LearnerStore` on the same struct, sharing the same `Mutex<Connection>`. No new struct, no second `open()`, no risk of two connections to the same file.

### Schema (v4)

```sql
-- ─── New lookup table ────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS understanding_depths (
    id   INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
);

-- ─── New tables ──────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS learners (
    id                            TEXT PRIMARY KEY,                                  -- UUID, matches sessions.learner_id
    name                          TEXT NOT NULL,
    age                           INTEGER NOT NULL,
    languages                     TEXT NOT NULL,                                     -- JSON array of ISO-639-1 codes
    created_at                    TEXT NOT NULL,                                     -- RFC 3339
    last_active                   TEXT NOT NULL,                                     -- RFC 3339
    -- LearningPreferences inline:
    pref_narrative                REAL NOT NULL,
    pref_socratic                 REAL NOT NULL,
    pref_visual                   REAL NOT NULL,
    pref_kinesthetic              REAL NOT NULL,
    typical_session_minutes       REAL NOT NULL,
    high_engagement_topics        TEXT NOT NULL,                                     -- JSON array of strings
    early_disengagement_secs      INTEGER NOT NULL,
    -- Engagement snapshot:
    current_engagement_state_id   INTEGER NOT NULL REFERENCES engagement_states(id)
);

CREATE TABLE IF NOT EXISTS learner_concepts (
    learner_id        TEXT NOT NULL REFERENCES learners(id) ON DELETE CASCADE,
    concept_id        INTEGER NOT NULL REFERENCES concepts(id),
    depth_id          INTEGER NOT NULL REFERENCES understanding_depths(id),
    confidence        REAL NOT NULL,
    encounter_count   INTEGER NOT NULL,
    last_encountered  TEXT,                                                          -- nullable, RFC 3339
    notes             TEXT NOT NULL DEFAULT '[]',                                    -- JSON array of strings
    PRIMARY KEY (learner_id, concept_id)
);

CREATE INDEX IF NOT EXISTS idx_learner_concepts_learner
    ON learner_concepts(learner_id);
```

Schema version bumps from 3 to 4. `apply_v4_migrations` follows the v3 template: `conn.unchecked_transaction()`, `CREATE TABLE IF NOT EXISTS` for every new object, `validate_and_seed_lookup` for `understanding_depths`, and `tx.commit()`. A partial failure rolls back to the pre-migration state — there is no half-v4 DB that subsequent saves miswrite to. The migration deliberately does not insert into `learners`; that's the CLI's job (see *Migration of existing per-learner DBs* under Key decisions).

### Catalog additions

In `primer-storage::catalog`, mirroring the existing `engagement_state_*` functions:

```rust
pub fn understanding_depth_id(d: UnderstandingDepth) -> i64 {
    match d {
        UnderstandingDepth::Unknown       => 1,
        UnderstandingDepth::Aware         => 2,
        UnderstandingDepth::Recall        => 3,
        UnderstandingDepth::Comprehension => 4,
        UnderstandingDepth::Application   => 5,
        UnderstandingDepth::Analysis      => 6,
    }
}

pub fn understanding_depth_name(d: UnderstandingDepth) -> &'static str {
    match d {
        UnderstandingDepth::Unknown       => "Unknown",
        UnderstandingDepth::Aware         => "Aware",
        UnderstandingDepth::Recall        => "Recall",
        UnderstandingDepth::Comprehension => "Comprehension",
        UnderstandingDepth::Application   => "Application",
        UnderstandingDepth::Analysis      => "Analysis",
    }
}

pub fn understanding_depth_from_id(id: i64) -> Option<UnderstandingDepth> { /* ALL.iter().find() */ }
pub fn expected_understanding_depths() -> Vec<(i64, &'static str)> { /* ALL.iter().map() */ }
```

Requires `UnderstandingDepth::ALL: &'static [Self]` in `primer-core::learner` — a small companion change, mirroring how `EngagementState::ALL` and `PedagogicalIntent::ALL` already exist.

### Save semantics

`save_learner(&LearnerModel)` runs in a single SQLite transaction:

1. `INSERT INTO learners (...) VALUES (...) ON CONFLICT(id) DO UPDATE SET (...)` — upserts the row in place. (Plain `INSERT OR REPLACE` is wrong: it's a DELETE-then-INSERT and would cascade through `learner_concepts.learner_id ON DELETE CASCADE` and wipe every concept we've already persisted. Same footgun as the `save_session` cascade we already learned about.)
2. For each `ConceptState` in `learner.concepts`:
   - `INSERT OR IGNORE INTO concepts (name) VALUES (?)` — ensures the FK target exists (open vocabulary, lazy population, mirrors `save_session`).
   - `SELECT id FROM concepts WHERE name = ?` — get the FK id.
   - `INSERT INTO learner_concepts (learner_id, concept_id, depth_id, confidence, encounter_count, last_encountered, notes) VALUES (...) ON CONFLICT(learner_id, concept_id) DO UPDATE SET (...)`.
3. Commit.

Cost: O(1 + N) writes per save, N = concept count. At ≤100 concepts per learner this is sub-millisecond. If profiling ever shows it matters, the obvious optimisation is to track a dirty-set on `Vec<ConceptState>` — explicit follow-up, not v1.

### `load_learner` semantics

Three SELECTs:

1. `SELECT * FROM learners` — at most one row (application invariant). Returns `None` → `Ok(None)`.
2. `SELECT lc.*, c.name FROM learner_concepts lc JOIN concepts c ON c.id = lc.concept_id WHERE lc.learner_id = ?` — every concept for the learner, with the joined string name to populate `ConceptState.concept_id`.
3. JSON-decode `languages`, `high_engagement_topics`, and each row's `notes`.

Result: a fully-populated `LearnerModel` with `current_engagement` set from the snapshot column and `recent_assessments` left empty (the dialogue manager rehydrates that from `turn_classifications` on resume — already does this today via `load_recent_assessments`).

## DialogueManager wiring

```rust
pub struct DialogueManager<'a> {
    pub learner: LearnerModel,
    pub session: Session,
    inference:    &'a dyn InferenceBackend,
    knowledge:    &'a dyn KnowledgeBase,
    storage:      Option<Arc<dyn SessionStore>>,
    learner_store: Option<Arc<dyn LearnerStore>>,    // new
    classifier:   Option<Arc<dyn EngagementClassifier>>,
    config:       PedagogyConfig,
}
```

Save points (all best-effort, warn-on-error, mirroring `save_session`):

| When                     | What                              | Why                                                          |
|--------------------------|-----------------------------------|--------------------------------------------------------------|
| `open_session`           | save_session, save_learner        | Snapshot `created_at` (first ever), `last_active`, baseline engagement |
| `resume_session`         | save_session, save_learner        | `last_active` updates; engagement snapshot may be stale      |
| After every Primer turn  | save_session, save_learner        | Hot path. Concept counts and engagement snapshot move every turn |
| Mid-stream error         | save_session, save_learner        | Encounter counts incremented before stream-start should still flush |
| `close_session`          | save_session, save_learner        | `current_engagement` reflects end-of-session, not mid-conversation transient |

`update_learner_model` is the choke-point — already called every turn ([dialogue_manager.rs:577](src/crates/primer-pedagogy/src/dialogue_manager.rs#L577)). The function extends to:

1. Bump `encounter_count` and `last_encountered` on any concepts the turn referenced. With concept-extraction still placeholder, this is a no-op for child turns; the path is wired so the next Phase 0.3 task plugs straight in.
2. Update `current_engagement` from `recent_assessments.last()` (the latest classification for this turn).
3. Fire `learner_store.save_learner(&self.learner)` in the warn-on-error pattern.

## CLI integration

In `main.rs`, after constructing the `SqliteSessionStore` (sketch — error handling collapsed for clarity; production code uses the same warn-on-error pattern as `save_session`):

```rust
let learner = match store.load_learner().await? {
    Some(mut existing) => {
        if existing.profile.name != cli.name {
            tracing::warn!(
                "CLI --name {:?} differs from persisted learner name {:?}; using persisted",
                cli.name, existing.profile.name
            );
        }
        existing.profile.age = cli.age;          // birthday case
        existing.profile.last_active = Utc::now();
        if let Err(e) = store.save_learner(&existing).await {
            tracing::warn!("save_learner on startup failed: {e}");
        }
        existing
    }
    None => {
        // First run on this file. Two sub-cases:
        //   1. Truly fresh DB → mint a new UUID.
        //   2. v3 DB with sessions but no learner row → adopt the most
        //      recent session's learner_id so existing sessions are not
        //      orphaned.
        let id = match store.most_recent_session_learner_id().await? {
            Some(uuid) => {
                tracing::info!("adopted learner_id {uuid} from existing sessions");
                uuid
            }
            None => Uuid::new_v4(),
        };
        let fresh = create_learner_with_id(id, &cli.name, cli.age);
        if let Err(e) = store.save_learner(&fresh).await {
            tracing::warn!("save_learner on startup failed: {e}");
        }
        fresh
    }
};
```

`create_learner_with_id` is a small refactor of the existing `create_learner(&str, u8)` helper to accept a pre-determined UUID instead of always minting a fresh one.

Persisted UUID always wins. `--name` mismatch logs a warning but does not refuse to start (a parent renaming a child would otherwise be locked out of their own data). `--age` updates persisted on every launch (covers birthdays).

For `--no-persist`, the `:memory:` `SqliteSessionStore` exposes `LearnerStore` too; `load_learner()` returns `Ok(None)` on first open, `most_recent_session_learner_id()` returns `Ok(None)` (in-memory DB has no prior sessions), and a fresh learner is minted in-memory. No file changes.

## Errors

No new variant. `PrimerError::Storage(String)` already covers the surface — the same wrapping pattern as `save_session` / `load_session`.

## Testing

### `primer-storage` unit tests (in-memory SQLite, async via `tokio::test`)

1. **v3 → v4 migration idempotency** — fresh DB, v3 DB, v4 DB all converge to `user_version = 4` with the same schema after `open()`. Migration must NOT insert a `learners` row.
2. **`understanding_depths` validate-and-seed** — fresh / partial / drift (name mismatch, unknown id) cases, mirroring the v3 `engagement_states` tests.
3. **First-run: empty DB** — `load_learner()` returns `Ok(None)`. `most_recent_session_learner_id()` returns `Ok(None)`.
4. **`most_recent_session_learner_id`: existing sessions, single `learner_id`** — fixture inserts a v3 DB with two sessions sharing one `learner_id`; method returns `Ok(Some(uuid))` with that id.
5. **`most_recent_session_learner_id`: multiple distinct `learner_id`s** — fixture inserts sessions with two different `learner_id`s and different `started_at`; method returns the one with the most-recent `started_at`. (Warning emission is the CLI's concern, not the storage layer's; this test covers the SELECT semantics only.)
6. **Round-trip** — save a `LearnerModel` with profile + 3 concepts (varying depths/confidence/encounter_count/notes) + non-default preferences + non-default engagement snapshot → load it back → all fields equal, JSON arrays preserved.
7. **Concept upsert idempotency** — save twice unchanged → row count and content unchanged.
8. **Concept update** — save → mutate `encounter_count` and `depth` in-memory → save again → DB reflects new values; one row, not two.
9. **Concept monotonicity** — save 3 concepts → drop one from in-memory `Vec` → save → all 3 still in DB.
10. **Every `UnderstandingDepth` variant round-trips** — one save with one concept per variant.
11. **Migration rollback** — fault-inject a missing `engagement_states` table; assert v4 columns/tables absent post-rollback (mirrors `apply_v2_migrations_rolls_back_on_failure`).
12. **FK enforcement** — `learner_concepts.depth_id = 99` rejected.

### `primer-pedagogy::dialogue_manager` integration tests

13. **End-to-end save** — `DialogueManager` with `Some(Arc<dyn LearnerStore>)` runs a scripted conversation; query DB → `learners.last_active` advanced, `current_engagement_state_id` reflects latest assessment.
14. **Divergence bug closed (CLI startup adoption)** — fixture: a v3 DB with sessions sharing UUID U1 and no `learners` row; bring it up to v4 by `SqliteSessionStore::open()`; run the CLI's first-run startup flow (`load_learner` → `most_recent_session_learner_id` → mint + `save_learner`). Assert the resulting `LearnerModel.profile.id == U1`. Then construct a `DialogueManager` and an `open_session` follow-up; `Session.learner_id` agrees with `LearnerModel.profile.id`.

### `primer-cli` integration tests

15. **Birthday case** — save with `age = 8`; reconstruct CLI startup with `--age = 9`; persisted age updates to 9; UUID and `created_at` unchanged.
16. **`--name` mismatch** — save with `name = "Binti"`; reconstruct CLI startup with `--name = "Other"`; warning emitted (capture via test subscriber); persisted name preserved.

### Manual REPL check (verification, not CI-blocking)

- `cargo run --bin primer -- --name Binti --age 8` → `sqlite3 ~/.primer/binti.db 'SELECT name, age FROM learners'` shows the row.
- Quit, re-run with `--age 9` → age updates, UUID stable.
- `cargo run --bin primer -- --name Binti --age 8 --no-persist` → no file changes.

**Test count delta:** roughly +12 in `primer-storage`, +4 in `primer-pedagogy::dialogue_manager`. Workspace total ≈ 195 → ~211.

## Risk register

- **`INSERT OR REPLACE` on `learners` would cascade-wipe `learner_concepts`.** Mitigated by using proper SQLite UPSERT (`INSERT … ON CONFLICT(id) DO UPDATE SET …`) on the upsert path. Same lesson the session-persistence work landed; the upsert path here mirrors that one exactly.
- **Two distinct `learner_id`s in `sessions` during CLI-startup adoption.** Most-recent wins, warning fires once. The alternative (refuse to start) would lock a parent out of their own data over a non-data-corrupting weirdness.
- **Startup vs per-turn error handling diverges.** `load_learner` and `most_recent_session_learner_id` failing on startup propagate via `?` — these failures imply the file is unreadable or the schema is corrupt, and continuing on a broken store is worse than failing loudly. Per-turn `save_learner` failures use the existing warn-on-error pattern — the in-memory model is intact, the next save catches up. Spec calls out this asymmetry to forestall a "why isn't this consistent?" review comment.
- **Persisted name vs `--name` mismatch.** The persisted name wins; warning fires. Same reasoning — a renamed child should not be locked out.
- **Concept state monotonicity could surprise a future contributor.** Documented on the trait method; explicit `forget_concept` is the path if forgetting becomes a real product surface.
- **JSON-in-TEXT for three `Vec<String>` fields lacks FK integrity.** Acceptable trade-off given how the fields are used; flippable to sub-tables in a later additive migration if a real query path appears.
- **Synchronous SQLite writes on the tokio runtime.** Same caveat as the existing `SqliteSessionStore`; revisit if a multi-process consumer ever appears.

## Acceptance checklist

- [ ] `apply_v4_migrations` in `primer-storage::schema`, idempotent + transactional. Creates schema and seeds `understanding_depths`. Does NOT insert any `learners` row.
- [ ] `understanding_depths` lookup table created and seeded; catalog functions added (`understanding_depth_id`, `understanding_depth_name`, `understanding_depth_from_id`, `expected_understanding_depths`).
- [ ] `UnderstandingDepth::ALL: &'static [Self]` added in `primer-core::learner`.
- [ ] `learners` and `learner_concepts` tables created with FKs and indices.
- [ ] `LearnerStore` trait in `primer-core::storage` with `load_learner` + `save_learner`. Re-exported from the lib root.
- [ ] `SessionStore::most_recent_session_learner_id()` method added to the existing trait. Implemented on `SqliteSessionStore`.
- [ ] `SqliteSessionStore` implements both `SessionStore` and `LearnerStore` on the same struct.
- [ ] `DialogueManager::new` takes `Option<Arc<dyn LearnerStore>>`; saves on open / resume / per-turn / mid-stream-error / close, warn-on-error.
- [ ] CLI startup: `load_learner` first; if `None`, `most_recent_session_learner_id` for adoption, falling back to `Uuid::new_v4()`; `save_learner` to materialise the row. Persisted UUID + name win on subsequent runs; `--age` updates persisted; `--name` mismatch warns.
- [ ] `create_learner_with_id` helper introduced (small refactor of `create_learner`).
- [ ] `serde_json` added to `primer-storage` dependencies (workspace pin if not already there).
- [ ] All new tests pass; `cargo test --workspace` clean.
- [ ] `cargo clippy --workspace --all-targets` clean.
- [ ] CLAUDE.md: schema is now v4; `LearnerStore` and `most_recent_session_learner_id` mentioned; the "in-memory `LearnerModel` and `Session.learner_id` divergence" gotcha removed (closed).
- [ ] ROADMAP.md: Phase 0.3 "Implement learner model persistence (SQLite)" bullet ticked.
