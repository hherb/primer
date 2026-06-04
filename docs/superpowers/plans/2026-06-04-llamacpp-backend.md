# LlamaCppBackend Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an in-process, embedded llama.cpp inference backend (`LlamaCppBackend`) that loads a GGUF model from a path and streams a Socratic response — runtime-selectable like every other backend, behind a non-default `llamacpp` cargo feature.

**Architecture:** Mirror the QNN backend's trait-abstracted-handle pattern. A `LlamaEngine` trait is the seam: the backend orchestration (mpsc + `spawn_blocking` + reasoning-filter strip + done/error handling), the trait, a test-only `MockLlamaEngine`, and all pure helpers are **always compiled and tested on the default `cargo test`**. Only `RealLlamaEngine`'s `llama-cpp-2` calls are `#[cfg(feature = "llamacpp")]`-gated. The engine emits **raw** tokens via an `on_token` callback; the backend strips reasoning markers, so that integration is CI-covered via the mock.

**Tech Stack:** Rust (edition 2024, toolchain 1.88), `llama-cpp-2` 0.1.146 (compiles llama.cpp via cmake/C++; CPU + Metal/CUDA/Vulkan passthrough features), `futures::channel::mpsc`, `tokio::task::spawn_blocking`, the existing shared `primer_core::reasoning` filter.

**Spec:** [docs/superpowers/specs/2026-06-04-llamacpp-backend-design.md](../specs/2026-06-04-llamacpp-backend-design.md)

**Conventions (from CLAUDE.md):**
- All cargo commands run from `src/`, invoked as `~/.cargo/bin/cargo +1.88 …` (rustup proxy honours the 1.88 pin; Homebrew rust would silently use 1.86).
- No magic numbers — every tunable goes to `primer-core::consts`.
- TDD: write the failing test first, watch it fail, implement minimally, watch it pass, commit.
- Files under ~500 lines where reasonable.

---

## File Structure

**Create:**
- `src/crates/primer-inference/src/llamacpp/mod.rs` — module docs + re-exports.
- `src/crates/primer-inference/src/llamacpp/params.rs` — pure helpers (`SamplerSpec`, gpu-layers / n_ctx resolution, stop-sequence tail match, `validate_gguf_path`) + their unit tests.
- `src/crates/primer-inference/src/llamacpp/engine.rs` — `LlamaEngine` trait + error type (always); `RealLlamaEngine` (cfg-gated body) + global `LlamaBackend` `OnceLock`.
- `src/crates/primer-inference/src/llamacpp/backend.rs` — `LlamaCppBackend` + streaming bridge + `MockLlamaEngine` (test-only) + tests.
- `src/crates/primer-inference/tests/llamacpp_smoke.rs` — `#[ignore]` real-model smoke (feature-gated).

**Modify:**
- `src/Cargo.toml` — add `llama-cpp-2` to `[workspace.dependencies]`.
- `src/crates/primer-inference/Cargo.toml` — `llamacpp` + GPU passthrough features; optional dep.
- `src/crates/primer-inference/src/lib.rs` — `pub mod llamacpp;` + `pub use`.
- `src/crates/primer-core/src/backend.rs` — `LLAMACPP_NAME_PREFIX` + test.
- `src/crates/primer-core/src/consts.rs` — `inference` consts module.
- `src/crates/primer-engine/Cargo.toml` — `llamacpp` (+GPU) features forwarding to `primer-inference`.
- `src/crates/primer-engine/src/wiring.rs` — `BackendParams` fields + `"llamacpp"` arm + `build_llamacpp_backend` + tests.
- `src/crates/primer-cli/Cargo.toml` — `llamacpp` (+GPU) features forwarding to engine + inference.
- `src/crates/primer-cli/src/main.rs` — CLI flags + `BackendParams` construction + model-rebind.
- `src/crates/primer-gui/Cargo.toml` — `llamacpp` (+GPU) features.
- `src/crates/primer-gui/src/config.rs` — `BackendConfig` / `View` / `Update` + defaults.
- `src/crates/primer-gui/src/wiring.rs` (or equivalent) — `"llamacpp"` arm in `resolve_main_model` + `BackendParams` build.
- `src/crates/primer-gui/ui/settings.js` — gather/populate the new fields.
- `CLAUDE.md`, `README.md`, `ROADMAP.md`, `src/crates/primer-inference/src/lib.rs` doc — docs.

---

## Task 1: Workspace dependency + feature flags (no code yet)

**Files:**
- Modify: `src/Cargo.toml` (`[workspace.dependencies]`)
- Modify: `src/crates/primer-inference/Cargo.toml`

- [ ] **Step 1: Add the workspace dependency**

In `src/Cargo.toml`, under `[workspace.dependencies]` (near the `minijinja = "2"` line), add:

```toml
# llama.cpp Rust binding (Phase 1.1 LlamaCppBackend). Compiles llama.cpp
# via cmake/C++, so it is ONLY pulled behind primer-inference's non-default
# `llamacpp` feature. Pinned because it is a 0.1.x crate (patch releases can
# break). Does NOT pull `ort`, so it is independent of the workspace's
# `=2.0.0-rc.10` ort pin.
llama-cpp-2 = "0.1.146"
```

- [ ] **Step 2: Add the feature flags + optional dep to primer-inference**

In `src/crates/primer-inference/Cargo.toml`, add to `[features]` (after the `qnn` feature):

```toml
# Embedded llama.cpp inference (Phase 1.1). Adds the `RealLlamaEngine`
# body behind `LlamaCppBackend`. Non-default — compiles llama.cpp (C++),
# so default builds and CI stay light. The backend type, the LlamaEngine
# trait, the mock, and the pure helpers all compile WITHOUT this feature;
# only the real engine's llama-cpp-2 calls are gated.
llamacpp = ["dep:llama-cpp-2"]
# GPU passthrough — forward to llama-cpp-2's own backend features.
llamacpp-metal = ["llamacpp", "llama-cpp-2/metal"]
llamacpp-cuda = ["llamacpp", "llama-cpp-2/cuda"]
llamacpp-vulkan = ["llamacpp", "llama-cpp-2/vulkan"]
```

And to `[dependencies]` (after the `minijinja` optional dep):

```toml
# Optional — only pulled when the `llamacpp` feature is enabled.
llama-cpp-2 = { workspace = true, optional = true }
```

Also remove the stale commented placeholder in `[dev-dependencies]`:
```toml
# llama.cpp bindings — uncomment when integrating:
# llama-cpp-2 = "0.1"
```

- [ ] **Step 3: Verify the manifest parses (feature off)**

Run: `cd src && ~/.cargo/bin/cargo +1.88 metadata --no-deps --format-version 1 >/dev/null`
Expected: exit 0, no error. (We do NOT build `--features llamacpp` yet — that compiles llama.cpp and is exercised only at the real-engine task on a Metal-capable machine.)

- [ ] **Step 4: Commit**

```bash
cd /Users/hherb/src/primer
git add src/Cargo.toml src/crates/primer-inference/Cargo.toml
git commit -m "build(inference): add llama-cpp-2 dep + llamacpp feature flags (Phase 1.1)"
```

---

## Task 2: `LLAMACPP_NAME_PREFIX` in primer-core

**Files:**
- Modify: `src/crates/primer-core/src/backend.rs`

- [ ] **Step 1: Write the failing test**

In `src/crates/primer-core/src/backend.rs`, inside `#[cfg(test)] mod tests`, add:

```rust
#[test]
fn llamacpp_prefix_value_and_not_small_context() {
    assert_eq!(LLAMACPP_NAME_PREFIX, "llamacpp:");
    // Local llama models commonly run 8K+ context; the constrained 3B
    // path is deferred (Phase 1.1 bullet c), so llamacpp is NOT small-context.
    assert!(!is_small_context_backend("llamacpp:Qwen3-7B-Q4_K_M"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd src && ~/.cargo/bin/cargo +1.88 test -p primer-core backend::tests::llamacpp_prefix -- --nocapture`
Expected: FAIL to compile — `cannot find value LLAMACPP_NAME_PREFIX`.

- [ ] **Step 3: Add the constant**

In `src/crates/primer-core/src/backend.rs`, after the `QNN_NAME_PREFIX` const, add:

```rust
/// Prefix that `LlamaCppBackend::name()` prepends to its model id
/// (e.g. `"llamacpp:Qwen3-7B-Q4_K_M"`).
///
/// Re-exported by `primer-inference` as
/// `primer_inference::llamacpp::LLAMACPP_NAME_PREFIX`. Unlike
/// [`QNN_NAME_PREFIX`], a `llamacpp:`-named backend is **not** treated as
/// small-context: local llama models commonly run 8K+ context. A future
/// constrained-device path can revisit [`is_small_context_backend`].
pub const LLAMACPP_NAME_PREFIX: &str = "llamacpp:";
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd src && ~/.cargo/bin/cargo +1.88 test -p primer-core backend::tests::llamacpp_prefix`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cd /Users/hherb/src/primer
git add src/crates/primer-core/src/backend.rs
git commit -m "feat(core): add LLAMACPP_NAME_PREFIX backend-name constant"
```

---

## Task 3: `inference` consts module

**Files:**
- Modify: `src/crates/primer-core/src/consts.rs`

- [ ] **Step 1: Write the failing test**

In `src/crates/primer-core/src/consts.rs`, at the bottom inside (or add) a `#[cfg(test)] mod tests`, add:

```rust
#[test]
fn inference_llamacpp_consts_are_sane() {
    use super::inference::*;
    assert_eq!(LLAMACPP_GPU_LAYERS_ALL, -1);
    assert_eq!(LLAMACPP_GPU_LAYERS_CPU, 0);
    // 0 = "use the model's trained context length" (llama.cpp convention).
    assert_eq!(LLAMACPP_DEFAULT_N_CTX, 0);
    // A fixed seed keeps a given prompt reproducible across runs.
    assert_eq!(LLAMACPP_DEFAULT_SAMPLER_SEED, 1234);
}
```

(If `consts.rs` has no `mod tests`, add `#[cfg(test)] mod tests { #[test] fn … }` at end-of-file.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cd src && ~/.cargo/bin/cargo +1.88 test -p primer-core consts::tests::inference_llamacpp`
Expected: FAIL to compile — `could not find inference in …`.

- [ ] **Step 3: Add the consts module**

In `src/crates/primer-core/src/consts.rs`, add a new top-level module (after the `speech` module):

```rust
/// Tunables for the embedded llama.cpp backend (Phase 1.1).
pub mod inference {
    /// `n_gpu_layers` value meaning "offload every layer to the GPU".
    /// The default when a GPU passthrough feature (metal/cuda/vulkan) is
    /// compiled in.
    pub const LLAMACPP_GPU_LAYERS_ALL: i32 = -1;

    /// `n_gpu_layers` value meaning "CPU only". The default on a plain
    /// `llamacpp` (no-GPU) build.
    pub const LLAMACPP_GPU_LAYERS_CPU: i32 = 0;

    /// Default `n_ctx`. `0` tells llama.cpp to use the model's own trained
    /// context length rather than imposing one.
    pub const LLAMACPP_DEFAULT_N_CTX: u32 = 0;

    /// Fixed RNG seed for the sampler so a given prompt + params is
    /// reproducible across runs. (`GenerationParams` carries no seed.)
    pub const LLAMACPP_DEFAULT_SAMPLER_SEED: u32 = 1234;
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd src && ~/.cargo/bin/cargo +1.88 test -p primer-core consts::tests::inference_llamacpp`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cd /Users/hherb/src/primer
git add src/crates/primer-core/src/consts.rs
git commit -m "feat(core): add inference consts module for llama.cpp defaults"
```

---

## Task 4: Pure helpers (`params.rs`)

**Files:**
- Create: `src/crates/primer-inference/src/llamacpp/params.rs`
- Create: `src/crates/primer-inference/src/llamacpp/mod.rs` (minimal, to host the module)
- Modify: `src/crates/primer-inference/src/lib.rs` (declare the module)

- [ ] **Step 1: Declare the module so the crate compiles**

Create `src/crates/primer-inference/src/llamacpp/mod.rs` with:

```rust
//! Embedded llama.cpp inference backend (Phase 1.1).
//!
//! The `LlamaCppBackend`, the `LlamaEngine` trait, a test-only mock, and the
//! pure helpers in [`params`] are ALWAYS compiled and host-tested on the
//! default `cargo test`. Only [`engine::RealLlamaEngine`]'s `llama-cpp-2`
//! calls are behind the `llamacpp` cargo feature. The engine emits raw
//! tokens; the backend strips reasoning markers (so that integration is
//! CI-covered via the mock).

pub mod params;
```

In `src/crates/primer-inference/src/lib.rs`, add after `pub mod stub;`:

```rust
pub mod llamacpp;
```

- [ ] **Step 2: Write the failing tests**

Create `src/crates/primer-inference/src/llamacpp/params.rs`:

```rust
//! Pure, feature-independent helpers for the llama.cpp backend.
//!
//! Everything here compiles and is tested WITHOUT the `llamacpp` cargo
//! feature, so the orchestration logic stays in CI's reach. The
//! feature-gated `RealLlamaEngine` consumes these helpers.

use primer_core::consts::inference::{
    LLAMACPP_DEFAULT_N_CTX, LLAMACPP_DEFAULT_SAMPLER_SEED, LLAMACPP_GPU_LAYERS_ALL,
    LLAMACPP_GPU_LAYERS_CPU,
};
use primer_core::error::InferenceError;
use primer_core::inference::GenerationParams;
use std::path::Path;

/// A feature-independent description of the sampler chain to build.
///
/// `RealLlamaEngine` (behind the `llamacpp` feature) turns this into a real
/// `llama_cpp_2::sampling::LlamaSampler`. Kept as plain data so it is
/// host-testable without the binding.
#[derive(Debug, Clone, PartialEq)]
pub struct SamplerSpec {
    pub top_p: f32,
    pub temperature: f32,
    pub seed: u32,
}

/// Derive the sampler chain description from generation params.
pub fn sampler_spec(params: &GenerationParams) -> SamplerSpec {
    SamplerSpec {
        top_p: params.top_p,
        temperature: params.temperature,
        seed: LLAMACPP_DEFAULT_SAMPLER_SEED,
    }
}

/// Resolve the `n_gpu_layers` value for model load.
///
/// An explicit CLI/GUI override wins. Otherwise default to "offload all"
/// when a GPU passthrough feature is compiled, else CPU-only.
pub fn resolve_gpu_layers(override_value: Option<i32>) -> i32 {
    if let Some(n) = override_value {
        return n;
    }
    #[cfg(any(
        feature = "llamacpp-metal",
        feature = "llamacpp-cuda",
        feature = "llamacpp-vulkan"
    ))]
    {
        LLAMACPP_GPU_LAYERS_ALL
    }
    #[cfg(not(any(
        feature = "llamacpp-metal",
        feature = "llamacpp-cuda",
        feature = "llamacpp-vulkan"
    )))]
    {
        LLAMACPP_GPU_LAYERS_CPU
    }
}

/// Resolve `n_ctx`. `None` → the model's trained default (encoded as 0).
pub fn resolve_n_ctx(override_value: Option<u32>) -> u32 {
    override_value.unwrap_or(LLAMACPP_DEFAULT_N_CTX)
}

/// True if `accumulated` ends with any of the (non-empty) stop sequences.
pub fn tail_matches_any_stop(accumulated: &str, stops: &[String]) -> bool {
    stops
        .iter()
        .any(|s| !s.is_empty() && accumulated.ends_with(s.as_str()))
}

/// Validate that `path` points to an existing GGUF file. Dev-facing error.
pub fn validate_gguf_path(path: &Path) -> Result<(), InferenceError> {
    if !path.is_file() {
        return Err(format!(
            "GGUF model file does not exist or is not a file: {}",
            path.display()
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn sampler_spec_carries_params_and_default_seed() {
        let p = GenerationParams {
            top_p: 0.8,
            temperature: 0.5,
            ..Default::default()
        };
        let spec = sampler_spec(&p);
        assert_eq!(spec.top_p, 0.8);
        assert_eq!(spec.temperature, 0.5);
        assert_eq!(spec.seed, LLAMACPP_DEFAULT_SAMPLER_SEED);
    }

    #[test]
    fn gpu_layers_explicit_override_wins() {
        assert_eq!(resolve_gpu_layers(Some(20)), 20);
        assert_eq!(resolve_gpu_layers(Some(0)), 0);
    }

    #[test]
    fn gpu_layers_default_matches_compiled_features() {
        // On a plain (no-GPU-feature) test build this is CPU (0). The value
        // is the const for whichever arm the cfg selects, so assert it is
        // exactly one of the two sentinels.
        let v = resolve_gpu_layers(None);
        assert!(v == LLAMACPP_GPU_LAYERS_ALL || v == LLAMACPP_GPU_LAYERS_CPU);
    }

    #[test]
    fn n_ctx_override_and_default() {
        assert_eq!(resolve_n_ctx(Some(4096)), 4096);
        assert_eq!(resolve_n_ctx(None), LLAMACPP_DEFAULT_N_CTX);
    }

    #[test]
    fn stop_sequence_tail_match() {
        let stops = vec!["</s>".to_string(), "\n\nUser:".to_string()];
        assert!(tail_matches_any_stop("hello</s>", &stops));
        assert!(tail_matches_any_stop("a\n\nUser:", &stops));
        assert!(!tail_matches_any_stop("hello there", &stops));
        // Empty stop strings never match.
        assert!(!tail_matches_any_stop("anything", &["".to_string()]));
    }

    #[test]
    fn validate_gguf_path_rejects_missing() {
        let missing = PathBuf::from("/no/such/model.gguf");
        assert!(validate_gguf_path(&missing).is_err());
    }

    #[test]
    fn validate_gguf_path_accepts_real_file() {
        let f = tempfile::NamedTempFile::new().unwrap();
        assert!(validate_gguf_path(f.path()).is_ok());
    }
}
```

Note: `tempfile` is already a dev-dependency of `primer-inference`.

- [ ] **Step 3: Run tests to verify they fail**

Run: `cd src && ~/.cargo/bin/cargo +1.88 test -p primer-inference llamacpp::params`
Expected: FAIL — the test module references items that compile, but if you wrote the tests first they should fail. (If you implemented helpers in the same file, expect PASS; the discipline here is the helpers + tests land together since they are tightly coupled pure functions. If you prefer strict RED, stub each helper with `todo!()` first, watch the panic, then implement.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd src && ~/.cargo/bin/cargo +1.88 test -p primer-inference llamacpp::params`
Expected: PASS (all 7 tests).

- [ ] **Step 5: Commit**

```bash
cd /Users/hherb/src/primer
git add src/crates/primer-inference/src/llamacpp/ src/crates/primer-inference/src/lib.rs
git commit -m "feat(inference): pure llama.cpp param helpers (sampler/gpu/n_ctx/stop/path)"
```

---

## Task 5: `LlamaEngine` trait + error (always compiled)

**Files:**
- Create: `src/crates/primer-inference/src/llamacpp/engine.rs`
- Modify: `src/crates/primer-inference/src/llamacpp/mod.rs`

- [ ] **Step 1: Declare the engine module**

In `src/crates/primer-inference/src/llamacpp/mod.rs`, add after `pub mod params;`:

```rust
pub mod engine;

pub use engine::LlamaEngine;
```

- [ ] **Step 2: Write the trait (with a doc-test as the failing test)**

Create `src/crates/primer-inference/src/llamacpp/engine.rs` with the trait portion (the `RealLlamaEngine` arrives in Task 7):

```rust
//! The `LlamaEngine` seam + the real `llama-cpp-2`-backed implementation.
//!
//! The trait is ALWAYS compiled so the backend orchestration is testable
//! with a mock on the default `cargo test`. `RealLlamaEngine` and its
//! `llama-cpp-2` calls are behind the `llamacpp` cargo feature.

use primer_core::error::Result;
use primer_core::inference::{GenerationParams, Prompt};

/// Abstraction over the blocking llama.cpp generation surface.
///
/// Implemented for real by [`RealLlamaEngine`] (feature-gated) and by a
/// test-only mock in the backend module. The single non-trivial method,
/// [`LlamaEngine::infer`], owns the blocking decode loop and emits RAW
/// token text — reasoning-marker stripping is the backend's job.
pub trait LlamaEngine: Send + Sync {
    /// Model identifier used to build `LlamaCppBackend::name()`
    /// (e.g. the GGUF file stem).
    fn model_id(&self) -> &str;

    /// Render a [`Prompt`] into the model's prompt string via its chat
    /// template. Cheap CPU; runs on the calling task.
    fn render_prompt(&self, prompt: &Prompt) -> Result<String>;

    /// Run the blocking decode loop over `rendered`. For each detokenized
    /// RAW piece, call `on_token(piece)`; stop early if it returns `false`
    /// (the consumer dropped the stream). Return `Ok(())` on natural
    /// completion (eos / max_tokens / a matched stop sequence), or `Err`
    /// on a decode failure.
    fn infer(
        &self,
        rendered: &str,
        params: &GenerationParams,
        on_token: &mut dyn FnMut(&str) -> bool,
    ) -> Result<()>;
}
```

- [ ] **Step 3: Run to verify the crate compiles**

Run: `cd src && ~/.cargo/bin/cargo +1.88 build -p primer-inference`
Expected: PASS (the trait compiles; no impl yet). This is the GREEN for "trait exists"; the behavioural tests live in Task 6 against the mock.

- [ ] **Step 4: Commit**

```bash
cd /Users/hherb/src/primer
git add src/crates/primer-inference/src/llamacpp/engine.rs src/crates/primer-inference/src/llamacpp/mod.rs
git commit -m "feat(inference): add LlamaEngine trait seam (always compiled)"
```

---

## Task 6: `LlamaCppBackend` + streaming bridge + mock (the core, host-tested)

**Files:**
- Create: `src/crates/primer-inference/src/llamacpp/backend.rs`
- Modify: `src/crates/primer-inference/src/llamacpp/mod.rs`
- Modify: `src/crates/primer-inference/src/lib.rs` (re-export)

- [ ] **Step 1: Declare the backend module + re-exports**

In `src/crates/primer-inference/src/llamacpp/mod.rs`, append:

```rust
pub mod backend;

pub use backend::LlamaCppBackend;
pub use primer_core::backend::LLAMACPP_NAME_PREFIX;
```

In `src/crates/primer-inference/src/lib.rs`, after `pub use stub::StubBackend;` add:

```rust
pub use llamacpp::LlamaCppBackend;
```

- [ ] **Step 2: Write the failing tests (mock-driven)**

Create `src/crates/primer-inference/src/llamacpp/backend.rs`:

```rust
//! [`LlamaCppBackend`] — the [`InferenceBackend`] impl for embedded llama.cpp.
//!
//! The backend holds an `Arc<dyn LlamaEngine>` and owns the streaming
//! bridge: render the prompt on the calling task, then run the engine's
//! blocking decode loop inside `spawn_blocking`, feeding each RAW piece
//! through the shared reasoning filter before forwarding to the consumer.
//! Putting the reasoning strip here (not in the engine) keeps it covered by
//! the default `cargo test` via the mock.

use std::sync::Arc;

use async_trait::async_trait;
use futures::channel::mpsc;
use primer_core::error::Result;
use primer_core::inference::{GenerationParams, InferenceBackend, Prompt, TokenChunk, TokenStream};
use primer_core::reasoning::{ReasoningFilter, ReasoningMarker, default_markers};

use super::engine::LlamaEngine;
use crate::reasoning_stream::{FilterAction, process_filtered_chunk};

pub use primer_core::backend::LLAMACPP_NAME_PREFIX;

/// Embedded llama.cpp inference backend.
pub struct LlamaCppBackend {
    name: String,
    engine: Arc<dyn LlamaEngine>,
    reasoning_markers: Vec<ReasoningMarker>,
}

impl LlamaCppBackend {
    /// Construct from any [`LlamaEngine`]. Uses the built-in reasoning
    /// markers; append custom pairs with [`Self::with_extra_markers`].
    pub fn new(engine: Arc<dyn LlamaEngine>) -> Self {
        let name = format!("{LLAMACPP_NAME_PREFIX}{}", engine.model_id());
        Self {
            name,
            engine,
            reasoning_markers: default_markers(),
        }
    }

    /// Append custom `(open, close)` reasoning-marker pairs. Builder style.
    pub fn with_extra_markers(mut self, extra: Vec<(String, String)>) -> Self {
        self.reasoning_markers
            .extend(extra.into_iter().map(|(o, c)| ReasoningMarker::new(o, c)));
        self
    }
}

#[async_trait]
impl InferenceBackend for LlamaCppBackend {
    fn name(&self) -> &str {
        &self.name
    }

    async fn is_available(&self) -> bool {
        // Construction implies the model loaded successfully.
        true
    }

    async fn generate_stream(
        &self,
        prompt: &Prompt,
        params: &GenerationParams,
    ) -> Result<TokenStream> {
        // Render on the calling task (cheap CPU).
        let rendered = self.engine.render_prompt(prompt)?;
        let engine = Arc::clone(&self.engine);
        let markers = self.reasoning_markers.clone();
        let params = params.clone();
        let (tx, rx) = mpsc::unbounded::<Result<TokenChunk>>();

        tokio::task::spawn_blocking(move || {
            let mut filter = ReasoningFilter::new(markers);
            let mut had_visible = false;

            // Forward each RAW piece through the reasoning filter as a
            // non-final chunk. Returns false (stop) if the consumer dropped.
            let mut on_token = |piece: &str| -> bool {
                match process_filtered_chunk(
                    &mut filter,
                    TokenChunk { text: piece.to_string(), done: false },
                    &mut had_visible,
                    "llamacpp",
                ) {
                    FilterAction::Nothing => true,
                    FilterAction::Forward(r) => tx.unbounded_send(r).is_ok(),
                    // A non-final chunk never yields Final; treat defensively
                    // as "keep going".
                    FilterAction::Final(r) => tx.unbounded_send(r).is_ok(),
                }
            };

            let result = engine.infer(&rendered, &params, &mut on_token);

            match result {
                Ok(()) => {
                    // Flush: feed a synthetic done chunk so a held-back
                    // visible tail still reaches the child and a
                    // reasoning-only stream surfaces ReasoningWithoutAnswer.
                    if let FilterAction::Final(r) = process_filtered_chunk(
                        &mut filter,
                        TokenChunk { text: String::new(), done: true },
                        &mut had_visible,
                        "llamacpp",
                    ) {
                        let _ = tx.unbounded_send(r);
                    }
                }
                Err(e) => {
                    // Decode failure: surface it; do NOT flush the filter
                    // (mirrors the HTTP backends' mid-stream error policy).
                    let _ = tx.unbounded_send(Err(e));
                }
            }
        });

        Ok(Box::pin(rx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use primer_core::error::PrimerError;

    /// A scripted engine for host tests. `pieces` are emitted as raw tokens
    /// in order; `fail_after` (if set) makes `infer` return an Err once that
    /// many pieces have been emitted.
    struct MockLlamaEngine {
        model_id: String,
        pieces: Vec<String>,
        fail_after: Option<usize>,
    }

    impl MockLlamaEngine {
        fn new(model_id: &str, pieces: &[&str]) -> Self {
            Self {
                model_id: model_id.to_string(),
                pieces: pieces.iter().map(|s| s.to_string()).collect(),
                fail_after: None,
            }
        }
        fn failing(model_id: &str, pieces: &[&str], fail_after: usize) -> Self {
            Self {
                model_id: model_id.to_string(),
                pieces: pieces.iter().map(|s| s.to_string()).collect(),
                fail_after: Some(fail_after),
            }
        }
    }

    impl LlamaEngine for MockLlamaEngine {
        fn model_id(&self) -> &str {
            &self.model_id
        }
        fn render_prompt(&self, prompt: &Prompt) -> Result<String> {
            Ok(format!("SYS:{}|MSGS:{}", prompt.system, prompt.messages.len()))
        }
        fn infer(
            &self,
            _rendered: &str,
            _params: &GenerationParams,
            on_token: &mut dyn FnMut(&str) -> bool,
        ) -> Result<()> {
            for (i, p) in self.pieces.iter().enumerate() {
                if !on_token(p) {
                    return Ok(()); // consumer dropped
                }
                if Some(i + 1) == self.fail_after {
                    return Err(PrimerError::Inference("mock decode failure".into()));
                }
            }
            Ok(())
        }
    }

    fn prompt() -> Prompt {
        Prompt {
            system: "be socratic".into(),
            messages: vec![],
        }
    }

    async fn collect(backend: &LlamaCppBackend) -> (String, bool) {
        let mut stream = backend
            .generate_stream(&prompt(), &GenerationParams::default())
            .await
            .unwrap();
        let mut out = String::new();
        let mut errored = false;
        while let Some(item) = stream.next().await {
            match item {
                Ok(c) => out.push_str(&c.text),
                Err(_) => {
                    errored = true;
                    break;
                }
            }
        }
        (out, errored)
    }

    #[test]
    fn name_is_prefixed_model_id() {
        let backend =
            LlamaCppBackend::new(Arc::new(MockLlamaEngine::new("Qwen3-7B", &["hi"])));
        assert_eq!(backend.name(), "llamacpp:Qwen3-7B");
    }

    #[tokio::test]
    async fn streams_all_pieces_then_done() {
        let backend = LlamaCppBackend::new(Arc::new(MockLlamaEngine::new(
            "m",
            &["Hello", ", ", "world"],
        )));
        let (out, errored) = collect(&backend).await;
        assert_eq!(out, "Hello, world");
        assert!(!errored);
    }

    #[tokio::test]
    async fn generate_aggregates_stream() {
        let backend =
            LlamaCppBackend::new(Arc::new(MockLlamaEngine::new("m", &["a", "b", "c"])));
        let text = backend
            .generate(&prompt(), &GenerationParams::default())
            .await
            .unwrap();
        assert_eq!(text, "abc");
    }

    #[tokio::test]
    async fn reasoning_markers_are_stripped() {
        // Raw model output contains a <think> block; the consumer must not
        // see it. This proves the strip integration on the DEFAULT build.
        let backend = LlamaCppBackend::new(Arc::new(MockLlamaEngine::new(
            "m",
            &["<think>plan the answer</think>", "The sky is blue."],
        )));
        let (out, errored) = collect(&backend).await;
        assert_eq!(out, "The sky is blue.");
        assert!(!errored);
    }

    #[tokio::test]
    async fn reasoning_only_yields_error() {
        let backend = LlamaCppBackend::new(Arc::new(MockLlamaEngine::new(
            "m",
            &["<think>only thinking, no answer"],
        )));
        let (out, errored) = collect(&backend).await;
        assert_eq!(out, "");
        assert!(errored);
    }

    #[tokio::test]
    async fn mid_stream_decode_error_propagates() {
        let backend = LlamaCppBackend::new(Arc::new(MockLlamaEngine::failing(
            "m",
            &["partial ", "answer"],
            1,
        )));
        let (out, errored) = collect(&backend).await;
        assert_eq!(out, "partial ");
        assert!(errored);
    }

    #[tokio::test]
    async fn custom_marker_is_stripped() {
        let backend = LlamaCppBackend::new(Arc::new(MockLlamaEngine::new(
            "m",
            &["a[[r]]hidden[[/r]]b"],
        )))
        .with_extra_markers(vec![("[[r]]".to_string(), "[[/r]]".to_string())]);
        let (out, errored) = collect(&backend).await;
        assert_eq!(out, "ab");
        assert!(!errored);
    }
}
```

- [ ] **Step 3: Run tests to verify they fail then pass**

Run: `cd src && ~/.cargo/bin/cargo +1.88 test -p primer-inference llamacpp::backend`
Expected: PASS (7 tests). If you stub `generate_stream` with `todo!()` first to see RED, do so, then paste the real body.

- [ ] **Step 4: Lint**

Run: `cd src && ~/.cargo/bin/cargo +1.88 clippy -p primer-inference --all-targets -- -D warnings`
Expected: exit 0.

- [ ] **Step 5: Commit**

```bash
cd /Users/hherb/src/primer
git add src/crates/primer-inference/src/llamacpp/backend.rs src/crates/primer-inference/src/llamacpp/mod.rs src/crates/primer-inference/src/lib.rs
git commit -m "feat(inference): LlamaCppBackend streaming bridge + reasoning strip (mock-tested)"
```

---

## Task 7: `RealLlamaEngine` (feature-gated llama-cpp-2 wrapper)

**Files:**
- Modify: `src/crates/primer-inference/src/llamacpp/engine.rs`

> This is the only task whose body requires `--features llamacpp` and a C++ toolchain (cmake + clang). Build/validate it on the Metal-capable MacBook. The default `cargo test` does NOT compile this code.

- [ ] **Step 1: Add the feature-gated real engine**

Append to `src/crates/primer-inference/src/llamacpp/engine.rs`:

```rust
#[cfg(feature = "llamacpp")]
mod real {
    use super::*;
    use std::path::Path;
    use std::sync::OnceLock;

    use llama_cpp_2::context::params::LlamaContextParams;
    use llama_cpp_2::llama_backend::LlamaBackend;
    use llama_cpp_2::llama_batch::LlamaBatch;
    use llama_cpp_2::model::params::LlamaModelParams;
    use llama_cpp_2::model::{AddBos, LlamaChatMessage, LlamaChatTemplate, LlamaModel, Special};
    use llama_cpp_2::sampling::LlamaSampler;
    use primer_core::error::{PrimerError, Result};
    use primer_core::inference::{GenerationParams, Prompt, Role};
    use tokio::sync::Mutex;

    use crate::llamacpp::params::{resolve_n_ctx, sampler_spec, tail_matches_any_stop, validate_gguf_path};

    /// Global llama.cpp backend handle. `LlamaBackend::init()` may be called
    /// only once per process; this lazily initialises it.
    static LLAMA_BACKEND: OnceLock<LlamaBackend> = OnceLock::new();

    fn backend_handle() -> Result<&'static LlamaBackend> {
        // OnceLock has no fallible get_or_init in stable until recently; use
        // get_or_try_init pattern via a helper.
        if let Some(b) = LLAMA_BACKEND.get() {
            return Ok(b);
        }
        let b = LlamaBackend::init()
            .map_err(|e| PrimerError::Inference(format!("llama.cpp init failed: {e}").into()))?;
        // Race: if another thread set it first, ours is dropped — harmless.
        let _ = LLAMA_BACKEND.set(b);
        Ok(LLAMA_BACKEND.get().expect("just set"))
    }

    /// Real llama-cpp-2-backed engine.
    pub struct RealLlamaEngine {
        model_id: String,
        model: std::sync::Arc<LlamaModel>,
        template: LlamaChatTemplate,
        n_ctx: u32,
        // llama.cpp forbids concurrent decode on one context.
        ctx_guard: Mutex<()>,
    }

    impl RealLlamaEngine {
        /// Load a GGUF model from `gguf_path`.
        pub fn new(gguf_path: &Path, n_gpu_layers: i32, n_ctx_override: Option<u32>) -> Result<Self> {
            validate_gguf_path(gguf_path).map_err(PrimerError::Inference)?;
            let backend = backend_handle()?;
            let model_params = LlamaModelParams::default().with_n_gpu_layers(n_gpu_layers);
            let model = LlamaModel::load_from_file(backend, gguf_path, &model_params)
                .map_err(|e| PrimerError::Inference(format!("GGUF load failed: {e}").into()))?;

            // Read the embedded chat template; fall back to a generic one.
            let template = model
                .chat_template(None)
                .unwrap_or_else(|_| {
                    LlamaChatTemplate::new(GENERIC_CHAT_TEMPLATE)
                        .expect("generic template is valid")
                });

            let model_id = gguf_path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "llamacpp-model".to_string());

            Ok(Self {
                model_id,
                model: std::sync::Arc::new(model),
                template,
                n_ctx: resolve_n_ctx(n_ctx_override),
                ctx_guard: Mutex::new(()),
            })
        }
    }

    /// Minimal fallback chat template for GGUFs that embed none.
    const GENERIC_CHAT_TEMPLATE: &str = "{% for m in messages %}{% if m.role == 'system' %}{{ m.content }}\n{% elif m.role == 'user' %}User: {{ m.content }}\n{% else %}Assistant: {{ m.content }}\n{% endif %}{% endfor %}Assistant:";

    impl LlamaEngine for RealLlamaEngine {
        fn model_id(&self) -> &str {
            &self.model_id
        }

        fn render_prompt(&self, prompt: &Prompt) -> Result<String> {
            let mut messages = Vec::with_capacity(prompt.messages.len() + 1);
            if !prompt.system.is_empty() {
                messages.push(
                    LlamaChatMessage::new("system".to_string(), prompt.system.clone())
                        .map_err(|e| PrimerError::Inference(format!("chat msg: {e}").into()))?,
                );
            }
            for m in &prompt.messages {
                let role = match m.role {
                    Role::System => "system",
                    Role::User => "user",
                    Role::Assistant => "assistant",
                };
                messages.push(
                    LlamaChatMessage::new(role.to_string(), m.content.clone())
                        .map_err(|e| PrimerError::Inference(format!("chat msg: {e}").into()))?,
                );
            }
            self.model
                .apply_chat_template(&self.template, &messages, true)
                .map_err(|e| PrimerError::Inference(format!("chat template: {e}").into()))
        }

        fn infer(
            &self,
            rendered: &str,
            params: &GenerationParams,
            on_token: &mut dyn FnMut(&str) -> bool,
        ) -> Result<()> {
            // Serialise concurrent callers (one context per engine).
            let _guard = self.ctx_guard.blocking_lock();
            let backend = backend_handle()?;

            let n_ctx = if self.n_ctx == 0 { None } else { Some(self.n_ctx) };
            let mut ctx = self
                .model
                .new_context(backend, LlamaContextParams::default().with_n_ctx(n_ctx.and_then(std::num::NonZeroU32::new)))
                .map_err(|e| PrimerError::Inference(format!("context: {e}").into()))?;

            let tokens = self
                .model
                .str_to_token(rendered, AddBos::Always)
                .map_err(|e| PrimerError::Inference(format!("tokenize: {e}").into()))?;

            let mut batch = LlamaBatch::new(512, 1);
            let last = tokens.len() - 1;
            for (i, tok) in tokens.iter().enumerate() {
                batch
                    .add(*tok, i as i32, &[0], i == last)
                    .map_err(|e| PrimerError::Inference(format!("batch: {e}").into()))?;
            }
            ctx.decode(&mut batch)
                .map_err(|e| PrimerError::Inference(format!("decode: {e}").into()))?;

            let spec = sampler_spec(params);
            let mut sampler = LlamaSampler::chain_simple([
                LlamaSampler::top_p(spec.top_p, 1),
                LlamaSampler::temp(spec.temperature),
                LlamaSampler::dist(spec.seed),
            ]);

            let mut n_cur = batch.n_tokens();
            let mut accumulated = String::new();
            let mut decoder = encoding_rs::UTF_8.new_decoder();

            for _ in 0..params.max_tokens {
                let token = sampler.sample(&ctx, batch.n_tokens() - 1);
                sampler.accept(token);
                if token == self.model.token_eos() {
                    break;
                }
                let piece = self
                    .model
                    .token_to_piece(token, &mut decoder, Special::Tokenize)
                    .map_err(|e| PrimerError::Inference(format!("detok: {e}").into()))?;
                accumulated.push_str(&piece);
                if !on_token(&piece) {
                    return Ok(()); // consumer dropped
                }
                if tail_matches_any_stop(&accumulated, &params.stop_sequences) {
                    break;
                }
                batch.clear();
                batch
                    .add(token, n_cur, &[0], true)
                    .map_err(|e| PrimerError::Inference(format!("batch: {e}").into()))?;
                n_cur += 1;
                ctx.decode(&mut batch)
                    .map_err(|e| PrimerError::Inference(format!("decode: {e}").into()))?;
            }
            Ok(())
        }
    }
}

#[cfg(feature = "llamacpp")]
pub use real::RealLlamaEngine;
```

> **Implementer note:** `llama-cpp-2`'s exact method signatures (e.g. `top_p(p, min_keep)`, `token_to_piece(..., Special)`, `with_n_ctx(Option<NonZeroU32>)`, `chat_template`, `LlamaChatTemplate::new`) vary slightly across 0.1.x patch releases. Treat the code above as the intended shape; reconcile against `cargo doc -p llama-cpp-2 --open` for 0.1.146 and adjust call sites if the compiler objects. The behavioural contract (raw pieces to `on_token`, stop on eos/max_tokens/stop-seq) is what matters and is fixed.

- [ ] **Step 2: Add `encoding_rs` if needed**

`token_to_piece` needs an `encoding_rs::Decoder`. Add to `src/crates/primer-inference/Cargo.toml` under `[dependencies]` only if not transitively available:

```toml
encoding_rs = { version = "0.8", optional = true }
```

and extend the feature: `llamacpp = ["dep:llama-cpp-2", "dep:encoding_rs"]`. (If `llama-cpp-2` re-exports a decoder or `token_to_piece` has a `String` convenience overload in 0.1.146, prefer that and skip this dep.)

- [ ] **Step 3: Build with the feature (Metal)**

Run (on the MacBook): `cd src && ~/.cargo/bin/cargo +1.88 build -p primer-inference --features llamacpp-metal`
Expected: PASS (compiles llama.cpp; first build is slow). Fix any 0.1.146 signature drift per the implementer note.

- [ ] **Step 4: Verify default build still green (feature off)**

Run: `cd src && ~/.cargo/bin/cargo +1.88 build -p primer-inference`
Expected: PASS — the `real` module is cfg-gated out; nothing references `llama-cpp-2`.

- [ ] **Step 5: Commit**

```bash
cd /Users/hherb/src/primer
git add src/crates/primer-inference/src/llamacpp/engine.rs src/crates/primer-inference/Cargo.toml
git commit -m "feat(inference): RealLlamaEngine llama-cpp-2 wrapper (feature-gated)"
```

---

## Task 8: Engine wiring (`build_backend` + `BackendParams`)

**Files:**
- Modify: `src/crates/primer-engine/Cargo.toml`
- Modify: `src/crates/primer-engine/src/wiring.rs`

- [ ] **Step 1: Add forwarding features**

In `src/crates/primer-engine/Cargo.toml` `[features]` (after `qnn`):

```toml
# Embedded llama.cpp backend (Phase 1.1). Adds the `llamacpp` arm to
# `build_backend` and pulls in primer-inference/llamacpp.
llamacpp = ["primer-inference/llamacpp"]
llamacpp-metal = ["primer-inference/llamacpp-metal"]
llamacpp-cuda = ["primer-inference/llamacpp-cuda"]
llamacpp-vulkan = ["primer-inference/llamacpp-vulkan"]
```

- [ ] **Step 2: Write the failing tests**

In `src/crates/primer-engine/src/wiring.rs`, inside `#[cfg(test)] mod tests`, add:

```rust
#[tokio::test]
async fn llamacpp_without_feature_returns_build_hint() {
    let params = BackendParams {
        gguf_path: Some(std::path::PathBuf::from("/tmp/model.gguf")),
        ..test_params()
    };
    let err = build_backend("llamacpp", "ignored".into(), &params)
        .await
        .unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("llamacpp"), "got: {msg}");
    assert!(msg.contains("feature"), "got: {msg}");
}

#[tokio::test]
async fn llamacpp_without_gguf_path_errors_clearly() {
    // Even with the feature off the path is validated first via the arm;
    // this asserts the dispatch reaches the llamacpp arm at all.
    let params = test_params();
    let err = build_backend("llamacpp", "ignored".into(), &params)
        .await
        .unwrap_err();
    assert!(format!("{err}").to_lowercase().contains("llamacpp"));
}
```

You will need a `test_params()` helper if one doesn't exist. If the existing tests construct `BackendParams` inline, add a small helper near the test module:

```rust
fn test_params() -> BackendParams {
    BackendParams {
        api_key: None,
        ollama_url: "http://localhost:11434".into(),
        openai_compat_url: "http://localhost:8000".into(),
        openai_compat_api_key: None,
        classifier_backend: None,
        classifier_model: None,
        extractor_backend: None,
        extractor_model: None,
        comprehension_backend: None,
        comprehension_model: None,
        qnn_bundle_dir: None,
        qnn_qairt_lib_dir: None,
        gguf_path: None,
        llamacpp_gpu_layers: None,
        llamacpp_n_ctx: None,
        reasoning_markers: Vec::new(),
    }
}
```

(Match field names to the existing struct; the three `llamacpp_*` fields are added in Step 3.)

- [ ] **Step 3: Add the `BackendParams` fields**

In `src/crates/primer-engine/src/wiring.rs`, add to `struct BackendParams` (after `qnn_qairt_lib_dir`):

```rust
    /// Path to the GGUF model file. Consumed by the `"llamacpp"` arm of
    /// [`build_backend`] only; ignored by every other arm. Always present
    /// in the struct (qnn-style) so the shape is identical across feature
    /// combinations. The CLI surfaces this by reusing `--model`.
    pub gguf_path: Option<PathBuf>,
    /// Explicit `n_gpu_layers` override for the llama.cpp backend
    /// (`--llamacpp-gpu-layers`). `None` ⇒ resolved by feature
    /// (`primer_inference::llamacpp::params::resolve_gpu_layers`).
    pub llamacpp_gpu_layers: Option<i32>,
    /// Explicit `n_ctx` override (`--llamacpp-n-ctx`). `None` ⇒ the model's
    /// trained default.
    pub llamacpp_n_ctx: Option<u32>,
```

- [ ] **Step 4: Add the `"llamacpp"` arm + the free fn**

In `build_backend`, add before the `other =>` arm:

```rust
        "llamacpp" => build_llamacpp_backend(params).await,
```

And add the two-cfg free fn (after `build_qnn_backend`'s `not` variant):

```rust
/// Construct the llama.cpp backend. Mirrors [`build_qnn_backend`]'s two-cfg
/// shape: feature-on validates the GGUF path and constructs the real engine;
/// feature-off returns a distinct build-time hint.
#[cfg(feature = "llamacpp")]
async fn build_llamacpp_backend(params: &BackendParams) -> Result<Arc<dyn InferenceBackend>> {
    use primer_inference::llamacpp::params::{resolve_gpu_layers, resolve_n_ctx};
    let gguf_path = params.gguf_path.as_ref().ok_or_else(|| {
        PrimerError::Inference(
            "--model <path-to.gguf> is required for --backend llamacpp".into(),
        )
    })?;
    let n_gpu_layers = resolve_gpu_layers(params.llamacpp_gpu_layers);
    let n_ctx = params.llamacpp_n_ctx;
    let _ = resolve_n_ctx(n_ctx); // keep import used; engine resolves too
    let engine = primer_inference::llamacpp::engine::RealLlamaEngine::new(
        gguf_path, n_gpu_layers, n_ctx,
    )?;
    let backend = primer_inference::LlamaCppBackend::new(Arc::new(engine))
        .with_extra_markers(params.reasoning_markers.clone());
    Ok(Arc::new(backend))
}

#[cfg(not(feature = "llamacpp"))]
async fn build_llamacpp_backend(_params: &BackendParams) -> Result<Arc<dyn InferenceBackend>> {
    Err(PrimerError::Inference(
        "llamacpp backend requires the `llamacpp` cargo feature. \
         Build with `cargo build --features primer-cli/llamacpp` \
         (or llamacpp-metal / llamacpp-cuda / llamacpp-vulkan for GPU)."
            .into(),
    ))
}
```

> Note: `RealLlamaEngine::new` is synchronous; the `async fn` wrapper keeps the dispatch uniform. If the borrow checker complains about `resolve_n_ctx` being unused on some feature combos, drop that line — the engine resolves `n_ctx` internally.

- [ ] **Step 5: Update ALL existing `BackendParams { … }` literals in this file**

Search `wiring.rs` for every `BackendParams {` (tests construct several). Add the three new fields (`gguf_path: None, llamacpp_gpu_layers: None, llamacpp_n_ctx: None,`) to each so the file compiles.

Run: `cd src && grep -n "BackendParams {" crates/primer-engine/src/wiring.rs`

- [ ] **Step 6: Run tests (feature off)**

Run: `cd src && ~/.cargo/bin/cargo +1.88 test -p primer-engine wiring`
Expected: PASS, including the two new `llamacpp_*` tests (build-hint + dispatch).

- [ ] **Step 7: Lint + commit**

```bash
cd /Users/hherb/src/primer
cd src && ~/.cargo/bin/cargo +1.88 clippy -p primer-engine --all-targets -- -D warnings && cd ..
git add src/crates/primer-engine/Cargo.toml src/crates/primer-engine/src/wiring.rs
git commit -m "feat(engine): wire llamacpp backend (build_backend arm + BackendParams)"
```

---

## Task 9: CLI flags + construction

**Files:**
- Modify: `src/crates/primer-cli/Cargo.toml`
- Modify: `src/crates/primer-cli/src/main.rs`

- [ ] **Step 1: Add forwarding features**

In `src/crates/primer-cli/Cargo.toml` `[features]` (after `qnn`):

```toml
# Embedded llama.cpp backend (Phase 1.1). Wires `--backend llamacpp` through
# to `LlamaCppBackend`. GPU variants forward to the binding's backend
# features. Non-default (compiles llama.cpp C++).
llamacpp = ["primer-inference/llamacpp", "primer-engine/llamacpp"]
llamacpp-metal = ["primer-inference/llamacpp-metal", "primer-engine/llamacpp-metal"]
llamacpp-cuda = ["primer-inference/llamacpp-cuda", "primer-engine/llamacpp-cuda"]
llamacpp-vulkan = ["primer-inference/llamacpp-vulkan", "primer-engine/llamacpp-vulkan"]
```

- [ ] **Step 2: Extend the `--backend` and `--model` doc + add two flags**

In `src/crates/primer-cli/src/main.rs`, update the `backend` doc comment to mention `llamacpp`, and the `model` doc to note `For llamacpp: filesystem path to the .gguf file — required.`

Add two new fields to `struct Cli` (near the qnn flags, but NOT cfg-gated — they're cheap and documented llamacpp-only):

```rust
    /// llama.cpp: number of model layers to offload to GPU. Default:
    /// all layers (-1) when built with a GPU feature, else CPU (0).
    /// Only meaningful with `--backend llamacpp`.
    #[arg(long, value_name = "N")]
    llamacpp_gpu_layers: Option<i32>,

    /// llama.cpp: context length (n_ctx). Default: the model's trained
    /// length. Only meaningful with `--backend llamacpp`.
    #[arg(long, value_name = "N")]
    llamacpp_n_ctx: Option<u32>,
```

- [ ] **Step 3: Thread into `BackendParams` construction**

At the `BackendParams { … }` literal (~line 874), add:

```rust
        gguf_path: if cli.backend == "llamacpp" {
            cli.model.clone().map(std::path::PathBuf::from)
        } else {
            None
        },
        llamacpp_gpu_layers: cli.llamacpp_gpu_layers,
        llamacpp_n_ctx: cli.llamacpp_n_ctx,
```

- [ ] **Step 4: Rebind `main_model` for llamacpp (so subsystem ids carry the real name)**

Find the qnn `main_model` rebind (`if cli.backend == "qnn" { backend.name()… }`) and extend it to also cover llamacpp:

```rust
    let main_model: String = if cli.backend == "qnn" || cli.backend == "llamacpp" {
        backend.name().to_string()
    } else {
        main_model
    };
```

(For llamacpp, `--model` is the GGUF path; the human-meaningful id is `backend.name()` = `"llamacpp:<stem>"`.)

- [ ] **Step 5: Resolve `main_model` placeholder before construction**

The CLI computes `main_model` before `build_backend`. For llamacpp the value passed to `build_backend` is unused by the arm (it reads `gguf_path`), but a non-empty placeholder avoids the "model required" guards elsewhere. Ensure the pre-construction `main_model` resolution does not hard-error when `--backend llamacpp` and `--model` is a path (it is a valid string, so existing logic is fine). If there's a "model required for X" check, confirm llamacpp is treated like ollama (model required) — the path doubles as the model.

Run: `cd src && grep -n "model.*required\|resolve_main_model\|qnn-pending" crates/primer-cli/src/main.rs`
Apply the same handling pattern llamacpp needs (model/path required; no placeholder dance like qnn since the path is real).

- [ ] **Step 6: Build (feature off) + help check**

Run: `cd src && ~/.cargo/bin/cargo +1.88 build -p primer-cli`
Expected: PASS. The two new flags appear in `--help` but error usefully if used with a non-llamacpp backend (they're just ignored — acceptable).

- [ ] **Step 7: Build with the feature (Metal) + a real smoke run is Task 11**

Run (MacBook): `cd src && ~/.cargo/bin/cargo +1.88 build -p primer-cli --features llamacpp-metal`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
cd /Users/hherb/src/primer
git add src/crates/primer-cli/Cargo.toml src/crates/primer-cli/src/main.rs
git commit -m "feat(cli): --backend llamacpp (--model = gguf path) + gpu-layers/n-ctx flags"
```

---

## Task 10: GUI config plumbing (build/click-through owner-gated)

**Files:**
- Modify: `src/crates/primer-gui/Cargo.toml`
- Modify: `src/crates/primer-gui/src/config.rs`
- Modify: `src/crates/primer-gui/src/wiring.rs` (or the file holding `resolve_main_model` + `BackendParams` build)
- Modify: `src/crates/primer-gui/ui/settings.js`

> The GUI's Tauri/webkit feature build + visual click-through is owner-gated (like other GUI voice work). This task wires the config DTOs + the build_backend call so a `--features llamacpp` GUI build is functionally complete; the owner validates the UI later.

- [ ] **Step 1: Add forwarding features**

In `src/crates/primer-gui/Cargo.toml` `[features]`:

```toml
llamacpp = ["primer-inference/llamacpp", "primer-engine/llamacpp"]
llamacpp-metal = ["primer-inference/llamacpp-metal", "primer-engine/llamacpp-metal"]
llamacpp-cuda = ["primer-inference/llamacpp-cuda", "primer-engine/llamacpp-cuda"]
llamacpp-vulkan = ["primer-inference/llamacpp-vulkan", "primer-engine/llamacpp-vulkan"]
```

- [ ] **Step 2: Add fields to `BackendConfig`, its `Default`, `BackendConfigView`, `BackendConfigUpdate`**

In `src/crates/primer-gui/src/config.rs`, add to `struct BackendConfig` (near `qnn_bundle_dir`):

```rust
    /// GGUF model path for the llamacpp backend.
    #[serde(default)]
    pub gguf_path: Option<PathBuf>,
    /// llama.cpp n_gpu_layers override (None = feature default).
    #[serde(default)]
    pub llamacpp_gpu_layers: Option<i32>,
    /// llama.cpp n_ctx override (None = model default).
    #[serde(default)]
    pub llamacpp_n_ctx: Option<u32>,
```

Add `gguf_path: None, llamacpp_gpu_layers: None, llamacpp_n_ctx: None,` to the `Default` impl. Mirror the three fields in `BackendConfigView` (read) and `BackendConfigUpdate` (write). **`BackendConfigUpdate` has NO `#[serde(default)]`** — every field is mandatory in the IPC payload, so `settings.js::gather()` (Step 4) MUST send all three.

- [ ] **Step 3: Wire into the GUI's `build_backend` path**

In the GUI wiring (where `BackendParams` is constructed and `resolve_main_model` lives), add the `"llamacpp"` arm to `resolve_main_model` returning a placeholder rebound to `backend.name()` after construction (mirror the existing qnn `"qnn-pending"` handling), and thread `gguf_path` / `llamacpp_gpu_layers` / `llamacpp_n_ctx` from `BackendConfig` into `BackendParams`.

Run: `cd src && grep -rn "resolve_main_model\|qnn-pending\|BackendParams {" crates/primer-gui/src/`
Apply the same pattern for llamacpp (`gguf_path` from `cfg.backend.gguf_path`).

- [ ] **Step 4: Update `settings.js`**

In `src/crates/primer-gui/ui/settings.js`, add `llamacpp` to the backend `<select>` options, a GGUF path field + the two numeric inputs (shown when kind === "llamacpp"), and ensure `gather()` sends `gguf_path` (null when blank), `llamacpp_gpu_layers`, `llamacpp_n_ctx` (null when empty) in the `update_settings` payload. Populate them in the modal-open handler from `BackendConfigView`.

- [ ] **Step 5: Build (feature off) — GUI default still compiles**

Run: `cd src && ~/.cargo/bin/cargo +1.88 build -p primer-gui`
Expected: PASS (the llamacpp option errors inline at runtime without the feature, qnn-style).

- [ ] **Step 6: Commit**

```bash
cd /Users/hherb/src/primer
git add src/crates/primer-gui/Cargo.toml src/crates/primer-gui/src/config.rs src/crates/primer-gui/src/wiring.rs src/crates/primer-gui/ui/settings.js
git commit -m "feat(gui): llamacpp backend config plumbing (path + gpu-layers + n-ctx)"
```

---

## Task 11: `#[ignore]` real-model smoke (feature-gated, on-demand)

**Files:**
- Create: `src/crates/primer-inference/tests/llamacpp_smoke.rs`
- Modify: `src/crates/primer-inference/Cargo.toml` (declare the test's required-features if needed)

- [ ] **Step 1: Write the ignored smoke test**

Create `src/crates/primer-inference/tests/llamacpp_smoke.rs`:

```rust
//! Real-model smoke for LlamaCppBackend. NEVER runs in default CI.
//!
//! Run on a Metal-capable machine with a small GGUF on disk:
//!   PRIMER_LLAMACPP_TEST_GGUF=/path/to/model.gguf \
//!     ~/.cargo/bin/cargo +1.88 test -p primer-inference \
//!       --features llamacpp-metal --test llamacpp_smoke -- --ignored --nocapture

#![cfg(feature = "llamacpp")]

use std::sync::Arc;

use primer_inference::LlamaCppBackend;
use primer_inference::llamacpp::engine::RealLlamaEngine;
use primer_core::inference::{GenerationParams, InferenceBackend, Message, Prompt, Role};

#[tokio::test]
#[ignore = "requires PRIMER_LLAMACPP_TEST_GGUF + a C++ toolchain + GPU"]
async fn generates_nonempty_response() {
    let gguf = std::env::var("PRIMER_LLAMACPP_TEST_GGUF")
        .expect("set PRIMER_LLAMACPP_TEST_GGUF to a .gguf path");
    let engine = RealLlamaEngine::new(std::path::Path::new(&gguf), -1, Some(2048))
        .expect("load model");
    let backend = LlamaCppBackend::new(Arc::new(engine));

    assert!(backend.name().starts_with("llamacpp:"));

    let prompt = Prompt {
        system: "You are a friendly Socratic tutor. Answer in one short sentence.".into(),
        messages: vec![Message {
            role: Role::User,
            content: "What color is the sky on a clear day?".into(),
        }],
    };
    let params = GenerationParams {
        max_tokens: 64,
        ..Default::default()
    };
    let out = backend.generate(&prompt, &params).await.expect("generate");
    println!("model said: {out}");
    assert!(!out.trim().is_empty(), "expected non-empty response");
}
```

- [ ] **Step 2: Confirm it is excluded from the default test run**

Run: `cd src && ~/.cargo/bin/cargo +1.88 test -p primer-inference`
Expected: PASS — the smoke test's file is `#![cfg(feature = "llamacpp")]`, so it is not even compiled without the feature; the always-compiled `llamacpp::*` unit tests still run and pass.

- [ ] **Step 3: Run the smoke on the MacBook with a real GGUF**

Download a small GGUF (e.g. a 1–3B Q4_K_M) to a local path, then:

Run: `cd src && PRIMER_LLAMACPP_TEST_GGUF=/path/to/model.gguf ~/.cargo/bin/cargo +1.88 test -p primer-inference --features llamacpp-metal --test llamacpp_smoke -- --ignored --nocapture`
Expected: prints a sentence; assertion passes. Record the model + rough tok/s in the commit message (informal — formal benchmarking is deferred bullet b).

- [ ] **Step 4: Commit**

```bash
cd /Users/hherb/src/primer
git add src/crates/primer-inference/tests/llamacpp_smoke.rs
git commit -m "test(inference): #[ignore] real-model llamacpp smoke (feature-gated)"
```

---

## Task 12: Full verification + docs

**Files:**
- Modify: `CLAUDE.md`, `README.md`, `ROADMAP.md`
- Modify: `src/crates/primer-inference/src/lib.rs` (doc comment)

- [ ] **Step 1: Full default-build verification (the required CI shape)**

Run, from `src/`:
```bash
~/.cargo/bin/cargo +1.88 fmt --all -- --check
~/.cargo/bin/cargo +1.88 clippy --workspace --all-targets -- -D warnings
~/.cargo/bin/cargo +1.88 test --workspace --no-fail-fast
```
Expected: all exit 0. Fix anything before proceeding.

- [ ] **Step 2: Feature-on clippy (not covered by --workspace)**

Run (MacBook): `cd src && ~/.cargo/bin/cargo +1.88 clippy -p primer-inference -p primer-engine --features primer-engine/llamacpp -- -D warnings`
Expected: exit 0. (Mirrors the CLAUDE.md note that feature-gated clippy is invisible to `--workspace` clippy.)

- [ ] **Step 3: Update `src/crates/primer-inference/src/lib.rs` doc**

Change the `LlamaCppBackend: (TODO)` line to:

```rust
//! - `LlamaCppBackend`: Embedded llama.cpp inference from a local GGUF
//!   (behind the `llamacpp` cargo feature; CPU + Metal/CUDA/Vulkan
//!   passthrough features). Phase 1.1.
```

- [ ] **Step 4: Update `CLAUDE.md`**

In the `primer-inference` bullet, add `LlamaCppBackend` to the backend list. Add a new gotcha bullet:

> **`LlamaCppBackend` is in-process llama.cpp behind the non-default `llamacpp` feature** (CPU; `llamacpp-metal`/`-cuda`/`-vulkan` add GPU). It mirrors the QNN trait-abstracted pattern: the `LlamaEngine` seam, the backend's mpsc + `spawn_blocking` streaming bridge, the reasoning-marker strip, and the pure helpers (`llamacpp::params`) are ALL compiled + tested on the default `cargo test` via a `MockLlamaEngine`; only `RealLlamaEngine`'s `llama-cpp-2` calls are feature-gated. The engine emits RAW tokens via an `on_token` callback and the backend strips reasoning (so `<think>…</think>` stripping is CI-covered, not only the shared `reasoning_stream` unit tests). The GGUF's embedded chat template is used via `model.apply_chat_template` (generic fallback if absent — no sidecar, unlike QNN's `primer-meta.json`). `LlamaBackend::init()` is once-per-process behind a `OnceLock`. The CLI reuses `--model` as the GGUF path; `--llamacpp-gpu-layers` / `--llamacpp-n-ctx` tune offload/context. `llama-cpp-2` does NOT pull `ort`, so it is independent of the `=2.0.0-rc.10` ort pin. `LLAMACPP_NAME_PREFIX = "llamacpp:"` is NOT small-context (local models commonly run 8K+).

- [ ] **Step 5: Update `README.md`**

In the Phase 1.1 / status area, note that local llama.cpp inference has shipped (in-process GGUF, feature-gated `llamacpp`, CPU + Metal/CUDA/Vulkan), benchmarking + local→cloud fallback still ahead.

- [ ] **Step 6: Update `ROADMAP.md`**

In `### 1.1 — llama.cpp integration`, check the first bullet:
```markdown
- [x] `LlamaCppBackend` via llama-cpp-2 bindings; GGUF loading from a configurable path (behind the non-default `llamacpp` feature; CPU + Metal/CUDA/Vulkan passthrough). Benchmarking (bullet 2) + local→cloud fallback (bullet 3) still open.
```

- [ ] **Step 7: Commit docs**

```bash
cd /Users/hherb/src/primer
git add CLAUDE.md README.md ROADMAP.md src/crates/primer-inference/src/lib.rs
git commit -m "docs: LlamaCppBackend Phase 1.1 (CLAUDE/README/ROADMAP/lib)"
```

- [ ] **Step 8: Open the PR**

```bash
cd /Users/hherb/src/primer
git push -u origin <branch>
gh pr create --title "feat(inference): LlamaCppBackend — in-process llama.cpp (Phase 1.1)" \
  --body "Implements Phase 1.1 bullet (a): an embedded, GGUF-loading llama.cpp \
inference backend behind the non-default \`llamacpp\` feature. Mirrors the QNN \
trait-abstracted pattern so the streaming bridge + reasoning strip are host-tested \
on the default cargo test via a mock engine; the real llama-cpp-2 wrapper is \
feature-gated. CLI \`--backend llamacpp --model <gguf>\`; GUI config plumbing wired \
(build owner-gated). Benchmarking (b) + local→cloud fallback (c) deferred.

Spec: docs/superpowers/specs/2026-06-04-llamacpp-backend-design.md
Plan: docs/superpowers/plans/2026-06-04-llamacpp-backend.md

🤖 Generated with [Claude Code](https://claude.com/claude-code)"
```

---

## Self-Review notes (for the implementer)

- **Spec coverage:** every spec section maps to a task — features (T1), name prefix (T2), consts (T3), pure helpers (T4), trait (T5), backend+strip+mock (T6), real engine (T7), engine wiring (T8), CLI (T9), GUI (T10), ignored smoke (T11), docs+verify (T12).
- **Type consistency:** the seam method is `infer(rendered, params, on_token: &mut dyn FnMut(&str) -> bool) -> Result<()>` everywhere (trait T5, mock T6, real T7). `SamplerSpec` fields `{top_p, temperature, seed}` are consistent T4↔T7. `BackendParams` gains exactly `gguf_path` / `llamacpp_gpu_layers` / `llamacpp_n_ctx`, used identically in T8/T9/T10.
- **The one genuinely fragile spot is Task 7** (llama-cpp-2 0.1.146 exact signatures). It is isolated behind the feature and the seam; if a signature differs, only `real::RealLlamaEngine` changes — the contract and all host tests are unaffected. The implementer note in T7 says to reconcile against `cargo doc`.
- **Default CI stays green** because every default-build step (T2/T3/T4/T6/T8 tests) runs with the feature OFF; the feature-on build/tests (T7/T11, and feature-on clippy in T12) are explicit MacBook steps.
