# Graceful API Errors — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the catch-all `PrimerError::Inference(String)` with a typed `InferenceError` enum, classify common failure modes from both inference backends (Anthropic, Ollama), retry transient conditions silently with bounded jittered backoff, and render user-facing messages through a single i18n-ready boundary. Closes the last open Phase 0.1 ROADMAP bullet.

**Architecture:** Three new pure modules in `primer-core` (`error::InferenceError`, `retry`, `i18n`) provide the data, orchestration, and rendering layers. Each backend gets pure `classify_*` functions that map status codes / transport errors onto `InferenceError`, then wraps the request-send + status-check phase of `generate_stream` in `retry_with_backoff`. The streaming consumer task downstream of that point is untouched (mid-stream errors keep their existing semantics — propagated cleanly, partial Primer turn dropped). The CLI renders via a single `Locale`-aware function; the future i18n PR adds locales as new variants and translation tables without touching backends.

**Tech Stack:** Rust 1.85+ (workspace edition 2024); `thiserror` for error variant ergonomics; `reqwest` for HTTP; `tokio` (with `test-util` for retry tests); `tracing` for both diagnostic warnings and the `primer::retry` event channel that future voice-mode bridging will subscribe to.

**Spec:** [docs/superpowers/specs/2026-05-04-graceful-api-errors-design.md](../specs/2026-05-04-graceful-api-errors-design.md)

**Working directory:** All `cargo` commands run from `/Users/hherb/src/primer/src/`.

**Branch:** `feature/graceful-api-errors` (already created; spec is committed at `79a7ce4`).

---

## Bundle 1 — Error data layer

### Task 1: Create the retry constants module

**Files:**
- Create: `crates/primer-core/src/consts.rs`
- Modify: `crates/primer-core/src/lib.rs`

**Background:** `primer-core` does not currently have a `consts.rs`. Per the no-magic-numbers convention all retry numerics go here; future numeric defaults shared across `primer-core` modules can join later.

- [ ] **Step 1: Create `crates/primer-core/src/consts.rs`**

```rust
//! Default values for invariant numerics shared across primer-core
//! modules. Per the no-magic-numbers convention, every numeric used
//! by primer-core helpers is defined here (or in a sibling settings
//! struct field for tunables).

/// Defaults for the retry helper. Mirrors the per-crate `consts.rs`
/// pattern used by `primer-classifier`, `primer-extractor`, etc.
pub mod retry {
    use std::time::Duration;

    /// Total attempts including the first.
    pub const DEFAULT_MAX_ATTEMPTS: u32 = 3;

    /// Initial backoff delay before the second attempt.
    pub const DEFAULT_BASE_DELAY: Duration = Duration::from_millis(250);

    /// Multiplicative growth factor between attempts (250 → 500 → 1000 ms).
    pub const DEFAULT_BACKOFF_FACTOR: u32 = 2;

    /// Jitter as a fraction of the computed delay (±50 %).
    pub const DEFAULT_JITTER_FRACTION: f32 = 0.5;

    /// Maximum honored Retry-After. Above this we give up immediately
    /// rather than block the conversational hot path.
    pub const DEFAULT_RETRY_AFTER_BUDGET: Duration = Duration::from_secs(5);
}
```

- [ ] **Step 2: Register the module in `crates/primer-core/src/lib.rs`**

Add `pub mod consts;` alphabetically next to the existing `pub mod` lines.

- [ ] **Step 3: Verify build**

```bash
cd /Users/hherb/src/primer/src && ~/.cargo/bin/cargo build -p primer-core
```

Expected: clean build, zero warnings.

- [ ] **Step 4: Stage but do not commit yet** — bundled with Tasks 2 and 3 in one commit.

```bash
git add crates/primer-core/src/consts.rs crates/primer-core/src/lib.rs
```

---

### Task 2: Define `InferenceError` enum

**Files:**
- Modify: `crates/primer-core/src/error.rs`
- Test: `crates/primer-core/src/error.rs` (inline `#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing tests**

Append to `crates/primer-core/src/error.rs`:

```rust
#[cfg(test)]
mod inference_error_tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn is_retryable_truth_table() {
        assert!(!InferenceError::Auth.is_retryable());
        assert!(InferenceError::RateLimited { retry_after: None }.is_retryable());
        assert!(
            InferenceError::RateLimited { retry_after: Some(Duration::from_secs(1)) }
                .is_retryable()
        );
        assert!(InferenceError::ServiceUnavailable.is_retryable());
        assert!(InferenceError::NetworkUnavailable.is_retryable());
        assert!(!InferenceError::ModelNotFound { model: "x".into() }.is_retryable());
        assert!(!InferenceError::Other("anything".into()).is_retryable());
    }

    #[test]
    fn from_string_lands_in_other() {
        let s: String = "boom".into();
        let e: InferenceError = s.into();
        assert!(matches!(e, InferenceError::Other(ref msg) if msg == "boom"));
    }

    #[test]
    fn from_str_lands_in_other() {
        let e: InferenceError = "boom".into();
        assert!(matches!(e, InferenceError::Other(ref msg) if msg == "boom"));
    }

    #[test]
    fn display_produces_dev_facing_english() {
        assert_eq!(format!("{}", InferenceError::Auth), "authentication failed");
        assert_eq!(format!("{}", InferenceError::ServiceUnavailable), "service unavailable");
        assert_eq!(format!("{}", InferenceError::NetworkUnavailable), "network unavailable");
        assert_eq!(
            format!("{}", InferenceError::ModelNotFound { model: "llama3.2".into() }),
            "model not found: llama3.2"
        );
        assert_eq!(
            format!("{}", InferenceError::Other("raw".into())),
            "raw"
        );
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cd /Users/hherb/src/primer/src && ~/.cargo/bin/cargo test -p primer-core inference_error_tests 2>&1 | tail -20
```

Expected: compile error — `InferenceError` undefined.

- [ ] **Step 3: Implement `InferenceError`**

Replace the existing `error.rs` body with:

```rust
//! Unified error types for the Primer system.

use std::time::Duration;
use thiserror::Error;

/// Categorized inference failure. Variant fields are language-neutral
/// data (durations, model names, status data) — never embed user-facing
/// English here. The `Display` impl produces *dev-facing* strings used
/// by `tracing::warn!`, panic messages, and test output. End-user
/// rendering routes through `primer_core::i18n::render_inference_error`.
#[derive(Debug, Clone, Error)]
pub enum InferenceError {
    /// 401 / 403 from a cloud API or other auth-shape failure.
    /// User remedy: check the API key.
    #[error("authentication failed")]
    Auth,

    /// 429 with optional Retry-After. The retry helper respects
    /// `retry_after` up to `RetrySettings.retry_after_budget`; longer
    /// waits are surfaced unchanged for the user to retry manually.
    #[error("rate limited (retry_after={retry_after:?})")]
    RateLimited { retry_after: Option<Duration> },

    /// 5xx from cloud, or daemon-down from Ollama.
    #[error("service unavailable")]
    ServiceUnavailable,

    /// Connect / timeout / DNS / TLS failure at the transport layer.
    #[error("network unavailable")]
    NetworkUnavailable,

    /// Ollama returned 404 for a configured model name. Not produced
    /// by the cloud backend (its missing-model surface differs).
    #[error("model not found: {model}")]
    ModelNotFound { model: String },

    /// Catch-all for unmapped conditions. Dev-facing only — the user
    /// never sees the inner string. Goes to `tracing::warn!` instead.
    #[error("{0}")]
    Other(String),
}

impl InferenceError {
    /// Whether the retry helper should attempt to recover this
    /// condition. `Auth`, `ModelNotFound`, and `Other` are surfaced
    /// immediately; the rest are transient by definition or by
    /// observation (network flap).
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::RateLimited { .. } | Self::ServiceUnavailable | Self::NetworkUnavailable,
        )
    }
}

impl From<&str> for InferenceError {
    fn from(s: &str) -> Self { Self::Other(s.to_string()) }
}

impl From<String> for InferenceError {
    fn from(s: String) -> Self { Self::Other(s) }
}

#[derive(Error, Debug)]
pub enum PrimerError {
    #[error("Inference error: {0}")]
    Inference(InferenceError),

    #[error("Speech error: {0}")]
    Speech(String),

    #[error("Knowledge base error: {0}")]
    Knowledge(String),

    #[error("Learner model error: {0}")]
    LearnerModel(String),

    #[error("Storage error: {0}")]
    Storage(String),

    #[error("Classification error: {0}")]
    Classification(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialisation error: {0}")]
    Serde(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, PrimerError>;

#[cfg(test)]
mod inference_error_tests {
    use super::*;
    use std::time::Duration;

    // ... (the four tests from Step 1, copied verbatim — they go inside this module)
}
```

(Copy the four tests from Step 1 into the `inference_error_tests` module at the end of the file.)

- [ ] **Step 4: Run the tests — they will compile but the rest of the workspace will not yet**

```bash
cd /Users/hherb/src/primer/src && ~/.cargo/bin/cargo test -p primer-core inference_error_tests 2>&1 | tail -20
```

Expected: `4 passed; 0 failed` for the new tests. The rest of the workspace will fail to compile until Task 3 lands the migration; that's expected.

- [ ] **Step 5: Stage but do not commit yet** — combined with Tasks 1 and 3.

```bash
git add crates/primer-core/src/error.rs
```

---

### Task 3: Migrate the 23 `PrimerError::Inference(...)` call sites

**Files:**
- Modify: each of the 23 sites listed below.

**Background:** Every existing call site constructs the variant; none destructure on the inner string (verified during planning). The change is mechanical: each site that passes a `String` (typically `format!(...)`) now needs an `.into()` to coerce to `InferenceError::Other`. Sites that already pass a `&str` followed by `.into()` (`"text".into()`) compile unchanged because `From<&str>` for `InferenceError` exists and resolves the same call.

**Sites that need `.into()` appended** (explicit list — copy-paste-friendly):

| File | Line | Pattern today |
|---|---|---|
| `crates/primer-inference/src/cloud.rs` | 164 | `PrimerError::Inference(format!(...))` |
| `crates/primer-inference/src/cloud.rs` | 180 | `PrimerError::Inference(format!(...))` |
| `crates/primer-inference/src/cloud.rs` | 185 | `Err(PrimerError::Inference(parsed.error.message))` |
| `crates/primer-inference/src/cloud.rs` | 246 | `.map_err(|e| PrimerError::Inference(format!(...)))` |
| `crates/primer-inference/src/cloud.rs` | 251 | `PrimerError::Inference(format!(...))` |
| `crates/primer-inference/src/cloud.rs` | 289 | `.send(Err(PrimerError::Inference(format!(...))))` |
| `crates/primer-inference/src/ollama.rs` | 98 | `PrimerError::Inference(format!(...))` |
| `crates/primer-inference/src/ollama.rs` | 164 | `.map_err(|e| PrimerError::Inference(format!(...)))` |
| `crates/primer-inference/src/ollama.rs` | 169 | `PrimerError::Inference(format!(...))` |
| `crates/primer-inference/src/ollama.rs` | 210 | `.send(Err(PrimerError::Inference(format!(...))))` |
| `crates/primer-classifier/src/llm.rs` | 327 | `Err(primer_core::error::PrimerError::Inference(...))` |
| `crates/primer-comprehension/src/llm.rs` | 481 | `Err(primer_core::error::PrimerError::Inference(...))` |
| `crates/primer-extractor/src/llm.rs` | 323 | `Err(primer_core::error::PrimerError::Inference(...))` |
| `crates/primer-cli/src/main.rs` | 327 | `PrimerError::Inference(format!(...))` |
| `crates/primer-cli/src/speech_loop.rs` | 1687 | `Err(primer_core::error::PrimerError::Inference(...))` |
| `crates/primer-pedagogy/src/dialogue_manager.rs` | 534 | `.map_err(|e| PrimerError::Inference(format!(...)))` |

**Sites that already compile unchanged** (the surrounding `.into()` re-resolves to `From<&str>` / `From<String>` for `InferenceError` — verify but don't edit):

| File | Line |
|---|---|
| `crates/primer-cli/src/main.rs` | 313 (`"...".into()`) |
| `crates/primer-cli/src/main.rs` | 372 (likely `"...".into()`) |
| `crates/primer-cli/src/main.rs` | 447 (likely `"...".into()`) |
| `crates/primer-cli/src/main.rs` | 513 (likely `"...".into()`) |
| `crates/primer-pedagogy/src/dialogue_manager.rs` | 1600 (`"...".into()`) |
| `crates/primer-pedagogy/src/dialogue_manager.rs` | 1694 (`"...".into()`) |
| `crates/primer-pedagogy/src/dialogue_manager.rs` | 3199 (`"...".into()`) |

- [ ] **Step 1: Apply `.into()` to each `format!(...)` site**

For each site in the first table, change

```rust
PrimerError::Inference(format!("Some message: {x}"))
```

to

```rust
PrimerError::Inference(format!("Some message: {x}").into())
```

For `cloud.rs:185` (which passes `parsed.error.message: String` directly) change

```rust
Err(PrimerError::Inference(parsed.error.message))
```

to

```rust
Err(PrimerError::Inference(parsed.error.message.into()))
```

- [ ] **Step 2: Build the workspace to confirm migration is complete**

```bash
cd /Users/hherb/src/primer/src && ~/.cargo/bin/cargo build --workspace 2>&1 | tail -30
```

Expected: clean build. If a site is missed, the compiler points to it with `expected InferenceError, found String`; add `.into()` there.

- [ ] **Step 3: Run the full test suite**

```bash
~/.cargo/bin/cargo test --workspace 2>&1 | grep -E "^test result|FAILED" | head -20
```

Expected: all 328 existing tests still pass + 4 new `InferenceError` tests = 332.

- [ ] **Step 4: Run lints and format check**

```bash
~/.cargo/bin/cargo clippy --workspace --all-targets 2>&1 | tail -10
~/.cargo/bin/cargo fmt --all -- --check
```

Both should pass clean.

- [ ] **Step 5: Commit Bundle 1**

```bash
git add -u crates/
git status   # confirm only the expected files changed
git commit -m "$(cat <<'EOF'
feat(core): add typed InferenceError enum

Replace the catch-all PrimerError::Inference(String) with
PrimerError::Inference(InferenceError) where InferenceError
distinguishes Auth, RateLimited, ServiceUnavailable, NetworkUnavailable,
ModelNotFound, and Other. Variant fields are locale-neutral so a future
i18n PR can attach translation without touching variant shape.

From<&str> and From<String> impls keep existing producers terse — they
fall through to InferenceError::Other. Existing 23 call sites migrate
mechanically by adding .into() on the inner format! result.

is_retryable() classifies which variants the upcoming retry helper will
attempt to recover; Auth, ModelNotFound, and Other surface immediately.

Display produces dev-facing English (logs, tracing); end-user rendering
is a separate concern handled in a later commit.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Bundle 2 — Retry helper

### Task 4: Add `tokio` `test-util` feature for retry tests

**Files:**
- Modify: `crates/primer-core/Cargo.toml`

**Background:** The retry tests use `tokio::time::pause()` for deterministic backoff timing. That requires the `test-util` feature, which is NOT in `tokio`'s `full` feature set.

- [ ] **Step 1: Add a dev-dependencies block to `crates/primer-core/Cargo.toml`**

Append after the existing `[dependencies]` section:

```toml
[dev-dependencies]
tokio = { workspace = true, features = ["test-util", "macros", "rt"] }
```

(Cargo unions features across normal and dev deps, so `test-util` is added on top of the workspace's `full` feature.)

- [ ] **Step 2: Verify the dev-dep is picked up**

```bash
cd /Users/hherb/src/primer/src && ~/.cargo/bin/cargo build -p primer-core --tests 2>&1 | tail -5
```

Expected: clean build.

- [ ] **Step 3: Stage** (combined with Task 5).

```bash
git add crates/primer-core/Cargo.toml
```

---

### Task 5: Implement `RetrySettings` and `retry_with_backoff`

**Files:**
- Create: `crates/primer-core/src/retry.rs`
- Modify: `crates/primer-core/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Create `crates/primer-core/src/retry.rs` with the test module first:

```rust
//! Bounded retry helper for transient `InferenceError` conditions.
//!
//! The helper is locale-neutral and HTTP-neutral — it accepts any
//! closure returning `Result<T, InferenceError>` and dispatches on
//! `InferenceError::is_retryable`. Each retry decision emits a
//! `tracing::info!` event on the `primer::retry` target so a future
//! voice-mode change can subscribe and play a bridging message.
//!
//! Defaults live in `primer_core::consts::retry`.

use crate::consts::retry as defaults;
use crate::error::InferenceError;
use std::future::Future;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct RetrySettings {
    /// Total attempts including the first.
    pub max_attempts: u32,
    /// Initial backoff before the second attempt.
    pub base_delay: Duration,
    /// Multiplicative growth factor between attempts.
    pub backoff_factor: u32,
    /// Jitter as a fraction of the computed delay (±jitter_fraction).
    pub jitter_fraction: f32,
    /// Cap on Retry-After we will honor. Longer waits surface immediately.
    pub retry_after_budget: Duration,
}

impl Default for RetrySettings {
    fn default() -> Self {
        Self {
            max_attempts: defaults::DEFAULT_MAX_ATTEMPTS,
            base_delay: defaults::DEFAULT_BASE_DELAY,
            backoff_factor: defaults::DEFAULT_BACKOFF_FACTOR,
            jitter_fraction: defaults::DEFAULT_JITTER_FRACTION,
            retry_after_budget: defaults::DEFAULT_RETRY_AFTER_BUDGET,
        }
    }
}

/// Compute the delay before the (attempt+1)-th try, given the previous
/// attempt index (0-based). Pure — no I/O, no time.
///
/// `delay = base_delay * backoff_factor^attempt * (1 + jitter * uniform(-1, 1))`.
/// `jitter_seed` lets tests pin the random component.
fn compute_delay(
    settings: &RetrySettings,
    attempt: u32,
    jitter_seed: f32,  // -1.0..=1.0
) -> Duration {
    let factor = settings.backoff_factor.saturating_pow(attempt) as u128;
    let base_ms = settings.base_delay.as_millis() * factor;
    let jitter_ms = (base_ms as f32) * settings.jitter_fraction * jitter_seed;
    let total = (base_ms as i128 + jitter_ms as i128).max(0) as u64;
    Duration::from_millis(total)
}

pub async fn retry_with_backoff<T, F, Fut>(
    settings: &RetrySettings,
    mut op: F,
) -> Result<T, InferenceError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, InferenceError>>,
{
    let mut attempt: u32 = 0;
    loop {
        match op().await {
            Ok(t) => return Ok(t),
            Err(e) => {
                let attempts_left = settings.max_attempts.saturating_sub(attempt + 1);
                if !e.is_retryable() || attempts_left == 0 {
                    return Err(e);
                }
                let delay = match &e {
                    InferenceError::RateLimited { retry_after: Some(d) } => {
                        if *d > settings.retry_after_budget {
                            return Err(e);
                        }
                        *d
                    }
                    _ => {
                        // Pseudo-random uniform(-1, 1). For Phase 0.1 we use
                        // a cheap LCG seeded by attempt; replace with rand
                        // crate if a stronger source is ever needed.
                        let seed = jitter_seed_for(attempt);
                        compute_delay(settings, attempt, seed)
                    }
                };
                tracing::info!(
                    target: "primer::retry",
                    attempt,
                    kind = %e,
                    delay_ms = delay.as_millis() as u64,
                    "retrying inference call",
                );
                tokio::time::sleep(delay).await;
                attempt += 1;
            }
        }
    }
}

/// Cheap deterministic-ish jitter source. Returns a value in `[-1, 1]`
/// derived from the attempt index — sufficient for spreading retries
/// without inviting a `rand` workspace dep for a single helper.
fn jitter_seed_for(attempt: u32) -> f32 {
    // Take a time-derived nibble so different sessions don't lock-step.
    let now_ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let mixed = now_ns.wrapping_mul(1103515245).wrapping_add(attempt);
    let unit = (mixed % 2001) as f32 / 1000.0; // [0, 2]
    unit - 1.0                                  // [-1, 1]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::Duration;

    fn run_async<Fut: Future<Output = T>, T>(fut: Fut) -> T {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .start_paused(true)
            .build()
            .unwrap()
            .block_on(fut)
    }

    fn settings_for_test() -> RetrySettings {
        RetrySettings {
            max_attempts: 3,
            base_delay: Duration::from_millis(100),
            backoff_factor: 2,
            jitter_fraction: 0.0, // deterministic — no jitter in tests
            retry_after_budget: Duration::from_secs(2),
        }
    }

    #[test]
    fn succeeds_on_first_attempt_with_one_call() {
        let calls = Rc::new(RefCell::new(0u32));
        let calls_for_op = calls.clone();
        let settings = settings_for_test();

        let result: Result<&'static str, InferenceError> = run_async(async move {
            retry_with_backoff(&settings, || {
                let calls_for_op = calls_for_op.clone();
                async move {
                    *calls_for_op.borrow_mut() += 1;
                    Ok("ok")
                }
            }).await
        });

        assert_eq!(result.unwrap(), "ok");
        assert_eq!(*calls.borrow(), 1);
    }

    #[test]
    fn succeeds_on_third_attempt() {
        let calls = Rc::new(RefCell::new(0u32));
        let calls_for_op = calls.clone();
        let settings = settings_for_test();

        let result: Result<&'static str, InferenceError> = run_async(async move {
            retry_with_backoff(&settings, || {
                let calls_for_op = calls_for_op.clone();
                async move {
                    let mut n = calls_for_op.borrow_mut();
                    *n += 1;
                    if *n < 3 {
                        Err(InferenceError::ServiceUnavailable)
                    } else {
                        Ok("ok")
                    }
                }
            }).await
        });

        assert_eq!(result.unwrap(), "ok");
        assert_eq!(*calls.borrow(), 3);
    }

    #[test]
    fn exhausts_attempts_then_surfaces_last_error() {
        let calls = Rc::new(RefCell::new(0u32));
        let calls_for_op = calls.clone();
        let settings = settings_for_test();

        let result: Result<(), InferenceError> = run_async(async move {
            retry_with_backoff(&settings, || {
                let calls_for_op = calls_for_op.clone();
                async move {
                    *calls_for_op.borrow_mut() += 1;
                    Err(InferenceError::ServiceUnavailable)
                }
            }).await
        });

        assert!(matches!(result, Err(InferenceError::ServiceUnavailable)));
        assert_eq!(*calls.borrow(), 3); // max_attempts
    }

    #[test]
    fn non_retryable_surfaces_immediately() {
        let calls = Rc::new(RefCell::new(0u32));
        let calls_for_op = calls.clone();
        let settings = settings_for_test();

        let result: Result<(), InferenceError> = run_async(async move {
            retry_with_backoff(&settings, || {
                let calls_for_op = calls_for_op.clone();
                async move {
                    *calls_for_op.borrow_mut() += 1;
                    Err(InferenceError::Auth)
                }
            }).await
        });

        assert!(matches!(result, Err(InferenceError::Auth)));
        assert_eq!(*calls.borrow(), 1); // no retry
    }

    #[test]
    fn respects_retry_after_within_budget() {
        let calls = Rc::new(RefCell::new(0u32));
        let calls_for_op = calls.clone();
        let settings = settings_for_test(); // budget = 2 s

        let result: Result<&'static str, InferenceError> = run_async(async move {
            retry_with_backoff(&settings, || {
                let calls_for_op = calls_for_op.clone();
                async move {
                    let mut n = calls_for_op.borrow_mut();
                    *n += 1;
                    if *n == 1 {
                        Err(InferenceError::RateLimited {
                            retry_after: Some(Duration::from_secs(1)),
                        })
                    } else {
                        Ok("ok")
                    }
                }
            }).await
        });

        assert_eq!(result.unwrap(), "ok");
        assert_eq!(*calls.borrow(), 2);
    }

    #[test]
    fn gives_up_when_retry_after_exceeds_budget() {
        let calls = Rc::new(RefCell::new(0u32));
        let calls_for_op = calls.clone();
        let settings = settings_for_test(); // budget = 2 s

        let result: Result<(), InferenceError> = run_async(async move {
            retry_with_backoff(&settings, || {
                let calls_for_op = calls_for_op.clone();
                async move {
                    *calls_for_op.borrow_mut() += 1;
                    Err(InferenceError::RateLimited {
                        retry_after: Some(Duration::from_secs(30)),
                    })
                }
            }).await
        });

        assert!(matches!(
            result,
            Err(InferenceError::RateLimited { retry_after: Some(d) }) if d == Duration::from_secs(30)
        ));
        assert_eq!(*calls.borrow(), 1); // gave up immediately
    }

    #[test]
    fn settings_default_matches_consts() {
        let s = RetrySettings::default();
        assert_eq!(s.max_attempts, defaults::DEFAULT_MAX_ATTEMPTS);
        assert_eq!(s.base_delay, defaults::DEFAULT_BASE_DELAY);
        assert_eq!(s.backoff_factor, defaults::DEFAULT_BACKOFF_FACTOR);
        assert_eq!(s.jitter_fraction, defaults::DEFAULT_JITTER_FRACTION);
        assert_eq!(s.retry_after_budget, defaults::DEFAULT_RETRY_AFTER_BUDGET);
    }

    #[test]
    fn compute_delay_grows_with_attempt() {
        let s = settings_for_test();
        let d0 = compute_delay(&s, 0, 0.0);
        let d1 = compute_delay(&s, 1, 0.0);
        let d2 = compute_delay(&s, 2, 0.0);
        assert_eq!(d0, Duration::from_millis(100));
        assert_eq!(d1, Duration::from_millis(200));
        assert_eq!(d2, Duration::from_millis(400));
    }
}
```

- [ ] **Step 2: Register the module**

Edit `crates/primer-core/src/lib.rs` and add `pub mod retry;` next to the existing `pub mod` lines (alphabetically near `pub mod knowledge;`).

- [ ] **Step 3: Run the tests**

```bash
cd /Users/hherb/src/primer/src && ~/.cargo/bin/cargo test -p primer-core retry 2>&1 | tail -15
```

Expected: 8 passed, 0 failed.

- [ ] **Step 4: Lint and format**

```bash
~/.cargo/bin/cargo clippy -p primer-core --all-targets 2>&1 | tail -10
~/.cargo/bin/cargo fmt -p primer-core
```

Both clean.

- [ ] **Step 5: Commit Bundle 2**

```bash
git add crates/primer-core/src/retry.rs crates/primer-core/src/lib.rs crates/primer-core/Cargo.toml
git commit -m "$(cat <<'EOF'
feat(core): add retry_with_backoff helper

Bounded jittered exponential backoff for InferenceError conditions
flagged is_retryable. Honors Retry-After when ≤ retry_after_budget;
gives up immediately on longer waits rather than blocking the
conversational hot path. Emits tracing::info events on the
"primer::retry" target so a future voice-mode change can subscribe
and play a "give me a moment" bridging phrase between attempts.

Defaults live in primer_core::consts::retry. Worst-case latency
before failure with defaults: ~1.75 s including jitter.

Tests use tokio::time::start_paused for deterministic backoff —
added test-util to primer-core dev-deps.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Bundle 3 — i18n render boundary

### Task 6: Create `primer-core::i18n` with `Locale` and `render_inference_error`

**Files:**
- Create: `crates/primer-core/src/i18n.rs`
- Modify: `crates/primer-core/src/lib.rs`

- [ ] **Step 1: Write the failing tests** (inline in the new file's `#[cfg(test)] mod tests`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::InferenceError;
    use std::time::Duration;

    #[test]
    fn locale_default_is_english() {
        assert_eq!(Locale::default(), Locale::English);
    }

    #[test]
    fn auth_message_mentions_api_key_env_var() {
        let s = render_inference_error(&InferenceError::Auth, &Locale::English);
        assert!(s.contains("ANTHROPIC_API_KEY"), "got: {s}");
    }

    #[test]
    fn rate_limited_with_retry_after_includes_seconds() {
        let s = render_inference_error(
            &InferenceError::RateLimited { retry_after: Some(Duration::from_secs(7)) },
            &Locale::English,
        );
        assert!(s.contains("7"), "rendered string should mention the seconds: {s}");
    }

    #[test]
    fn rate_limited_without_retry_after_uses_generic_phrasing() {
        let s = render_inference_error(
            &InferenceError::RateLimited { retry_after: None },
            &Locale::English,
        );
        assert!(s.to_lowercase().contains("busy") || s.to_lowercase().contains("moment"),
                "got: {s}");
    }

    #[test]
    fn service_unavailable_message_suggests_try_again() {
        let s = render_inference_error(&InferenceError::ServiceUnavailable, &Locale::English);
        assert!(s.to_lowercase().contains("try again"), "got: {s}");
    }

    #[test]
    fn network_unavailable_message_mentions_network() {
        let s = render_inference_error(&InferenceError::NetworkUnavailable, &Locale::English);
        assert!(s.to_lowercase().contains("network") || s.to_lowercase().contains("connect"),
                "got: {s}");
    }

    #[test]
    fn model_not_found_includes_model_name_and_pull_hint() {
        let s = render_inference_error(
            &InferenceError::ModelNotFound { model: "llama3.2".into() },
            &Locale::English,
        );
        assert!(s.contains("llama3.2"), "got: {s}");
        assert!(s.to_lowercase().contains("pull"), "got: {s}");
    }

    #[test]
    fn other_does_not_leak_inner_dev_string() {
        let s = render_inference_error(
            &InferenceError::Other("RAW_DEV_STRING_FOO".into()),
            &Locale::English,
        );
        assert!(!s.contains("RAW_DEV_STRING_FOO"),
                "Other's inner string must not reach users; got: {s}");
    }
}
```

- [ ] **Step 2: Implement `Locale` and `render_inference_error`**

Prepend the implementation to `crates/primer-core/src/i18n.rs` so the file ends up with the `use` statements + types + tests:

```rust
//! User-facing message rendering.
//!
//! This module is the single i18n boundary for the inference-error
//! surface. Variant authors keep `InferenceError` fields locale-neutral
//! (durations, model-name strings, status data); this function
//! translates them into a human-readable message for the configured
//! `Locale`.
//!
//! Phase 0.1 ships only `Locale::English`. Adding locales is a future
//! PR: append variants here and add corresponding match arms in
//! `render_inference_error` (or replace the body with a fluent / gettext
//! lookup). The function signature is the stable contract.
//!
//! **i18n contract (load-bearing — do not violate):**
//!   - This is the ONLY place user-facing English appears in the
//!     inference error path. Producers (`classify_*`) emit data; this
//!     function translates data to text.
//!   - The `Other` variant is dev-facing — its inner string is NEVER
//!     rendered to users. We surface a generic localized message and
//!     route the inner string to `tracing::warn!` at the call site.

use crate::error::InferenceError;

/// User-facing locale.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Locale {
    #[default]
    English,
}

/// Translate an `InferenceError` into a user-facing message in the
/// requested `Locale`. See module docs for the i18n contract.
pub fn render_inference_error(err: &InferenceError, locale: &Locale) -> String {
    match locale {
        Locale::English => render_english(err),
    }
}

fn render_english(err: &InferenceError) -> String {
    use InferenceError::*;
    match err {
        Auth =>
            "Authentication failed. Please check your ANTHROPIC_API_KEY in .env or ~/.primer_env.".into(),
        RateLimited { retry_after: Some(d) } =>
            format!("The service is busy right now. Please try again in {} seconds.", d.as_secs()),
        RateLimited { retry_after: None } =>
            "The service is busy right now. Please try again in a moment.".into(),
        ServiceUnavailable =>
            "The service is temporarily unavailable. Please try again in a moment.".into(),
        NetworkUnavailable =>
            "Cannot reach the service. Please check your network connection.".into(),
        ModelNotFound { model } =>
            format!("Model '{model}' is not available. For Ollama, run `ollama pull {model}`."),
        Other(_) =>
            "Something unexpected went wrong. Please try again. (See logs for details.)".into(),
    }
}

// ... (the test module from Step 1 goes below)
```

- [ ] **Step 3: Register the module**

Edit `crates/primer-core/src/lib.rs` to add `pub mod i18n;` next to the other `pub mod` lines.

- [ ] **Step 4: Run the tests**

```bash
cd /Users/hherb/src/primer/src && ~/.cargo/bin/cargo test -p primer-core i18n 2>&1 | tail -15
```

Expected: 8 passed, 0 failed.

- [ ] **Step 5: Lint and format**

```bash
~/.cargo/bin/cargo clippy -p primer-core --all-targets 2>&1 | tail -5
~/.cargo/bin/cargo fmt -p primer-core
```

- [ ] **Step 6: Commit Bundle 3**

```bash
git add crates/primer-core/src/i18n.rs crates/primer-core/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(core): add i18n::render_inference_error boundary

The single place user-facing inference-error English lives. Locale enum
ships with one variant (English) today; future i18n PR adds variants
and (or) swaps the match arms for a fluent / gettext lookup. The
function signature is the stable contract.

Other(String)'s inner dev-string is explicitly not rendered to users;
the test asserts a known dev marker doesn't reach the output.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Bundle 4 — Cloud backend integration

### Task 7: Add the cloud `classify_*` pure functions

**Files:**
- Modify: `crates/primer-inference/src/cloud.rs`
- Modify: `crates/primer-inference/Cargo.toml` (verify it depends on `primer-core` already)

- [ ] **Step 1: Write the failing tests**

Append to the `#[cfg(test)] mod tests` block in `crates/primer-inference/src/cloud.rs`:

```rust
mod classify_tests {
    use super::*;
    use primer_core::error::InferenceError;
    use reqwest::header::{HeaderMap, HeaderValue};
    use reqwest::StatusCode;
    use std::time::Duration;

    fn empty_headers() -> HeaderMap { HeaderMap::new() }

    fn headers_with_retry_after(seconds: u64) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("retry-after", HeaderValue::from(seconds));
        h
    }

    #[test]
    fn classifies_401_as_auth() {
        let e = classify_anthropic_status(StatusCode::UNAUTHORIZED, &empty_headers(), "");
        assert!(matches!(e, InferenceError::Auth));
    }

    #[test]
    fn classifies_403_as_auth() {
        let e = classify_anthropic_status(StatusCode::FORBIDDEN, &empty_headers(), "");
        assert!(matches!(e, InferenceError::Auth));
    }

    #[test]
    fn classifies_429_with_retry_after_header() {
        let e = classify_anthropic_status(
            StatusCode::TOO_MANY_REQUESTS,
            &headers_with_retry_after(7),
            "rate limited",
        );
        match e {
            InferenceError::RateLimited { retry_after: Some(d) } =>
                assert_eq!(d, Duration::from_secs(7)),
            other => panic!("expected RateLimited(Some(7s)), got {other:?}"),
        }
    }

    #[test]
    fn classifies_429_without_retry_after_header() {
        let e = classify_anthropic_status(
            StatusCode::TOO_MANY_REQUESTS,
            &empty_headers(),
            "",
        );
        assert!(matches!(e, InferenceError::RateLimited { retry_after: None }));
    }

    #[test]
    fn classifies_500_as_service_unavailable() {
        let e = classify_anthropic_status(StatusCode::INTERNAL_SERVER_ERROR, &empty_headers(), "");
        assert!(matches!(e, InferenceError::ServiceUnavailable));
    }

    #[test]
    fn classifies_503_as_service_unavailable() {
        let e = classify_anthropic_status(StatusCode::SERVICE_UNAVAILABLE, &empty_headers(), "");
        assert!(matches!(e, InferenceError::ServiceUnavailable));
    }

    #[test]
    fn classifies_unknown_4xx_as_other_with_status_in_body() {
        let e = classify_anthropic_status(StatusCode::IM_A_TEAPOT, &empty_headers(), "tea");
        match e {
            InferenceError::Other(s) => assert!(s.contains("418") && s.contains("tea"), "got: {s}"),
            other => panic!("expected Other, got {other:?}"),
        }
    }

    #[test]
    fn parse_retry_after_handles_integer_seconds() {
        let h = headers_with_retry_after(42);
        assert_eq!(parse_retry_after(&h), Some(Duration::from_secs(42)));
    }

    #[test]
    fn parse_retry_after_handles_missing_header() {
        assert_eq!(parse_retry_after(&empty_headers()), None);
    }

    #[test]
    fn parse_retry_after_drops_http_date_form() {
        // HTTP-date form is silently dropped (returns None) rather than
        // attempting to parse the date — see spec §Retry helper.
        let mut h = HeaderMap::new();
        h.insert("retry-after", HeaderValue::from_static("Wed, 21 Oct 2026 07:28:00 GMT"));
        assert_eq!(parse_retry_after(&h), None);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cd /Users/hherb/src/primer/src && ~/.cargo/bin/cargo test -p primer-inference classify_tests 2>&1 | tail -15
```

Expected: compile error — functions undefined.

- [ ] **Step 3: Implement `classify_anthropic_status`, `parse_retry_after`, and `classify_reqwest_error`**

Add these three functions to `crates/primer-inference/src/cloud.rs` near the top of the file (after the `use` lines, before `pub struct CloudBackend`):

```rust
/// Translate a non-success Anthropic HTTP response into an
/// `InferenceError`. Pure — no I/O, no `Self` reference.
///
/// `body` is the response body text used only as the dev-facing payload
/// of `InferenceError::Other`; users never see it (the i18n render layer
/// substitutes a generic message for `Other`).
pub(crate) fn classify_anthropic_status(
    status: reqwest::StatusCode,
    headers: &reqwest::header::HeaderMap,
    body: &str,
) -> primer_core::error::InferenceError {
    use primer_core::error::InferenceError::*;
    match status.as_u16() {
        401 | 403 => Auth,
        429 => RateLimited { retry_after: parse_retry_after(headers) },
        500..=599 => ServiceUnavailable,
        _ => Other(format!("Anthropic returned {status}: {body}")),
    }
}

/// Parse a `Retry-After` header in integer-seconds form. The
/// HTTP-date form is silently dropped (returns `None`) — Anthropic
/// uses the integer-seconds form in practice and parsing the date
/// form would cost a `chrono`/`httpdate` dep for a rare case.
pub(crate) fn parse_retry_after(
    headers: &reqwest::header::HeaderMap,
) -> Option<std::time::Duration> {
    headers.get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .map(std::time::Duration::from_secs)
}

/// Translate a transport-level `reqwest::Error` (no HTTP response yet)
/// into an `InferenceError`. Connect / timeout / request-build failures
/// map to `NetworkUnavailable`; everything else falls through to
/// `Other`. Used by both backend modules — kept in this file because
/// it has no Anthropic-specific knowledge but is small enough that
/// a dedicated shared module is overkill.
pub(crate) fn classify_reqwest_error(e: &reqwest::Error) -> primer_core::error::InferenceError {
    if e.is_connect() || e.is_timeout() || e.is_request() {
        primer_core::error::InferenceError::NetworkUnavailable
    } else {
        primer_core::error::InferenceError::Other(format!("reqwest: {e}"))
    }
}
```

- [ ] **Step 4: Run the new tests**

```bash
~/.cargo/bin/cargo test -p primer-inference classify_tests 2>&1 | tail -15
```

Expected: 10 passed, 0 failed.

- [ ] **Step 5: Stage** (combined with Task 8).

```bash
git add crates/primer-inference/src/cloud.rs
```

---

### Task 8: Wire `CloudBackend::generate_stream` to use `retry_with_backoff`

**Files:**
- Modify: `crates/primer-inference/src/cloud.rs`

**Background:** The retry boundary is the request-send + status-check phase only. The streaming body consumer downstream is untouched.

- [ ] **Step 1: Add a `retry_settings` field to `CloudBackend`**

Replace the `CloudBackend` struct with:

```rust
pub struct CloudBackend {
    client: reqwest::Client,
    api_endpoint: String,
    api_key: String,
    model: String,
    retry_settings: primer_core::retry::RetrySettings,
}

impl CloudBackend {
    pub fn new(api_endpoint: String, api_key: String, model: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_endpoint,
            api_key,
            model,
            retry_settings: primer_core::retry::RetrySettings::default(),
        }
    }
}
```

- [ ] **Step 2: Restructure `generate_stream` to use the retry helper**

Replace the existing `generate_stream` method body (the part from `let api_messages = ...` through the `if !status.is_success() { ... return Err(...) }` block; keep everything from `let (mut tx, rx) = ...` onward) with:

```rust
async fn generate_stream(
    &self,
    prompt: &Prompt,
    params: &GenerationParams,
) -> Result<TokenStream> {
    let api_messages: Vec<ApiMessage> = prompt
        .messages
        .iter()
        .map(|m| ApiMessage {
            role: match m.role {
                Role::System => "user".to_string(),
                Role::User => "user".to_string(),
                Role::Assistant => "assistant".to_string(),
            },
            content: m.content.clone(),
        })
        .collect();

    let request = ApiRequest {
        model: self.model.clone(),
        max_tokens: params.max_tokens,
        system: prompt.system.clone(),
        messages: api_messages,
        temperature: params.temperature,
        stop_sequences: params.stop_sequences.clone(),
        stream: true,
    };

    // Retry the send + status-check phase only. Once we have a 2xx
    // response and start consuming the byte stream, mid-stream errors
    // propagate as before (the partial Primer turn is dropped at a
    // higher layer).
    let response = primer_core::retry::retry_with_backoff(
        &self.retry_settings,
        || async {
            let resp = self
                .client
                .post(format!("{}/v1/messages", self.api_endpoint))
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .header("accept", "text/event-stream")
                .json(&request)
                .send()
                .await
                .map_err(|e| classify_reqwest_error(&e))?;
            let status = resp.status();
            if !status.is_success() {
                let headers = resp.headers().clone();
                let body = resp.text().await.unwrap_or_default();
                return Err(classify_anthropic_status(status, &headers, &body));
            }
            Ok(resp)
        },
    )
    .await
    .map_err(PrimerError::Inference)?;

    let (mut tx, rx) = mpsc::unbounded::<Result<TokenChunk>>();
    let mut bytes_stream = response.bytes_stream();

    // Fire-and-forget streaming consumer. Mid-stream errors continue
    // to surface as PrimerError::Inference(InferenceError::Other(...))
    // via the From<String> impl on the existing format! sites; we
    // intentionally do not attempt to retry mid-stream.
    tokio::spawn(async move {
        let mut buf = SseBuffer::new();
        'outer: loop {
            match bytes_stream.next().await {
                Some(Ok(bytes)) => {
                    buf.extend(&bytes);
                    while let Some(ev) = buf.next_event() {
                        match parse_anthropic_event(&ev) {
                            Ok(Some(chunk)) => {
                                let done = chunk.done;
                                if tx.send(Ok(chunk)).await.is_err() {
                                    break 'outer;
                                }
                                if done {
                                    break 'outer;
                                }
                            }
                            Ok(None) => {}
                            Err(e) => {
                                let _ = tx.send(Err(e)).await;
                                break 'outer;
                            }
                        }
                    }
                }
                Some(Err(e)) => {
                    let _ = tx
                        .send(Err(PrimerError::Inference(
                            format!("Anthropic byte stream error: {e}").into(),
                        )))
                        .await;
                    break 'outer;
                }
                None => break 'outer,
            }
        }
    });

    Ok(Box::pin(rx))
}
```

Note: the `request` struct is built once outside the closure. Inside the closure we borrow `&request`, `&self.client`, `&self.api_endpoint`, `&self.api_key` — the closure does not move them. This works because `retry_with_backoff` takes `mut op: F` and the produced future borrows from the enclosing scope.

If the borrow checker complains about lifetimes (the closure needs to outlive each future), an alternative is `let api_endpoint = &self.api_endpoint;` etc bound before the closure — but try the natural form first.

- [ ] **Step 3: Add `Clone` to `ApiRequest`**

If the borrow approach fails, fall back to cloning `request` inside the closure. Add `#[derive(Clone, ...)]` to `ApiRequest` and use `request.clone()` inside. The current `ApiRequest` derives `Debug, Serialize`; add `Clone` to the derive list. Check whether borrowing works first; only do this fallback if needed.

- [ ] **Step 4: Build and run the existing CloudBackend tests**

```bash
cd /Users/hherb/src/primer/src && ~/.cargo/bin/cargo test -p primer-inference 2>&1 | tail -15
```

Expected: all existing cloud tests pass + the 10 new `classify_tests` tests pass.

- [ ] **Step 5: Lint and format**

```bash
~/.cargo/bin/cargo clippy -p primer-inference --all-targets 2>&1 | tail -5
~/.cargo/bin/cargo fmt -p primer-inference
```

- [ ] **Step 6: Commit Bundle 4**

```bash
git add crates/primer-inference/src/cloud.rs
git commit -m "$(cat <<'EOF'
feat(cloud): typed errors and bounded retry

CloudBackend::generate_stream now classifies request-send and
status-check failures into typed InferenceError variants and retries
transient ones (RateLimited / ServiceUnavailable / NetworkUnavailable)
via primer_core::retry::retry_with_backoff. Auth and unknown
status codes surface immediately.

The streaming body consumer downstream is untouched: mid-stream
errors continue to propagate cleanly and the partial Primer turn
drops at the dialogue-manager layer.

Three new pure helpers (classify_anthropic_status, parse_retry_after,
classify_reqwest_error) — unit-tested with synthetic status codes
and headers, no live HTTP needed. classify_reqwest_error will be
shared with the Ollama backend in the next commit.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Bundle 5 — Ollama backend integration

### Task 9: Add the Ollama `classify_ollama_status` pure function and wire retry

**Files:**
- Modify: `crates/primer-inference/src/ollama.rs`

**Background:** `classify_reqwest_error` is reused verbatim from `cloud.rs` — keep it `pub(crate)` and import it from the sibling module.

- [ ] **Step 1: Write the failing tests**

Append to the existing `#[cfg(test)] mod tests` block in `crates/primer-inference/src/ollama.rs`:

```rust
mod classify_ollama_tests {
    use super::*;
    use primer_core::error::InferenceError;
    use reqwest::StatusCode;

    #[test]
    fn classifies_404_with_model_not_found_body() {
        let body = r#"{"error":"model 'llama3.2' not found, try pulling it first"}"#;
        let e = classify_ollama_status(StatusCode::NOT_FOUND, body, "llama3.2");
        match e {
            InferenceError::ModelNotFound { model } => assert_eq!(model, "llama3.2"),
            other => panic!("expected ModelNotFound, got {other:?}"),
        }
    }

    #[test]
    fn classifies_404_without_model_body_as_other() {
        let e = classify_ollama_status(StatusCode::NOT_FOUND, "page not found", "llama3.2");
        assert!(matches!(e, InferenceError::Other(_)));
    }

    #[test]
    fn classifies_500_as_service_unavailable() {
        let e = classify_ollama_status(StatusCode::INTERNAL_SERVER_ERROR, "", "llama3.2");
        assert!(matches!(e, InferenceError::ServiceUnavailable));
    }

    #[test]
    fn classifies_503_as_service_unavailable() {
        let e = classify_ollama_status(StatusCode::SERVICE_UNAVAILABLE, "", "llama3.2");
        assert!(matches!(e, InferenceError::ServiceUnavailable));
    }

    #[test]
    fn classifies_other_4xx_as_other() {
        let e = classify_ollama_status(StatusCode::BAD_REQUEST, "bad payload", "llama3.2");
        match e {
            InferenceError::Other(s) => assert!(s.contains("400") && s.contains("bad payload")),
            other => panic!("expected Other, got {other:?}"),
        }
    }
}
```

- [ ] **Step 2: Run tests to verify failure**

```bash
cd /Users/hherb/src/primer/src && ~/.cargo/bin/cargo test -p primer-inference classify_ollama_tests 2>&1 | tail -10
```

Expected: compile error — `classify_ollama_status` undefined.

- [ ] **Step 3: Implement `classify_ollama_status` and import the shared transport classifier**

Add at the top of `crates/primer-inference/src/ollama.rs` after the existing `use` lines:

```rust
use crate::cloud::classify_reqwest_error;
```

(If `cloud` is not currently visible from `ollama` — check `crates/primer-inference/src/lib.rs` — make sure `pub mod cloud;` is exposed. It already is in current code.)

Add the new pure function near the top of the file:

```rust
/// Translate a non-success Ollama HTTP response into an
/// `InferenceError`. Pure — no I/O, no `Self`.
///
/// `requested_model` is the model name the user supplied via `--model`,
/// used to populate `ModelNotFound { model }` when Ollama signals a
/// missing model. `body` is the dev-facing payload for `Other`.
pub(crate) fn classify_ollama_status(
    status: reqwest::StatusCode,
    body: &str,
    requested_model: &str,
) -> primer_core::error::InferenceError {
    use primer_core::error::InferenceError::*;
    let body_lower = body.to_lowercase();
    match status.as_u16() {
        404 if body_lower.contains("model") && body_lower.contains("not") =>
            ModelNotFound { model: requested_model.to_string() },
        500..=599 => ServiceUnavailable,
        _ => Other(format!("Ollama returned {status}: {body}")),
    }
}
```

- [ ] **Step 4: Wire `OllamaBackend` to use the retry helper**

Replace the `OllamaBackend` struct and the `generate_stream` request-send block (the part from `let response = self.client.post(...)` through `if !status.is_success() { ... return Err(...) }`) with the retry-wrapped form.

First, update the struct (find the existing struct definition):

```rust
pub struct OllamaBackend {
    client: reqwest::Client,
    base_url: String,
    model: String,
    retry_settings: primer_core::retry::RetrySettings,
}

impl OllamaBackend {
    pub fn new(base_url: String, model: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url,
            model,
            retry_settings: primer_core::retry::RetrySettings::default(),
        }
    }
}
```

(Keep the existing `pub fn new` signature compatible — it already takes `(base_url, model)`.)

Then replace the send + status-check block in `generate_stream` with the retry-wrapped form:

```rust
let response = primer_core::retry::retry_with_backoff(
    &self.retry_settings,
    || async {
        let resp = self
            .client
            .post(format!("{}/api/chat", self.base_url))
            .json(&request)
            .send()
            .await
            .map_err(|e| classify_reqwest_error(&e))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(classify_ollama_status(status, &body, &self.model));
        }
        Ok(resp)
    },
)
.await
.map_err(PrimerError::Inference)?;
```

Keep the streaming consumer task (the `tokio::spawn(async move { ... })` block) unchanged — only the `.into()`-suffix sites in it (the existing `format!(...)` strings inside the byte-stream-error branch) carry over from Bundle 1.

- [ ] **Step 5: Run all `primer-inference` tests**

```bash
~/.cargo/bin/cargo test -p primer-inference 2>&1 | tail -15
```

Expected: pre-existing tests still pass + 5 new `classify_ollama_tests` pass.

- [ ] **Step 6: Lint and format**

```bash
~/.cargo/bin/cargo clippy -p primer-inference --all-targets 2>&1 | tail -5
~/.cargo/bin/cargo fmt -p primer-inference
```

- [ ] **Step 7: Commit Bundle 5**

```bash
git add crates/primer-inference/src/ollama.rs
git commit -m "$(cat <<'EOF'
feat(ollama): typed errors and bounded retry

OllamaBackend::generate_stream now classifies request-send and
status-check failures into typed InferenceError variants and retries
transient ones via primer_core::retry::retry_with_backoff. 404
responses with a "model not found" body produce ModelNotFound{model}
so the user sees the actionable "ollama pull <model>" message.

classify_reqwest_error is shared with cloud.rs (imported via
crate::cloud::classify_reqwest_error) — same connect/timeout/request
mapping for both backends.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Bundle 6 — CLI render integration

### Task 10: Plumb `Locale` and call `render_inference_error` at the REPL error site

**Files:**
- Modify: `crates/primer-cli/src/main.rs`

**Background:** The existing `Err(e) => eprintln!("Error generating response: {e}\n")` arm at lines 1100-1105 is the only place a child-facing inference error reaches the terminal. Other `Err` paths in the file (backend construction, classifier construction, etc.) print developer-facing diagnostics and stay unchanged.

- [ ] **Step 1: Add `Locale` to the imports**

Near the top of `main.rs` next to the other `primer_core::` imports:

```rust
use primer_core::i18n::{render_inference_error, Locale};
```

- [ ] **Step 2: Declare a single `cli_locale` binding once at startup**

Find the spot in `main()` where the CLI parses args and constructs subsystems (a few lines before the REPL loop begins). Add:

```rust
// Single locale-loading site for the future i18n PR to extend.
// Phase 0.1 ships only English; Locale::default() == Locale::English.
let cli_locale: Locale = Locale::default();
```

- [ ] **Step 3: Replace the error-render arm at the REPL loop**

Find this block (around `crates/primer-cli/src/main.rs:1100`):

```rust
Err(e) => {
    if prefix_printed {
        println!();
    }
    eprintln!("Error generating response: {e}\n");
}
```

Replace with:

```rust
Err(PrimerError::Inference(inf)) => {
    if prefix_printed {
        println!();
    }
    tracing::warn!(error = %inf, "inference failed; surfacing user-friendly message");
    eprintln!("{}\n", render_inference_error(&inf, &cli_locale));
}
Err(other) => {
    if prefix_printed {
        println!();
    }
    eprintln!("Error: {other}\n");
}
```

- [ ] **Step 4: Verify the build, including the speech feature**

```bash
cd /Users/hherb/src/primer/src && ~/.cargo/bin/cargo build --workspace 2>&1 | tail -10
~/.cargo/bin/cargo build --workspace --features primer-cli/speech 2>&1 | tail -10
```

Both should succeed.

- [ ] **Step 5: Run the full test suite**

```bash
~/.cargo/bin/cargo test --workspace 2>&1 | grep -E "^test result|FAILED" | head -10
```

Expected: 328 (pre-existing) + 4 (Bundle 1) + 8 (Bundle 2) + 8 (Bundle 3) + 10 (Bundle 4) + 5 (Bundle 5) ≈ **363 tests, all passing**. Final count to be confirmed.

- [ ] **Step 6: Lint and format**

```bash
~/.cargo/bin/cargo clippy --workspace --all-targets 2>&1 | tail -5
~/.cargo/bin/cargo fmt --all -- --check
```

Both clean.

- [ ] **Step 7: Commit Bundle 6**

```bash
git add crates/primer-cli/src/main.rs
git commit -m "$(cat <<'EOF'
feat(cli): graceful inference-error rendering

Errors surfaced from the REPL hot path now render through
primer_core::i18n::render_inference_error — a child sees an actionable
English message ("Authentication failed. Please check your
ANTHROPIC_API_KEY...") instead of raw HTTP status + body. The dev-facing
error is still emitted via tracing::warn! for diagnostics.

Locale is plumbed via a single Locale::default() binding at startup so
the future i18n PR has one place to add CLI-flag/env loading.

Non-inference errors (backend construction, etc) keep their existing
"Error: {e}" rendering — only the conversational failure surface is
touched.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Bundle 7 — Manual smoke and documentation

### Task 11: Manual smoke against real backends

**Files:** none (verification only).

**Background:** Manual verification against live backends per spec acceptance criteria #4-#7. Cannot be automated without integration test infra (deferred). Run each smoke and record the output in the commit message of Task 12.

- [ ] **Step 1: Smoke a bad API key (acceptance #4)**

```bash
cd /Users/hherb/src/primer/src
ANTHROPIC_API_KEY=sk-bogus ~/.cargo/bin/cargo run --bin primer -- \
    --backend cloud --name SmokeTester --age 9
# At the prompt, type: hello
```

Expected stderr: `Authentication failed. Please check your ANTHROPIC_API_KEY in .env or ~/.primer_env.` Should NOT contain "401", "{"type":"error"…", "Anthropic returned".

- [ ] **Step 2: Smoke a working key (acceptance #5)**

```bash
~/.cargo/bin/cargo run --bin primer -- --backend cloud --name SmokeTester --age 9
# At the prompt, type a question; verify a normal response streams back.
```

Expected: identical happy-path behaviour to before. No regression.

- [ ] **Step 3: Smoke Ollama with the daemon stopped (acceptance #6)**

If you have ollama installed:

```bash
ollama stop  # or pkill ollama
~/.cargo/bin/cargo run --bin primer -- --backend ollama --model llama3.2 \
    --name SmokeTester --age 9
# At the prompt, type: hello
```

Expected: `Cannot reach the service. Please check your network connection.` No "Connection refused (os error 61)".

If you do not have ollama installed at all, skip this step and document in the Task 12 commit.

- [ ] **Step 4: Smoke a missing model (acceptance #7)**

```bash
ollama serve &  # if needed
~/.cargo/bin/cargo run --bin primer -- --backend ollama \
    --model nonexistent-model:latest --name SmokeTester --age 9
# Type: hello
```

Expected: `Model 'nonexistent-model:latest' is not available. For Ollama, run \`ollama pull nonexistent-model:latest\`.`

- [ ] **Step 5: Record findings**

Capture the four smoke outputs (or note skipped steps) for the docs commit message in Task 12.

---

### Task 12: Update CLAUDE.md, ROADMAP.md, README.md, NEXT_SESSION.md

**Files:**
- Modify: `CLAUDE.md`
- Modify: `ROADMAP.md`
- Modify: `README.md`
- Modify: `NEXT_SESSION.md`
- Create: `docs/handoffs/<timestamp>-graceful-api-errors.md` (snapshot copy)

- [ ] **Step 1: Tick the Phase 0.1 ROADMAP bullet**

Find this line in `ROADMAP.md:19`:

```
- [ ] Handle API errors gracefully (rate limits, network drops, invalid key) with clear user-facing messages — partial. Mid-stream errors propagate cleanly and the partial Primer turn is dropped. Full retry/backoff on rate limits is still TODO.
```

Replace with:

```
- ✅ Handle API errors gracefully — done. PrimerError::Inference now carries a typed InferenceError (Auth | RateLimited{retry_after} | ServiceUnavailable | NetworkUnavailable | ModelNotFound{model} | Other). Both cloud and Ollama backends classify pre-stream failures and retry transient ones via primer_core::retry::retry_with_backoff (default 3 attempts, jittered exponential backoff, honors Retry-After ≤ 5s). User-facing messages render via primer_core::i18n::render_inference_error — single i18n boundary; future locale support adds variants without touching producers. See [docs/superpowers/specs/2026-05-04-graceful-api-errors-design.md](docs/superpowers/specs/2026-05-04-graceful-api-errors-design.md).
```

If the ROADMAP shows a "Phase 0.1 complete" subheading or progress count, update that too.

- [ ] **Step 2: Add gotcha bullets to CLAUDE.md**

Find the "Conventions and gotchas worth knowing" section. Add these bullets in a sensible spot near the existing inference-related ones:

```
- **`PrimerError::Inference` carries a typed `InferenceError`.** Six variants (`Auth`, `RateLimited { retry_after }`, `ServiceUnavailable`, `NetworkUnavailable`, `ModelNotFound { model }`, `Other(String)`). Variant fields are locale-neutral data; user-facing rendering is `primer_core::i18n::render_inference_error(err, locale)`. `From<&str>` and `From<String>` impls let producers say `format!(...).into()` for unmapped conditions. The `Other` variant is dev-facing only — its inner string never reaches users; the i18n layer substitutes a generic message and the call site logs the full error via `tracing::warn!`.
- **Retries happen at the request-send + status-check phase only.** Once `generate_stream` starts consuming bytes from a 2xx response, mid-stream errors propagate cleanly and the partial Primer turn drops at the dialogue-manager layer (existing behaviour). Both cloud and Ollama wrap the pre-stream phase in `primer_core::retry::retry_with_backoff`.
- **Retry budget defaults are tuned for conversational latency.** 3 attempts × ~250ms × 2 = ~1.75 s worst case. `retry-after` headers ≤ 5 s are honored exactly; longer waits surface immediately. Defaults in `primer_core::consts::retry`. Each retry decision emits `tracing::info!(target: "primer::retry", attempt, kind, delay_ms)` — a future voice-mode change can subscribe and play a "give me a moment" bridging phrase.
- **`primer_core::i18n::render_inference_error` is the single i18n boundary** for the inference-error surface. Phase 0.1 ships English-only via `Locale::English`. Adding locales is purely additive — no producer touches anything when the i18n PR lands. **Discipline:** never embed user-facing English in `InferenceError` variant fields (no `hint: String`); never render `Other`'s inner string to users. See [docs/background_research/i18n_design.md](docs/background_research/i18n_design.md) for the long-form i18n vision.
```

- [ ] **Step 3: Update the "What works today" / status header in README.md**

Find the section in `README.md` that lists what currently works. Add a short bullet:

```
- Graceful inference-error handling — typed `InferenceError` variants, bounded jittered retry on transient conditions (rate limits, 5xx, network flap), single i18n-ready render boundary. Closes the last open Phase 0.1 bullet.
```

If there's a "Still ahead" / "Phase 0.X" section that mentions API error handling, remove the now-done bullet.

- [ ] **Step 4: Rewrite `NEXT_SESSION.md`** — see the Final Wrap-Up section below for full content. (Implementer fills this in once the bundle is complete; the wrap-up section provides the template.)

- [ ] **Step 5: Save handoff snapshot**

```bash
ts=$(date +%Y-%m-%dT%H%M%z)
cp NEXT_SESSION.md "docs/handoffs/${ts}-graceful-api-errors.md"
```

- [ ] **Step 6: Commit Bundle 7**

```bash
git add CLAUDE.md ROADMAP.md README.md NEXT_SESSION.md docs/handoffs/
git commit -m "$(cat <<'EOF'
docs: graceful API errors landed, Phase 0.1 closed

ROADMAP Phase 0.1 final bullet ticked. CLAUDE.md gains four gotchas
covering the typed InferenceError surface, retry-at-request-only
discipline, retry budget tuning, and the single i18n boundary.
README "What works today" picks up the bullet. NEXT_SESSION.md
updated with Phase 0.3 next-task options + handoff snapshot.

Manual smoke results captured in NEXT_SESSION.md "verified state"
section.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Final wrap-up: NEXT_SESSION.md template

After Task 12 commits, the implementer rewrites `NEXT_SESSION.md` with this structure (drawing from the existing template in the repo):

1. **Header:** date, brief description ("After the graceful-API-errors branch landed: typed `InferenceError`, retry helper, i18n boundary; 328 → ~363 tests; Phase 0.1 closed").
2. **First moves on next session start.** (Boilerplate from current NEXT_SESSION.md.)
3. **Branch status.** PR-or-merge options for `feature/graceful-api-errors`.
4. **What's now on the feature branch (so you don't redo it).** Bundle-by-bundle summary with commit SHAs.
5. **Open Phase 0.3 tasks (vocabulary tracking, session-time tracking).** Re-state Options A and B from the prior NEXT_SESSION.md as the obvious next picks now that 0.1 is done.
6. **Open watch-outs / risks.** Carry forward the relevant ones from prior NEXT_SESSION.md; add the post-0.1 list:
   - Pre-flight auth check at startup deferred (covered by friendly first-turn error).
   - HTTP-date `Retry-After` form silently dropped — log a follow-up if observed in the wild.
   - `is_connect()` heuristic conflates "no internet" with "Ollama daemon not running" — friendly message is generic enough; hint-discriminating is a follow-up.
   - Voice-mode bridging hook: data is in `tracing::info!(target: "primer::retry", ...)` events. Voice loop change to subscribe and play a canned bridging phrase remains TODO.
7. **Exact commands needed to resume.** (Updated test count, branch name.)

---

## Self-review checklist

After the implementer completes all tasks, before reporting done:

1. **Spec coverage.** Each acceptance criterion in §Acceptance criteria has a Step or Task that satisfies it. Walk the list; confirm each is addressed.
2. **Test count delta.** Spec estimated +25 to +35; final count from Step 5 of Task 10 should be in that range.
3. **No new clippy warnings.** Final `cargo clippy --workspace --all-targets` is clean.
4. **No new fmt drift.** Final `cargo fmt --all -- --check` passes.
5. **Manual smoke evidence captured.** All four acceptance smokes in Task 11 either passed or were documented as skipped with a clear reason.
6. **Voice-mode hook contract preserved.** Confirm `tracing::info!(target: "primer::retry", ...)` events fire on real retries (use `RUST_LOG=primer::retry=info` during a smoke that triggers a 429 or simulated outage; this can be deferred if a real 429 is hard to reproduce).
7. **No production code reads from `Other`'s inner string.** `grep -rn "InferenceError::Other(" src/crates/` should show only constructors and tests, never destructure-and-display.

If any check fails, fix it before declaring the work complete.
