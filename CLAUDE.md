# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project shape

The Primer is a Socratic AI learning companion for children. The codebase is a **Rust workspace under `src/`** (not the repo root) — every cargo command must be run from `src/`. The workspace targets Rust **edition 2024** (requires Rust 1.85+). No system dependencies: SQLite is bundled, TLS uses rustls.

The repo is at the "early skeleton" stage: trait architecture and module boundaries exist, the code compiles, and a text REPL works against a stub backend or the Anthropic Claude API. There is no local inference, no speech pipeline, and no hardware integration yet. See [ROADMAP.md](ROADMAP.md) for phase plan and [primer_technical_spec.md](primer_technical_spec.md) for the long-form vision.

## Common commands

All commands run from `src/`:

```bash
cargo build                                          # build the workspace
cargo run --bin primer                               # REPL with stub backend (no API key needed)
cargo run --bin primer -- --backend cloud --name Binti --age 8                # uses ANTHROPIC_API_KEY env var
cargo run --bin primer -- --backend cloud --model claude-opus-4-7             # override the default cloud model
cargo run --bin primer -- --backend ollama --model llama3.2                   # local Ollama at http://localhost:11434
cargo run --bin primer -- --session-db /tmp/explicit.db                       # override default session DB location
cargo run --bin primer -- --resume <uuid>                                     # resume a past session (no greeting)
cargo test                                           # run all tests (parser + dialogue manager coverage)
cargo test -p primer-pedagogy                        # tests for one crate
cargo test -p primer-pedagogy decide_intent          # single test by name substring
cargo clippy --workspace --all-targets               # lint
cargo fmt                                            # format
RUST_LOG=debug cargo run --bin primer                # verbose tracing output
```

CLI flags exposed by `primer-cli`: `--backend stub|cloud|ollama`, `--model` (Anthropic id for cloud, defaults to `claude-sonnet-4-6`; required local tag for ollama), `--ollama-url` (default `http://localhost:11434`), `--name`, `--age`, `--knowledge-db <path>` (defaults to `:memory:`), `--session-db <path>` (defaults to `~/.primer/<slugified-name>.db`, created if missing; pass an explicit path only to override), `--resume <uuid>` (resume a past session by UUID; reads from `--session-db`; errors if file or id is missing; no greeting on resume), `--api-key` (or `ANTHROPIC_API_KEY` env). Type `quit`, `exit`, or `bye` to end the REPL.

Env files are auto-loaded at startup. Two locations checked, in order: (1) project-local `.env` (dotenvy searches cwd and ancestors), then (2) user-global `~/.primer_env`. Earlier sources win, so a per-repo `.env` overrides the home file. Copy `.env.example` → `.env` for a per-repo config, or drop `ANTHROPIC_API_KEY=...` into `~/.primer_env` to share across projects. Both `.env` and `*.local` variants are gitignored.

## Architecture: trait-based hardware abstraction

The central design principle is that **the pedagogical engine is decoupled from any specific inference backend, speech engine, or knowledge store via traits in `primer-core`**. Backend selection is a runtime config choice, not a code change. This is what allows Phase 0 (cloud) to share code with Phase 1 (llama.cpp / QNN NPU / RKNN NPU) and Phase 2 (Whisper + Piper).

The six crates form a layered dependency graph:

```
primer-cli  →  primer-pedagogy  →  primer-core  ←  primer-inference, primer-speech, primer-knowledge, primer-storage
```

- **`primer-core`** — the only crate that defines public traits (`InferenceBackend`, `KnowledgeBase`, `SpeechToText`, `TextToSpeech`) plus shared types (`LearnerModel`, `Session`, `Turn`, `PedagogicalIntent`, `EngagementState`, `UnderstandingDepth`, `Prompt`, `Passage`). Everything else depends on this.
- **`primer-inference`** — `StubBackend` (canned Socratic responses, no model), `CloudBackend` (Anthropic Messages API), and `OllamaBackend` (local Ollama `/api/chat`, useful for prototype testing against real models without integrating llama.cpp directly). `LlamaCppBackend`, `QnnBackend`, `RknnBackend` are TODOs.
- **`primer-speech`** — stub-only today; speech is a Phase 2 concern.
- **`primer-knowledge`** — `SqliteKnowledgeBase` using FTS5 with BM25 ranking. The schema is created on `open()` if missing. The retrieve path negates FTS5's negative rank so higher-score = more-relevant in the public API.
- **`primer-storage`** — `SqliteSessionStore` persists every conversation `Session` (one DB per child, separate from the RAG corpus on privacy grounds). Schema is normalised: every categorical text column (`speaker`, `pedagogical_intent`, `concept`) is a foreign key into a lookup table. The `speakers` and `pedagogical_intents` lookup tables are seeded and validated against the canonical Rust enums on every `open()` — drift is a hard error. `concepts` is open-vocabulary and lazily populated. The trait now exposes `save_session`, `load_session(id) -> Option<Session>`, and `retrieve_session_turns(session_id, query, k, exclude_indices_at_or_after)` — the last two underpin the `--resume` flow and the FTS5-backed long-term memory layer. Schema is at version 2: v2 added `sessions.summary` + `sessions.summary_through_turn_index` (rolling LLM summary of pre-window turns) and a `turn_text_fts` FTS5 virtual table over `turns.text` with triggers keeping it in sync. `apply_v2_migrations` is idempotent — it runs on every open and brings v1 DBs to v2 in place. `DialogueManager` accepts an optional `&dyn SessionStore`; when set, it saves after every turn (success or mid-stream error), on `open_session`, on `resume_session`, and on `close_session`. Save failures `tracing::warn!` instead of propagating. Implementation pattern mirrors `primer-knowledge`: a single `rusqlite::Connection` wrapped in `Mutex`, async trait methods with synchronous bodies — acceptable at conversation turn rates; revisit under profiling if a multi-process consumer ever appears.
- **`primer-pedagogy`** — the Primer's "soul". Two modules:
  - `prompt_builder` builds the system prompt (encoding the Socratic method as instructions, varying by age, engagement, and intent), assembles messages from session turns, and contains `decide_intent()` — the heuristic that picks the next `PedagogicalIntent`. `build_prompt` takes a `summary` (rolling pre-window summary) and a slice of `retrieved_older` turns; both are injected as system-prompt sections so the chat-message timeline (the last `context_window_turns` turns) stays linear.
  - `dialogue_manager` orchestrates a session: record child turn → decide intent → retrieve knowledge → retrieve long-term memory (summary + FTS-relevant older turns) → build prompt → call inference → record Primer turn → refresh summary if due → update learner model. New entry point `resume_session(loaded)` replaces `open_session()` for resumed flows: it swaps in the loaded `Session`, clears `ended_at`, regenerates the summary unconditionally, and persists. Summary refresh threshold: every `context_window_turns` turns of new pre-window content (default 20). Summarization goes through `InferenceBackend::summarize` — stub returns a canned string; cloud/ollama default-impl through `generate`.
- **`primer-cli`** — the REPL binary (binary name `primer`, not `primer-cli`).

`DialogueManager` holds `&dyn InferenceBackend` and `&dyn KnowledgeBase` (borrowed references, not owned), so swapping backends is a CLI/config decision and the engine is testable with stubs.

## Conventions and gotchas worth knowing

- **The workspace root is `src/Cargo.toml`, not the repo root.** All workspace deps are pinned there via `[workspace.dependencies]`; per-crate `Cargo.toml` files use `.workspace = true`.
- The binary is named `primer` (see `[[bin]]` in `primer-cli/Cargo.toml`), so `cargo run --bin primer`.
- **Real-backend streaming is live.** `CloudBackend::generate_stream` consumes Anthropic SSE (`event:`/`data:` framing, hand-rolled `SseBuffer` + `parse_anthropic_event`). `OllamaBackend::generate_stream` consumes NDJSON (one JSON object per `\n`-terminated line, `NdjsonBuffer` + `parse_ollama_line`). Both forward chunks via a `futures::channel::mpsc::unbounded` driven by a spawned tokio task. The stub backend still emits one final chunk by design — that's a degenerate but valid stream and shouldn't be "fixed".
- **`CloudBackend` currently maps `Role::System` to `"user"` in the messages array** (system instructions are a top-level field on the Anthropic API). `OllamaBackend` instead prepends a `system`-role message because Ollama's chat API has no separate system field. Watch this divergence when reworking prompt assembly.
- **`DialogueManager` exposes both `respond_to` and `respond_to_streaming(input, FnMut(&str))`.** The non-streaming method is now a thin wrapper around the streaming one with a no-op callback. On a mid-stream error, the partial Primer turn is **not** recorded into the session — the child turn stays, the Primer turn is dropped, and the error is returned.
- **Cloud model defaults to `claude-sonnet-4-6`** but is now overridable via `--model`. For ollama, `--model` is required (e.g., `--model llama3.2`).
- **`update_learner_model` in `dialogue_manager.rs` is a deliberate placeholder** — it only does a crude word-count heuristic for `EngagementState`. Real comprehension assessment is a Phase 0.3 task. Concept extraction from child input is also a placeholder (always empty `concepts` vec).
- **`decide_intent()` is the brain of the Socratic behaviour.** It is the most important function to test rigorously when adding pedagogical features. Tests are listed as a Phase 0.3 priority.
- Knowledge base default is `:memory:` (empty). Retrieval just returns no passages and the prompt builder gracefully omits the knowledge section. Bootstrapping a corpus is a Phase 0.2 task.
- Errors: use `primer_core::error::PrimerError` (variants per subsystem) and the `Result<T>` alias. Wrap external errors via `.map_err(|e| PrimerError::Inference(format!("...: {e}")))`-style.
- **Categorical text columns are normalised into lookup tables and stored as integer foreign keys.** Applies to every SQLite schema in this codebase (sessions, learner-model concepts when Phase 0.3 lands, etc.). For closed Rust enums (`Speaker`, `PedagogicalIntent`, `UnderstandingDepth`), the Rust enum is the single source of truth; the DB lookup table is a derived projection that the storage layer regenerates and validates against on every `open()`. Drift (mismatched name on a known id, unknown id) is a hard error — there is **no** API exposed for mutating lookup tables. Adding a new variant means: edit the Rust source, recompile, next `open()` seeds the new row. Retired integer IDs are never reused.
- **Schema migrations are idempotent column-add patterns.** `apply_v2_migrations` checks `pragma_table_info` before each ALTER and uses `CREATE VIRTUAL TABLE IF NOT EXISTS` / `CREATE TRIGGER IF NOT EXISTS`. Bumping `USER_VERSION` is one half of a migration; the other half is making the new objects safe to re-create on a re-open. Reject `existing_version > USER_VERSION` (newer-than-this-build); silently update old `existing_version != USER_VERSION` to current after running migrations.
- **FTS5 query inputs are sanitized.** Child input is fed through `sanitize_fts_phrase`: tokens are stripped of non-alphanumeric chars, FTS5 reserved keywords (`AND`, `OR`, `NOT`, `NEAR`) are dropped, surviving tokens are quoted and OR-joined. Empty result short-circuits the query. This is what allows `retrieve_session_turns` to take raw child input safely. BM25 ranking + `LIMIT k` keeps the OR-join from producing noise.
- **Long-term memory is a system-prompt concern, not a chat-message concern.** The chat `messages` array passed to inference is exactly `session.recent_turns(context_window_turns)`. Summary and retrieved-older turns are injected into the system prompt. This keeps the timeline the model sees linear and the context budget bounded even across hours of conversation.
- **Sessions persist by default.** Without `--session-db`, the CLI defaults to `~/.primer/<slugified-name>.db`. The directory is created on first run; an unwritable home is a hard error rather than a silent in-memory fallback. `--resume <uuid>` requires the file to exist (no auto-create on a typo) and errors clearly when the id isn't present.

## Pedagogical principles encoded in the system prompt

These constrain prompt-builder and dialogue-manager changes — if a change makes the Primer more answer-y or more engagement-maximising, it is wrong:

- The Primer asks more questions than it answers; pure factual questions get a direct answer **then** a Socratic pivot.
- The Primer never tries to maximise engagement. It detects frustration/disengagement and offers breaks, scaffolding, or session close — never guilt.
- All learner data is local; cloud inference sends turns per-request only.
- Comprehension is verified through transfer questions, application, and contradiction probing — not assumed from a confident-sounding response.
