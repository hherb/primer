# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-05-02 (after learner-model-persistence landing: schema v4, `LearnerStore` trait, CLI adoption flow, 223 tests).

## First moves when you start

1. Read [CLAUDE.md](CLAUDE.md) — repo conventions, gotchas, build commands. **Workspace root is `src/`, not the repo root** — every cargo command runs from `src/`.
2. Skim [ROADMAP.md](ROADMAP.md). Phase 0.1 streaming + `--model` + conversation persistence + **resume-past-session + long-term memory** are checked off. Open Phase 0.1 item: graceful API-error handling. Four Phase 0.3 bullets (Encouragement reachable, factual-question routing, session-length-aware Disengaging, **learner-model SQLite persistence**) are now ticked; four remain open.
3. From `src/`: `cargo build && cargo test`. Should be green: **223 tests** across the workspace.
4. From `src/`: `cargo clippy --workspace --all-targets`. Should be 100 % clean (no warnings).
5. **Don't assume nothing changed since this brief was written.** Read the current state of the files you intend to touch first — Horst may have made interim changes.

## What's now on main (so you don't redo it)

### Resume past session + long-term memory — Phase 0.1 closed (PR #2, merged into `main`)

What landed in PR #2:
- **`SessionStore::load_session(uuid) -> Option<Session>`** and **`retrieve_session_turns(session_id, query, k, exclude_indices_at_or_after) -> Vec<Turn>`** added to the trait. Implemented on `SqliteSessionStore`. Catalog reverse-lookups (`speaker_from_id`, `intent_from_id`) lost their `#[allow(dead_code)]` — they're load-bearing now. The trait method has a doc'd "index-lag" invariant: callers must use `total - window` for `exclude_indices_at_or_after` because the in-memory child turn is one save ahead of the FTS index.
- **`--resume <uuid>`** CLI flag. Reads from `--session-db`. Errors clearly when the file or the id is missing (no auto-create on a typo). Replaces the greeting with a "resumed N prior turns" line. Works against any backend.
- **`--session-db` defaults to `~/.primer/<slugified-name>.db`** instead of `:memory:`. Slug rule: **NFC-normalized, Unicode-aware** — accepts any char `is_alphanumeric()` (Latin/Cyrillic/CJK), lowercased where case applies, with non-letters collapsed to `-`. `José`, `Łukasz`, `Соня`, `美咲` all map to sensible filenames; NFC vs NFD encodings of the same name produce one slug. Parent directory is created on first run; an unwritable `~/.primer/` is a hard error rather than a silent fallback.
- **`--no-persist`** CLI flag. Runs against an in-memory SQLite store; nothing on disk. Mutually exclusive with `--resume` and `--session-db` (clap enforces at parse time).
- **First-run banner.** The very first time the default-path file is created, the CLI prints a one-line note explaining where the data lives and how to opt out. Silent on subsequent runs.
- **Long-term memory layer** — the necessary partner to resume on long sessions:
  - **Rolling LLM summary** of pre-window turns: `Session.summary` + `Session.summary_through_turn_index` columns (schema v2). New trait method `InferenceBackend::summarize(turns, target_chars) -> String` with a default impl that builds a one-shot prompt and dispatches to `generate`; `StubBackend` overrides with a deterministic canned string. Two refresh cadences in `DialogueManager`: `refresh_summary_if_due` for active conversation (K-threshold — re-summarize only when `context_window_turns` new pre-window turns have accumulated, default K=20); `refresh_summary_if_stale` for resume (refresh only when the loaded summary doesn't yet cover the current pre-window range — a current summary is preserved verbatim, no LLM call).
  - **FTS5 retrieval over session turns**: `turn_text_fts` virtual table over `turns.text` with `AFTER INSERT/DELETE/UPDATE` triggers keeping it in sync. `DialogueManager::retrieve_long_term_memory` pulls top-3 relevant older turns each respond cycle; `sanitize_fts_phrase` strips non-alphanumerics, drops `AND`/`OR`/`NOT`/`NEAR`, OR-joins quoted tokens — child input goes in raw, FTS5 sees a safe expression.
  - **Both surfaced as system-prompt sections** ("Earlier in this conversation" and "Relevant prior moments"). The chat-message timeline the model sees stays exactly equal to `recent_turns(context_window_turns)`. Context budget is bounded across hours of conversation regardless of resume length.
- **Schema migration v1 → v2**: idempotent ALTERs (with `pragma_table_info` checks) + `CREATE VIRTUAL TABLE IF NOT EXISTS` + `CREATE TRIGGER IF NOT EXISTS`, **the whole body wrapped in a single transaction** (`conn.unchecked_transaction()`) so a partial failure rolls back to the pre-migration state instead of leaving a half-v2 DB that subsequent saves miswrite to. Existing v1 DBs migrate in place on first open and the FTS index is backfilled from existing turn rows.
- **`DialogueManager::resume_session(loaded)`** replaces `open_session()` for resumed flows: swaps in the loaded `Session`, clears `ended_at`, runs `refresh_summary_if_stale`, persists. The in-memory `LearnerModel` (built from CLI flags) is not reconciled with `loaded.learner_id`; deliberate, documented, deferred to learner-persistence work.
- **`build_prompt` signature changed**: now takes `summary: &str` and `retrieved_older: &[Turn]`. Every existing caller updated.
- **`resolve_session_db_path(explicit, home, name, no_persist)`** is a pure path computation — `HOME` is looked up once in `main` and passed in. Tests pass synthetic paths instead of mutating `HOME`.

### Learner-model SQLite persistence — Phase 0.3 (merged into `main`)

What landed in PR #5:
- **Schema v4** — three new objects added by `apply_v4_migrations` (idempotent, single-transaction, schema-only): `understanding_depths` lookup table (seeded from the closed `UnderstandingDepth` enum via the existing validate-and-seed helper), `learners` table (one row per DB file — application invariant — holds `LearnerProfile` + `LearningPreferences` + a `current_engagement_state_id` snapshot, with `Vec<String>` fields encoded as JSON in TEXT columns), and `learner_concepts` junction (PK `(learner_id, concept_id)`, FKs into `learners` / `concepts` / `understanding_depths`, with `notes` as JSON-in-TEXT).
- **`LearnerStore` trait** in `primer-core::storage` with `load_learner` + `save_learner`. Implemented on `SqliteSessionStore` (same struct, both traits). `save_learner` is a single-transaction upsert that uses `INSERT … ON CONFLICT(id) DO UPDATE` on `learners` (so we don't cascade-wipe `learner_concepts` via the FK — same lesson `save_session` learned) and `INSERT … ON CONFLICT(learner_id, concept_id) DO UPDATE` on `learner_concepts`. **Concepts are monotonic** — dropping a `ConceptState` from `LearnerModel.concepts` does NOT delete the on-disk row. Forgetting is an explicit operation that has no surface yet.
- **`SessionStore::most_recent_session_learner_id() -> Result<Option<Uuid>>`** — returns the `learner_id` of the most-recent session in the DB, used by the CLI's first-run-after-upgrade adoption flow.
- **CLI adoption flow** in `main.rs`. On startup: `load_learner` first; if `Some`, persisted UUID + name win (warn on `--name` mismatch via both `tracing::warn!` AND `eprintln!` so it's visible at default log levels; `--age` updates persisted to cover birthdays); if `None`, call `most_recent_session_learner_id` to inherit an existing `sessions.learner_id` (eliminating orphan sessions on v3 → v4 upgrade) or mint fresh; either way `save_learner` materialises the row. New `create_learner_with_id(id, name, age)` helper takes a pre-determined UUID.
- **`DialogueManager` wired** with `Option<Arc<dyn LearnerStore>>` between `storage` and `classifier`. `save_learner` now fires at every existing `save_session` site (open_session, resume_session, per-turn — covers Ok and mid-stream Err — close_session) under the same warn-on-error policy.
- **Closes the divergence bug.** The previously-documented "in-memory `LearnerModel` and `Session.learner_id` can diverge after resume" footgun is gone — the persisted UUID always wins, and the test `divergence_bug_closed_via_cli_startup_flow` proves the v3-DB-with-orphan-session adoption case end-to-end.
- **Test count 195 → 223.** Coverage added across `primer-core` (UnderstandingDepth::ALL), `primer-storage` (catalog, v4 migration with rollback test, FK enforcement, `most_recent_session_learner_id`, `LearnerStore` round-trip, monotonicity, every-variant), `primer-pedagogy` (DialogueManager wiring + divergence-bug-closed integration test), and `primer-cli` (birthday + name-mismatch).

### Engagement classifier — Phase 0.3 intent gaps (merged into `main`)

What landed in the engagement-classifier work:
- **`primer-classifier` crate** — new crate. `EngagementClassifier` trait; `LlmEngagementClassifier` (wraps `Arc<dyn InferenceBackend>`, separate dep, no `primer-inference` build dep); `StubEngagementClassifier` (`with_response`/`with_script` constructors for deterministic tests). Soft-fail policy: errors log via `tracing::warn!` and return `EngagementAssessment::unknown_low_confidence(...)` — classifier failures never surface to the child.
- **`ClassifierSettings`** in `primer-classifier::config` — all classifier parameters (`history_depth`, `blocking_timeout`, `confidence_threshold`, `recent_child_turns`, `max_output_chars`, `generation_max_tokens`, `generation_temperature`, `generation_top_p`). Every numeric backed by `consts.rs` defaults (no magic numbers).
- **Schema v3 in `primer-storage`** — `turn_classifications` table persists every `EngagementAssessment` (session_id, turn_index, dominant_state, confidence, reasoning, classifier_model, latency_ms, timestamp). `apply_v3_migrations` is idempotent, single-transaction, same pattern as v2.
- **Classifier flow in `DialogueManager`** — at the START of each turn, awaits the previous turn's classifier `JoinHandle` (bounded by `blocking_timeout`, default 500ms); result feeds `learner.recent_assessments` before `decide_intent` runs. After the Primer's response is saved, spawns `tokio::spawn(classifier.classify(...))`. Classifier and storage held as `Arc<dyn ...>` (spawn requires `'static`); backend and knowledge remain borrowed `&dyn`.
- **Three Phase 0.3 ROADMAP bullets ticked:**
  1. `Encouragement` is now reachable from `decide_intent` — "frustrated but still trying" → `Encouragement`; "frustrated and stuck" → `Scaffolding`.
  2. Factual-question patterns detected — "what is X?" / "how does Y work?" route to `DirectAnswer`, `AnswerThenPivot` on the next turn. Both were unreachable before.
  3. `Disengaging` is now session-length-aware — routes to `Encouragement` early, `SessionClose` only after a meaningful session duration.
- **`--verbose` CLI flag** — prints `[intent]` and `[classifier]` debug lines to stderr; stdout stays clean for piping. Also ticks a Phase 0.4 bullet.
- **`--classifier-backend`, `--classifier-model`, `--classifier-timeout-ms`** CLI flags — override the classifier's inference backend independently of the main chat backend.
- **Test count 119 → 180.** Coverage added across `primer-classifier` (new), `primer-pedagogy` (classifier integration), `primer-storage` (v3 migration + `turn_classifications`), and extended `decide_intent` tests.

### Verification status

- `cargo build --workspace` → clean.
- `cargo test --workspace` → **223 passed, 0 failed.**
- `cargo clippy --workspace --all-targets` → fully clean.
- `cargo fmt --check` → clean.
- Manual REPL end-to-end against the stub backend: `--no-persist --name "José"` runs in-memory and the Unicode name renders; first-run banner shows on first default-path open and is silent on second; `--no-persist --resume <uuid>` rejected at clap parse.
- Migration: v2 covered by `migrate_v1_db_with_turns_adds_columns_and_backfills_fts` and `apply_v2_migrations_rolls_back_on_failure`; v3 by analogous `apply_v3_migrations_is_idempotent` and schema-inspection tests; v4 by `apply_v4_migrations_creates_three_tables`, `apply_v4_migrations_is_idempotent`, `apply_v4_migrations_does_not_insert_learners_row`, and `apply_v4_migrations_rolls_back_on_failure` (fault-injects a name collision against the v4 index name; verifies all three new tables are absent post-rollback).

## The next task: pick one

Phase 0.1 is complete except for graceful API-error handling. Phase 0.2 (knowledge bootstrapping) and Phase 0.3 (pedagogical engine) are open. Four Phase 0.3 bullets have landed (`Encouragement` reachable, factual-question routing, session-length-aware `Disengaging`, **learner-model SQLite persistence**). Four Phase 0.3 bullets remain open:

1. Concept extraction from child responses (currently always `vec![]` for child turns — the persistence path is wired and waiting).
2. LLM-based comprehension classifier (assess genuine understanding vs. parroting vs. confusion).
3. Per-child vocabulary tracking with spaced repetition (will reuse `learner_concepts` with `concept_id = "vocab:..."`; see ROADMAP 0.3 for the full schema constraint).
4. Session time tracking with gentle break suggestions.

Recommended in order of leverage and unblockedness:

### Option A (recommended) — Concept extraction from child responses (Phase 0.3, naturally next)

The biggest unblock. `learner_concepts` exists, `save_learner` runs every turn, but `turn.concepts` is still `vec![]` for child input — so the concept counts and depths are frozen at zero. Even simple keyword matching against a small concept taxonomy ("gravity", "photosynthesis", "Rayleigh", etc.) would start populating `learner_concepts` rows and exercising the path that just landed. The minimal v1 is a `extract_concepts(text: &str) -> Vec<String>` function in `primer-pedagogy` that the dialogue manager calls when adding the child's turn. A more ambitious v2 plugs in an LLM classifier (overlaps with bullet #2). Talk to Horst about how taxonomical-vs-LLM you want to be.

### Option B — Graceful API errors (Phase 0.1 leftover)

Mid-stream errors propagate cleanly today, but rate-limit / network-drop / invalid-key all bubble up as raw `PrimerError::Inference("Generation failed: ...")`. Minimum viable shape: detect 401/403 (bad key), 429 (rate limit), 5xx (server) at the top of `CloudBackend::generate_stream` and surface a user-friendly message. Optional: retry-with-jittered-backoff for 429.

### Option C — Knowledge base bootstrapping (Phase 0.2, heavier)

Needs a Python ingestion script for Simple English Wikipedia → SQLite FTS5 + a 50-100 hand-picked seed corpus. Skip unless explicitly asked.

### Option D — Better summarization & retrieval

The summary-and-retrieval layer is a deliberately simple v1: full re-summarization every K turns, BM25 retrieval with OR-joined tokens. Possible refinements once you have qualitative feedback:
- Incremental summarization (extend the existing summary instead of rebuilding from scratch).
- Stop-word filtering before OR-joining tokens, so common-question words like "what" don't dominate the FTS query.
- Embedding-based retrieval as an alternative to BM25 — would need a local embedding model or an API.
- Drop summary regeneration cost on the cloud backend when token-budget is constrained (e.g. cheaper haiku for summary even when sonnet is the chat model).

Don't start any of these without evidence the simpler version is a problem.

## Files most relevant to start in

- **Option A (concept extraction)**: `src/crates/primer-pedagogy/src/dialogue_manager.rs` (the call site is in `update_learner_model` around line 600 — it's already wired to call something; you need to give it something to call). `src/crates/primer-core/src/conversation.rs` (`Turn.concepts: Vec<String>`). For the LLM-classifier variant, mirror `primer-classifier` for the trait shape. The `learner_concepts` schema is in place — concepts that come back from extraction will round-trip through `save_learner` automatically.
- **Option B (graceful API errors)**: `src/crates/primer-inference/src/cloud.rs` (top of `generate_stream`), `src/crates/primer-core/src/error.rs` (you'll likely add a new `PrimerError` variant or two).
- **Option D (better summarization & retrieval)**: `src/crates/primer-storage/src/lib.rs` (`sanitize_fts_phrase`, `retrieve_session_turns`), `src/crates/primer-pedagogy/src/dialogue_manager.rs` (`refresh_summary_if_due`, `refresh_summary_if_stale`, `retrieve_long_term_memory`).

## Patterns to reuse, not reinvent

- **Categorical text → integer FK lookup tables.** Mandatory for any new SQLite schema. Closed Rust enum = source of truth, validate-and-seed on `open()`. See `primer-storage::schema::validate_and_seed_lookup`.
- **Idempotent schema migrations, transaction-wrapped.** `apply_v2_migrations` / `apply_v3_migrations` / `apply_v4_migrations` in `primer-storage::schema` are the templates: `pragma_table_info` checks before each ALTER, `CREATE … IF NOT EXISTS` for tables/triggers, detect "did the table exist before this open" if you need to backfill, and **wrap the whole body in `conn.unchecked_transaction()` + `tx.commit()`** so a partial failure rolls back. Schema is currently at v4 (`learners`, `learner_concepts`, `understanding_depths`); next migration is v5.
- **Append-only persistence.** If a Rust-side type is append-only in memory, persistence should mirror that — don't DELETE-then-INSERT. The `INSERT OR REPLACE` ↔ FK-cascade footgun is real.
- **Streaming bridge.** `bytes_stream` → parser buffer → `mpsc::unbounded` → `Box::pin(rx)`. See `cloud.rs::generate_stream` and `ollama.rs::generate_stream`.
- **FTS5 query sanitization.** `sanitize_fts_phrase` in `primer-storage::lib`: alphanumeric-only tokens, drop FTS5 reserved keywords, OR-join quoted tokens. Pair with `LIMIT k` and BM25 ranking.
- **System-prompt layering for context that isn't part of the chat timeline.** Long-term memory (summary + retrieved older turns) lives in the system prompt, never in the messages list. The chat `messages` array stays exactly = `recent_turns(window)`. Future "ambient context" features (current time, user mood, learner-model summary) should follow the same pattern.
- **Pure path computations take their environment as parameters.** `resolve_session_db_path(explicit, home, name, no_persist)` reads no env vars; tests pass synthetic paths without mutating `HOME`. Apply the same pattern to any future helper that would otherwise touch the process environment.
- **TDD discipline.** Watch tests fail. Watch them pass. The recent persistence work caught a real bug (FK cascade) with a test written before the implementation was rechecked. The rollback test for `apply_v2_migrations` similarly required a fault-injection design (call the migration on a connection without `turns` so backfill fails) — the discipline is worth it.
- **Code-review fixes in-place.** The user's preference: when a review surfaces multiple issues, fix them all in one pass on the same branch rather than deferring to follow-ups.
- **`.env` and `~/.primer_env`** are auto-loaded. Don't tell the user to `export` again. New secrets/env vars get documented in `.env.example`.

## Watch-outs (still relevant)

- **`Role::System → "user"`** mapping in `cloud.rs` versus the leading `system` message in `ollama.rs` are different on purpose. Both are correct for their respective APIs.
- **`update_learner_model` in `dialogue_manager.rs` is still a placeholder** (word-count heuristic only). Real comprehension assessment is Phase 0.3.
- **Concept extraction (`turn.concepts`) is still always `vec![]` for child turns.** Also Phase 0.3.
- **The stub backend emits one final chunk by design** — degenerate but valid stream. Don't "fix" it.
- **`top_p` is silently dropped from cloud requests.** If you ever extend the cloud backend to use it conditionally, the `api_request_omits_top_p` test will fail and remind you to update it.
- **The streaming-task spawns are fire-and-forget.** Long-lived deployments (Phase 2/3) will want a cancellation token.
- **Save failures from `SqliteSessionStore` are logged, not propagated.** Watch out if a future feature needs to *know* a save succeeded.
- **`save_session` assumes append-only memory.** If you ever introduce a way to mutate or delete already-recorded turns in `Session`, the storage layer's `SELECT COUNT(*)`-based skip will silently drop your changes. Document any such mutation up front.
- **Synchronous summarization on the hot path.** Each summary refresh is one extra inference call per ~K turns, blocking the turn it triggers on. Acceptable at default K=20; if perceived latency is bad, defer to a background tokio task — don't optimize prematurely.
- **`LearnerStore::save_learner` is monotonic on concepts.** `learner_concepts` rows are upserted but never deleted by `save_learner` — a concept dropped from `LearnerModel.concepts` survives on disk. If you need explicit forgetting later, add an explicit method; don't make it a side effect. Same shape as the `Session.turns` append-only invariant.
- **`learners` table assumes one row per DB file.** This is application-level only — the schema doesn't enforce it. `load_learner` uses `ORDER BY id LIMIT 1` defensively, but a future bug that inserts a second row would silently lose data on the next `save_learner`. If multi-learner-per-file ever lands, drop the invariant explicitly and rework the trait.
- **Persisted name vs `--name` mismatch fires both `tracing::warn!` and `eprintln!`.** The eprintln is intentional — `RUST_LOG=warn` shouldn't be required to see "your typo'd name was ignored." Don't quietly drop it.
- **`dirs` crate is intentionally not in workspace deps.** The CLI uses `std::env::var("HOME")` for both `~/.primer_env` and `~/.primer/`. If you ever need cross-platform home dir support, add `dirs` then; until then, keep the dep surface small.
- **`unicode-normalization` is a workspace dep now.** Used by `primer-cli::slug` for NFC normalization. If you ever stash filenames anywhere else from user-supplied strings, route them through the same path.
- **`retrieve_session_turns` index-lag invariant.** Documented on the trait method: callers must use `exclude_indices_at_or_after = total - window` because the in-memory child turn is one save ahead of the FTS index. Lower bounds (e.g. `total - window - 1`) silently return stale or duplicate results.

## Reporting back

When you finish or hit a blocker:
- State plainly what you got working and what you didn't, by acceptance criterion.
- If you exposed bugs in existing behaviour (e.g., another FK-cascade-style footgun), flag them separately from the assigned task — Horst should decide whether to fix now or file.
- If you discover that Horst did interim work that changes the plan, flag it explicitly. Read the file as it actually is, not as this brief describes it.
