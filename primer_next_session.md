# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-04-30 (after the session-resume + long-term-memory PR shipped on `feature/resume-session-with-memory`).

## First moves when you start

1. Read [CLAUDE.md](CLAUDE.md) — repo conventions, gotchas, build commands. **Workspace root is `src/`, not the repo root** — every cargo command runs from `src/`.
2. Skim [ROADMAP.md](ROADMAP.md). Phase 0.1 streaming + `--model` + conversation persistence + **resume-past-session + long-term memory** are checked off. Open Phase 0.1 item: graceful API-error handling. Phase 0.3 is the bulk of what's still ahead.
3. From `src/`: `cargo build && cargo test`. Should be green: **109 tests** across the workspace.
4. From `src/`: `cargo clippy --workspace --all-targets`. Should be 100 % clean (no warnings).
5. **Don't assume nothing changed since this brief was written.** Read the current state of the files you intend to touch first — Horst may have made interim changes.

## What was shipped most recently (so you don't redo it)

### Resume past session + long-term memory — Phase 0.1 closed (PR `feature/resume-session-with-memory`)

What landed:
- **`SessionStore::load_session(uuid) -> Option<Session>`** and **`retrieve_session_turns(session_id, query, k, exclude_indices_at_or_after) -> Vec<Turn>`** added to the trait. Implemented on `SqliteSessionStore`. Catalog reverse-lookups (`speaker_from_id`, `intent_from_id`) lost their `#[allow(dead_code)]` — they're load-bearing now.
- **`--resume <uuid>`** CLI flag. Reads from `--session-db`. Errors clearly when the file or the id is missing (no auto-create on a typo). Replaces the greeting with a "resumed N prior turns" line. Works against any backend (stub backend just replays history; new turns get stub responses, but historical context is real).
- **`--session-db` defaults to `~/.primer/<slugified-name>.db`** instead of `:memory:`. Slug rule: lowercase ASCII alphanumerics, runs of other chars collapsed to `-`, leading/trailing `-` stripped, empty → `default`. Parent directory is created on first run; an unwritable `~/.primer/` is a hard error rather than a silent fallback. Pass `--session-db /explicit/path.db` to override.
- **Long-term memory layer** — the necessary partner to resume on long sessions:
  - **Rolling LLM summary** of pre-window turns: `Session.summary` + `Session.summary_through_turn_index` columns (schema v2). New trait method `InferenceBackend::summarize(turns, target_chars) -> String` with a default impl that builds a one-shot prompt and dispatches to `generate`; `StubBackend` overrides with a deterministic canned string. `DialogueManager::refresh_summary_if_due` regenerates the summary on resume (`force=true`) and every `context_window_turns` (default 20) further turns of pre-window content.
  - **FTS5 retrieval over session turns**: `turn_text_fts` virtual table over `turns.text` with `AFTER INSERT/DELETE/UPDATE` triggers keeping it in sync. `DialogueManager::retrieve_long_term_memory` pulls top-3 relevant older turns each respond cycle; `sanitize_fts_phrase` strips non-alphanumerics, drops `AND`/`OR`/`NOT`/`NEAR`, OR-joins quoted tokens — child input goes in raw, FTS5 sees a safe expression.
  - **Both surfaced as system-prompt sections** ("Earlier in this conversation" and "Relevant prior moments"). The chat-message timeline the model sees stays exactly equal to `recent_turns(context_window_turns)`. Context budget is bounded across hours of conversation regardless of resume length.
- **Schema migration v1 → v2**: idempotent ALTERs (with `pragma_table_info` checks) + `CREATE VIRTUAL TABLE IF NOT EXISTS` + `CREATE TRIGGER IF NOT EXISTS`. Existing v1 DBs migrate in place on first open and the FTS index is backfilled from existing turn rows. New rule captured in [CLAUDE.md](CLAUDE.md): bumping `USER_VERSION` is one half of a migration; making the new objects safe to re-create on a re-open is the other half.
- **`DialogueManager::resume_session(loaded)`** replaces `open_session()` for resumed flows: swaps in the loaded `Session`, clears `ended_at`, runs a forced summary refresh, persists. The in-memory `LearnerModel` (built from CLI flags) is not reconciled with `loaded.learner_id`; deliberate, documented, deferred to learner-persistence work.
- **`build_prompt` signature changed**: now takes `summary: &str` and `retrieved_older: &[Turn]`. Every existing caller updated. Test stubs (`CountingStore`) implement the new trait methods.

### Verification status

- `cargo build --workspace` → clean.
- `cargo test --workspace` → **109 passed, 0 failed.** (71 baseline + 38 new: 18 storage, 12 pedagogy, 7 CLI, 1 inference.)
- `cargo clippy --workspace --all-targets` → fully clean.
- `cargo fmt --check` → clean.
- Manual REPL end-to-end against the stub backend with `HOME=/tmp/...`: 25-turn fresh session → quit → DB at `~/.primer/testkid.db` with 51 turns and `summary_through_turn_index = 21`; resume by UUID → 51 prior turns loaded with no greeting, +1 turn, `summary_through_turn_index → 31`; bad-UUID resume → clear error and non-zero exit.
- Migration: covered by `migrate_v1_db_with_turns_adds_columns_and_backfills_fts` (hand-rolls a v1 schema on disk, opens via the store, asserts columns and FTS rows present afterwards).

## The next task: pick one

Phase 0.1 is now complete except for graceful API-error handling. Phase 0.2 (knowledge bootstrapping) and Phase 0.3 (pedagogical engine) are open. Recommended in order of leverage and unblockedness:

### Option A (recommended) — `decide_intent()` gap fixes (Phase 0.3, three small bugs)

The characterization tests in `primer-pedagogy::prompt_builder::tests` document three current gaps with named tests like `frustrated_does_not_currently_return_encouragement` and `factual_question_pattern_does_not_currently_return_direct_answer`. Each gap is a separate ROADMAP 0.3 bullet:

1. Make `Encouragement` reachable. Currently `Frustrated` always returns `Scaffolding`; the split needs a comprehension/engagement signal — "frustrated and stuck" → Scaffolding, "frustrated but still trying" → Encouragement.
2. Detect factual-question patterns ("what is X?", "how does Y work?") and route to `DirectAnswer`, with `AnswerThenPivot` on the next turn. Both intents are currently unreachable.
3. Make `Disengaging` session-length-aware. Route to `Encouragement` early, `SessionClose` only after a meaningful duration. Pairs with the (still-open) session-time-tracking bullet.

For each: read the existing characterization test, design the new behaviour with Horst, flip the test from "does not currently" to "now does", add a fresh characterization test that the negative case still picks the old branch. **Update the system prompt in `prompt_builder.rs` if intent semantics change** — the prompt encodes per-intent guidance and may need wording adjustments.

### Option B — Learner-model SQLite persistence (Phase 0.3, design exists)

There's a fairly detailed spec at `docs/superpowers/specs/2026-04-30-session-persistence-sqlite-design.md` that already sketches a `learner_concepts` table reusing `concepts(id)` as an FK plus a new `understanding_depths` lookup table for `UnderstandingDepth`. This is the natural extension of `primer-storage`: validate-and-seed for closed enums, append-only writes (or upsert-by-natural-key in this case), SQLite separate from RAG. Some upfront benefit now that resume exists — a returning child can pick up not just the conversation but their concept-mastery state.

Slightly bigger than Option A — wants a real design conversation about exactly which fields persist. Don't start without re-reading the spec and confirming current shape with Horst.

### Option C — Graceful API errors (Phase 0.1 leftover)

Mid-stream errors propagate cleanly today, but rate-limit / network-drop / invalid-key all bubble up as raw `PrimerError::Inference("Generation failed: ...")`. Minimum viable shape: detect 401/403 (bad key), 429 (rate limit), 5xx (server) at the top of `CloudBackend::generate_stream` and surface a user-friendly message. Optional: retry-with-jittered-backoff for 429.

### Option D — Knowledge base bootstrapping (Phase 0.2, heavier)

Needs a Python ingestion script for Simple English Wikipedia → SQLite FTS5 + a 50-100 hand-picked seed corpus. Skip unless explicitly asked.

### Option E — Better summarization & retrieval

The summary-and-retrieval layer that just landed is a deliberately simple v1: full re-summarization every K turns, BM25 retrieval with OR-joined tokens. Possible refinements to consider once you have qualitative feedback:
- Incremental summarization (extend the existing summary instead of rebuilding from scratch).
- Stop-word filtering before OR-joining tokens, so common-question words like "what" don't dominate the FTS query.
- Embedding-based retrieval as an alternative to BM25 — would need a local embedding model or an API.
- Drop summary regeneration cost on the cloud backend when token-budget is constrained (e.g. cheaper haiku for summary even when sonnet is the chat model).

Don't start any of these without evidence the simpler version is a problem.

## Files most relevant to start in

- **Option A**: `src/crates/primer-pedagogy/src/prompt_builder.rs` (`decide_intent()` and the system-prompt builder), and the existing characterization tests at the bottom of the same file.
- **Option B**: `docs/superpowers/specs/2026-04-30-session-persistence-sqlite-design.md` (spec), `src/crates/primer-core/src/learner.rs` (the `LearnerModel` / `ConceptState` / `UnderstandingDepth` types), `src/crates/primer-storage/src/{lib,schema,catalog}.rs` (extend the existing pattern). Note: schema is now at v2; new fields would land as a v2 → v3 migration using the `apply_v2_migrations`-shaped pattern.
- **Option C**: `src/crates/primer-inference/src/cloud.rs` (top of `generate_stream`), `src/crates/primer-core/src/error.rs` (you'll likely add a new `PrimerError` variant or two).
- **Option E**: `src/crates/primer-storage/src/lib.rs` (`sanitize_fts_phrase`, `retrieve_session_turns`), `src/crates/primer-pedagogy/src/dialogue_manager.rs` (`refresh_summary_if_due`, `retrieve_long_term_memory`).

## Patterns to reuse, not reinvent

- **Categorical text → integer FK lookup tables.** Mandatory for any new SQLite schema. Closed Rust enum = source of truth, validate-and-seed on `open()`. See `primer-storage::schema::validate_and_seed_lookup`.
- **Idempotent schema migrations.** `apply_v2_migrations` in `primer-storage::schema` is the template: `pragma_table_info` checks before each ALTER, `CREATE … IF NOT EXISTS` for tables/triggers, detect "did the table exist before this open" if you need to backfill.
- **Append-only persistence.** If a Rust-side type is append-only in memory, persistence should mirror that — don't DELETE-then-INSERT. The `INSERT OR REPLACE` ↔ FK-cascade footgun is real.
- **Streaming bridge.** `bytes_stream` → parser buffer → `mpsc::unbounded` → `Box::pin(rx)`. See `cloud.rs::generate_stream` and `ollama.rs::generate_stream`.
- **FTS5 query sanitization.** `sanitize_fts_phrase` in `primer-storage::lib`: alphanumeric-only tokens, drop FTS5 reserved keywords, OR-join quoted tokens. Pair with `LIMIT k` and BM25 ranking.
- **System-prompt layering for context that isn't part of the chat timeline.** Long-term memory (summary + retrieved older turns) lives in the system prompt, never in the messages list. The chat `messages` array stays exactly = `recent_turns(window)`. Future "ambient context" features (current time, user mood, learner-model summary) should follow the same pattern.
- **TDD discipline.** Watch tests fail. Watch them pass. The recent persistence work caught a real bug (FK cascade) with a test written before the implementation was rechecked. Don't skip the failing-test step.
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
- **In-memory `LearnerModel` and `Session.learner_id` can diverge after resume.** The CLI builds the LearnerModel from `--name`/`--age` flags; the loaded session keeps its original `learner_id`. Deferred to learner-persistence work (Option B).
- **`dirs` crate is intentionally not in workspace deps.** The CLI uses `std::env::var("HOME")` for both `~/.primer_env` and `~/.primer/`. If you ever need cross-platform home dir support, add `dirs` then; until then, keep the dep surface small.

## Reporting back

When you finish or hit a blocker:
- State plainly what you got working and what you didn't, by acceptance criterion.
- If you exposed bugs in existing behaviour (e.g., another FK-cascade-style footgun), flag them separately from the assigned task — Horst should decide whether to fix now or file.
- If you discover that Horst did interim work that changes the plan, flag it explicitly. Read the file as it actually is, not as this brief describes it.
