# Inference and pedagogy

This chapter walks through the two halves of the Primer's "thinking" pipeline: the inference layer that produces text, and the pedagogical engine that decides what to ask for. If you want to add a new model backend, change how the Primer responds to a particular kind of child input, or just understand the lifecycle of a single conversational turn, start here.

## The InferenceBackend trait

Every LLM in the system — cloud-hosted Claude, a local Ollama daemon, a future llama.cpp build, the canned-response stub used in tests — sits behind a single trait. The pedagogical engine holds a `&dyn InferenceBackend` and never knows which one it is talking to. The contract is five methods, of which three are required (no default impl): `name` (the human-readable backend identifier), `is_available` (a runtime readiness check), and `generate_stream` (an asynchronous stream of token chunks). The remaining two — `generate` (one-shot collected response) and `summarize` (condense a slice of turns into a paragraph of long-term memory) — ship with default impls that delegate down to `generate_stream`, so a real backend usually only has to override `generate_stream` plus the two `name` / `is_available` methods.

`generate_stream` is the canonical surface because it's the one the conversation loop actually drives: streaming lets the TTS pipeline (Phase 2) begin speaking before generation is complete, and a chunk-at-a-time REPL feels less laggy than a wait-for-the-whole-paragraph one. The non-streaming `generate` exists only because some flows — the rolling-summary refresh, the structured-output classifier / extractor / comprehension prompts — want a complete string and don't benefit from incremental delivery, so the trait provides them a default that collects the stream once it lands.

```rust
// src/crates/primer-core/src/inference.rs
#[async_trait]
pub trait InferenceBackend: Send + Sync {
    fn name(&self) -> &str;
    async fn is_available(&self) -> bool;

    async fn generate(&self, prompt: &Prompt, params: &GenerationParams) -> Result<String> { /* default impl */ }

    async fn generate_stream(
        &self,
        prompt: &Prompt,
        params: &GenerationParams,
    ) -> Result<TokenStream>;

    async fn summarize(&self, turns: &[Turn], target_chars: usize) -> Result<String> { /* default impl */ }
}
```

Full source at [inference.rs](../../src/crates/primer-core/src/inference.rs). The `Prompt` type carries a `system: String` plus a `Vec<Message>` of role-tagged history; `TokenStream` is `Pin<Box<dyn Stream<Item = Result<TokenChunk>> + Send>>`.

## The three concrete backends

### StubBackend

Canned Socratic responses, no model, no network. It implements the trait with a small lookup keyed on intent and emits one final `TokenChunk` to satisfy the streaming signature — a degenerate but valid stream that the rest of the pipeline handles correctly. This is the default for `cargo run --bin primer` (no API key required) and the only backend the workspace tests rely on, which is why most of the dialogue manager's behaviour can be exercised with no model at all. See [stub.rs](../../src/crates/primer-inference/src/stub.rs).

### CloudBackend (Anthropic SSE)

The Anthropic Messages API streams via Server-Sent Events: `event:` and `data:` lines separated by blank lines, with JSON payloads that carry `content_block_delta` events. `CloudBackend` parses this with a hand-rolled `SseBuffer` and a `parse_anthropic_event` function — no SSE crate dependency. The byte stream is consumed by a spawned tokio task that forwards parsed chunks through a `futures::channel::mpsc::unbounded` channel; the `TokenStream` returned to the caller wraps the receiver. One detail to know: the Anthropic API treats system instructions as a top-level field on the request body, not a message role, so `CloudBackend` maps `Role::System` entries from the `Prompt`'s message list to `"user"` when serialising. See [cloud.rs](../../src/crates/primer-inference/src/cloud.rs).

### OllamaBackend (NDJSON)

Ollama's `/api/chat` endpoint streams newline-delimited JSON: one complete object per `\n`-terminated line, each carrying a `message.content` fragment and a final `done: true` line. `OllamaBackend` parses this with `NdjsonBuffer` and `parse_ollama_line`, with the same spawned-task-plus-mpsc shape as the cloud backend. The divergence here is that Ollama's chat API has no separate system field, so the backend prepends a `system`-role message to the messages array rather than mapping it onto a `"user"` slot. See [ollama.rs](../../src/crates/primer-inference/src/ollama.rs).

> **Gotcha:** The two backends differ in how they handle system messages — `CloudBackend` rewrites `Role::System` to `"user"` (because the real system field is sent at the top level), while `OllamaBackend` prepends a `system`-role message inline. Watch this divergence when reworking prompt assembly; it's an easy place to silently leak instruction text into the wrong slot.

## The retry layer

Retries happen only at the request-send and status-check phase. The cloud and Ollama backends each wrap their pre-stream HTTP call in `primer_core::retry::retry_with_backoff` — once `generate_stream` has begun consuming bytes from a 2xx response, mid-stream errors propagate cleanly and the partial Primer turn is dropped at the dialogue-manager layer (the child turn stays; the half-formed response does not). This deliberately keeps the retry surface narrow: a retry that re-issued mid-stream would either replay tokens the child has already heard or silently re-prompt the model, both of which we want to avoid.

The retry helper lives in `primer-core` and is HTTP-neutral — no `reqwest` dependency. Backends translate their own status codes via `classify_anthropic_status` / `classify_ollama_status` into `InferenceError` variants before invoking it; the helper inspects `is_retryable()` and the optional `retry_after` to decide what to do.

Defaults are tuned for conversational latency. Three attempts × ~250 ms × 2 ≈ ~1.75 s worst case before the failure surfaces to the dialogue manager. `Retry-After` headers up to 5 s are honoured exactly; longer waits surface immediately rather than leaving a child staring at a 30-second silent gap. Each retry decision emits `tracing::info!(target: "primer::retry", attempt, kind, delay_ms)` so a future voice-mode change can subscribe and play a "give me a moment" bridging phrase. See [retry.rs](../../src/crates/primer-core/src/retry.rs) and the per-subsystem defaults in [consts.rs](../../src/crates/primer-core/src/consts.rs).

## The typed InferenceError

Inference failures are categorised so the retry helper, the i18n layer, and the call site can each act on them coherently. `InferenceError` lives in [error.rs](../../src/crates/primer-core/src/error.rs) and has six variants:

- `Auth` — 401 / 403, or any auth-shape failure.
- `RateLimited { retry_after: Option<Duration> }` — 429 with the optional header parsed.
- `ServiceUnavailable` — 5xx, or daemon-down on Ollama.
- `NetworkUnavailable` — connect / timeout / DNS / TLS failure at the transport layer.
- `ModelNotFound { model: String }` — Ollama 404 on a configured model name.
- `Other(String)` — catch-all for unmapped conditions, plus `From<&str>` and `From<String>` impls so producers can write `format!(...).into()`.

> **Why:** `Other`'s inner string is dev-facing only — it never reaches users. The i18n layer in `primer_core::i18n::render_inference_error` substitutes a generic message for `Other` and the call site logs the full error via `tracing::warn!`. Never embed user-facing English in `InferenceError` variant fields (no `hint: String`); never render `Other`'s inner string to the child. This is the discipline that lets a future locale PR be purely additive.

## DialogueManager

`DialogueManager` is the orchestrator that turns a child's input into a Primer turn. It records the child's message, decides the pedagogical intent, retrieves passages from the knowledge base and relevant older turns from long-term memory, builds the system prompt, calls the inference backend, records the response, refreshes the rolling summary if due, and schedules the post-turn classifier / extractor / comprehension tasks. Held entirely as borrowed `&dyn` references to the backends and the knowledge base, plus `Arc<dyn ...>` for the classifier and stores it spawns work with — swapping backends is a CLI-layer wiring change, not a code change.

It lives at [src/crates/primer-pedagogy/src/dialogue_manager/](../../src/crates/primer-pedagogy/src/dialogue_manager/) and is a directory module split by axis. The split is deliberate: each file is small enough to read in one sitting, and the boundaries match the things you are likely to change together. When adding a new method, place it in the file matching its responsibility and use `pub(super)` visibility for cross-module access.

- `mod.rs` — struct, type aliases, and module glue
- `lifecycle.rs` — `new` / `open_session` / `resume_session` / `close_session` plus accessors
- `turn.rs` — the `respond_to_streaming` orchestrator and its private helpers for child-turn recording, prompt assembly, inference streaming, primer-turn recording, persistence, and the spawn-task helpers (embedding, classification, post-response)
- `background.rs` — await / drain / apply for the classifier, extractor, and comprehension tasks
- `retrieval.rs` — knowledge-base and long-term-memory retrieval
- `summary.rs` — the two summary-refresh cadences (active vs. resume)
- `learner_update.rs` — the engagement-state heuristic (still a placeholder, word-count-based)
- `apply.rs` — pure free functions plus their unit tests

> **Note:** When adding a new method to `DialogueManager`, place it in the file matching its responsibility and use `pub(super)` visibility for cross-module access. Tests follow the same axis split — `tests/lifecycle_tests.rs`, `tests/turn_tests.rs`, `tests/background_tests.rs`, with shared mocks and builders in `dialogue_manager/test_support.rs` (sibling of `mod.rs`, not under the `tests/` subdir).

## decide_intent()

`decide_intent` is the brain of the Socratic behaviour: a deterministic heuristic that reads the learner model and the recent session turns and returns a `PedagogicalIntent`. It is the single most important function to test rigorously when adding pedagogical features, because every turn passes through it and every changed branch shifts what the Primer says next. The function has three layers of fallback (`decide_intent` → `decide_intent_at` → `decide_intent_at_with_pack`) so callers without a clock or a locale pack still get sensible defaults. It lives in [prompt_builder.rs](../../src/crates/primer-pedagogy/src/prompt_builder.rs).

Characterization tests in the inline `prompt_builder::tests` module pin the current behaviour — eighteen cases at the time of writing, covering factual-question routing, the `Encouragement` / `SessionClose` split for sustained disengagement, time-driven `SuggestBreak`, and the engagement-state overrides that always win over wallclock cadence. These tests are how the codebase keeps the Socratic principle from drifting under refactoring pressure.

> **Note:** When you change intent routing, add a characterization test for the new branch BEFORE the implementation. The test pins the routing case so a future refactor can't quietly regress it; the order — test first — is what makes this work, because writing the test against an existing implementation tends to encode the bug rather than the intent.

---

### Recipe — Add a new `InferenceBackend`

Worked example: adding a hypothetical `OpenAIBackend`.

1. **Implement the trait.** Create `primer-inference/src/openai.rs`. Implement `generate_stream` against the OpenAI chat-completions endpoint; let the default `generate` and `summarize` impls do their work unless you have a reason to override them.

2. **Hand-roll the streaming.** OpenAI uses SSE like Anthropic; you can lift `SseBuffer` from [cloud.rs](../../src/crates/primer-inference/src/cloud.rs) or write a `parse_openai_event` analogue. Forward parsed chunks via `futures::channel::mpsc::unbounded` driven by a spawned tokio task — the same shape both existing backends use.

3. **Map errors to `InferenceError`.** Add a `classify_openai_status(status: StatusCode) -> Option<InferenceError>` helper. Map 401 → `Auth`, 429 → `RateLimited { retry_after }` (parse the `Retry-After` header), 404 → `ModelNotFound { model }`, 5xx → `ServiceUnavailable`, transport errors → `NetworkUnavailable`, anything else → `Other(...)`. Never embed user-facing English in the variant fields — keep them as locale-neutral data.

4. **Wrap the pre-stream phase in `retry_with_backoff`.** Pattern lifted from [cloud.rs](../../src/crates/primer-inference/src/cloud.rs):

   ```rust
   // src/crates/primer-inference/src/openai.rs (excerpt)
   let resp = retry_with_backoff(retry_settings, || async {
       let r = client.post(&url).headers(headers.clone()).json(&body).send().await?;
       if !r.status().is_success() {
           return Err(classify_openai_status(r.status()));
       }
       Ok(r)
   }).await?;
   ```

   The retry helper handles `is_retryable()` and `Retry-After` honouring for you; you only have to do the status-to-variant mapping correctly.

5. **Wire into the CLI.** Backend selection lives in [main.rs](../../src/crates/primer-cli/src/main.rs). Find the dispatch site (search for `match cli.backend.as_str()`) and add an `"openai"` arm that constructs your new backend with the API key, model, and any other config. The CLI uses string-based dispatch, not a typed enum, so there's no separate `Backend` enum file.

6. **Add the `--backend openai` clap variant.** Plus `--openai-api-key` (or env-var fallback) following the existing `--api-key` / `ANTHROPIC_API_KEY` pattern in [primer-cli](../../src/crates/primer-cli/).

7. **Write integration tests.** Stub the HTTP layer with `wiremock` or similar. At minimum cover: success → chunks arrive in order; 429 → retry honours `Retry-After`; mid-stream disconnect → error propagates and the dialogue-manager-layer test confirms the partial turn is not recorded.

8. **Update [README.md](../../README.md)** to mention the new backend in the supported-backends list, and (if behaviour diverges) add a one-line gotcha to [CLAUDE.md](../../CLAUDE.md).

### Recipe — Add a new `PedagogicalIntent`

Worked example: adding `Celebrate` (acknowledge a breakthrough moment, then pivot back to inquiry).

1. **Add the variant to the enum.** Edit the `PedagogicalIntent` definition in [conversation.rs](../../src/crates/primer-core/src/conversation.rs). The enum is unit-only (no explicit discriminants — IDs are assigned separately).

   ```rust
   // src/crates/primer-core/src/conversation.rs
   pub enum PedagogicalIntent {
       // ...existing variants...
       Celebrate,
   }
   ```

2. **Add the variant to `PedagogicalIntent::ALL`.** Same file. The `ALL` array is the single source of truth that the storage layer's lookup-table seeder walks. A unit test (`assert_eq!(PedagogicalIntent::ALL.len(), N)`) will fail if you skip this — that's a feature.

   ```rust
   // src/crates/primer-core/src/conversation.rs
   impl PedagogicalIntent {
       pub const ALL: &'static [PedagogicalIntent] = &[
           // ...existing variants...
           PedagogicalIntent::Celebrate,
       ];
   }
   ```

3. **Bump the length assertion.** The test at `conversation.rs:146` pins the count. Change `assert_eq!(PedagogicalIntent::ALL.len(), 9);` to match your new total.

4. **Assign the integer ID.** Edit [catalog.rs](../../src/crates/primer-storage/src/catalog.rs). Add a match arm to BOTH `intent_id()` and `intent_name()` (they have to stay in sync — there's a unit test for that). Pick the next unused id; never reuse a retired one — historical rows in the session DB carry the old id, and reusing the integer would silently rewrite history.

   ```rust
   // src/crates/primer-storage/src/catalog.rs
   pub fn intent_id(intent: PedagogicalIntent) -> i64 {
       match intent {
           // ...existing arms...
           PedagogicalIntent::Celebrate => 10,
       }
   }

   pub fn intent_name(intent: PedagogicalIntent) -> &'static str {
       match intent {
           // ...existing arms...
           PedagogicalIntent::Celebrate => "celebrate",
       }
   }
   ```

5. **Add a routing branch in `decide_intent`.** In [prompt_builder.rs](../../src/crates/primer-pedagogy/src/prompt_builder.rs), add the heuristic that routes to `Celebrate`. Keep it deterministic — engagement-state overrides should still win, and the time-driven `SuggestBreak` branch should still take precedence over a celebration when both apply.

6. **Extend the prompt builder.** In `build_system_prompt_with_pack_and_vocab`, add a section for the `Celebrate` intent describing how the Primer should celebrate (briefly, then pivot back to inquiry — never break principle 2).

7. **Add a characterization test.** In `prompt_builder.rs`'s `tests` module:

   ```rust
   // src/crates/primer-pedagogy/src/prompt_builder.rs (tests module)
   #[test]
   fn celebrate_when_child_makes_correct_synthesis() {
       let intent = decide_intent(&fixture_for_synthesis_moment());
       assert_eq!(intent, PedagogicalIntent::Celebrate);
   }
   ```

8. **Open any test session DB.** The `validate_and_seed_lookup` helper in `primer-storage/src/catalog.rs` walks `PedagogicalIntent::ALL` on every `open()`; once steps 2-4 above are done, your new variant gets seeded automatically. No migration step required.
