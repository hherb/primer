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

Full source at [inference.rs](../../src/crates/primer-core/src/inference.rs). The `Prompt` type carries a `system: String` plus a `Vec<Message>` of role-tagged history; `TokenStream` is `Pin<Box<dyn Stream<Item = Result<TokenChunk>> + Send>>`. Each `TokenChunk` also carries a `finish_reason: FinishReason` (`Stop` | `Length`) that is meaningful only on the terminal chunk — see [Context-limit graceful recovery](#context-limit-graceful-recovery) below. `GenerationParams` additionally carries an optional `routing: Option<RoutingSignals>` field that only the `RouterBackend` reads; every other backend ignores it.

## The concrete backends

The trait has grown well past the original three implementations. They split into two groups: the **leaf backends** (each talks to one model or service) and the **decorator backends** (`FallbackBackend`, `RouterBackend`) that wrap one or two leaves to add reliability or routing. This section covers the leaves; the decorators have their own sections below.

Each leaf backend owns its model name at construction — `InferenceBackend::generate(prompt, params)` does **not** take a model parameter. So picking a different model for, say, the classifier means constructing a *separate* backend instance with that model, not reusing the chat backend with a per-call override.

### StubBackend

Canned Socratic responses, no model, no network. It implements the trait with a small lookup keyed on intent and emits one final `TokenChunk` to satisfy the streaming signature — a degenerate but valid stream that the rest of the pipeline handles correctly. This is the default for `cargo run --bin primer` (no API key required) and the only backend the workspace tests rely on, which is why most of the dialogue manager's behaviour can be exercised with no model at all. See [stub.rs](../../src/crates/primer-inference/src/stub.rs).

### CloudBackend (Anthropic SSE)

The Anthropic Messages API streams via Server-Sent Events: `event:` and `data:` lines separated by blank lines, with JSON payloads that carry `content_block_delta` events. `CloudBackend` parses this with a hand-rolled `SseBuffer` and a `parse_anthropic_event` function — no SSE crate dependency. The byte stream is consumed by a spawned tokio task that forwards parsed chunks through a `futures::channel::mpsc::unbounded` channel; the `TokenStream` returned to the caller wraps the receiver. One detail to know: the Anthropic API treats system instructions as a top-level field on the request body, not a message role, so `CloudBackend` maps `Role::System` entries from the `Prompt`'s message list to `"user"` when serialising. See [cloud.rs](../../src/crates/primer-inference/src/cloud.rs).

### OllamaBackend (NDJSON)

Ollama's `/api/chat` endpoint streams newline-delimited JSON: one complete object per `\n`-terminated line, each carrying a `message.content` fragment and a final `done: true` line. `OllamaBackend` parses this with `NdjsonBuffer` and `parse_ollama_line`, with the same spawned-task-plus-mpsc shape as the cloud backend. The divergence here is that Ollama's chat API has no separate system field, so the backend prepends a `system`-role message to the messages array rather than mapping it onto a `"user"` slot. See [ollama.rs](../../src/crates/primer-inference/src/ollama.rs).

> **Gotcha:** The backends differ in how they handle system messages — `CloudBackend` rewrites `Role::System` to `"user"` (because the real system field is sent at the top level), while `OllamaBackend` and `OpenAiCompatBackend` both prepend a `system`-role message inline. Watch this divergence when reworking prompt assembly; it's an easy place to silently leak instruction text into the wrong slot.

### OpenAiCompatBackend (OpenAI chat-completions SSE)

Any server speaking the OpenAI `/v1/chat/completions` SSE format — oMLX, LM Studio, vLLM, llama.cpp's own `--server`, or remote providers like Together, Groq, and OpenRouter — goes through `OpenAiCompatBackend`. The SSE format is structurally different from Anthropic's (it carries bare `data:` payloads rather than typed `event:` + `data:` pairs), so the parser is deliberately separate: `OpenAiSseBuffer` yields the `data:` payloads, ignores comment / event / id lines, and terminates on either `data: [DONE]` or a non-null `finish_reason`. Error classification lives in `classify_openai_compat_status` (401/403 → `Auth`, 429 → `RateLimited`, 404+model → `ModelNotFound`, 5xx → `ServiceUnavailable`). A `Authorization: Bearer` header is sent when an API key is configured (optional for local servers, required for remote providers). See [openai_compat/](../../src/crates/primer-inference/src/openai_compat/).

### LlamaCppBackend (in-process llama.cpp)

`LlamaCppBackend` runs GGUF inference in-process via the `llama-cpp-2` bindings, behind the non-default `llamacpp` cargo feature (with `llamacpp-metal` / `-cuda` / `-vulkan` variants for GPU passthrough). There is no network and no daemon — the model is loaded from a path and inference runs in the same process. It mirrors the trait-abstracted seam pattern: a `LlamaEngine` trait (`engine.rs`) sits between the backend and `llama-cpp-2`, so the backend's mpsc + `spawn_blocking` streaming bridge, the reasoning-marker strip, and the pure helpers are all compiled and tested on the default `cargo test` via a `MockLlamaEngine`; only `RealLlamaEngine`'s `llama-cpp-2` calls are feature-gated. The GGUF's embedded chat template is applied via `model.apply_chat_template` (with a generic fallback when absent — no sidecar). See [llamacpp/](../../src/crates/primer-inference/src/llamacpp/).

> **Gotcha:** The GPU-variant feature chaining is load-bearing. The `llamacpp-metal` / `-cuda` / `-vulkan` features in `primer-engine`, `primer-cli`, and `primer-gui` each list the base `llamacpp` feature as a dependency. Without it, a GPU-only build leaves the base feature inactive and the wiring layer falls through to the "rebuild with --features llamacpp" stub even though llama.cpp is fully compiled. Verify after touching these with `cargo tree -p primer-cli --features llamacpp-metal -f '{p} -> {f}' | grep primer-engine`.

### QnnBackend (Qualcomm Hexagon NPU)

`QnnBackend` drives the Qualcomm Hexagon NPU through the Genie SDK, behind the non-default `qnn` cargo feature. Like the llamacpp backend it is built on a trait-abstracted seam (a `GenieLibrary` / `GenieDialog` surface) so the construction → smoke-check → streaming-query → drop path is host-tested without any FFI symbol; the real impl wraps the `primer-qnn-sys` runtime-`dlopen` of `libGenie.so`. The model id comes from the bundle's `primer-meta.json`, not from `--model`. Genie forbids concurrent queries against one dialog handle, so `generate_stream` serialises callers through a mutex and runs the query inside `tokio::task::spawn_blocking`. See [qnn/](../../src/crates/primer-inference/src/qnn/).

> **Note:** QNN is the codebase's only **small-context** backend today. Its `name()` is `"qnn:<model_id>"`; the dialogue manager detects that prefix via `primer_core::backend::is_small_context_backend` (anchored on `QNN_NAME_PREFIX = "qnn:"`) and switches to a shorter recent-turn window (8 vs 20), a smaller KB top-K (3 vs 5), and a hard system-prompt token ceiling assembled by the pure `primer_core::prompt_budget` helpers (the Socratic base prompt is never trimmed; lower-value optional sections drop first). The prefix const lives in `primer-core` — re-exported as `primer_inference::qnn::QNN_NAME_PREFIX` — precisely because `primer-pedagogy` matches on it but cannot depend on `primer-inference`. A future 4K-bound non-Qualcomm backend joins by adding its prefix to `is_small_context_backend`; no rename needed.

`RknnBackend` (Rockchip NPU) remains a TODO — the trait is ready for it.

## Reasoning-token stripping

Reasoning-mode models (DeepSeek-R1, QwQ, Qwen3, Gemma4) emit a chain-of-thought block before their visible answer. None of that may reach a child. `primer_core::reasoning::ReasoningFilter` is a stateful streaming filter — robust to markers split across chunks — that strips it. It is wired into `OllamaBackend`, `OpenAiCompatBackend`, and `LlamaCppBackend` `generate_stream` (and therefore `generate`, which aggregates the stream) via the shared `primer_inference::reasoning_stream::process_filtered_chunk` helper. The classifier / extractor / comprehension subsystems inherit the strip because they call the same backends.

The built-in marker table (`primer_core::consts::reasoning::DEFAULT_MARKERS`) covers symmetric `<think>…</think>` and Gemma4's asymmetric `<|channel>…<channel|>`. Custom pairs can be appended at runtime: the CLI `--reasoning-marker '<OPEN>' '</CLOSE>'` (repeatable), and the GUI's Settings → Inference backend "Reasoning markers" textarea (one `open close` pair per line). Suppressed reasoning is logged at `tracing::debug!(target: "primer::reasoning")`.

> **Why:** If a model reasons but emits no visible answer, the backend surfaces `InferenceError::ReasoningWithoutAnswer`, rendered through the i18n boundary as a friendly "thinking problem, try again". The partial turn then drops at the dialogue-manager layer like any mid-stream error — the child never sees raw chain-of-thought, even in the degenerate all-reasoning case.

## Context-limit graceful recovery

When a model's context window fills before it finishes, the terminal stream chunk carries `FinishReason::Length` instead of `FinishReason::Stop`. The dialogue manager detects this, streams a locale-aware apology to the child, and auto-retries with a progressively smaller prompt via the pure `PromptBudgetTier` ladder (Full → drop-knowledge → drop-LTM + shrink window), up to two retries, then soft-stops with a gentle cue. Only the final clean answer is persisted; the apology and soft-stop strings live in the prompt packs (en/de/hi). The visible self-correction is deliberately pedagogical — it models that no answer source is invariably right.

There are **five producers** of `FinishReason::Length`, each mapped by a small pure helper so the recovery loop stays backend-agnostic:

- **QNN** — Genie's `GENIE_STATUS_WARNING_CONTEXT_EXCEEDED` (`= 4` in `GenieCommon.h`).
- **CloudBackend** — Anthropic's `stop_reason: "max_tokens"`, threaded onto the terminal chunk by `AnthropicEventTranslator` (Anthropic delivers it in a separate `message_delta` event).
- **OllamaBackend** — `done_reason: "length"`.
- **OpenAiCompatBackend** — `finish_reason: "length"`.
- **LlamaCppBackend** — the `LlamaEngine::infer` signature returns `Result<FinishReason>`, reporting `Length` when the token / context budget is exhausted without a natural stop.

> **Gotcha:** `Length` takes precedence over `ReasoningWithoutAnswer`. If a reply is truncated *and* everything emitted stayed inside a `<think>` block (no visible answer), the shared `process_filtered_chunk` filter lets `Length` win — the dialogue manager apologises and retries with a smaller prompt (which may let the model finish reasoning and produce a visible answer) rather than giving up the turn. A *clean* (`Stop`) all-reasoning reply still surfaces `ReasoningWithoutAnswer`, because a retry would not help.

## FallbackBackend — local→cloud reliability

`FallbackBackend` is an opt-in decorator that wraps a primary and a secondary `Arc<dyn InferenceBackend>` for reliability. It is wired via the CLI `--fallback-backend` / `--fallback-model` flags (their presence is the explicit cloud-fallback consent; absent ⇒ local-only, the privacy default) and mirrored in the GUI's Settings → Inference backend. The supported direction is local-primary → cloud-fallback.

Two key invariants: it falls back **only at the pre-stream boundary** (the `generate_stream().await` `Result`) — a mid-stream error propagates and the partial turn drops as usual, so the Primer never double-speaks — and its `name()` returns the **primary's** name verbatim, which is load-bearing for `is_small_context_backend` detection. Construction-time fallback (when the primary fails to build at startup) is handled by the pure `plan_main_backend` truth table. See [fallback.rs](../../src/crates/primer-inference/src/fallback.rs).

## RouterBackend — per-turn inference routing

`RouterBackend` is the Phase 1.3 per-turn router, built by the same wiring path as `FallbackBackend`. `--router-mode local-only|cloud-preferred|hybrid` (default `local-only`, today's behaviour) selects the policy; `hybrid` / `cloud-preferred` wrap the `--backend` primary plus the `--fallback-*` secondary. Configuring a router with no secondary is a hard construction error.

The pure policy lives in `primer_core::router`: `complexity_score()` combines an intent weight, a retrieved-passage term, and a message-length / question term, and `order_legs(mode, score)` picks the leg order. The dialogue manager threads each turn's `RoutingSignals` (its `PedagogicalIntent` + retrieved-passage count) into `GenerationParams.routing`; non-router backends ignore it. Like `FallbackBackend`, `name()` returns the primary's name and fallover happens only at the pre-stream boundary.

> **Note:** Latency-aware switching has shipped but is config-gated and **off by default** (`--primary-ttft-budget-ms` / the GUI "Primary TTFT budget (ms)" field). The router owns a rolling primary-leg TTFT EMA (router-owned so cloud-routed turns can't pollute the local estimate) and folds a `latency_term` into the `hybrid` score only when a budget is set and exceeded. It is a *nudge*, not a circuit-breaker: a slow local leg tips a borderline turn to the secondary while a trivial turn stays local, which keeps routine turns sampling the local TTFT so the EMA self-heals. With no budget set, behaviour is byte-identical to the no-latency router.

## The retry layer

Retries happen only at the request-send and status-check phase. The cloud and Ollama backends each wrap their pre-stream HTTP call in `primer_core::retry::retry_with_backoff` — once `generate_stream` has begun consuming bytes from a 2xx response, mid-stream errors propagate cleanly and the partial Primer turn is dropped at the dialogue-manager layer (the child turn stays; the half-formed response does not). This deliberately keeps the retry surface narrow: a retry that re-issued mid-stream would either replay tokens the child has already heard or silently re-prompt the model, both of which we want to avoid.

The retry helper lives in `primer-core` and is HTTP-neutral — no `reqwest` dependency. Backends translate their own status codes via `classify_anthropic_status` / `classify_ollama_status` into `InferenceError` variants before invoking it; the helper inspects `is_retryable()` and the optional `retry_after` to decide what to do.

Defaults are tuned for conversational latency. Three attempts × ~250 ms × 2 ≈ ~1.75 s worst case before the failure surfaces to the dialogue manager. `Retry-After` headers up to 5 s are honoured exactly; longer waits surface immediately rather than leaving a child staring at a 30-second silent gap. Each retry decision emits `tracing::info!(target: "primer::retry", attempt, kind, delay_ms)` so a future voice-mode change can subscribe and play a "give me a moment" bridging phrase. See [retry.rs](../../src/crates/primer-core/src/retry.rs) and the per-subsystem defaults in [consts.rs](../../src/crates/primer-core/src/consts.rs).

## The typed InferenceError

Inference failures are categorised so the retry helper, the i18n layer, and the call site can each act on them coherently. `InferenceError` lives in [error.rs](../../src/crates/primer-core/src/error.rs) and has seven variants:

- `Auth` — 401 / 403, or any auth-shape failure.
- `RateLimited { retry_after: Option<Duration> }` — 429 with the optional header parsed.
- `ServiceUnavailable` — 5xx, or daemon-down on Ollama.
- `NetworkUnavailable` — connect / timeout / DNS / TLS failure at the transport layer.
- `ModelNotFound { model: String }` — Ollama 404 on a configured model name.
- `ReasoningWithoutAnswer` — a reasoning-mode model emitted only a chain-of-thought block and no visible answer (see [Reasoning-token stripping](#reasoning-token-stripping)). Not retryable.
- `Other(String)` — catch-all for unmapped conditions, plus `From<&str>` and `From<String>` impls so producers can write `format!(...).into()`.

> **Why:** `Other`'s inner string is dev-facing only — it never reaches users. The i18n layer in `primer_core::i18n::render_inference_error` substitutes a generic message for `Other` and the call site logs the full error via `tracing::warn!`. Never embed user-facing English in `InferenceError` variant fields (no `hint: String`); never render `Other`'s inner string to the child. This is the discipline that lets a future locale PR be purely additive.

> **Gotcha:** Do **not** re-wrap a typed `InferenceError` back into `Other` at the inference-call site. The old re-wrap (formerly around `dialogue_manager.rs:534`, now in the inference path of `dialogue_manager/turn.rs::stream_inference_response`) was removed during the Phase 0.1 i18n work — it would have flattened `Auth` / `RateLimited` / `Length` etc. into an opaque string and defeated both the retry helper and the context-limit recovery loop. Propagate typed errors via `?`.

## DialogueManager

`DialogueManager` is the orchestrator that turns a child's input into a Primer turn. It records the child's message, decides the pedagogical intent, retrieves passages from the knowledge base and relevant older turns from long-term memory, builds the system prompt, calls the inference backend, records the response, refreshes the rolling summary if due, and schedules the post-turn classifier / extractor / comprehension tasks. Held entirely as borrowed `&dyn` references to the backends and the knowledge base, plus `Arc<dyn ...>` for the classifier and stores it spawns work with — swapping backends is a CLI-layer wiring change, not a code change.

It lives at [src/crates/primer-pedagogy/src/dialogue_manager/](../../src/crates/primer-pedagogy/src/dialogue_manager/) and is a directory module split by axis. The split is deliberate: each file is small enough to read in one sitting, and the boundaries match the things you are likely to change together. When adding a new method, place it in the file matching its responsibility and use `pub(super)` visibility for cross-module access.

- `mod.rs` — struct, type aliases, and module glue
- `lifecycle.rs` — `new` / `open_session` / `resume_session` / `close_session` plus accessors
- `turn.rs` — the `respond_to_streaming` orchestrator and its private helpers for child-turn recording, prompt assembly, inference streaming (`stream_inference_response`), primer-turn recording, persistence, the context-limit recovery loop, and the spawn-task helpers (embedding, classification, post-response)
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

Worked example: adding a hypothetical `OpenAIBackend`. (A real OpenAI chat-completions backend already ships as `OpenAiCompatBackend` — read [openai_compat/](../../src/crates/primer-inference/src/openai_compat/) as the closest reference implementation. The recipe below is kept as a generic walkthrough.)

The trait surface a new backend has to satisfy is unchanged: implement `name`, `is_available`, and `generate_stream` (the only required method); `generate` and `summarize` come for free via the default impls. Each backend owns its model name at construction — no per-call model parameter.

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

5. **Strip reasoning tokens.** If the backend may serve reasoning-mode models, route each chunk through `primer_inference::reasoning_stream::process_filtered_chunk` (the shared helper `OllamaBackend` / `OpenAiCompatBackend` / `LlamaCppBackend` all use) so chain-of-thought never reaches a child. This also gives you `ReasoningWithoutAnswer` handling for free.

6. **Map the finish reason.** Translate the provider's truncation signal to `FinishReason::Length` on the terminal `TokenChunk` (every other reason stays `FinishReason::Stop`). Write a small pure `map_*_finish_reason` helper, mirroring the existing five producers. This is what hooks your backend into the dialogue manager's context-limit recovery loop — no dialogue-manager change is needed.

7. **Wire into the CLI.** Backend selection lives in [main.rs](../../src/crates/primer-cli/src/main.rs) and the shared wiring helpers in [primer-engine](../../src/crates/primer-engine/). The CLI uses string-based dispatch, not a typed enum, so there's no separate `Backend` enum file; add an `"openai"` arm that constructs your new backend with the API key, model, and any other config. (Constructing the decorators — `FallbackBackend`, `RouterBackend` — goes through `primer_engine::wiring::build_main_backend`, which both the CLI and the GUI share.)

8. **Add the `--backend openai` clap variant.** Plus `--openai-api-key` (or env-var fallback) following the existing `--api-key` / `ANTHROPIC_API_KEY` pattern in [primer-cli](../../src/crates/primer-cli/).

9. **Write integration tests.** Stub the HTTP layer with `wiremock` or similar. At minimum cover: success → chunks arrive in order; 429 → retry honours `Retry-After`; mid-stream disconnect → error propagates and the dialogue-manager-layer test confirms the partial turn is not recorded; truncation → terminal chunk carries `FinishReason::Length`.

10. **Update [README.md](../../README.md)** to mention the new backend in the supported-backends list, and (if behaviour diverges) add a one-line gotcha to [CLAUDE.md](../../CLAUDE.md).

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
