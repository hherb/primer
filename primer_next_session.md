# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated by previous session:** 2026-04-29.

## First moves when you start

1. Read [CLAUDE.md](CLAUDE.md) — repo conventions, gotchas, build commands. Workspace root is `src/`, not the repo root.
2. Skim [ROADMAP.md](ROADMAP.md) — Phase 0 is the active work; the items here come from Phase 0.1.
3. Run `cd src && cargo build` to confirm a clean baseline before changing anything. The previous session left it green.
4. Don't assume nothing changed since this brief was written. Read the current `cloud.rs`, `ollama.rs`, `dialogue_manager.rs`, and `primer-cli/src/main.rs` first — the user may have done work in between, and you don't want to undo it.

## What the previous session shipped (so you don't redo it)

- **Ollama backend** in `primer-inference/src/ollama.rs`. Posts to `/api/chat` non-streaming. `think: false` is set so reasoning models (Qwen3 etc.) don't burn the `num_predict` budget on hidden traces. Empty content is now an explicit error, not a silent blank.
- **`--model` CLI flag**, optional override for cloud (defaults to `claude-sonnet-4-6`), required for ollama. New `--ollama-url` flag, default `http://localhost:11434`.
- **Stronger age-banded language guidance** in `prompt_builder.rs`: a `language_guidance_for_age()` helper plus an explicit "Vocabulary discipline" section in the system prompt. Includes a staged-repetition rule for newly-introduced words.
- **Roadmap entries**: per-child vocabulary spaced-repetition store added to Phase 0.3; consented cross-child language corpus added to Phase 4. Schema constraint linking the two is recorded in 0.3.
- **CLAUDE.md** created.

End-to-end conversation against a real local model (Qwen3 via Ollama) is verified working.

## The next task: end-to-end streaming

**Goal:** when the user types a question, tokens appear progressively in the terminal as the model generates them, instead of the whole response landing at once after a 3–5 second wait.

**Why now:**
- Time-to-first-token dominates perceived latency for conversational UX. The current "wait, then dump" feels worse than a slower model that drips tokens.
- Phase 2 (TTS) absolutely needs this — Piper synthesises sentence-by-sentence; without streaming LLM output, you can't begin TTS until full generation completes, doubling end-to-end latency.
- The trait already supports it: `InferenceBackend::generate_stream()` returns a `TokenStream` (`Pin<Box<dyn Stream<Item = Result<TokenChunk>> + Send>>`). Both real backends just stub it by emitting one final chunk. No trait change needed — fix the implementations.

## Implementation plan, in order

Do these sequentially. Don't try to land all three in one commit — the diffs interact through the dialogue manager.

### Step 1 — Ollama streaming (do this first; simpler protocol, no auth, fastest feedback loop)

Ollama's chat API streams NDJSON when `stream: true`. One JSON object per line:

```
{"model":"...", "message":{"role":"assistant","content":"Hello"}, "done":false}
{"model":"...", "message":{"role":"assistant","content":" there"}, "done":false}
{"model":"...", "message":{"role":"assistant","content":""}, "done":true, "total_duration":...}
```

Implementation:
- Flip `stream: true` in `ChatRequest`.
- Get the response body as a byte stream (`reqwest::Response::bytes_stream()`).
- Build a `Stream<Item = Result<TokenChunk>>` that:
  - Buffers incoming bytes.
  - Splits on `\n` (use `tokio_util::codec::LinesCodec` or hand-roll — bytes from reqwest are already chunked, but a single chunk may contain a partial line, and a line may straddle two chunks).
  - Parses each complete line as a `ChatChunk` (existing struct) and emits `TokenChunk { text: chunk.message.content, done: chunk.done }`.
  - Propagates parse errors as `Result::Err` items in the stream.

Watch-outs:
- A network chunk might end mid-line. Buffer the remainder until the next chunk delivers a `\n`.
- The final `done: true` chunk has empty `content`. Don't error out on empty content for that one — the previous-session check for empty content was on the whole response, which won't apply to streaming. Drop that check or move it to "no content received across the entire stream".
- UTF-8 boundaries: lines are full JSON, so multi-byte chars are inside JSON strings — no special handling needed if you split on `\n` byte and parse the whole line.

### Step 2 — Anthropic SSE streaming

Anthropic's Messages API streams via Server-Sent Events when `"stream": true`. Format is `event: <type>\ndata: <json>\n\n`.

Event types you need to handle:
- `content_block_delta` — `{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}` — emit a `TokenChunk`.
- `message_stop` — emit final `TokenChunk { text: "", done: true }`.
- Others (`message_start`, `content_block_start`, `content_block_stop`, `message_delta`, `ping`) — ignore or trace-log.
- `error` events — propagate as `Result::Err`.

Implementation:
- Add `"stream": true` to `ApiRequest`.
- Parse SSE either by hand (small parser, ~30 lines) or with `reqwest-eventsource`. Recommendation: hand-roll. The format is simple, the dependency footprint matters for embedded targets later, and the parser is independent of any reqwest version.
- A line-based SSE parser:
  - Split incoming bytes by `\n`.
  - Accumulate lines starting with `event:` and `data:` until you hit a blank line (= end of event).
  - Dispatch on event type.

Watch-outs:
- The Anthropic API ignores empty `system` strings but rejects empty `messages`. Make sure you're still constructing valid requests.
- `Role::System` is currently mapped to `"user"` in the messages array (system is the top-level field). Don't break that during the refactor.
- A `TokenChunk` with empty text and `done: false` is fine but pointless — filter them out at the source if you can.

### Step 3 — Make `DialogueManager` stream tokens out to the caller

`respond_to()` currently does `inference.generate(...)` which collects internally. The dialogue manager still needs the full accumulated response (to record the turn and update the learner model), so we don't want to push streaming all the way through as a `Stream` return type — we want a callback.

Add a sibling method:

```rust
pub async fn respond_to_streaming<F>(
    &mut self,
    child_input: &str,
    mut on_chunk: F,
) -> Result<String>
where
    F: FnMut(&str),
```

Behaviour:
1. Same setup as `respond_to`: record child turn, decide intent, retrieve knowledge, build prompt.
2. Call `inference.generate_stream(&prompt, &params).await?` to get a `TokenStream`.
3. Loop with `stream.next().await`, accumulating `TokenChunk::text` into a `String` AND calling `on_chunk(&chunk.text)` for each.
4. After the stream completes, do the existing post-generation work (record Primer turn, update learner model) using the accumulated string.
5. Return the full accumulated string.

Keep `respond_to` as a thin wrapper that calls `respond_to_streaming` with a no-op closure, so existing callers don't break.

### Step 4 — CLI prints tokens incrementally

In `primer-cli/src/main.rs`, replace the current call:

```rust
match dm.respond_to(input).await {
    Ok(response) => println!("\nPrimer: {response}\n"),
    ...
}
```

with:

```rust
print!("\nPrimer: ");
stdout.flush()?;
let result = dm.respond_to_streaming(input, |chunk| {
    print!("{chunk}");
    let _ = io::stdout().flush();
}).await;
println!("\n");
match result {
    Ok(_) => {}
    Err(e) => eprintln!("Error generating response: {e}"),
}
```

The break-suggestion check and prompt redraw stay where they are.

### Step 5 — Update CLAUDE.md

Remove the gotcha line that says streaming isn't actually implemented yet. Replace with whatever's true after the change. The prompt-assembly divergence between Cloud and Ollama (system as top-level field vs leading message) still applies — don't delete that note.

## Files you'll touch

- `src/crates/primer-inference/src/ollama.rs` — streaming impl
- `src/crates/primer-inference/src/cloud.rs` — streaming impl + SSE parser
- `src/crates/primer-pedagogy/src/dialogue_manager.rs` — add `respond_to_streaming`, keep `respond_to` as wrapper
- `src/crates/primer-cli/src/main.rs` — print incrementally
- `CLAUDE.md` — update gotchas

You shouldn't need to touch `primer-core`. The traits and types already support streaming; only the implementations need to actually do it.

## Acceptance criteria — how you know you're done

Run all three:

```bash
cd src
# 1. Stub backend still works (one-shot chunk, fine)
cargo run --bin primer

# 2. Ollama streams visibly token-by-token
cargo run --bin primer -- --backend ollama --model llama3.2 --name Aiyana --age 8

# 3. Cloud streams visibly token-by-token
cargo run --bin primer -- --backend cloud --name Aiyana --age 8
```

For 2 and 3:
- Tokens must appear progressively, not in one burst.
- The recorded turn (in `session.turns`) must contain the full assembled response — verify by adding `RUST_LOG=debug` and a `tracing::debug!` log of the final accumulated text, or by adding a quick `--debug-print-session` flag if you want a clean test path.
- Conversation flow is unchanged: the next prompt redraw appears only after the stream completes, learner-model updates still happen, break suggestions still fire.

If you can't get streaming visibly working, **say so explicitly** in your final report — don't claim success because the code compiles. The whole point of this task is the user-facing latency change. Verify in the terminal.

## Watch-outs and gotchas (read before starting)

- **Don't break the stub backend.** It already emits one final chunk, which is a degenerate but valid stream. Fine to leave as-is. Test it after the changes to confirm.
- **Mid-stream errors.** A network drop during generation should propagate as `Result::Err` from `next().await`. The dialogue manager should stop accumulating, log the error, and either return the partial response or an error — your call, but be deliberate. Recording a half-formed Primer turn into the session is bad; better to skip the record on error.
- **The Ollama empty-content error in `ollama.rs` is non-streaming logic.** It applies to the current single-shot path. When you switch to streaming, that check is wrong (per-chunk content is often empty). Move the "did we get anything?" check to "after the stream completes, was the accumulated string non-empty?".
- **Don't add new dependencies casually.** The repo deliberately keeps the dep tree small (rustls, no native-tls, no eventsource crate). Adding `reqwest-eventsource` is fine if it pulls in nothing surprising; check `cargo tree` first. Hand-rolling a 30-line SSE parser is also fine and probably preferred.
- **Streaming changes parser locality.** With non-streaming, malformed JSON kills the request cleanly. With streaming, a single bad line can desync your parser for the rest of the stream. Be defensive: skip unparseable lines with a `tracing::warn!` rather than aborting.
- **Backpressure is automatic.** `TokenStream` is pull-based via `next().await`. The CLI's blocking `print!` + `flush()` per chunk creates natural backpressure on the underlying HTTP read. Don't add buffers or channels you don't need.

## Queued behind streaming (don't do these in the same session)

These are next on the Phase 0 list once streaming lands:

- **Conversation persistence** — save/load `Session` as JSON so a child can pick up where they left off. Touches `primer-core::conversation`, `primer-cli`. Probably an `--load-session <path>` and auto-save on exit.
- **Knowledge base bootstrapping** — Python ingestion script for Simple English Wikipedia → SQLite FTS5. Tune `RetrievalParams` defaults against real queries.
- **Tests for `decide_intent()`** — unit tests covering frustration → scaffolding, disengagement → close, short response → comprehension check, etc. The Primer's brain is currently untested.
- **Concept extraction** — replace the placeholder empty `concepts: vec![]` in turn metadata with at least keyword-matching against a small concept taxonomy.

If you finish streaming with time to spare and the user hasn't redirected you, ask which of these to start on rather than picking unilaterally — they have priorities you can't see from the code.

## Reporting back

When you've finished or hit a blocker:

- State plainly what you got working and what you didn't, by acceptance criterion.
- If you verified streaming in the terminal, say so. If you only verified that `cargo build` is clean, say only that.
- If you discovered the user did interim work that changes the plan, flag it explicitly.
