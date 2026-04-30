# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-04-30 (after the SQLite session-persistence PR shipped and got an in-place review pass).

## First moves when you start

1. Read [CLAUDE.md](CLAUDE.md) — repo conventions, gotchas, build commands. **Workspace root is `src/`, not the repo root** — every cargo command runs from `src/`.
2. Skim [ROADMAP.md](ROADMAP.md). Phase 0.1 streaming + `--model` + conversation persistence are checked off. Open Phase 0.1 item: graceful API-error handling. Phase 0.3 is the bulk of what's still ahead.
3. From `src/`: `cargo build && cargo test`. Should be green: **71 tests** across the workspace.
4. From `src/`: `cargo clippy --workspace --all-targets`. Should be 100 % clean (no warnings — the historical `StubBackend::new_without_default` baseline was retired in `d44323f`).
5. **Don't assume nothing changed since this brief was written.** Read the current state of the files you intend to touch first — Horst may have made interim changes.

## What was shipped most recently (so you don't redo it)

### SQLite session persistence — Phase 0.1 closed (PR #1, merged as `56baca1`)

- **New crate `primer-storage`** with `SqliteSessionStore`. Per-child SQLite file, separate from the RAG corpus on privacy grounds. Schema is normalised: every categorical text column is an integer FK into a lookup table.
- **Lookup tables (`speakers`, `pedagogical_intents`)** are seeded from the canonical Rust enums (`Speaker::ALL`, `PedagogicalIntent::ALL`) on every `open()`. Drift (mismatched name on a known id, unknown id) is a hard error. There is **no public API for mutating lookup tables** — adding a new variant means edit Rust source + recompile + next `open()` seeds the new row. Retired integer ids are never reused.
- **`concepts`** is open-vocabulary, lazily populated via `INSERT OR IGNORE INTO concepts (name)`. Note `concepts.name` is the TEXT label; `turn_concepts.concept_id` is the INTEGER FK to `concepts.id`. Same word, different things — be careful when reading SQL.
- **`save_session` is append-only and idempotent.** It does NOT DELETE-then-INSERT turns; it `SELECT COUNT(*)` and inserts only `session.turns[count..]`. Turn rowids stay stable across saves — pinned by `turn_ids_are_stable_across_appending_saves`. The `sessions` row uses a real UPSERT (`ON CONFLICT(id) DO UPDATE SET ended_at = excluded.ended_at`); `INSERT OR REPLACE` would have cascaded through the FK and wiped every turn (we found this with the new test, hence the rewrite).
- **`DialogueManager` accepts `Option<&dyn SessionStore>`** and saves after every turn (success path AND mid-stream-error path), on `open_session` (greeting), and on `close_session` (so `ended_at` lands on disk). `close_session` is now `async`. Save failures `tracing::warn!` — they don't propagate, so a flaky disk doesn't tear down the conversation.
- **CLI**: `--session-db <path>` (defaults to `:memory:`, parity with `--knowledge-db`). Both defaults share an `IN_MEMORY` const at the top of `main.rs`.
- **Project-wide rule captured in CLAUDE.md**: every categorical text column in this codebase's SQLite schemas must be normalised into a lookup table and stored as an integer FK. Closed Rust enums are the source of truth; the storage layer regenerates and validates the lookup table on every `open()`.

### Code-review pass on the same PR (commits `0b43a24`, `d44323f`)

Caught and fixed in-place rather than as follow-ups:
- DELETE+INSERT → append-only writes (described above; the FK-cascade bug fell out of this).
- `unchecked_transaction` → standard `transaction()` via `let mut conn = self.conn.lock().unwrap()`.
- `concepts.concept_id` (TEXT label) renamed to `concepts.name` to disambiguate from the `turn_concepts.concept_id` integer FK.
- `#[allow(dead_code)]` on the catalog module → per-function on `intent_from_id` / `speaker_from_id` only.
- New `IN_MEMORY` const in `primer-cli/main.rs`.
- Sync-Mutex caveat documented in the `primer-storage` crate docstring.
- `StubBackend::new()` deleted (was zero-arg, unused — call sites use `Box::new(StubBackend)` against the unit struct directly). Killed the last clippy warning.

### Docs

- README: status note now mentions per-turn SQLite persistence; architecture corrected from six → seven crates; new `primer-storage` section; `--session-db` in the CLI options table.
- ROADMAP: Phase 0.1 persistence ticked; test-coverage line refreshed to 71-tests breakdown; completed items use ✅ instead of `[x]` for visual contrast; contributors table uses real names (Bernd Brinkmann, Frithjof Herb) and adds Claude as the AI pair-programmer.

### Verification status

- `cargo build --workspace` → clean.
- `cargo test --workspace` → **71 passed, 0 failed.**
- `cargo clippy --workspace --all-targets` → fully clean (no warnings).
- `cargo fmt --check` → clean.
- Manual REPL with `--session-db /tmp/x.db` was sketched in the PR test plan but not run end-to-end by Claude — Horst may or may not have done it. If you change anything in the save path, run the manual checklist from PR #1's description.

## The next task: pick one

Phase 0.1 is now complete except for graceful API-error handling. Phase 0.2 (knowledge bootstrapping) and Phase 0.3 (pedagogical engine) are open. Recommended in order of leverage and unblockedness:

### Option A (recommended) — Resume past session (`--resume <session-id>`)

Natural follow-up to the persistence work that just landed. The schema already supports it — purely additive surface.

Shape:
- Add `async fn load_session(&self, id: Uuid) -> Result<Option<Session>>` to the `SessionStore` trait in `primer-core/src/storage.rs`.
- Implement on `SqliteSessionStore`: SELECT the session row, SELECT all turns ordered by `turn_index`, SELECT `turn_concepts` joined to `concepts.name` and group by turn. Reverse-map `speaker_id` and `intent_id` via `catalog::speaker_from_id` / `catalog::intent_from_id` (both already exist, currently `#[allow(dead_code)]` precisely because this was anticipated).
- Add `--resume <uuid>` to `primer-cli`: when present, call `load_session` and pass the loaded `Session` into a new `DialogueManager::resume_session(...)` constructor (instead of `open_session`'s greeting). If the id is unknown, error out clearly.
- Optional follow-up flag: `--list-sessions` to print recent sessions for the current learner.

Decisions: probably keep the loaded session's existing `learner_id` (no flag for re-assigning — out of scope). Decide whether resuming a session with an `ended_at` timestamp clears it (probably yes — re-opening means the session is active again).

Tests: load round-trip, load-of-unknown-id, load-with-concepts. Mirror `primer-storage` test style.

### Option B — `decide_intent()` gap fixes (Phase 0.3, three small bugs)

The characterization tests that landed in `primer-pedagogy::prompt_builder::tests` explicitly document three current gaps with named tests like `frustrated_does_not_currently_return_encouragement` and `factual_question_pattern_does_not_currently_return_direct_answer`. Each gap is a separate ROADMAP 0.3 bullet:

1. Make `Encouragement` reachable. Currently `Frustrated` always returns `Scaffolding`; the split needs a comprehension/engagement signal — "frustrated and stuck" → Scaffolding, "frustrated but still trying" → Encouragement.
2. Detect factual-question patterns ("what is X?", "how does Y work?") and route to `DirectAnswer`, with `AnswerThenPivot` on the next turn. Both intents are currently unreachable.
3. Make `Disengaging` session-length-aware. Route to `Encouragement` early, `SessionClose` only after a meaningful duration. Pairs with the (still-open) session-time-tracking bullet.

For each one: read the existing characterization test, design the new behaviour with Horst, flip the test from "does not currently" to "now does", add a fresh characterization test that the negative case still picks the old branch. **Update the system prompt in `prompt_builder.rs` if intent semantics change** — the prompt encodes per-intent guidance and may need wording adjustments.

### Option C — Learner-model SQLite persistence (Phase 0.3, design exists)

There's a fairly detailed spec at `docs/superpowers/specs/2026-04-30-session-persistence-sqlite-design.md` that already sketches a `learner_concepts` table reusing `concepts(id)` as an FK plus a new `understanding_depths` lookup table for `UnderstandingDepth`. This is the natural extension of `primer-storage` and reuses every pattern you just shipped: validate-and-seed for closed enums, append-only writes (or upsert-by-natural-key in this case), SQLite separate from RAG.

Slightly bigger than Option A — wants a real design conversation about exactly which fields persist. Don't start without re-reading the spec and confirming current shape with Horst.

### Option D — Graceful API errors (Phase 0.1 leftover)

Mid-stream errors propagate cleanly today, but rate-limit / network-drop / invalid-key all bubble up as raw `PrimerError::Inference("Generation failed: ...")`. Minimum viable shape: detect 401/403 (bad key), 429 (rate limit), 5xx (server) at the top of `CloudBackend::generate_stream` and surface a user-friendly message. Optional: retry-with-jittered-backoff for 429.

### Option E — Knowledge base bootstrapping (Phase 0.2, heavier)

Needs a Python ingestion script for Simple English Wikipedia → SQLite FTS5 + a 50-100 hand-picked seed corpus. Skip unless explicitly asked.

## Files most relevant to start in

- **Option A**: `src/crates/primer-core/src/storage.rs` (trait), `src/crates/primer-storage/src/lib.rs` and `catalog.rs` (impl + reverse lookups), `src/crates/primer-cli/src/main.rs` (CLI surface), `src/crates/primer-pedagogy/src/dialogue_manager.rs` (probably needs a `resume_session` constructor).
- **Option B**: `src/crates/primer-pedagogy/src/prompt_builder.rs` (`decide_intent()` and the system-prompt builder), and the existing characterization tests at the bottom of the same file.
- **Option C**: `docs/superpowers/specs/2026-04-30-session-persistence-sqlite-design.md` (spec), `src/crates/primer-core/src/learner.rs` (the `LearnerModel` / `ConceptState` / `UnderstandingDepth` types), `src/crates/primer-storage/src/{lib,schema,catalog}.rs` (extend the existing pattern).
- **Option D**: `src/crates/primer-inference/src/cloud.rs` (top of `generate_stream`), `src/crates/primer-core/src/error.rs` (you'll likely add a new `PrimerError` variant or two).

## Patterns to reuse, not reinvent

- **Categorical text → integer FK lookup tables.** Mandatory for any new SQLite schema. Closed Rust enum = source of truth, validate-and-seed on `open()`. See `primer-storage::schema::validate_and_seed_lookup` for the implementation pattern.
- **Append-only persistence.** If a Rust-side type is append-only in memory, persistence should mirror that — don't DELETE-then-INSERT. The `INSERT OR REPLACE` ↔ FK-cascade footgun is real (see commit `0b43a24` for the gory details).
- **Streaming bridge.** `bytes_stream` → parser buffer → `mpsc::unbounded` → `Box::pin(rx)`. Don't add `tokio-stream` or `reqwest-eventsource`. See `cloud.rs::generate_stream` and `ollama.rs::generate_stream` for canonical shapes.
- **TDD discipline.** Watch tests fail. Watch them pass. The recent persistence work caught a real bug (FK cascade) with a test that was written before the implementation was rechecked. Don't skip the failing-test step.
- **Code-review fixes in-place.** The user's preference, captured this session: when a review surfaces multiple issues, fix them all in one pass on the same branch rather than deferring to follow-ups.
- **`.env` and `~/.primer_env`** are auto-loaded. Don't tell the user to `export` again. New secrets/env vars get documented in `.env.example`.

## Watch-outs (still relevant)

- **`Role::System → "user"`** mapping in `cloud.rs` versus the leading `system` message in `ollama.rs` are different on purpose. Both are correct for their respective APIs.
- **`update_learner_model` in `dialogue_manager.rs` is still a placeholder** (word-count heuristic only). Real comprehension assessment is Phase 0.3 (Option C above touches the persistence side; the assessment itself is a separate bullet).
- **Concept extraction (`turn.concepts`) is still always `vec![]` for child turns.** Also Phase 0.3.
- **The stub backend emits one final chunk by design** — degenerate but valid stream. Don't "fix" it.
- **`top_p` is silently dropped from cloud requests.** If you ever extend the cloud backend to use it conditionally, the `api_request_omits_top_p` test will fail and remind you to update it.
- **The two streaming-task spawns are fire-and-forget.** Long-lived deployments (Phase 2/3) will want a cancellation token; for the CLI today the consumer-drop semantic is fine.
- **Save failures from `SqliteSessionStore` are logged, not propagated.** Watch out if a future feature needs to *know* a save succeeded (e.g., transactional outbox patterns) — you'd need to thread the result back up rather than warning.
- **`save_session` assumes append-only memory.** If you ever introduce a way to mutate or delete already-recorded turns in `Session`, the storage layer's `SELECT COUNT(*)`-based skip will silently drop your changes. Document any such mutation up front and re-think the save path.

## Reporting back

When you finish or hit a blocker:
- State plainly what you got working and what you didn't, by acceptance criterion.
- If you exposed bugs in existing behaviour (e.g., another FK-cascade-style footgun), flag them separately from the assigned task — Horst should decide whether to fix now or file.
- If you discover that Horst did interim work that changes the plan, flag it explicitly. Read the file as it actually is, not as this brief describes it.
