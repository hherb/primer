# Storage and sessions

This chapter is about how the Primer remembers conversations. It covers the [SessionStore](../../src/crates/primer-core/src/storage.rs) and [LearnerStore](../../src/crates/primer-core/src/storage.rs) traits, the [SqliteSessionStore](../../src/crates/primer-storage/src/store/mod.rs) implementation, the schema-migration discipline that lets old DBs upgrade in place without ever running a destructive migration, the lookup-table normalisation rule that keeps every categorical text column in sync with its Rust enum, the append-only `save_session` invariant and the explicit backfill paths that work around it, the FTS5 sanitisation layer that lets raw child input drive search safely, and the long-term-memory + turn-embedding-on-save pipelines that surface relevant older turns into future system prompts. One recipe at the end walks through adding a new schema version end-to-end.

If you read chapter 3 you already saw the dialogue manager hand finished turns to a `SessionStore`; this chapter is what happens once it does.

## Privacy split

Each child gets their own session database, kept entirely separate from the RAG corpus that lives in the knowledge base. By default the CLI puts it at `~/.primer/<slug>.db`, where `<slug>` is the NFC-normalised, Unicode-aware slug of the learner's name (so `José` and the decomposed `José` map to the same file). The directory is created on first run; an unwritable home is a hard error rather than a silent in-memory fallback.

The split exists because the two stores hold fundamentally different data. The session DB holds **conversational PII**: every turn the child has spoken, the rolling LLM summary, per-turn engagement and comprehension classifications, the longitudinal concept-mastery record, and (since v8) per-turn embeddings of the child's words. The knowledge base, by contrast, holds **public-domain reference text**: hand-drafted CC0 passages and CC-BY-SA Wikipedia leads. Mixing them — even "just for prototyping" — would cross a line the project has chosen not to cross.

> **Why:** Per-child session DBs are separate from the RAG corpus because they contain conversational PII; never merge them, and never mirror the session DB into a shared corpus.

## SessionStore and LearnerStore traits

Two traits split the persistence surface, both defined in [storage.rs](../../src/crates/primer-core/src/storage.rs):

```rust
// src/crates/primer-core/src/storage.rs
#[async_trait]
pub trait SessionStore: Send + Sync {
    async fn save_session(&self, session: &Session) -> Result<()>;
    async fn load_session(&self, id: Uuid) -> Result<Option<Session>>;

    async fn retrieve_session_turns(
        &self,
        session_id: Uuid,
        query: &str,
        k: usize,
        exclude_indices_at_or_after: usize,
    ) -> Result<Vec<Turn>>;

    async fn save_classification(/* … */) -> Result<()>;
    async fn load_recent_assessments(/* … */) -> Result<Vec<EngagementAssessment>>;
    async fn most_recent_session_learner_id(&self) -> Result<Option<Uuid>>;

    async fn update_turn_concepts(
        &self,
        session_id: SessionId,
        turn_index: usize,
        concepts: &[String],
    ) -> Result<()>;
    async fn update_exchange_concepts(/* … */) -> Result<()>;
    async fn save_comprehensions(/* … */) -> Result<()>;

    async fn save_turn_embedding(/* … */) -> Result<()> { Ok(()) }

    async fn retrieve_session_turns_hybrid(/* … */) -> Result<Vec<Turn>> {
        // default: BM25-only fallback
    }
}

#[async_trait]
pub trait LearnerStore: Send + Sync {
    async fn load_learner(&self) -> Result<Option<LearnerModel>>;
    async fn save_learner(&self, learner: &LearnerModel) -> Result<()>;
}
```

Both are implemented by [SqliteSessionStore](../../src/crates/primer-storage/src/store/mod.rs) — one type, two trait impls. The split is for testability and for the long-term shape: a future contributor who wants an alternative learner store (think a profile-only export tool) does not have to reimplement the much-larger session API. `SessionStore::save_turn_embedding` and `SessionStore::retrieve_session_turns_hybrid` carry default impls so that backends without vector support stay valid — the dialogue manager calls those methods unconditionally.

The `Result<Option<…>>` shape on `load_session`, `load_learner`, and `most_recent_session_learner_id` is deliberate. "Not found" is not an error — it's a normal first-run signal. Reserving `Err` for genuine I/O failures keeps the CLI free of "is this an expected miss or a real problem?" conditionals.

Locale is a per-learner property, persisted via `LearnerStore`. On first save it's written to `learners.locale` (added in schema v6, default `'en'`); on `--resume` (and on the GUI's start-new-session path) a `--language` / Settings locale that disagrees with the resumed learner's stored locale is a **hard error**, not a silent re-tag. The reasoning is that the knowledge base, STT/TTS, and prompt pack all key off locale, and `concepts.concept_language_tag` would otherwise start tagging new rows under the wrong language — corrupting the longitudinal vocabulary record. Resolution is either reverting the locale to the stored value or starting fresh under a new DB.

## Schema-migration pattern

[primer-storage](../../src/crates/primer-storage/src/schema.rs) follows one shape across every migration. Read [schema.rs](../../src/crates/primer-storage/src/schema.rs) and you will see `apply_v2_migrations`, `apply_v3_migrations`, … `apply_v8_migrations` — eight migrations layered additively, each idempotent, each transaction-wrapped, each safe to run on a fresh DB or on any older DB being upgraded. The constant `USER_VERSION` lives at the top of that file (currently `8`). Open path lives in [store/mod.rs](../../src/crates/primer-storage/src/store/mod.rs) and runs every migration in order on every open.

The pattern, distilled from `apply_v8_migrations`:

```rust
// src/crates/primer-storage/src/schema.rs
pub(crate) fn apply_v8_migrations(conn: &Connection) -> Result<()> {
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| PrimerError::Storage(format!("v8 migration: failed to begin tx: {e}")))?;

    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS embedding_models(
            id   INTEGER PRIMARY KEY,
            name TEXT NOT NULL UNIQUE,
            dim  INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS embeddings_turns(
            turn_id   INTEGER PRIMARY KEY REFERENCES turns(id) ON DELETE CASCADE,
            model_id  INTEGER NOT NULL REFERENCES embedding_models(id),
            vec       BLOB NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_embeddings_turns_model
            ON embeddings_turns(model_id);",
    )
    .map_err(|e| PrimerError::Storage(format!("v8 migration: create tables: {e}")))?;

    tx.commit()
        .map_err(|e| PrimerError::Storage(format!("v8 migration: commit: {e}")))?;
    Ok(())
}
```

Three things are doing all the work here. First, `conn.unchecked_transaction()` wraps the whole body so a partial failure (disk full halfway through, FK violation in step three) rolls back to the pre-migration state — no half-applied schema that subsequent saves would silently miswrite to. Second, every DDL is `CREATE … IF NOT EXISTS`, which makes the migration safe to re-run on an already-upgraded DB. Third, when a column add is needed instead of a new table, the helper `column_exists(&tx, "table", "column")` (also in [schema.rs](../../src/crates/primer-storage/src/schema.rs)) gates the `ALTER TABLE` so it runs once and only once. See `apply_v6_migrations` and `apply_v7_migrations` for the column-add shape.

After the migrations run, the open path also calls [validate_and_seed_lookup](../../src/crates/primer-storage/src/schema.rs) for every lookup table backed by a Rust enum — `speakers`, `pedagogical_intents`, `engagement_states`, `understanding_depths`. That helper compares what's on disk against the canonical `(id, name)` pairs from [catalog.rs](../../src/crates/primer-storage/src/catalog.rs) (which the Rust enums project into), inserts any missing rows, and **returns a hard error** if a known id has the wrong name or if the table has an id the build doesn't know about.

> **Gotcha:** A schema `USER_VERSION` newer than this build is a hard error rejected at open. Older is silently upgraded. The reasoning: a newer-than-this-build DB might have rows the current code can't safely read, but an older DB is exactly what migrations are designed to bring forward.

## Lookup-table normalisation

Every categorical text column in this codebase is stored as an integer foreign key into a lookup table — never as inline text. That applies to `speakers`, `pedagogical_intents`, `engagement_states`, `understanding_depths`, `concepts`, `classifiers`, `comprehension_classifiers`, and the per-table `embedding_models` registries.

For columns backed by a **closed** Rust enum (`Speaker`, `PedagogicalIntent`, `EngagementState`, `UnderstandingDepth`), the Rust source is the single source of truth. The DB lookup table is a derived projection — [catalog.rs](../../src/crates/primer-storage/src/catalog.rs) exposes `expected_speakers()`, `expected_intents()`, `expected_engagement_states()`, `expected_understanding_depths()`, all of which return `Vec<(i64, &'static str)>`. The open path runs `validate_and_seed_lookup` against each of those slices on every open. There is **no API for mutating lookup tables**. Adding a new enum variant means: edit the Rust enum, recompile, next open seeds the new row. Retired integer ids are never reused — once an id has had a meaning, that meaning is canonical.

For columns backed by an **open** vocabulary (`concepts`, `classifiers`, `comprehension_classifiers`), the row is created lazily on first reference. `update_turn_concepts` and friends use `INSERT OR IGNORE` patterns so that two parallel tasks racing to insert the same concept name end up with one row.

A few text columns deliberately stay as JSON-in-TEXT rather than being normalised: `learners.languages`, `learners.high_engagement_topics`, `learner_concepts.notes`. Those are open-vocabulary, free-text lists owned by the learner; they're not FK targets, not queried by exact match, not shared across rows, and not bounded. Normalising them would buy nothing the `concepts` table doesn't already prove. See the long comment above `CREATE_LEARNERS_TABLE` in [schema.rs](../../src/crates/primer-storage/src/schema.rs) for the full reasoning.

> **Gotcha:** Drift in lookup tables is a hard error — `validate_and_seed_lookup` refuses to open a DB whose lookup-table state disagrees with the Rust enum. There is no API for mutating lookup tables. Adding a variant means recompile + reopen.

## The append-only invariant on save_session

`save_session` is called after every turn, repeatedly, on the same session as it grows. To stay idempotent and cheap it does one specific thing on the `turns` table: it inserts only the turns that aren't already on disk (see the `.skip(persisted_count)` loop in [session_save.rs](../../src/crates/primer-storage/src/store/session_save.rs)). Turns already persisted are skipped without comparison.

This is correct for the in-memory `Session` type, which is itself append-only — turns get added at the tail, never modified in place. But it has one consequence the dialogue manager has to work around: any data the post-response background tasks (the concept extractor, the comprehension classifier, the embedder) want to attach to a turn after `save_session` has already run cannot be attached through `save_session`. The skip rule would silently drop their writes.

The fix is explicit backfill methods: [update_turn_concepts](../../src/crates/primer-core/src/storage.rs) and [update_exchange_concepts](../../src/crates/primer-core/src/storage.rs) for concept tags, [save_comprehensions](../../src/crates/primer-core/src/storage.rs) for per-concept depth assessments, [save_turn_embedding](../../src/crates/primer-core/src/storage.rs) for vectors. Each one resolves `(session_id, turn_index)` to a row id, does an `INSERT OR IGNORE` (or upsert, for embeddings) into the appropriate junction table, and returns. Calling them twice with the same data is a no-op.

If you find yourself wanting to move concept persistence back into `save_session` because "it would be simpler" — don't. Doing so requires changing the skip rule, which breaks the append-only invariant, which then makes `save_session` non-idempotent on the turns table. The current shape is load-bearing.

## FTS5 query sanitisation

`retrieve_session_turns` takes raw child input as its `query`. To pass that to FTS5's `MATCH` operator without letting punctuation, quotes, or reserved keywords crash the query parser, the storage layer runs everything through [sanitize_fts_phrase](../../src/crates/primer-storage/src/store/fts.rs) first.

The transformation is simple: split on whitespace, strip every non-alphanumeric character per token (kills `*`, `^`, `:`, `"`, `(`, `)`, slashes, etc.), drop the FTS5 reserved keywords `AND` / `OR` / `NOT` / `NEAR` (case-insensitive), wrap each surviving token in double quotes (so any character the tokenizer would otherwise see as special is inert), and OR-join the lot. An empty result short-circuits the query — FTS5 rejects `MATCH ''`, so the caller skips the search and returns an empty result.

OR-joining (rather than implicit-AND) is a deliberate choice. If sanitization introduces noise tokens — say, a stripped contraction leaving a stray fragment — implicit-AND would torpedo the entire query. With OR, BM25 ranking + the caller's `LIMIT k` keep the result list focused on the most relevant matches and the noise tokens just don't contribute. The unit tests at the bottom of [fts.rs](../../src/crates/primer-storage/src/store/fts.rs) pin the behaviour for empty input, single token, multiple tokens, reserved keywords, FTS5-active punctuation, Unicode alphanumerics, and punctuation-only input.

## Long-term memory

Long-term memory is a **system-prompt concern**, not a chat-message concern. The `messages` array passed to inference is exactly `session.recent_turns(context_window_turns)` — a linear, recent-turns-only slice. Anything older lands in the system prompt as two distinct sections: the rolling LLM summary, and a small set of FTS-retrieved older turns relevant to the current child question.

The summary lives on the `sessions` row (`sessions.summary`, `sessions.summary_through_turn_index`) — both columns added in v2. The dialogue manager refreshes it on a K-threshold (re-summarize when `context_window_turns` new pre-window turns have accumulated, default K=20) so per-turn dialogue doesn't trigger an LLM call every time the boundary advances. On `--resume`, a separate "is the summary stale?" check fires and re-summarises only if uncovered pre-window content exists.

The retrieved-older-turns slice comes from `retrieve_session_turns` (or its hybrid variant). The dialogue manager passes the current child input as `query`, the BM25/RRF result is filtered to exclude the in-window tail (the `exclude_indices_at_or_after` parameter), and up to `k` turns are injected into the system prompt as a "you may have said this before" section.

Two design wins fall out of this split. The chat timeline the model sees stays linear — no jumbled-order surprises from interleaving retrieved snippets with current dialogue. And the context budget stays bounded across hours of conversation: regardless of session length, the prompt is at most `summary + retrieved_older + recent_window` tokens, all of which have fixed caps.

## Turn-embedding-on-save

Hybrid retrieval over session turns (the dense leg of `retrieve_session_turns_hybrid`) needs every old turn to have an embedding. The dialogue manager handles that with a `tokio::spawn` fire-and-forget task that runs after `save_session` returns, embedding the most-recent (child, primer) turns and calling `SessionStore::save_turn_embedding` for each.

The storage layer makes this safe to do without coordination. `save_turn_embedding` is idempotent at the row level — `embeddings_turns` has `turn_id` as its `PRIMARY KEY`, so a second call for the same turn overwrites the existing vector. Re-spawning over already-embedded turns, or spawning a backfill while a fresh save is running, both end at the same correct state. The `embedding_models` registry validates `(name, dim)` on every write — a mismatched dim under an existing model name is a hard error, which is exactly the bug class we want loud rather than silent.

There's one subtlety worth knowing. The just-saved turn may not have a vector yet at the next call to `retrieve_session_turns_hybrid`, because the embed task is fire-and-forget and not awaited. That's fine: recent turns are inside the context window so the model sees them directly already; only **older** turns rely on retrieval, and by the time a turn is old enough to have aged out of the context window the embed task has long since completed. If the embed task ever fails outright (model load error, transient I/O), the dense leg silently has fewer candidates for that turn, the BM25 leg still works, and the next session opens with a backfill opportunity.

## Schema versions, in order

Single source of truth for what each migration does is [schema.rs](../../src/crates/primer-storage/src/schema.rs). At a glance:

| v | What it added |
|---|---|
| 2 | `sessions.summary` + `sessions.summary_through_turn_index`; `turn_text_fts` FTS5 virtual table over `turns.text`, plus insert/delete/update triggers |
| 3 | `engagement_states` lookup; `classifiers`; `turn_classifications` (per-turn `EngagementAssessment` rows) |
| 4 | `understanding_depths` lookup; `learners`; `learner_concepts` (per-learner concept-mastery state) |
| 5 | `comprehension_classifiers` lookup; `turn_comprehensions` (per `(turn, concept, classifier)` depth assessments) |
| 6 | `learners.locale` (BCP-47 short pack id, default `'en'`) + `concepts.concept_language_tag` (default `'en'`) |
| 7 | `learner_concepts.box_level` (Leitner-box scheduler, default `0`) |
| 8 | `embedding_models` registry + `embeddings_turns` (per-turn f32 BLOB vectors for hybrid long-term memory) |

If you find a discrepancy between this table and the `apply_vN_migrations` doc-comments in [schema.rs](../../src/crates/primer-storage/src/schema.rs), trust the source — that's the file the open path actually runs.

---

### Recipe — Add a schema migration

You want to add a hypothetical "vibe" column to `turns` plus a `vibes` lookup table. The end-to-end change is six steps.

**1. Bump `USER_VERSION` in [schema.rs](../../src/crates/primer-storage/src/schema.rs).** From `8` to `9`. While you're there, add a paragraph to the doc-comment on `USER_VERSION` describing what v9 adds — the comment block above the constant is the change-log readers turn to first.

**2. Add `apply_v9_migrations` following the `apply_v8_migrations` template.** Same shape: open `conn.unchecked_transaction()`, gate any `ALTER TABLE` on `column_exists`, use `CREATE TABLE IF NOT EXISTS` for new tables, commit, return.

```rust
// src/crates/primer-storage/src/schema.rs
pub(crate) fn apply_v9_migrations(conn: &Connection) -> Result<()> {
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| PrimerError::Storage(format!("v9 migration: failed to begin tx: {e}")))?;

    if !column_exists(&tx, "turns", "vibe")? {
        tx.execute_batch(
            "ALTER TABLE turns ADD COLUMN vibe INTEGER NOT NULL DEFAULT 0;",
        )
        .map_err(|e| {
            PrimerError::Storage(format!("v9 migration: ALTER turns ADD vibe: {e}"))
        })?;
    }

    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS vibes (
            id   INTEGER PRIMARY KEY,
            name TEXT NOT NULL UNIQUE
        );",
    )
    .map_err(|e| PrimerError::Storage(format!("v9 migration: create vibes: {e}")))?;

    tx.commit()
        .map_err(|e| PrimerError::Storage(format!("v9 migration: commit: {e}")))?;
    Ok(())
}
```

**3. Wire it into the open path.** In [store/mod.rs](../../src/crates/primer-storage/src/store/mod.rs), call `apply_v9_migrations(&conn)?` after the `apply_v8_migrations` line and before the `validate_and_seed_lookup` block. Order matters — validate-and-seed should see the post-migration schema.

**4. Add a migration round-trip test.** Following the pattern in the `v4_tests` sibling module ([schema/v4_tests.rs](../../src/crates/primer-storage/src/schema/v4_tests.rs), declared with `#[cfg(test)] mod v4_tests;` from [schema.rs](../../src/crates/primer-storage/src/schema.rs)), add a test that builds a fresh v8 connection, asserts the new schema isn't there yet, runs `apply_v9_migrations`, and asserts the new column / table is present. Add an idempotency test (run the migration twice, assert no error and no duplicate rows). If you want full coverage, also add a fault-injection rollback test in the style of `apply_v4_migrations_rolls_back_on_failure`.

**5. If the migration touches an enum-backed lookup table**, extend [catalog.rs](../../src/crates/primer-storage/src/catalog.rs) with the new `expected_<table>()` function, add a `validate_and_seed_lookup` call in [store/mod.rs](../../src/crates/primer-storage/src/store/mod.rs) right after the existing four, and write a `expected_<table>_covers_all_variants` test that pins the projection against the Rust enum (the existing `expected_engagement_states_covers_all_variants` and `expected_understanding_depths_covers_all_variants` tests are the pattern). The "vibes" example above is open-vocabulary so this step doesn't apply, but if `Vibe` were a closed Rust enum it would.

**6. Update [CLAUDE.md](../../CLAUDE.md).** There's a "Schema is at user_version 8." sentence in the `primer-storage` paragraph; bump it to 9 and add the new migration to the v2..v8 chain summary. The same paragraph notes that "the existing v2..v7 chain is the template" — extend that to v2..v8 (or whatever the current top is).

After all six steps, `cargo test -p primer-storage` should pass on a fresh DB and on every fixture from earlier versions. If any test breaks because it hardcoded an older `USER_VERSION` constant, that's the test telling you the version bump landed correctly — fix the hardcode to use `USER_VERSION` directly.
