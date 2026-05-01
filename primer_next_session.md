# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-05-01 (after engagement-classifier landing: Phase 0.3 intent gaps fixed, `primer-classifier` crate, schema v3, 180 tests).

## First moves when you start

1. Read [CLAUDE.md](CLAUDE.md) — repo conventions, gotchas, build commands. **Workspace root is `src/`, not the repo root** — every cargo command runs from `src/`.
2. Skim [ROADMAP.md](ROADMAP.md). Phase 0.1 streaming + `--model` + conversation persistence + **resume-past-session + long-term memory** are checked off. Open Phase 0.1 item: graceful API-error handling. Three Phase 0.3 bullets (Encouragement reachable, factual-question routing, session-length-aware Disengaging) are now ticked; five remain open.
3. From `src/`: `cargo build && cargo test`. Should be green: **180 tests** across the workspace.
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
- `cargo test --workspace` → **180 passed, 0 failed.**
- `cargo clippy --workspace --all-targets` → fully clean.
- `cargo fmt --check` → clean.
- Manual REPL end-to-end against the stub backend: `--no-persist --name "José"` runs in-memory and the Unicode name renders; first-run banner shows on first default-path open and is silent on second; `--no-persist --resume <uuid>` rejected at clap parse.
- Migration: covered by `migrate_v1_db_with_turns_adds_columns_and_backfills_fts` (hand-rolls a v1 schema on disk, opens via the store, asserts columns and FTS rows present afterwards) and `apply_v2_migrations_rolls_back_on_failure` (fault-injects a missing `turns` table; verifies the column ALTERs and FTS table did NOT persist). v3 migration covered by analogous `apply_v3_migrations_is_idempotent` and schema-inspection tests.

## The next task: pick one

Phase 0.1 is complete except for graceful API-error handling. Phase 0.2 (knowledge bootstrapping) and Phase 0.3 (pedagogical engine) are open. Three Phase 0.3 bullets (`Encouragement` reachable, factual-question routing, session-length-aware `Disengaging`) landed with the engagement classifier. Five Phase 0.3 bullets remain open:

1. Concept extraction from child responses (currently always `vec![]`).
2. LLM-based comprehension classifier (assess genuine understanding vs. parroting vs. confusion).
3. Learner-model SQLite persistence so concept-mastery state survives across sessions.
4. Per-child vocabulary tracking with spaced repetition (see ROADMAP 0.3 for the full schema constraint).
5. Session time tracking with gentle break suggestions.

Recommended in order of leverage and unblockedness:

### Option A (recommended) — Learner-model SQLite persistence (Phase 0.3, design exists)

There's a fairly detailed spec at `docs/superpowers/specs/2026-04-30-session-persistence-sqlite-design.md` that already sketches a `learner_concepts` table reusing `concepts(id)` as an FK plus a new `understanding_depths` lookup table for `UnderstandingDepth`. This is the natural extension of `primer-storage`: validate-and-seed for closed enums, append-only writes (or upsert-by-natural-key in this case), SQLite separate from RAG. Some upfront benefit now that resume exists — a returning child can pick up not just the conversation but their concept-mastery state. **This also closes the documented "in-memory `LearnerModel` and `Session.learner_id` can diverge after resume" gap.** Schema would land as a v3 → v4 migration (v3 is the `turn_classifications` table from the engagement-classifier work).

Wants a real design conversation about exactly which fields persist. Don't start without re-reading the spec and confirming current shape with Horst.

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

- **Option A**: `docs/superpowers/specs/2026-04-30-session-persistence-sqlite-design.md` (spec), `src/crates/primer-core/src/learner.rs` (the `LearnerModel` / `ConceptState` / `UnderstandingDepth` types), `src/crates/primer-storage/src/{lib,schema,catalog}.rs` (extend the existing pattern). Schema is now at v3; new fields land as a v3 → v4 migration using the `apply_v3_migrations`-shaped pattern (idempotent column-add, **wrapped in a single transaction**).
- **Option B**: `src/crates/primer-inference/src/cloud.rs` (top of `generate_stream`), `src/crates/primer-core/src/error.rs` (you'll likely add a new `PrimerError` variant or two).
- **Option D**: `src/crates/primer-storage/src/lib.rs` (`sanitize_fts_phrase`, `retrieve_session_turns`), `src/crates/primer-pedagogy/src/dialogue_manager.rs` (`refresh_summary_if_due`, `refresh_summary_if_stale`, `retrieve_long_term_memory`).

## Patterns to reuse, not reinvent

- **Categorical text → integer FK lookup tables.** Mandatory for any new SQLite schema. Closed Rust enum = source of truth, validate-and-seed on `open()`. See `primer-storage::schema::validate_and_seed_lookup`.
- **Idempotent schema migrations, transaction-wrapped.** `apply_v2_migrations` / `apply_v3_migrations` in `primer-storage::schema` are the templates: `pragma_table_info` checks before each ALTER, `CREATE … IF NOT EXISTS` for tables/triggers, detect "did the table exist before this open" if you need to backfill, and **wrap the whole body in `conn.unchecked_transaction()` + `tx.commit()`** so a partial failure rolls back. Schema is currently at v3 (`turn_classifications`); next migration is v4.
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
- **In-memory `LearnerModel` and `Session.learner_id` can diverge after resume.** The CLI builds the LearnerModel from `--name`/`--age` flags; the loaded session keeps its original `learner_id`. Closed by Option A (learner-model persistence).
- **`dirs` crate is intentionally not in workspace deps.** The CLI uses `std::env::var("HOME")` for both `~/.primer_env` and `~/.primer/`. If you ever need cross-platform home dir support, add `dirs` then; until then, keep the dep surface small.
- **`unicode-normalization` is a workspace dep now.** Used by `primer-cli::slug` for NFC normalization. If you ever stash filenames anywhere else from user-supplied strings, route them through the same path.
- **`retrieve_session_turns` index-lag invariant.** Documented on the trait method: callers must use `exclude_indices_at_or_after = total - window` because the in-memory child turn is one save ahead of the FTS index. Lower bounds (e.g. `total - window - 1`) silently return stale or duplicate results.

## Reporting back

When you finish or hit a blocker:
- State plainly what you got working and what you didn't, by acceptance criterion.
- If you exposed bugs in existing behaviour (e.g., another FK-cascade-style footgun), flag them separately from the assigned task — Horst should decide whether to fix now or file.
- If you discover that Horst did interim work that changes the plan, flag it explicitly. Read the file as it actually is, not as this brief describes it.
