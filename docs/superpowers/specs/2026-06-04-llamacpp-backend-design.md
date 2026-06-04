# LlamaCppBackend — Phase 1.1 Local Inference (design)

**Date:** 2026-06-04
**Status:** approved (brainstorm)
**Roadmap:** Phase 1.1 bullet (a) — `LlamaCppBackend` via bindings; GGUF loading from a configurable path.
**Scope this session:** bullet (a) only. Benchmarking (bullet b — needs downloaded models + Metal/CUDA/Vulkan machines) and automatic local→cloud fallback (bullet c — overlaps Phase 1.3's inference router) are explicitly deferred.

## Goal

Add an in-process, embedded llama.cpp inference backend so the Phase 0
Socratic conversation can run offline against a local GGUF model, without
an HTTP server. This complements (does not replace) `OpenAiCompatBackend`,
which already covers the *out-of-process* llama.cpp `--server` path.

The backend is a runtime-selectable `InferenceBackend` like every other,
so no engine/pedagogy code changes — only construction wiring.

## Why a new in-process backend (not reuse openai-compat)

`OpenAiCompatBackend` requires the user to run `llama-server` separately.
Phase 1.1's point is *embedded* inference: one binary, no sidecar process,
the model loaded into the Primer's own address space. This is the path that
the future hardware enclosure (Phase 3) and the standalone-phone story
depend on.

## Binding choice

`llama-cpp-2` (crate published by utilityai; repo `utilityai/llama-cpp-rs`;
current version **0.1.146**, pinned `"0.1.146"` in `[workspace.dependencies]`).
The most maintained, widely-used Rust binding; already pencilled into
`primer-inference/Cargo.toml` as the intended dependency. It does **not** pull
`ort`, so it does not interact with the workspace's `=2.0.0-rc.10` ort pin. It compiles
llama.cpp via cmake/C++ — therefore the backend **must** be a non-default
cargo feature (CLAUDE.md: "No system dependencies for default builds").

Confirmed API surface (from the crate's current docs):
- `LlamaBackend::init()` — global, once per process.
- `LlamaModel::load_from_file(&backend, path, &LlamaModelParams::default().with_n_gpu_layers(n))`.
- `model.new_context(&backend, LlamaContextParams::default().with_n_ctx(Some(n)))`.
- `model.chat_template(None)` → embedded GGUF chat template; `model.apply_chat_template(&tmpl, &[LlamaChatMessage], add_ass)` → prompt string.
- Decode loop: `model.str_to_token`, `LlamaBatch`, `ctx.decode`, `LlamaSampler::chain_simple([top_p, temp, dist])`, `sampler.sample/accept`, `model.token_to_piece`, `model.token_eos()`.

## Architecture (Approach A — mirror the QNN backend)

The genuine design constraint is **testability**: the default `cargo test`
CI job cannot compile llama.cpp or download a multi-GB GGUF, yet the
streaming-bridge orchestration (mpsc + `spawn_blocking` + done-handling +
reasoning-filter wiring) must be covered in CI, not only by an ignored
real-model test.

Solution: a `LlamaEngine` trait seam (exact analogue of QNN's
`GenieDialog::query_streaming`). The backend orchestration, the trait, a
test-only mock, and the pure helpers are **always compiled and tested on a
plain `cargo test`**. Only `RealLlamaEngine`'s `llama-cpp-2` calls are
`#[cfg(feature = "llamacpp")]`-gated. This is a deliberate, small
improvement over the QNN layout (where the whole module is feature-gated
and its tests need `--features qnn`).

### Module layout

```
crates/primer-inference/src/llamacpp/
  mod.rs        module docs + re-exports (LlamaCppBackend, LLAMACPP_NAME_PREFIX,
                LlamaEngine, params helpers)
  backend.rs    LlamaCppBackend: construction, name(), is_available(),
                generate_stream() — the mpsc + spawn_blocking bridge. Always compiled.
  engine.rs     LlamaEngine trait (always) + RealLlamaEngine (cfg-gated body,
                llama-cpp-2 wrapper, global LlamaBackend OnceLock) + errors.
  params.rs     pure helpers: GenerationParams → SamplerSpec (a
                feature-independent description, NOT a LlamaSampler),
                n_gpu_layers resolution, n_ctx resolution, stop-sequence tail
                match, validate_gguf_path. Always compiled.
  tests.rs      host tests via MockLlamaEngine (always) + #[ignore] real-model
                smoke (feature-gated).
```

`primer-inference/src/lib.rs`: `pub mod llamacpp;` always; `pub use
llamacpp::LlamaCppBackend;` always (the backend type is feature-independent —
it holds an `Arc<dyn LlamaEngine>`).

### The `LlamaEngine` seam

```rust
pub trait LlamaEngine: Send + Sync {
    fn model_id(&self) -> &str;
    fn render_prompt(&self, prompt: &Prompt) -> Result<String>;
    /// Run the blocking decode loop. For each detokenized RAW piece, call
    /// `on_token(piece)`; stop early if it returns `false` (the consumer
    /// dropped). Return `Ok(())` on natural completion (eos / max_tokens /
    /// stop-sequence), `Err` on a decode failure.
    fn infer(
        &self,
        rendered: &str,
        params: &GenerationParams,
        on_token: &mut dyn FnMut(&str) -> bool,
    ) -> Result<()>;
}
```

- `render_prompt` applies the chat template (cheap CPU; runs on the calling task).
- `infer` owns the blocking decode loop; called inside `spawn_blocking`. It
  emits **raw, unfiltered** token text — reasoning-marker stripping is the
  backend's job (see below), which keeps that logic in the always-compiled,
  host-tested path rather than inside the feature-gated engine.

### `RealLlamaEngine` (`#[cfg(feature = "llamacpp")]`)

- Holds `Arc<LlamaModel>` (Send+Sync), the loaded `LlamaChatTemplate`, and
  `Mutex<LlamaContext>` (llama.cpp forbids concurrent decode on one context
  — same constraint as Genie's dialog; the backend takes `blocking_lock()`
  inside `spawn_blocking`).
- Global `static LLAMA_BACKEND: OnceLock<LlamaBackend>` initialised lazily on
  first construction (llama.cpp's `init()` is once-per-process).
- `new(gguf_path, n_gpu_layers, n_ctx)`:
  1. `validate_gguf_path` (pure helper) — path exists + is a file → dev-facing
     `InferenceError` on failure.
  2. `LlamaModel::load_from_file(.with_n_gpu_layers(n_gpu_layers))`.
  3. `model.new_context(.with_n_ctx(n_ctx))`.
  4. `model.chat_template(None)`; **if the GGUF carries none**, fall back to a
     minimal generic template (system block + `User:`/`Assistant:` turns) so
     load never hard-fails on a template-less GGUF.
  5. `model_id` = GGUF file stem.
  - No ABI smoke check (in-process, not a dlopen'd FFI surface like QNN).
- `infer`: tokenize → batch decode loop → build a `LlamaSampler` chain from
  the `SamplerSpec` that `params.rs` derived (top_p, temp, dist with a default
  seed const since `GenerationParams` has no seed field) → `token_to_piece` →
  call `on_token(piece)` with each **raw** piece. Halt on `token_eos()`, on
  `max_tokens`, when any `params.stop_sequences` matches the accumulated raw
  output tail (via the pure `tail_matches_any_stop` helper), or when
  `on_token` returns `false`. Return `Ok(())` on completion / `Err` on a
  decode failure — the backend translates that into the terminal stream chunk.

### Reasoning filter (in the backend bridge, not the engine)

The reasoning-marker strip lives in `LlamaCppBackend`'s `spawn_blocking`
bridge, wrapping the engine's raw `on_token` stream through
`reasoning_stream::process_filtered_chunk` with `default_markers()` + the
extra pairs threaded via `BackendParams.reasoning_markers` — identical
semantics to `OllamaBackend`/`OpenAiCompatBackend`, so `<think>…</think>` and
Gemma4-style markers never reach a child, and `ReasoningWithoutAnswer` is
surfaced when the model reasons but emits no visible answer. Putting the
filter here (not in `RealLlamaEngine`) is deliberate: the mock engine emits
raw `<think>` content and the **default `cargo test`** asserts the consumer
sees it stripped — the integration is CI-covered, not only the shared
`reasoning_stream` unit tests.

### `LlamaCppBackend` (always compiled)

```rust
pub struct LlamaCppBackend {
    name: String,                                       // cached "llamacpp:{model_id}"
    engine: Arc<dyn LlamaEngine>,
    reasoning_markers: Vec<primer_core::reasoning::ReasoningMarker>,
}
```

- `new(engine)` uses `default_markers()`; `with_extra_markers(extra)` appends
  custom pairs (mirrors `OllamaBackend`). `name` = `"llamacpp:{model_id()}"`.
- `generate_stream`: `engine.render_prompt(prompt)?` on the calling task;
  build `mpsc::unbounded`; clone the `Arc<dyn LlamaEngine>`, the markers, and
  `params`; in `spawn_blocking`, construct a `ReasoningFilter`, run the
  engine's raw `on_token` pieces through `process_filtered_chunk`
  (`unbounded_send` each `Forward`; stop on send failure), then flush a
  synthetic done chunk on `Ok(())` (or forward the `Err`); return the receiver
  wrapped as `TokenStream`.
- `generate`: trait default (collects the stream).
- `name()`: cached `"llamacpp:{model_id}"`.
- `is_available()`: `true` (construction succeeded ⇒ model loaded).

### Naming / context budget

`LLAMACPP_NAME_PREFIX = "llamacpp:"` added to `primer-core::backend` beside
`QNN_NAME_PREFIX`. **Not** small-context: `is_small_context_backend` is
unchanged (local llama models commonly run 8K+; the constrained 3B path is
deferred bullet c).

## Cargo features (`primer-inference`)

- `llamacpp` → `dep:llama-cpp-2` (CPU). Non-default.
- `llamacpp-metal` → `["llamacpp", "llama-cpp-2/metal"]`.
- `llamacpp-cuda` → `["llamacpp", "llama-cpp-2/cuda"]`.
- `llamacpp-vulkan` → `["llamacpp", "llama-cpp-2/vulkan"]`.

`primer-cli` / `primer-gui` re-export passthrough features
(`primer-cli/llamacpp`, `primer-cli/llamacpp-metal`, …) forwarding to
`primer-inference/*`, mirroring how `qnn` is forwarded.

Metal is build+smoke-validated on the dev MacBook this session; CUDA/Vulkan
are wired but hardware-validated later (deferred bullet b).

## Wiring

### `primer-engine::wiring`

`BackendParams` gains (always present, qnn-style; ignored by other arms):
- `gguf_path: Option<PathBuf>`
- `llamacpp_gpu_layers: Option<i32>`
- `llamacpp_n_ctx: Option<u32>`

`build_backend` gains a `"llamacpp"` arm → `build_llamacpp_backend(params)`,
a free fn with the two-cfg shape of `build_qnn_backend`:
- feature on: validate `gguf_path` (required), resolve gpu-layers/n_ctx via
  `params.rs`, construct `RealLlamaEngine`, wrap in `LlamaCppBackend`.
- feature off: return `"llamacpp backend requires the `llamacpp` cargo
  feature. Build with `cargo build --features primer-cli/llamacpp`."` — a
  distinct build-time hint, not the generic "unknown backend".

### `primer-cli`

- `--backend llamacpp`.
- GGUF path reuses **`--model <path-to-gguf>`** (ollama semantics: `--model`
  is the model identifier; for llamacpp the identifier is the path). No new
  path flag.
- `--llamacpp-gpu-layers <N>` (optional). Default resolved in `params.rs`:
  `-1` (offload all) when any GPU feature is compiled, else `0`.
- `--llamacpp-n-ctx <N>` (optional). Default `0` = use the model's trained
  context length.
- Both numeric flags declared always but documented llamacpp-only.

### `primer-gui`

- Add `llamacpp` to the backend dropdown; GGUF path picker + the two numeric
  fields in `BackendConfig` / `BackendConfigUpdate`. **Gotcha:** no
  `#[serde(default)]` on `BackendConfigUpdate`, so `settings.js::gather()`
  must send the new fields (path as null when blank, numbers as null when
  empty) or `update_settings` fails to deserialize.
- "Always shown, errors inline without the feature" — qnn pattern.
- `wiring::resolve_main_model` `"llamacpp"` arm returns a placeholder rebound
  to `backend.name()` after construction (qnn pattern).
- GUI config plumbing is wired this session; the GUI feature build +
  click-through is **owner-gated** (Tauri/webkit), like other GUI voice work.

## Testing (TDD, RED→GREEN)

Default `cargo test` (the required CI job) — always compiled:
- `params.rs`: `SamplerSpec` mapping from `GenerationParams` (incl. default
  seed const);
  gpu-layers resolution per feature; n_ctx resolution; stop-sequence tail
  matching; `validate_gguf_path` (missing / not-a-file / ok).
- `backend.rs` via `MockLlamaEngine`: streaming forwards all chunks + a
  terminal done; `generate` aggregates; mid-stream `Err` propagates;
  `name()` == `"llamacpp:<id>"`; reasoning markers stripped when the mock
  emits `<think>…</think>` content.
- `primer-core::backend`: `LLAMACPP_NAME_PREFIX`; `llamacpp:`-named backend
  is **not** small-context.
- `primer-engine::wiring`: `build_backend("llamacpp", …)` without the feature
  → the rebuild hint; `BackendParams` round-trips the new fields;
  feature-off `build_llamacpp_backend` shape.

Feature-gated, on demand only:
- `#[ignore]` real-model smoke under `--features llamacpp`: GGUF path from env
  `PRIMER_LLAMACPP_TEST_GGUF`; generate a few tokens; assert non-empty.
  Run: `cargo test -p primer-inference --features llamacpp --test … -- --ignored`.

Not in scope: an Android cross-compile CI guard for `llamacpp` (llama.cpp
Android/Vulkan validation is deferred bullet b). Default CI stays green
because the feature is off.

## Error handling

All dev-facing strings go in `InferenceError::Other`; user-facing rendering
stays at the existing `i18n::render_inference_error` boundary; no new
user-facing English in variant fields. Load failure / missing GGUF → clear
dev-facing `Other`, logged by the CLI via `tracing::warn!` as today.

## Docs to update on completion

- `CLAUDE.md`: primer-inference bullet (add `LlamaCppBackend`); new gotchas
  (global `LlamaBackend::init` OnceLock; GGUF-embedded chat template with
  generic fallback; feature matrix; `--model` reused as GGUF path).
- `primer-inference/src/lib.rs` doc comment (`LlamaCppBackend` now shipped).
- `README.md`: Phase 1.1 status line.
- `ROADMAP.md`: check Phase 1.1 bullet (a); note (b)/(c) still open.

## Out of scope (explicit)

- Benchmarking on Metal/CUDA/Vulkan (bullet b) — needs models + machines.
- Automatic local→cloud fallback + 3B fallback (bullet c) — Phase 1.3 router.
- Android `llamacpp` cross-compile / on-device validation.
- Multi-model / model-swap at runtime (one GGUF per backend instance).

## Patterns reused from the codebase

- QNN's trait-abstracted handle + mock for host-testable streaming.
- mpsc + `spawn_blocking` + `blocking_lock()` decode serialisation.
- Shared `reasoning_stream::process_filtered_chunk` reasoning strip.
- `build_*_backend` two-cfg free-fn shape + distinct build-time hint.
- `*_NAME_PREFIX` const in `primer-core::backend` shared across the crate boundary.
- All tunables (default gpu-layers, default n_ctx) as named consts, no magic numbers.
