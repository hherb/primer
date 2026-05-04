# Graceful API Errors — Design Spec

**Date:** 2026-05-04
**Phase:** 0.1 (final remaining bullet)
**Author:** Claude (with Horst Herb)
**Status:** Approved by user, pending implementation plan

## Goal

Replace the current raw-string inference-error surface with a typed `InferenceError` enum, classify the common transient and configuration failures of both inference backends (Anthropic cloud, Ollama), retry transient conditions silently with bounded jittered backoff, and render user-facing messages through a single i18n-ready boundary.

This closes the last open Phase 0.1 bullet on the ROADMAP:

> Handle API errors gracefully (rate limits, network drops, invalid key) with clear user-facing messages — partial. Mid-stream errors propagate cleanly and the partial Primer turn is dropped. Full retry/backoff on rate limits is still TODO.

After this PR: a child whose parent typo'd the API key sees an actionable English message ("Authentication failed. Please check your ANTHROPIC_API_KEY in .env or ~/.primer_env.") rather than `Anthropic API returned 401 Unauthorized: {"type":"error","error":{"type":"invalid_api_key"...`. A 429 burst is silently absorbed by the retry helper (≤ ~1.75 s extra latency at default settings). A child who lost wifi sees "Cannot reach the service. Please check your network connection."

## Non-goals

- **Pre-flight authentication check at startup.** A first-turn auth failure with a clean message is acceptable UX; adding a startup ping costs a network call on every launch (and conflicts with strict-offline-first when the user is on `--backend stub`).
- **Retry around the streaming body.** Once tokens start flowing, mid-stream errors propagate as today and the partial Primer turn drops. Retry is strictly around the request-send + status-check phase.
- **Implementing actual i18n.** Phase 0.1 ships English-only. The design positions the i18n boundary so a future PR can add locales as new variants and translation tables without touching backends, retry, or error variants. See [docs/background_research/i18n_design.md](../../background_research/i18n_design.md) for the long-form i18n vision.
- **Externalising other English strings** (CLI banners, `--verbose` prefixes, prompt-builder content). Out of scope; folded into the future dedicated i18n session.
- **A `--locale` CLI flag.** A flag that only accepts one value is worse UX than no flag. The i18n PR adds the flag at the same time it adds the variants.
- **A cancellation token on streaming-task spawns.** Existing fire-and-forget pattern is unchanged. Listed in NEXT_SESSION watch-outs already.
- **Refactoring `dialogue_manager.rs`** (3356 lines). Scheduled for a dedicated session; this work touches one error-render call site there.

## Architecture

### Module map

```
primer-core/src/error.rs            — InferenceError enum (replaces String payload)
primer-core/src/retry.rs    [NEW]   — retry_with_backoff helper + RetrySettings
primer-core/src/i18n.rs     [NEW]   — Locale enum, render_inference_error()
primer-core/src/consts.rs           — retry::* defaults (max_attempts, base_delay, …)
primer-inference/src/cloud.rs       — classify_anthropic_status(), retry-wrapped send
primer-inference/src/ollama.rs      — classify_ollama_status(), retry-wrapped send
primer-cli/src/main.rs              — calls render_inference_error at the existing
                                       Err(e) branch in the REPL loop
```

Three crates touched (`primer-core`, `primer-inference`, `primer-cli`). No new crate. Workspace crate count stays at 10.

### Three responsibilities, three modules

| Concern | Where | Pure? | i18n-touching? |
|---|---|---|---|
| Categorise what happened | `primer-core::error::InferenceError` | data only | no — locale-neutral fields |
| Decide whether to retry, then orchestrate retry loop | `primer-core::retry::retry_with_backoff` | yes | no |
| Translate category → human-readable message | `primer-core::i18n::render_inference_error` | yes | yes — the only place |

Each is a pure module with a small public surface. The retry helper takes a closure and returns `Result<T, InferenceError>` — it is independent of `reqwest`, `Anthropic`, and `Ollama`. The render function takes `&InferenceError` + `&Locale` and returns `String` — independent of where the error came from.

The two backend `classify_*` functions are pure too (status code + headers + body → `InferenceError`), unit-testable without live HTTP. The only impure code is the surrounding `client.post(...).send().await` glue, which is already there.

## Data model

### `InferenceError`

```rust
// primer-core/src/error.rs
use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Clone, Error)]
pub enum InferenceError {
    /// 401 / 403 from a cloud API, or any other auth-shape failure.
    /// User remedy: check the API key.
    #[error("authentication failed")]
    Auth,

    /// 429 with optional Retry-After header. If the server demands a wait
    /// longer than `RetrySettings.retry_after_budget`, the retry helper
    /// gives up and surfaces this variant unchanged.
    #[error("rate limited (retry_after={retry_after:?})")]
    RateLimited { retry_after: Option<Duration> },

    /// 5xx from cloud, or daemon-down from Ollama.
    /// User remedy: try again later.
    #[error("service unavailable")]
    ServiceUnavailable,

    /// Connect / timeout / DNS / TLS failure at the transport layer.
    /// User remedy: check the network.
    #[error("network unavailable")]
    NetworkUnavailable,

    /// Ollama returned 404 with "model not found" (or similar) for a
    /// model name the user supplied via --model. Cloud equivalent doesn't
    /// exist — the cloud API rejects unknown models with a different shape
    /// that maps to Auth or Other.
    #[error("model not found: {model}")]
    ModelNotFound { model: String },

    /// Catch-all for unmapped conditions. **Dev-facing only.** Never
    /// rendered to users as-is — the user sees a generic localized message
    /// and the inner string goes to `tracing::warn!` for diagnostics.
    #[error("{0}")]
    Other(String),
}

impl InferenceError {
    /// Whether the retry helper should attempt to recover.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::RateLimited { .. } | Self::ServiceUnavailable | Self::NetworkUnavailable,
        )
    }
}

impl From<&str>   for InferenceError { fn from(s: &str)   -> Self { Self::Other(s.into()) } }
impl From<String> for InferenceError { fn from(s: String) -> Self { Self::Other(s) } }
```

### `PrimerError::Inference` migration

```rust
// before
pub enum PrimerError { Inference(String), ... }

// after
pub enum PrimerError { Inference(InferenceError), ... }
```

Existing producers that say `PrimerError::Inference("msg".into())` continue to compile because of the `From<&str>` and `From<String>` impls on `InferenceError` — they fall through to the `Other` variant. There are about 15 such sites across the workspace; none need to change semantically. Two new producers (`classify_anthropic_status` and `classify_ollama_status`) explicitly construct the categorized variants.

The `#[error(...)]` strings on `InferenceError` are **dev-facing English** intended for `tracing::warn!`, panic messages, and test output. User-facing rendering routes through a separate function (§ Render layer).

## Retry helper

### Settings

```rust
// primer-core/src/retry.rs
use std::time::Duration;

pub struct RetrySettings {
    /// Total attempts including the first. Default: 3.
    pub max_attempts: u32,
    /// Initial backoff delay. Default: 250 ms.
    pub base_delay: Duration,
    /// Multiplicative factor between attempts. Default: 2 (250 → 500 → 1000).
    pub backoff_factor: u32,
    /// Jitter as a fraction of the computed delay. Default: 0.5 (±50 %).
    pub jitter_fraction: f32,
    /// Maximum honored Retry-After. If the server demands longer, give up.
    /// Default: 5 s.
    pub retry_after_budget: Duration,
}

impl Default for RetrySettings { /* reads from primer-core::consts::retry */ }
```

All five defaults live in `primer-core::consts::retry` per the no-magic-numbers rule.

### Function

```rust
pub async fn retry_with_backoff<T, F, Fut>(
    settings: &RetrySettings,
    mut op: F,
) -> Result<T, InferenceError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, InferenceError>>,
{
    // 1. Attempt op(). On Ok(t), return Ok(t).
    // 2. On Err(e):
    //    - If !e.is_retryable() OR attempt+1 >= max_attempts, return Err(e).
    //    - If e is RateLimited { retry_after: Some(d) }:
    //        if d > retry_after_budget, return Err(e) immediately (don't wait).
    //        else sleep d (no jitter on top — the server told us how long).
    //    - Else compute delay = base_delay * backoff_factor^attempt
    //        plus jitter_fraction * delay * uniform(-1, 1).
    //    - tracing::info!(target: "primer::retry", attempt, kind = ?e, delay_ms);
    //    - tokio::time::sleep(delay).await;
    //    - loop.
}
```

### Retryability table

| Variant | Retryable | Rationale |
|---|---|---|
| `Auth` | no | Config error; retrying with the same key gives the same answer |
| `ModelNotFound` | no | Config error; the model name won't appear |
| `RateLimited` | yes | Transient by definition |
| `ServiceUnavailable` | yes | Transient (5xx, daemon flap) |
| `NetworkUnavailable` | yes | Transient (wifi flap, DNS hiccup) |
| `Other` | no | Unknown — surface immediately rather than amplify |

Worst-case latency before failure with defaults (3 attempts; 2 retries): ~250 ms + ~500 ms = ~750 ms median, ≤ ~1.75 s with jitter. Acceptable in conversational flow. If `retry_after` arrives ≤ budget, that supersedes — we sleep the requested duration once and retry.

### Voice-mode bridging hook

Each retry decision emits

```
tracing::info!(target: "primer::retry", attempt, kind = ?err, delay_ms = …);
```

A future voice-mode change can subscribe to `primer::retry` via `tracing_subscriber` and play a canned "give me a moment" phrase when the first retry event fires. No new API surface required now; the data is in the event.

## Backend integration

### Cloud (`primer-inference/src/cloud.rs`)

Two new pure functions:

```rust
fn classify_anthropic_status(
    status: reqwest::StatusCode,
    headers: &reqwest::header::HeaderMap,
    body: &str,  // for Other-variant payload only; not user-facing
) -> InferenceError {
    use InferenceError::*;
    match status.as_u16() {
        401 | 403 => Auth,
        429 => RateLimited { retry_after: parse_retry_after(headers) },
        500..=599 => ServiceUnavailable,
        _ => Other(format!("Anthropic returned {status}: {body}")),
    }
}

fn classify_reqwest_error(e: &reqwest::Error) -> InferenceError {
    if e.is_connect() || e.is_timeout() || e.is_request() {
        InferenceError::NetworkUnavailable
    } else {
        InferenceError::Other(format!("reqwest: {e}"))
    }
}

fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    // Anthropic returns Retry-After as integer seconds (or HTTP-date, rare).
    // Parse the seconds form; ignore the date form (return None).
    headers.get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .map(Duration::from_secs)
}
```

`generate_stream` is restructured so the request build → send → status-check phase runs inside a closure passed to `retry_with_backoff`. The closure returns `Result<reqwest::Response, InferenceError>`; on success the Response is then consumed by the existing streaming path (untouched).

```rust
let response = retry_with_backoff(&self.retry_settings, || async {
    let resp = self.client.post(&self.url)
        .json(&request)
        .send()
        .await
        .map_err(|e| classify_reqwest_error(&e))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let headers = resp.headers().clone();
        let body = resp.text().await.unwrap_or_default();
        return Err(classify_anthropic_status(status, &headers, &body));
    }
    Ok(resp)
}).await?;

// streaming path unchanged from here down
```

`CloudBackend` gains a `retry_settings: RetrySettings` field, defaulted via `RetrySettings::default()`. No new constructor parameter for now (callers don't need to override).

### Ollama (`primer-inference/src/ollama.rs`)

Symmetric. New pure function:

```rust
fn classify_ollama_status(
    status: reqwest::StatusCode,
    body: &str,  // dev-facing
    requested_model: &str,
) -> InferenceError {
    use InferenceError::*;
    match status.as_u16() {
        404 if body.to_lowercase().contains("model") && body.to_lowercase().contains("not") =>
            ModelNotFound { model: requested_model.into() },
        500..=599 => ServiceUnavailable,
        _ => Other(format!("Ollama returned {status}: {body}")),
    }
}
```

`classify_reqwest_error` is reusable verbatim across both backends — same logic. Lift it to `primer-inference/src/lib.rs` (or a small shared module) and import from both backend files.

Connection-refused (Ollama daemon not running) surfaces as `reqwest::Error` with `is_connect() == true`, mapped to `NetworkUnavailable` by the shared classifier. The user-facing render for `NetworkUnavailable` is generic ("Cannot reach the service…"); a dedicated "Ollama isn't running; try `ollama serve`" hint would require differentiating "no daemon" from "no internet" which the classifier can't reliably do at the transport layer. Acceptable trade-off for Phase 0.1 — the dev-facing log line via `tracing::warn!` carries the underlying `reqwest::Error` for diagnostics.

## Render layer (i18n boundary)

```rust
// primer-core/src/i18n.rs

/// User-facing locale.
///
/// Phase 0.1 ships only `English`. Future i18n work adds variants
/// here and corresponding match arms in `render_inference_error`.
/// Keep `InferenceError` field types locale-neutral (`Duration`,
/// model name strings, status code data) — never embed English
/// instructions in error variants.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Locale {
    #[default]
    English,
}

/// Translate an InferenceError into a user-facing message.
///
/// **i18n contract**:
///   - This is the ONLY place user-facing English appears in the inference
///     error path. Producers (`classify_*`) emit data; this function
///     translates data to text.
///   - The `Other` variant is dev-facing — its inner string is NEVER
///     rendered to users. We surface a generic localized message and
///     route the inner string to `tracing::warn!` at the call site.
///   - Future i18n PR replaces these match arms with a fluent / gettext
///     lookup keyed on variant name + structured fields. The function
///     signature does not change.
pub fn render_inference_error(err: &InferenceError, locale: &Locale) -> String {
    match locale {
        Locale::English => render_english(err),
    }
}

fn render_english(err: &InferenceError) -> String {
    use InferenceError::*;
    match err {
        Auth => "Authentication failed. Please check your ANTHROPIC_API_KEY in .env or ~/.primer_env.".into(),
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
```

## CLI integration

The error-render branch of the REPL loop ([primer-cli/src/main.rs:1100-1105](../../../src/crates/primer-cli/src/main.rs#L1100)) becomes:

```rust
Err(PrimerError::Inference(inf)) => {
    if prefix_printed { println!(); }
    tracing::warn!(error = %inf, "inference failed; surfacing user-friendly message");
    eprintln!("{}\n", render_inference_error(&inf, &cli_locale));
}
Err(other) => {
    if prefix_printed { println!(); }
    eprintln!("Error: {other}\n");
}
```

`cli_locale: Locale` is set once at startup (`Locale::default()` for now) and threaded down. Single locale-loading site for the i18n PR to extend.

## Tests

### `primer-core::error`
- `InferenceError::is_retryable` truth table — 6 variants asserted.
- `From<&str>` / `From<String>` constructors land in `Other`.
- `Display` (via `thiserror`) produces expected dev-facing English for each variant.

### `primer-core::retry`
- Succeeds on first attempt — closure called once.
- Succeeds on attempt N — closure called N times, retry events emitted N-1 times.
- Exhausts attempts — final `Err` matches the last failure's variant; `max_attempts` closure invocations.
- Non-retryable variant — surfaces immediately, no sleep, no retry event.
- Respects `retry-after` ≤ budget — sleeps exactly `d`, no extra jitter.
- Gives up on `retry-after` > budget — closure called once, surfaces unchanged.

All retry tests use a counter closure and `tokio::time::pause()` for deterministic backoff timing — no real sleeping in unit tests.

### `primer-core::i18n`
- `render_inference_error` covers all 6 variants under `Locale::English`.
- Each rendered string contains the right semantic markers ("API key" for Auth, "Ollama pull" for ModelNotFound, the formatted seconds for RateLimited-with-header, etc.).
- `Other`'s inner string does not appear in the rendered output.
- `Locale::default() == Locale::English`.

### `primer-inference::cloud`
- `classify_anthropic_status` — 401, 403, 429-with-retry-after, 429-without, 500, 503, unknown-200 (defensive).
- `parse_retry_after` — integer seconds, missing header, HTTP-date form (silently dropped, returns `None`), malformed.
- `classify_reqwest_error` mapping — limited to errors we can construct synthetically; document the live cases inline.

### `primer-inference::ollama`
- `classify_ollama_status` — 404-with-model-not-found body extracts the model name; 404 without that body falls to `Other`; 500 → `ServiceUnavailable`.

### Integration
- Existing CLI smoke loop unchanged. `cargo run --bin primer` with stub backend continues to work.
- One end-to-end test that a forced 401 from a mock-server returns `PrimerError::Inference(Auth)` (using `wiremock` if it's already in dev-deps; if not, defer to manual smoke per the existing pattern).

Estimated test count delta: **+25 to +35**.

## Risks and trade-offs

1. **Breaking change to `PrimerError::Inference(...)`.** The `From<&str>` / `From<String>` impls preserve existing call sites at compile time. Pattern-matching call sites (a small number — mostly tests) will need a cosmetic update from `Inference(s)` to `Inference(InferenceError::Other(s))` or equivalent. Search-and-fix is mechanical.

2. **`is_connect()` heuristic for transport errors.** Mapping all `reqwest::Error::is_connect()` outcomes to `NetworkUnavailable` conflates "no internet" with "Ollama daemon not running." Acceptable for Phase 0.1: the user-facing message is generic enough ("Cannot reach the service") to be technically correct in both cases; the dev log carries the full underlying error. A future enhancement could differentiate by looking at the URL host (`localhost` → daemon-down hint).

3. **Retry helper does not surface intermediate progress to the streaming consumer.** During the retry phase the consumer's `Stream` hasn't been created yet, so this is fine. But any future change that overlaps retry with stream consumption needs to re-think this.

4. **`retry-after` parsing is integer-seconds only.** Anthropic returns this form in practice. The HTTP-date form is silently dropped (we treat as absent). If we ever observe HTTP-date `retry-after` in the wild, parsing it is a small follow-up.

5. **`Other(String)` is dev-only forever.** If a future contributor surfaces `Other`'s inner string to the user, they break the i18n contract. Mitigation: the `render_english` match arm for `Other` explicitly ignores the inner string and returns the generic message; reviewers and the docstring make the rule explicit.

6. **Retry-helper unit tests rely on `tokio::time::pause()`.** Requires the `test-util` feature on tokio, which is dev-only — no production cost.

## Out of scope (carried forward, not addressed here)

- Pre-flight auth check at startup.
- Cancellation token on streaming-task spawns.
- Differentiating "Ollama daemon not running" from "no internet."
- HTTP-date `retry-after` parsing.
- Migrating other English strings (CLI banners, prompts, verbose prefixes) to the i18n boundary.
- A `--locale` CLI flag.

## Acceptance criteria

1. `PrimerError::Inference` carries a typed `InferenceError`; existing producers compile via the `From` impls.
2. `cargo build --workspace` clean; `cargo clippy --workspace --all-targets` clean; `cargo fmt --all -- --check` clean.
3. `cargo test --workspace` green with +25 to +35 new tests (final count to be confirmed in the implementation).
4. Manual smoke against Anthropic with an obviously-bad API key surfaces "Authentication failed. Please check your ANTHROPIC_API_KEY…" — no raw status code, no API body text.
5. Manual smoke against Anthropic with a working key still works end-to-end (no behavioural regression on the happy path).
6. Manual smoke against Ollama with the daemon stopped surfaces "Cannot reach the service. Please check your network connection." — no `Connection refused (os error 61)` reaching the user.
7. Manual smoke against Ollama with `--model nonexistent-model:latest` surfaces "Model 'nonexistent-model:latest' is not available. For Ollama, run `ollama pull …`."
8. Mid-stream errors (the existing path) continue to drop the partial Primer turn and surface a (now also localized) message — no regression.
9. ROADMAP Phase 0.1 bullet ticked; NEXT_SESSION.md updated; README crate tree (no change), "What works today," "Still ahead" reflect the landed state.
