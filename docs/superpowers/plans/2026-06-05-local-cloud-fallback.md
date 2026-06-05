# Local→Cloud Inference Fallback Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an opt-in `FallbackBackend` so a failing/unavailable local backend (llama.cpp / QNN) transparently falls back to a configured secondary (typically cloud), at startup and at the pre-stream boundary — never mid-stream.

**Architecture:** A decorator (`FallbackBackend`) implementing `InferenceBackend` wraps a primary + secondary `Arc<dyn InferenceBackend>`; it delegates `name()` to the primary (load-bearing for the context-budget heuristic) and falls back only when `primary.generate_stream().await` returns `Err` (the pre-stream boundary). A pure `plan_main_backend` decision function plus a thin `build_main_backend` glue in `primer-engine::wiring` handle startup construction fallback. The `DialogueManager` is unchanged. CLI-first; GUI mirror is a deferred follow-up.

**Tech Stack:** Rust (edition 2024, toolchain 1.88), `async_trait`, `futures` streams, `tokio`, `clap`. Run all cargo commands from `src/` with `~/.cargo/bin/cargo +1.88`.

**Spec:** `docs/superpowers/specs/2026-06-05-local-cloud-fallback-design.md`

---

## File Structure

- **Create** `crates/primer-inference/src/fallback.rs` — `FallbackBackend` decorator + unit tests (mock backends, no network).
- **Modify** `crates/primer-inference/src/lib.rs` — `pub mod fallback;` + `pub use fallback::FallbackBackend;`.
- **Modify** `crates/primer-core/src/consts.rs` — add `pub mod inference { pub const DEFAULT_CLOUD_MODEL: &str = "claude-sonnet-4-6"; }`.
- **Modify** `crates/primer-engine/src/wiring.rs` — add `MainBackendPlan` enum + `plan_main_backend` pure fn + `resolve_fallback_model` pure fn + `build_main_backend` glue + two `BackendParams` fields; update the four in-file test `BackendParams` constructors.
- **Modify** `crates/primer-cli/src/main.rs` — two clap flags (`--fallback-backend`, `--fallback-model`), populate the two `BackendParams` fields (resolving the model), call `build_main_backend`, DRY the cloud-default string to the new const.
- **Modify** `crates/primer-gui/src/wiring.rs` — set the two new `BackendParams` fields to `None` (compile only; GUI fallback wiring is a separate issue).
- **Docs** `CLAUDE.md`, `README.md`, `ROADMAP.md` — reflect the shipped fallback.

All cargo commands run from `/Users/hherb/src/primer/src`.

---

### Task 1: `DEFAULT_CLOUD_MODEL` constant (no magic strings)

**Files:**
- Modify: `crates/primer-core/src/consts.rs`
- Modify: `crates/primer-cli/src/main.rs:849-853` (DRY the inline string)

- [ ] **Step 1: Add the const module to `consts.rs`**

Add near the other `pub mod` blocks (e.g. after the `retry` module, before `vocab`):

```rust
/// Inference-backend defaults.
pub mod inference {
    /// Default Anthropic model id used when the cloud backend is selected
    /// without an explicit model (primary `--model` or `--fallback-model`).
    pub const DEFAULT_CLOUD_MODEL: &str = "claude-sonnet-4-6";
}
```

- [ ] **Step 2: DRY the existing CLI cloud-default to the const**

In `crates/primer-cli/src/main.rs`, the `main_model` resolution currently hardcodes the string. Change the `"cloud"` arm:

```rust
        "cloud" => cli
            .model
            .clone()
            .unwrap_or_else(|| primer_core::consts::inference::DEFAULT_CLOUD_MODEL.to_string()),
```

- [ ] **Step 3: Build to verify it compiles**

Run: `~/.cargo/bin/cargo +1.88 build -p primer-core -p primer-cli`
Expected: compiles clean (the const resolves; CLI still builds).

- [ ] **Step 4: Commit**

```bash
git add crates/primer-core/src/consts.rs crates/primer-cli/src/main.rs
git commit -m "refactor(consts): add inference::DEFAULT_CLOUD_MODEL; DRY CLI cloud default"
```

---

### Task 2: `FallbackBackend` decorator (TDD)

**Files:**
- Create: `crates/primer-inference/src/fallback.rs`
- Modify: `crates/primer-inference/src/lib.rs`
- Test: inline `#[cfg(test)] mod tests` in `fallback.rs`

- [ ] **Step 1: Write the failing tests (create `fallback.rs` with impl stubbed to `unimplemented!`)**

Create `crates/primer-inference/src/fallback.rs`:

```rust
//! Fallback inference backend — an opt-in decorator that serves a primary
//! backend and falls back to a secondary one when the primary fails
//! *before any token streams* (the pre-stream boundary).
//!
//! Trigger policy (see docs/superpowers/specs/2026-06-05-local-cloud-fallback-design.md):
//! - `generate_stream` tries the primary. If `primary.generate_stream().await`
//!   returns `Ok(stream)`, that stream is returned verbatim — a later (mid-stream)
//!   error propagates and the partial turn drops at the dialogue-manager layer,
//!   exactly as without a fallback. **No mid-stream fallback / re-generation.**
//! - If the primary returns `Err` (pre-stream failure), the secondary is tried.
//!
//! Startup/construction fallback (primary fails to *build*) is handled one layer
//! up in `primer-engine::wiring::build_main_backend`, not here.

use async_trait::async_trait;
use std::sync::Arc;

use primer_core::error::Result;
use primer_core::inference::{GenerationParams, InferenceBackend, Prompt, TokenStream};

/// Decorator wrapping a primary and a secondary backend.
pub struct FallbackBackend {
    primary: Arc<dyn InferenceBackend>,
    secondary: Arc<dyn InferenceBackend>,
}

impl FallbackBackend {
    /// Construct a fallback wrapper. `primary` is tried first on every
    /// request; `secondary` serves only when the primary fails pre-stream.
    pub fn new(primary: Arc<dyn InferenceBackend>, secondary: Arc<dyn InferenceBackend>) -> Self {
        Self { primary, secondary }
    }
}

#[async_trait]
impl InferenceBackend for FallbackBackend {
    /// Returns the **primary's** name verbatim. Load-bearing: the per-backend
    /// context budget (`primer_core::backend::is_small_context_backend`,
    /// prefix-anchored on `"qnn:"`) keys off `name()`. The prompt window is
    /// sized once per turn for the common case (primary); a cloud secondary
    /// handles a smaller window fine.
    fn name(&self) -> &str {
        self.primary.name()
    }

    async fn is_available(&self) -> bool {
        self.primary.is_available().await || self.secondary.is_available().await
    }

    async fn generate_stream(
        &self,
        prompt: &Prompt,
        params: &GenerationParams,
    ) -> Result<TokenStream> {
        match self.primary.generate_stream(prompt, params).await {
            Ok(stream) => Ok(stream),
            Err(e) => {
                tracing::warn!(
                    target: "primer::fallback",
                    primary = self.primary.name(),
                    secondary = self.secondary.name(),
                    error = %e,
                    "primary backend failed pre-stream; falling back to secondary"
                );
                self.secondary.generate_stream(prompt, params).await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::{StreamExt, stream};
    use primer_core::error::PrimerError;
    use primer_core::inference::TokenChunk;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// What a `MockBackend` does when `generate_stream` is called.
    #[derive(Clone)]
    enum Behavior {
        /// Pre-stream OK: emit one `done` chunk with this text.
        Ok(String),
        /// Pre-stream error (the `.await` itself returns `Err`).
        PreStreamErr,
        /// Pre-stream OK, but the first stream item is an `Err` (mid-stream).
        MidStreamErr,
    }

    struct MockBackend {
        name: String,
        calls: Arc<AtomicUsize>,
        behavior: Behavior,
    }

    impl MockBackend {
        /// Returns the backend plus a shared call-counter for assertions.
        fn new(name: &str, behavior: Behavior) -> (Arc<Self>, Arc<AtomicUsize>) {
            let calls = Arc::new(AtomicUsize::new(0));
            let b = Arc::new(Self {
                name: name.to_string(),
                calls: calls.clone(),
                behavior,
            });
            (b, calls)
        }
    }

    #[async_trait]
    impl InferenceBackend for MockBackend {
        fn name(&self) -> &str {
            &self.name
        }
        async fn is_available(&self) -> bool {
            !matches!(self.behavior, Behavior::PreStreamErr)
        }
        async fn generate_stream(
            &self,
            _prompt: &Prompt,
            _params: &GenerationParams,
        ) -> Result<TokenStream> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            match &self.behavior {
                Behavior::Ok(text) => {
                    let chunk = TokenChunk { text: text.clone(), done: true };
                    Ok(Box::pin(stream::once(async move { Ok(chunk) })))
                }
                Behavior::PreStreamErr => {
                    Err(PrimerError::Inference("primary pre-stream down".into()))
                }
                Behavior::MidStreamErr => Ok(Box::pin(stream::once(async {
                    Err(PrimerError::Inference("mid-stream boom".into()))
                }))),
            }
        }
    }

    fn prompt() -> Prompt {
        Prompt { system: String::new(), messages: vec![] }
    }

    /// Drive a stream to completion, accumulating text or surfacing the error.
    async fn drive(mut s: TokenStream) -> Result<String> {
        let mut out = String::new();
        while let Some(item) = s.next().await {
            let chunk = item?;
            out.push_str(&chunk.text);
            if chunk.done {
                break;
            }
        }
        Ok(out)
    }

    #[tokio::test]
    async fn primary_ok_uses_primary_and_never_calls_secondary() {
        let (primary, pcalls) = MockBackend::new("llamacpp:m", Behavior::Ok("hi".into()));
        let (secondary, scalls) = MockBackend::new("cloud", Behavior::Ok("SHOULD-NOT".into()));
        let fb = FallbackBackend::new(primary, secondary);
        let out = drive(fb.generate_stream(&prompt(), &GenerationParams::default()).await.unwrap())
            .await
            .unwrap();
        assert_eq!(out, "hi");
        assert_eq!(pcalls.load(Ordering::SeqCst), 1);
        assert_eq!(scalls.load(Ordering::SeqCst), 0, "secondary must not be called");
    }

    #[tokio::test]
    async fn primary_pre_stream_err_falls_back_to_secondary() {
        let (primary, pcalls) = MockBackend::new("llamacpp:m", Behavior::PreStreamErr);
        let (secondary, scalls) = MockBackend::new("cloud", Behavior::Ok("from-cloud".into()));
        let fb = FallbackBackend::new(primary, secondary);
        let out = drive(fb.generate_stream(&prompt(), &GenerationParams::default()).await.unwrap())
            .await
            .unwrap();
        assert_eq!(out, "from-cloud");
        assert_eq!(pcalls.load(Ordering::SeqCst), 1);
        assert_eq!(scalls.load(Ordering::SeqCst), 1, "secondary must serve the turn");
    }

    #[tokio::test]
    async fn mid_stream_err_propagates_and_secondary_not_called() {
        let (primary, pcalls) = MockBackend::new("llamacpp:m", Behavior::MidStreamErr);
        let (secondary, scalls) = MockBackend::new("cloud", Behavior::Ok("SHOULD-NOT".into()));
        let fb = FallbackBackend::new(primary, secondary);
        // generate_stream().await is Ok (pre-stream succeeded); the error appears
        // mid-stream while driving — it must propagate, NOT fall back.
        let stream = fb.generate_stream(&prompt(), &GenerationParams::default()).await.unwrap();
        let result = drive(stream).await;
        assert!(result.is_err(), "mid-stream error must propagate");
        assert_eq!(pcalls.load(Ordering::SeqCst), 1);
        assert_eq!(scalls.load(Ordering::SeqCst), 0, "no mid-stream fallback");
    }

    #[tokio::test]
    async fn both_pre_stream_err_propagates_secondary_error() {
        let (primary, _) = MockBackend::new("llamacpp:m", Behavior::PreStreamErr);
        let (secondary, scalls) = MockBackend::new("cloud", Behavior::PreStreamErr);
        let fb = FallbackBackend::new(primary, secondary);
        let result = fb.generate_stream(&prompt(), &GenerationParams::default()).await;
        assert!(result.is_err(), "both legs down ⇒ error");
        assert_eq!(scalls.load(Ordering::SeqCst), 1, "secondary was attempted");
    }

    #[tokio::test]
    async fn name_returns_primary_name() {
        let (primary, _) = MockBackend::new("qnn:Qwen3-4B", Behavior::Ok("x".into()));
        let (secondary, _) = MockBackend::new("cloud", Behavior::Ok("y".into()));
        let fb = FallbackBackend::new(primary, secondary);
        assert_eq!(fb.name(), "qnn:Qwen3-4B");
    }

    #[tokio::test]
    async fn is_available_true_if_either_leg_available() {
        // primary unavailable (PreStreamErr ⇒ is_available false), secondary ok
        let (primary, _) = MockBackend::new("llamacpp:m", Behavior::PreStreamErr);
        let (secondary, _) = MockBackend::new("cloud", Behavior::Ok("z".into()));
        let fb = FallbackBackend::new(primary, secondary);
        assert!(fb.is_available().await);
    }
}
```

- [ ] **Step 2: Wire the module into the crate**

In `crates/primer-inference/src/lib.rs`, add after the other `pub mod` lines:

```rust
pub mod fallback;
```

and after the other `pub use` lines:

```rust
pub use fallback::FallbackBackend;
```

- [ ] **Step 3: Run the tests to verify they pass**

Run: `~/.cargo/bin/cargo +1.88 test -p primer-inference fallback`
Expected: 6 tests pass (`primary_ok_…`, `primary_pre_stream_err_…`, `mid_stream_err_…`, `both_pre_stream_err_…`, `name_returns_primary_name`, `is_available_…`).

- [ ] **Step 4: Lint**

Run: `~/.cargo/bin/cargo +1.88 clippy -p primer-inference --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/primer-inference/src/fallback.rs crates/primer-inference/src/lib.rs
git commit -m "feat(inference): FallbackBackend decorator (pre-stream fallback, never mid-stream)"
```

---

### Task 3: `plan_main_backend` pure decision function (TDD)

**Files:**
- Modify: `crates/primer-engine/src/wiring.rs`
- Test: inline `#[cfg(test)] mod main_backend_plan_tests` in `wiring.rs`

- [ ] **Step 1: Write the failing tests**

Add to `crates/primer-engine/src/wiring.rs` (near the bottom, before the final `}` of the file is not needed — append a new test module):

```rust
#[cfg(test)]
mod main_backend_plan_tests {
    use super::{MainBackendPlan, plan_main_backend};

    #[test]
    fn primary_ok_no_fallback_is_primary_alone() {
        assert_eq!(plan_main_backend(true, false, false), MainBackendPlan::PrimaryAlone);
        assert_eq!(plan_main_backend(true, false, true), MainBackendPlan::PrimaryAlone);
    }

    #[test]
    fn primary_ok_fallback_built_is_wrapped() {
        assert_eq!(plan_main_backend(true, true, true), MainBackendPlan::Wrapped);
    }

    #[test]
    fn primary_ok_fallback_failed_is_primary_alone() {
        assert_eq!(plan_main_backend(true, true, false), MainBackendPlan::PrimaryAlone);
    }

    #[test]
    fn primary_failed_secondary_built_is_secondary_alone() {
        assert_eq!(plan_main_backend(false, true, true), MainBackendPlan::SecondaryAlone);
    }

    #[test]
    fn primary_failed_secondary_failed_is_fail() {
        assert_eq!(plan_main_backend(false, true, false), MainBackendPlan::Fail);
    }

    #[test]
    fn primary_failed_no_fallback_is_fail() {
        assert_eq!(plan_main_backend(false, false, false), MainBackendPlan::Fail);
        assert_eq!(plan_main_backend(false, false, true), MainBackendPlan::Fail);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail to compile**

Run: `~/.cargo/bin/cargo +1.88 test -p primer-engine main_backend_plan`
Expected: FAIL — `MainBackendPlan` / `plan_main_backend` not found.

- [ ] **Step 3: Implement the enum + pure function**

Add to `crates/primer-engine/src/wiring.rs` (place it just above `build_backend`):

```rust
/// Outcome of the main-backend construction decision. See the truth table
/// in docs/superpowers/specs/2026-06-05-local-cloud-fallback-design.md.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MainBackendPlan {
    /// Use the primary backend alone (no fallback, or fallback unbuildable).
    PrimaryAlone,
    /// Primary failed to build; use the secondary alone (startup fallback).
    SecondaryAlone,
    /// Both built and a fallback is configured; wrap them in `FallbackBackend`.
    Wrapped,
    /// Nothing usable; surface the primary's construction error.
    Fail,
}

/// Pure decision: given whether each leg built and whether a fallback was
/// configured, choose how to assemble the main backend. No I/O.
pub fn plan_main_backend(
    primary_built: bool,
    fallback_configured: bool,
    secondary_built: bool,
) -> MainBackendPlan {
    match (primary_built, fallback_configured, secondary_built) {
        (true, false, _) => MainBackendPlan::PrimaryAlone,
        (true, true, true) => MainBackendPlan::Wrapped,
        (true, true, false) => MainBackendPlan::PrimaryAlone,
        (false, true, true) => MainBackendPlan::SecondaryAlone,
        (false, true, false) => MainBackendPlan::Fail,
        (false, false, _) => MainBackendPlan::Fail,
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `~/.cargo/bin/cargo +1.88 test -p primer-engine main_backend_plan`
Expected: 6 tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/primer-engine/src/wiring.rs
git commit -m "feat(engine): plan_main_backend pure decision fn for construction fallback"
```

---

### Task 4: `BackendParams` fields + `resolve_fallback_model` (TDD)

**Files:**
- Modify: `crates/primer-engine/src/wiring.rs` (struct fields, pure fn, 4 test constructors)
- Test: inline `#[cfg(test)] mod resolve_fallback_model_tests`

- [ ] **Step 1: Add the two `BackendParams` fields**

In `crates/primer-engine/src/wiring.rs`, add to the `BackendParams` struct (after `reasoning_markers`):

```rust
    /// Opt-in fallback secondary backend name (`stub`/`cloud`/`ollama`/
    /// `openai-compat`). `None` ⇒ no fallback ⇒ local-only (the privacy
    /// default). Consumed only by [`build_main_backend`]; the flag's
    /// presence is the explicit cloud-fallback consent.
    pub fallback_backend: Option<String>,
    /// Model for the fallback secondary. Resolution rules live in
    /// [`resolve_fallback_model`]. `None` is valid (cloud defaults; stub
    /// ignores it; ollama/openai-compat error).
    pub fallback_model: Option<String>,
```

- [ ] **Step 2: Update the four in-file test `BackendParams` constructors**

Search `wiring.rs` for every `BackendParams {` literal in `#[cfg(test)]` modules (there are four: `classifier_construction_tests::params_with`, `extractor_construction_tests::params`, `comprehension_construction_tests::params`, `qnn_dispatch_tests::params`). Add these two lines to each, after `reasoning_markers: Vec::new(),`:

```rust
            fallback_backend: None,
            fallback_model: None,
```

- [ ] **Step 3: Write the failing tests for `resolve_fallback_model`**

Append to `crates/primer-engine/src/wiring.rs`:

```rust
#[cfg(test)]
mod resolve_fallback_model_tests {
    use super::resolve_fallback_model;
    use primer_core::consts::inference::DEFAULT_CLOUD_MODEL;

    #[test]
    fn stub_ignores_model() {
        assert_eq!(resolve_fallback_model("stub", None).unwrap(), None);
        assert_eq!(resolve_fallback_model("stub", Some("x".into())).unwrap(), None);
    }

    #[test]
    fn cloud_defaults_when_unset() {
        assert_eq!(
            resolve_fallback_model("cloud", None).unwrap(),
            Some(DEFAULT_CLOUD_MODEL.to_string())
        );
    }

    #[test]
    fn cloud_uses_explicit_model() {
        assert_eq!(
            resolve_fallback_model("cloud", Some("claude-opus-4-7".into())).unwrap(),
            Some("claude-opus-4-7".to_string())
        );
    }

    #[test]
    fn ollama_requires_model() {
        assert!(resolve_fallback_model("ollama", None).is_err());
        assert_eq!(
            resolve_fallback_model("ollama", Some("llama3.2".into())).unwrap(),
            Some("llama3.2".to_string())
        );
    }

    #[test]
    fn openai_compat_requires_model() {
        assert!(resolve_fallback_model("openai-compat", None).is_err());
        assert_eq!(
            resolve_fallback_model("openai-compat", Some("Qwen3-8B".into())).unwrap(),
            Some("Qwen3-8B".to_string())
        );
    }
}
```

- [ ] **Step 4: Run to verify failure**

Run: `~/.cargo/bin/cargo +1.88 test -p primer-engine resolve_fallback_model`
Expected: FAIL — `resolve_fallback_model` not found.

- [ ] **Step 5: Implement `resolve_fallback_model`**

Add to `crates/primer-engine/src/wiring.rs` (just below `plan_main_backend`):

```rust
/// Resolve the fallback secondary's model from its backend name + optional
/// explicit `--fallback-model`. Pure — no I/O.
///
/// - `stub` ⇒ `Ok(None)` (model ignored by the stub backend).
/// - `cloud` ⇒ `Ok(Some(model | DEFAULT_CLOUD_MODEL))`.
/// - `ollama` / `openai-compat` ⇒ model required ⇒ `Err` when `None`.
/// - any other name ⇒ `Ok(model)` passthrough (`build_backend` rejects the
///   unknown name later with its own clear error).
pub fn resolve_fallback_model(
    backend: &str,
    model: Option<String>,
) -> std::result::Result<Option<String>, String> {
    match backend {
        "stub" => Ok(None),
        "cloud" => Ok(Some(
            model.unwrap_or_else(|| {
                primer_core::consts::inference::DEFAULT_CLOUD_MODEL.to_string()
            }),
        )),
        "ollama" | "openai-compat" => model.map(Some).ok_or_else(|| {
            format!("--fallback-model is required when --fallback-backend is {backend}")
        }),
        _ => Ok(model),
    }
}
```

- [ ] **Step 6: Run to verify pass + whole-crate test (the 4 constructor edits must still compile)**

Run: `~/.cargo/bin/cargo +1.88 test -p primer-engine`
Expected: all pass (the new `resolve_fallback_model_tests`, the `main_backend_plan_tests`, and every pre-existing wiring test still green).

- [ ] **Step 7: Commit**

```bash
git add crates/primer-engine/src/wiring.rs
git commit -m "feat(engine): BackendParams fallback fields + resolve_fallback_model pure fn"
```

---

### Task 5: `build_main_backend` glue (TDD)

**Files:**
- Modify: `crates/primer-engine/src/wiring.rs`
- Test: inline `#[cfg(test)] mod build_main_backend_tests`

- [ ] **Step 1: Write the failing tests**

Append to `crates/primer-engine/src/wiring.rs`:

```rust
#[cfg(test)]
mod build_main_backend_tests {
    use super::*;

    fn params(fallback_backend: Option<&str>, fallback_model: Option<&str>) -> BackendParams {
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
            fallback_backend: fallback_backend.map(String::from),
            fallback_model: fallback_model.map(String::from),
        }
    }

    /// No fallback configured ⇒ primary alone (unchanged behavior).
    #[tokio::test]
    async fn no_fallback_returns_primary() {
        let p = params(None, None);
        let b = build_main_backend("stub", "m".into(), &p).await.unwrap();
        assert_eq!(b.name(), "stub");
    }

    /// Primary fails to build, fallback (stub) builds ⇒ secondary alone.
    /// `unknown` is an unbuildable backend name, so the primary leg errors.
    #[tokio::test]
    async fn primary_unbuildable_falls_back_to_secondary() {
        let p = params(Some("stub"), None);
        let b = build_main_backend("unknown-backend", "m".into(), &p)
            .await
            .unwrap();
        // Secondary stub served ⇒ its name surfaces.
        assert_eq!(b.name(), "stub");
    }

    /// Primary builds, fallback fails to build ⇒ primary alone, no error.
    #[tokio::test]
    async fn fallback_unbuildable_keeps_primary() {
        let p = params(Some("unknown-backend"), Some("m"));
        let b = build_main_backend("stub", "m".into(), &p).await.unwrap();
        assert_eq!(b.name(), "stub");
    }

    /// Both unbuildable ⇒ error (the primary's construction error).
    #[tokio::test]
    async fn both_unbuildable_errors() {
        let p = params(Some("unknown-fallback"), Some("m"));
        let r = build_main_backend("unknown-primary", "m".into(), &p).await;
        assert!(r.is_err());
    }

    /// Fallback configured as ollama without a model ⇒ resolve error surfaces.
    #[tokio::test]
    async fn fallback_ollama_without_model_errors() {
        let p = params(Some("ollama"), None);
        let r = build_main_backend("stub", "m".into(), &p).await;
        assert!(r.is_err(), "ollama fallback needs a model");
    }
}
```

Note: because `FallbackBackend::name()` returns the primary's name, the
`Wrapped` happy path is covered structurally by the unit tests in Task 2 (the
decorator), not re-asserted here — these wiring tests pin the *construction
decision* via observable name() differences.

- [ ] **Step 2: Run to verify failure**

Run: `~/.cargo/bin/cargo +1.88 test -p primer-engine build_main_backend`
Expected: FAIL — `build_main_backend` not found.

- [ ] **Step 3: Implement `build_main_backend`**

Add to `crates/primer-engine/src/wiring.rs` (just below `build_backend`). Note the `FallbackBackend` import:

```rust
/// Construct the main inference backend, applying the opt-in construction
/// fallback. When `params.fallback_backend` is `None`, this is exactly
/// `build_backend(primary_name, primary_model, params)`.
///
/// Otherwise it builds both legs and applies [`plan_main_backend`]:
/// - `Wrapped` ⇒ `FallbackBackend { primary, secondary }`;
/// - `SecondaryAlone` ⇒ secondary alone (primary was unavailable at startup);
/// - `PrimaryAlone` ⇒ primary alone (no fallback, or fallback unbuildable);
/// - `Fail` ⇒ the primary's construction error.
pub async fn build_main_backend(
    primary_name: &str,
    primary_model: String,
    params: &BackendParams,
) -> Result<Arc<dyn InferenceBackend>> {
    use primer_inference::FallbackBackend;

    // Warn on a pointless same-backend fallback (still works, just no resilience).
    if let Some(fb) = params.fallback_backend.as_deref() {
        if fb == primary_name {
            tracing::warn!(
                backend = primary_name,
                "--fallback-backend equals --backend; no resilience gain"
            );
        }
    }

    let primary = build_backend(primary_name, primary_model, params).await;

    let Some(fb_name) = params.fallback_backend.clone() else {
        // No fallback configured: today's behavior verbatim.
        return primary;
    };

    // A fallback IS configured. Resolve the secondary model (may error for a
    // required-model backend) and try to build it.
    let fb_model = resolve_fallback_model(&fb_name, params.fallback_model.clone())
        .map_err(|m| PrimerError::Inference(m.into()))?;
    let secondary = build_backend(&fb_name, fb_model.unwrap_or_default(), params).await;

    match plan_main_backend(primary.is_ok(), true, secondary.is_ok()) {
        MainBackendPlan::Wrapped => {
            // Both built — wrap. unwrap() is safe: plan returned Wrapped only
            // because both is_ok() were true.
            let primary = primary.expect("Wrapped implies primary built");
            let secondary = secondary.expect("Wrapped implies secondary built");
            Ok(Arc::new(FallbackBackend::new(primary, secondary)))
        }
        MainBackendPlan::SecondaryAlone => {
            tracing::warn!(
                primary = primary_name,
                secondary = %fb_name,
                "primary backend unavailable at startup; using fallback backend alone"
            );
            secondary
        }
        MainBackendPlan::PrimaryAlone => {
            // Reached only when primary built but secondary did not.
            if let Err(ref e) = secondary {
                tracing::warn!(
                    secondary = %fb_name,
                    error = %e,
                    "fallback backend failed to construct; using primary alone"
                );
            }
            primary
        }
        MainBackendPlan::Fail => {
            // Both failed: surface the primary's (most informative) error.
            primary
        }
    }
}
```

- [ ] **Step 4: Run to verify pass**

Run: `~/.cargo/bin/cargo +1.88 test -p primer-engine build_main_backend`
Expected: 5 tests pass.

- [ ] **Step 5: Full crate test + lint**

Run: `~/.cargo/bin/cargo +1.88 test -p primer-engine && ~/.cargo/bin/cargo +1.88 clippy -p primer-engine --all-targets -- -D warnings`
Expected: all green.

- [ ] **Step 6: Commit**

```bash
git add crates/primer-engine/src/wiring.rs
git commit -m "feat(engine): build_main_backend wires opt-in construction + pre-stream fallback"
```

---

### Task 6: CLI flags + call-site swap + GUI compile-fix

**Files:**
- Modify: `crates/primer-cli/src/main.rs` (2 clap flags, BackendParams fields, call swap)
- Modify: `crates/primer-gui/src/wiring.rs` (2 BackendParams fields = `None`)

- [ ] **Step 1: Add the two clap flags to the CLI `struct Cli`**

In `crates/primer-cli/src/main.rs`, add after the `model` field (around line 84):

```rust
    /// Opt-in fallback inference backend: "stub", "cloud", "ollama", or
    /// "openai-compat". When set, a failing/unavailable primary `--backend`
    /// falls back to this one at startup and before any token streams (never
    /// mid-stream). Absent ⇒ no fallback ⇒ local-only. Setting cloud here is
    /// the explicit consent to send a child's turn to the cloud on fallback.
    #[arg(long)]
    fallback_backend: Option<String>,

    /// Model for `--fallback-backend`. For cloud, defaults to the standard
    /// cloud model; for ollama/openai-compat a model is required; stub ignores
    /// it.
    #[arg(long)]
    fallback_model: Option<String>,
```

- [ ] **Step 2: Populate the two `BackendParams` fields at the CLI construction site**

In `crates/primer-cli/src/main.rs`, in the `BackendParams { … }` literal (ends ~line 940), add after `reasoning_markers: …,`:

```rust
        fallback_backend: cli.fallback_backend.clone(),
        fallback_model: cli.fallback_model.clone(),
```

- [ ] **Step 3: Swap the call site from `build_backend` to `build_main_backend`**

In `crates/primer-cli/src/main.rs`, line 943 is the ONLY code reference to `build_backend` in this file (the other matches are comments), so replace it in the import. Change the import line (line 36) from:

```rust
    BackendParams, IN_MEMORY, build_backend, build_classifier, build_comprehension,
```

to:

```rust
    BackendParams, IN_MEMORY, build_classifier, build_comprehension, build_main_backend,
```

Then update the construction call (~line 942-943):

```rust
    let backend: Arc<dyn InferenceBackend> =
        match build_main_backend(&cli.backend, main_model.clone(), &backend_params).await {
            Ok(b) => b,
            Err(e) => {
                eprintln!("Error constructing backend: {e}");
                std::process::exit(1);
            }
        };
```

- [ ] **Step 4: Fix the GUI `BackendParams` construction to compile**

In `crates/primer-gui/src/wiring.rs`, in the `BackendParams { … }` literal (~line 137), add after `reasoning_markers: …,`:

```rust
        // GUI fallback wiring is a separate follow-up issue; set to None so the
        // GUI keeps today's single-backend behavior. CLI exposes the fallback.
        fallback_backend: None,
        fallback_model: None,
```

- [ ] **Step 5: Build CLI + GUI and verify `--help` shows the flags**

Run: `~/.cargo/bin/cargo +1.88 build -p primer-cli -p primer-gui`
Expected: both compile.

Run: `~/.cargo/bin/cargo +1.88 run -q -p primer-cli -- --help`
Expected: output lists `--fallback-backend <FALLBACK_BACKEND>` and `--fallback-model <FALLBACK_MODEL>`.

- [ ] **Step 6: Manual smoke — startup fallback to stub when the primary is unbuildable**

Run (a deliberately unbuildable primary, falling back to stub, one scripted turn):

```bash
echo "hello" | ~/.cargo/bin/cargo +1.88 run -q -p primer-cli -- \
  --backend ollama --model definitely-not-installed:0b \
  --fallback-backend stub --name Test --age 8 --no-persist
```

Expected: the session does NOT hard-exit on a missing primary; it prints the stub's canned Socratic response (because ollama at the default URL with a bogus model fails pre-stream and the per-turn fallback serves stub). If a local ollama is running and accepts the connection, instead use `--backend openai-compat --openai-compat-url http://127.0.0.1:1 ...` to force a primary connection failure. The point is: a primary failure yields a stub reply, not a crash.

- [ ] **Step 7: Commit**

```bash
git add crates/primer-cli/src/main.rs crates/primer-gui/src/wiring.rs
git commit -m "feat(cli): --fallback-backend/--fallback-model; route through build_main_backend"
```

---

### Task 7: Full verification + docs

**Files:**
- Modify: `CLAUDE.md`, `README.md`, `ROADMAP.md` (repo root)

- [ ] **Step 1: Whole-workspace verify (the standard loop)**

Run from `src/` (format-write first so the reflowed CLI import group is canonical, then check):

```bash
~/.cargo/bin/cargo +1.88 fmt --all
~/.cargo/bin/cargo +1.88 fmt --all -- --check
~/.cargo/bin/cargo +1.88 clippy --workspace --all-targets -- -D warnings
~/.cargo/bin/cargo +1.88 test --workspace --no-fail-fast
```

Expected: all green.

- [ ] **Step 2: Update `ROADMAP.md`**

In the Phase 1.1 section, update bullet (c). Change:

```markdown
- [ ] Automatic local→cloud fallback; 3B fallback path for constrained devices.
```

to:

```markdown
- [x] Automatic local→cloud fallback (opt-in `--fallback-backend`/`--fallback-model`; `FallbackBackend` decorator falls back at startup + pre-stream, never mid-stream; CLI shipped, GUI mirror is a follow-up). 3B constrained-device chain still ahead.
```

And update the `1.1` status emoji line if it reads as fully blocked on this bullet (leave 🟡 — benchmarking bullet 2 is still open).

- [ ] **Step 3: Update `README.md`**

In the user-facing status / inference section, add a sentence noting the opt-in fallback. Find the inference-backends paragraph and append:

```markdown
An optional local→cloud fallback (`--fallback-backend cloud --fallback-model <id>`) keeps a session alive when a local backend is unavailable at startup or fails before streaming a turn; it is off by default so a local-only setup never silently uses the cloud.
```

(Place it next to the existing backend-selection description; match surrounding tone.)

- [ ] **Step 4: Update `CLAUDE.md`**

Add a bullet to the conventions/gotchas list (near the `LlamaCppBackend` / backend notes):

```markdown
- **`FallbackBackend` is an opt-in local→cloud reliability decorator** (`primer-inference/src/fallback.rs`), wired via `primer-engine::wiring::build_main_backend`. It wraps a primary + secondary `Arc<dyn InferenceBackend>`; `name()` returns the **primary's** name verbatim (load-bearing for `is_small_context_backend`). It falls back ONLY at the `generate_stream().await` `Result` boundary (pre-stream) — a mid-stream error propagates and the partial turn drops as usual (never mid-stream, no double-speaking). Startup/construction fallback is the pure `plan_main_backend(primary_built, fallback_configured, secondary_built)` truth table. CLI: `--fallback-backend` + `--fallback-model` (absent ⇒ local-only, the privacy default; the flag's presence is the explicit cloud-fallback consent). The GUI mirror is a deferred follow-up — `primer-gui` sets the two `BackendParams.fallback_*` fields to `None`. NOT the Phase 1.3 router (per-turn complexity/latency routing); the decorator is a building block for it. Subsystem backends (classifier/extractor/comprehension) inherit fallback only when they `Arc::clone` the wrapped main backend — independent subsystem fallback is a deliberate non-goal (they already soft-fail).
```

- [ ] **Step 5: Commit docs**

```bash
git add CLAUDE.md README.md ROADMAP.md
git commit -m "docs: document opt-in local→cloud FallbackBackend (Phase 1.1 bullet c)"
```

- [ ] **Step 6: Push the branch and open the PR**

```bash
git push -u origin feat/local-cloud-fallback
gh pr create --base main --title "feat(inference): opt-in local→cloud fallback (Phase 1.1 bullet c)" \
  --body "$(cat <<'EOF'
Implements Phase 1.1 bullet (c): an opt-in `FallbackBackend` decorator that
serves a failing/unavailable local backend from a configured secondary
(typically cloud), at startup and at the pre-stream boundary — never mid-stream.

- `FallbackBackend` (primer-inference) — decorator; `name()` delegates to the
  primary (context-budget invariant); falls back only on the pre-stream
  `generate_stream().await` `Err`.
- `plan_main_backend` pure truth table + `resolve_fallback_model` + thin
  `build_main_backend` glue (primer-engine).
- CLI `--fallback-backend` / `--fallback-model`; default off ⇒ local-only.
- DialogueManager unchanged. GUI mirror deferred to a follow-up issue.

Spec: docs/superpowers/specs/2026-06-05-local-cloud-fallback-design.md
Plan: docs/superpowers/plans/2026-06-05-local-cloud-fallback.md

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

- [ ] **Step 7: Open the GUI-mirror follow-up issue**

```bash
gh issue create --title "GUI mirror for opt-in local→cloud fallback" --body "$(cat <<'EOF'
CLI shipped the opt-in fallback (`--fallback-backend`/`--fallback-model`) in
the Phase 1.1 bullet (c) PR. Mirror it in the GUI:

- `BackendConfig`/`BackendConfigView`/`BackendConfigUpdate` gain
  `fallback_backend: Option<String>` + `fallback_model: Option<String>`.
  The `Update` DTO has NO `#[serde(default)]`, so `settings.js::gather()`
  MUST send both (nullable) or `update_settings` fails to deserialize.
- `primer-gui/src/wiring.rs::build_with_strategy` calls `build_main_backend`
  instead of `build_backend` (today it sets the two fields to `None`).
- Settings → Inference backend: a "Fallback backend" picker + optional
  fallback-model field; reuse `resolve_main_model`-style resolution.

Spec: docs/superpowers/specs/2026-06-05-local-cloud-fallback-design.md (Scope).
EOF
)"
```

---

## Notes for the implementer

- **Always run cargo from `src/` with `~/.cargo/bin/cargo +1.88`** — the repo pins 1.88 and Homebrew rust will silently use the wrong version (see CLAUDE.md).
- **Run `~/.cargo/bin/cargo +1.88 fmt --all` before every commit.** The repo's optional pre-commit hook runs `fmt --all -- --check` on `.rs` changes; an unformatted import group (e.g. the reflowed CLI `use primer_engine::{…}` block in Task 6) would block the commit otherwise.
- **`PrimerError::Inference` takes `Into<InferenceError>`**; `"msg".into()` and `format!(...).into()` both land in `InferenceError::Other`. That's why `build_main_backend` does `.map_err(|m| PrimerError::Inference(m.into()))`.
- **The four pre-existing `BackendParams` test constructors** in `wiring.rs` MUST get the two new fields or the crate won't compile — Task 4 Step 2 is not optional.
- **Do not touch `DialogueManager`** — the whole point is that it sees `Arc<dyn InferenceBackend>` and never knows a fallback exists.
- **Feature-gated clippy is invisible to `--workspace` clippy.** This change touches no feature-gated code paths (FallbackBackend is always compiled), so the standard `--workspace` clippy in Task 7 is sufficient.
