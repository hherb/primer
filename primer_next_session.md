# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated by previous session:** 2026-04-29 (after streaming + review fixes + cloud verification).

## First moves when you start

1. Read [CLAUDE.md](CLAUDE.md) — repo conventions, gotchas, build commands. **Workspace root is `src/`, not the repo root** — every cargo command runs from `src/`.
2. Skim [ROADMAP.md](ROADMAP.md). Phase 0.1 streaming + `--model` are checked off. The next-up items are mostly in Phase 0.3 (pedagogical engine refinement) and the rest of Phase 0.1 (conversation persistence, graceful API-error handling).
3. From `src/`: `cargo build && cargo test`. The previous session left both green with **24 tests** total (parser-level + dialogue-manager + serialisation guard).
4. Don't assume nothing changed since this brief was written. Read the current `cloud.rs`, `ollama.rs`, `dialogue_manager.rs`, `prompt_builder.rs`, and `primer-cli/src/main.rs` first — the user may have made interim changes.

## What the previous session shipped (so you don't redo it)

End-to-end token streaming, fully verified with real backends, plus a small pile of review fixes and ergonomics:

### Streaming (Phase 0.1)

- **Ollama NDJSON streaming** (`primer-inference/src/ollama.rs`). `stream: true`, drains `bytes_stream()` into `NdjsonBuffer` (handles partial lines split across HTTP chunks, lossy UTF-8 decoding so bad bytes surface as U+FFFD instead of vanishing), parses each line with `parse_ollama_line` into a `TokenChunk`, forwards via `futures::channel::mpsc::unbounded`. **8 unit tests** cover line-splitting, JSON parsing, and the lossy-UTF-8 path.
- **Anthropic SSE streaming** (`primer-inference/src/cloud.rs`). `"stream": true`, hand-rolled `SseBuffer` (handles `event:`/`data:` framing, blank-line terminators, `:keepalive` heartbeats, CRLF, exact-one-space stripping after `data:` per spec). `parse_anthropic_event` translates `content_block_delta` → text token, `message_stop` → done, `error` → `Err`, ignores `ping`/`message_start`/`content_block_start`/`content_block_stop`/`message_delta`. **10 unit tests** including a serialisation guard locking in that `top_p` is NOT in the request body.
- **Wire pattern** (identical in both backends): build `(tx, rx)` from `mpsc::unbounded::<Result<TokenChunk>>()`, spawn a tokio task that drains `bytes_stream`, feeds the parser buffer, forwards each parsed chunk via `tx.send(...)`. Return `Ok(Box::pin(rx))`. The spawn is fire-and-forget — see the comment in each backend explaining the cancellation-token TODO.
- **`DialogueManager::respond_to_streaming(input, FnMut(&str))`** (`primer-pedagogy/src/dialogue_manager.rs`). The original `respond_to` is a thin wrapper with a no-op closure. **On a mid-stream error, the partial Primer turn is NOT recorded** — child turn stays, Primer turn dropped, error returned. **6 unit tests** with a `ScriptedBackend` test fixture and an `EmptyKnowledge` stub.
- **CLI** (`primer-cli/src/main.rs`). Uses `respond_to_streaming` with `print!`+`flush` per chunk. The `"Primer: "` prefix is held back until the first non-empty chunk, so a connection failure doesn't leave a dangling label above the error message.

### Review-driven fixes from the same session

- `NdjsonBuffer::pop_line` no longer silently drops invalid-UTF-8 lines (lossy decode + log via the normal parse-error path).
- Removed double-wrapped error in `respond_to_streaming` (`inspect_err` instead of redundant `map_err`).
- Anthropic API request now omits `top_p` entirely — `claude-sonnet-4-6` and later 400 if both `temperature` and `top_p` are set. We picked `temperature` as the canonical knob; `GenerationParams.top_p` is still respected by Ollama. Locked in by the `api_request_omits_top_p` test.
- SSE parser strips exactly one space after `data:` (per RFC) instead of `trim_start()`. Test guards against regression.
- Doc comment in `parse_anthropic_event` notes that we currently treat `message_delta` as benign — `stop_reason` from this event (e.g. `max_tokens` truncation) is silently ignored.

### Dev ergonomics

- **`.env` and `~/.primer_env` auto-loaded at startup** via `dotenvy`. Project-local `.env` wins over `~/.primer_env`. Both `*.local` patterns gitignored. `.env.example` template at the repo root.
- README and ROADMAP updated to reflect current state.

### Verification status

- `cargo build --workspace` → clean.
- `cargo test --workspace` → **24 passed, 0 failed**.
- `cargo clippy --workspace --all-targets` → clean, except for one **pre-existing** unrelated warning on `StubBackend::new` (suggests `Default` impl).
- **Stub** REPL: works (single-chunk degenerate stream).
- **Ollama** REPL with `qwen3.5:4b-q8_0`: produces correct Socratic output. Tokens visibly drip in an interactive terminal. Independently confirmed via `curl -N http://localhost:11434/api/chat` → NDJSON arrives 20–50 ms apart per token.
- **Cloud** REPL with `claude-sonnet-4-6`: produces a clean Socratic response. Independently confirmed via raw `curl -N` against `/v1/messages` → SSE `content_block_delta` events arrive 100–300 ms apart for a longer prompt. The pipeline preserves that timing.

## The next task: pick one

All Phase 0.1 streaming work is done. The unblocked items, in order of recommended priority:

### Option A (recommended) — `decide_intent()` unit tests (Phase 0.3)

`primer-pedagogy/src/prompt_builder.rs` contains `decide_intent()`, the heuristic that picks the next `PedagogicalIntent` from learner state, recent turns, and conversation length. **The Primer's brain is currently untested** — the previous session put TDD scaffolding in place (`ScriptedBackend`, `EmptyKnowledge`, `test_learner()` factory in `dialogue_manager.rs::tests`) but didn't reach into `prompt_builder`.

Cheap to start: `decide_intent` is pure, no IO, no async. Read it first, write tests for what it actually does, then add tests for what it *should* do (and propose fixes as a follow-up if the actual and the should-be diverge).

Suggested cases to consider — verify by reading the function first, the heuristic may already do some of this:
- Engaged child, normal-length response → default `SocraticQuestion`.
- `EngagementState::Frustrated` → `Scaffolding` or `Encouragement`.
- `EngagementState::Disengaging` → `SessionClose` near session end, or `Encouragement` early.
- Very short child response → `ComprehensionCheck`.
- Pure factual question pattern ("what is X?") → `DirectAnswer`, with a `AnswerThenPivot` follow-up at the next turn.
- Long session, child still engaged → `Extension`.

Add tests to a new `prompt_builder::tests` module (or follow whatever pattern is already there). They should be sync `#[test]` fns — no tokio runtime needed.

### Option B — Conversation persistence (remaining Phase 0.1 item)

ROADMAP.md's last open Phase-0.1 task. Shape:
- `--load-session <path>` to deserialise a `Session` from JSON at startup.
- `--save-session <path>` (or auto-save on graceful exit) to serialise the current `Session`.
- `Session` already derives `Serialize`/`Deserialize` (see `primer-core/src/conversation.rs:62`). The actual disk I/O is small.

Decisions to make: (1) When loading, do you re-use the loaded session's `learner_id` or assign a new one? Probably the former; offer a flag for the latter. (2) Auto-save on `quit`/`exit`/`bye` only, or also on `Ctrl-C`? The latter wants `tokio::signal`.

### Option C — Graceful API-error handling (Phase 0.1, partial)

The brief flagged this and the previous session only got partway: mid-stream errors propagate cleanly, but rate-limit / network-drop / invalid-key cases just bubble up as raw `PrimerError::Inference`. A minimal improvement: detect 401/403 (bad key), 429 (rate limit), 5xx (server) at the top of `generate_stream` for `CloudBackend` and surface a user-friendly message via a new `PrimerError` variant. Could pair with a small retry-with-jittered-backoff for 429.

### Option D — Knowledge base bootstrapping (Phase 0.2, heavier)

Needs a Python ingestion script for Simple English Wikipedia → SQLite FTS5. Skip unless explicitly asked; A/B/C are faster value.

## Files most relevant to start in

- `src/crates/primer-pedagogy/src/prompt_builder.rs` — for Option A. Where `decide_intent()` lives.
- `src/crates/primer-pedagogy/src/dialogue_manager.rs` — read the existing `tests` module for the test-fixture pattern (`ScriptedBackend`, `EmptyKnowledge`).
- `src/crates/primer-cli/src/main.rs` — for Option B.
- `src/crates/primer-core/src/conversation.rs` — for Option B, `Session` is already Serde-able.
- `src/crates/primer-inference/src/cloud.rs` — for Option C, plus see the `parse_anthropic_event` error branch as the template.

## Patterns to reuse, not reinvent

- **Streaming bridge**: `bytes_stream` → parser buffer → `mpsc::unbounded` → `Box::pin(rx)`. Don't add `tokio-stream` or `reqwest-eventsource`; the workspace deliberately keeps the dep tree small. See `ollama.rs::generate_stream` for the canonical shape.
- **Test fixtures for dialogue manager tests**: `ScriptedBackend`, `EmptyKnowledge`, `test_learner()` in `dialogue_manager::tests`. Lift into a shared `pub(crate)` location only when a second consumer actually needs them.
- **TDD discipline expected.** Watch tests fail. Watch them pass. Don't write production code first. The previous session followed this religiously and every implementation step landed first-try, including the two bugs the cloud test caught (the `top_p` 400 and the silent-UTF-8 drop).
- **`.env` and `~/.primer_env`** are auto-loaded — don't tell the user to `export` again. If you need a new secret/env var, document it in `.env.example`.

## Watch-outs (still relevant)

- `Role::System → "user"` mapping in `cloud.rs` and the leading `system` message in `ollama.rs` are different on purpose. Both are correct for their respective APIs.
- `update_learner_model` in `dialogue_manager.rs` is still a placeholder (word-count heuristic only). Real comprehension assessment is Phase 0.3.
- Concept extraction (`turn.concepts`) is still always `vec![]` for child turns. Also Phase 0.3.
- The stub backend still emits one final chunk by design — that's a degenerate but valid stream. Don't "fix" it.
- `top_p` is silently dropped from cloud requests. If you ever extend the cloud backend to use it conditionally, the `api_request_omits_top_p` test will fail and remind you to update it.
- The two streaming-task spawns are fire-and-forget. Long-lived deployments (Phase 2/3) will want a cancellation token; for the CLI today the consumer-drop semantic is fine.

## Reporting back

When you finish or hit a blocker:
- State plainly what you got working and what you didn't, by acceptance criterion.
- If `decide_intent()` tests exposed bugs in the heuristic, flag those separately from the test work — the user should decide whether to fix the heuristic now or file it.
- If you discover the user did interim work that changes the plan, flag it explicitly.
