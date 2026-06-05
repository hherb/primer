# Automatic local→cloud inference fallback — design

**Date:** 2026-06-05
**Phase:** 1.1 bullet (c) — "Automatic local→cloud fallback; 3B fallback path for constrained devices."
**Status:** approved (brainstorming), pre-implementation.

## Problem

A local inference backend (`LlamaCppBackend`, `QnnBackend`) can be unavailable
or fail: the GGUF won't load, the NPU is absent, the device is too constrained,
or a single turn errors before any token streams. Today that is a hard failure —
the session either won't start or the turn drops. We want an **opt-in** path that
falls back to a configured secondary backend (typically cloud) so the
conversation keeps working.

This is the **reliability fallback** only. It is deliberately NOT the Phase 1.3
inference *router* (per-turn complexity/latency routing) — that is a separate,
larger effort. The decorator introduced here is a clean building block a future
router can compose or replace.

## Decisions (locked during brainstorming)

1. **Trigger = startup + pre-stream, never mid-stream.** Fall back when the
   primary is unavailable at startup (construction fails) OR a turn errors
   *before* any token is streamed. Once streaming has begun, a mid-stream error
   propagates and the partial turn drops as it does today — no fallback, no
   double-speaking.
2. **Privacy = explicit opt-in, default off.** Falling back to cloud sends the
   child's turn to Anthropic, changing the local-first privacy posture
   (`[[project_strict_offline_first]]`). Fallback only happens when the user
   explicitly configures a fallback backend. Absent configuration ⇒ local-only,
   surfacing the error rather than silently phoning home. The flag's presence
   *is* the consent.
3. **Config surface = `--fallback-backend` + `--fallback-model`.** Primary stays
   `--backend`/`--model`. The new flags name the secondary and reuse existing
   credential flags. GUI mirrors with a "Fallback backend" picker.
4. **Chain depth = single secondary now, architected for N later.** One
   primary → one fallback hop. The 3B constrained-device hop (big-local →
   small-local → cloud) needs real device-tuning data and is a later extension;
   the design must not preclude an ordered chain but must not build it now.

## Architecture (Approach A — decorator + construction fallback)

`build_backend()` returns `Arc<dyn InferenceBackend>`, which the
`DialogueManager` already holds. A fallback fits as a **decorator** (GoF pattern,
not a language feature): a struct implementing `InferenceBackend` that wraps a
primary + secondary `Arc<dyn InferenceBackend>` and delegates. The DM is
**completely unchanged**.

The pre-stream / mid-stream boundary maps *exactly* onto the
`generate_stream().await` `Result` boundary, which is what makes the "never
mid-stream" rule fall out for free.

### Component 1 — `FallbackBackend` (`primer-inference/src/fallback.rs`)

```rust
pub struct FallbackBackend {
    primary: Arc<dyn InferenceBackend>,
    secondary: Arc<dyn InferenceBackend>,
}

#[async_trait]
impl InferenceBackend for FallbackBackend {
    // Load-bearing: return the PRIMARY's name verbatim. The per-backend context
    // budget (primer_core::backend::is_small_context_backend, prefix-anchored on
    // "qnn:") keys off name(). The prompt window is sized once per turn for the
    // common case (primary); a cloud secondary handles a smaller window fine.
    fn name(&self) -> &str { self.primary.name() }

    async fn is_available(&self) -> bool {
        self.primary.is_available().await || self.secondary.is_available().await
    }

    async fn generate_stream(&self, prompt: &Prompt, params: &GenerationParams)
        -> Result<TokenStream>
    {
        match self.primary.generate_stream(prompt, params).await {
            Ok(stream) => Ok(stream),               // verbatim — no mid-stream fallback
            Err(e) => {
                tracing::warn!(target: "primer::fallback",
                    primary = self.primary.name(), secondary = self.secondary.name(),
                    error = %e, "primary failed pre-stream; falling back to secondary");
                self.secondary.generate_stream(prompt, params).await
            }
        }
    }
}
```

`generate` and `summarize` use the trait's default impls, which call
`self.generate_stream` — so they inherit the pre-stream fallback automatically.
No override needed.

Constructor: `FallbackBackend::new(primary, secondary)`.

~80 LOC impl + ~120 LOC tests. Exported from `primer-inference/src/lib.rs`.

### Component 2 — construction fallback (`primer-engine/src/wiring.rs`)

Two new `BackendParams` fields (plain, not feature-gated):

```rust
pub fallback_backend: Option<String>,
pub fallback_model: Option<String>,
```

A **pure decision function**, exhaustively unit-testable without constructing any
backend:

```rust
pub enum MainBackendPlan { PrimaryAlone, SecondaryAlone, Wrapped, Fail }

pub fn plan_main_backend(
    primary_built: bool, fallback_configured: bool, secondary_built: bool,
) -> MainBackendPlan
```

Truth table:

| primary builds | fallback configured | secondary builds | → plan | rationale |
|---|---|---|---|---|
| ✓ | no  | —  | `PrimaryAlone`   | today's behavior, unchanged |
| ✓ | yes | ✓  | `Wrapped`        | the happy fallback path |
| ✓ | yes | ✗  | `PrimaryAlone`   | a fallback that won't construct shouldn't kill a working primary (warn) |
| ✗ | yes | ✓  | `SecondaryAlone` | startup failure → cloud ("NPU absent / GGUF missing") (warn) |
| ✗ | yes | ✗  | `Fail`           | both dead → surface the primary error (most informative) |
| ✗ | no  | —  | `Fail`           | today's behavior, unchanged |

Thin glue `build_main_backend(primary_name, primary_model, params)`:
1. `build_backend(primary…)` → capture `Ok`/`Err`.
2. If a fallback is configured, resolve its model (see Config) and
   `build_backend(secondary…)` → capture `Ok`/`Err`.
3. Call `plan_main_backend(...)`; materialize the chosen plan:
   - `PrimaryAlone` → return primary `Arc`.
   - `SecondaryAlone` → return secondary `Arc` (emit a `tracing::warn!` that the
     primary was unavailable at startup).
   - `Wrapped` → `Arc::new(FallbackBackend::new(primary, secondary))`.
   - `Fail` → return the primary's construction error.

`build_backend` itself is unchanged (still the per-type dispatch). The subsystem
builders (`build_classifier`/`build_extractor`/`build_comprehension`) are
**unchanged**: an `Arc::clone` of the wrapped main backend transparently inherits
fallback; a subsystem that builds its own backend stays single-backend (those
already soft-fail — giving them independent fallback is an explicit non-goal).

### Component 3 — call-site swap

- CLI `primer-cli/src/main.rs` (~line 942): call `build_main_backend` instead of
  `build_backend`.
- GUI `primer-gui/src/wiring.rs::build_with_strategy` (~line 179): **deferred to a
  follow-up issue** (see Scope).

## Config surface (CLI)

Two new flags on `primer-cli`, both `Option`, absent ⇒ no fallback ⇒ local-only:

- `--fallback-backend <stub|cloud|ollama|openai-compat>` — the secondary type.
- `--fallback-model <name>` — the secondary's model. Resolution:
  - `cloud` + unset ⇒ default to the existing cloud default `claude-sonnet-4-6`.
  - `ollama`/`openai-compat` + unset ⇒ error (model required, same rule as the
    primary).
  - `stub` ⇒ ignored.

Reuses existing creds/URL flags (`--api-key`/`ANTHROPIC_API_KEY`, `--ollama-url`,
`--openai-compat-url`/`--openai-compat-api-key`) via the shared `BackendParams`.

Guard: `--fallback-backend` == `--backend` (e.g. cloud→cloud) is pointless;
emit a `tracing::warn!` but do not hard-error (it works, just no resilience gain).

Both `BackendParams` construction sites (CLI `main.rs`, the test constructors in
`wiring.rs`) get the two new fields.

## Error handling & edge cases

- **Never mid-stream:** once `primary.generate_stream().await` is `Ok`, the
  stream is returned verbatim; a later `Err` item propagates and the partial turn
  drops at the DM layer as today.
- **Pre-stream fallback:** one `tracing::warn!(target: "primer::fallback", …)`
  with both names + the primary error. No user-facing per-turn message.
- **Secondary also fails pre-stream:** its `Err` propagates (turn fails as today);
  both errors visible in logs.
- **Retry interaction:** each leg keeps its own existing pre-stream retry budget.
  Fallback fires *after* the primary exhausts its retries. Worst case =
  primary-retries then secondary-retries. Documented; no new retry layer.
- **`name()` returns primary's name:** asserted by a test and documented inline,
  since it is the subtle load-bearing bit for `is_small_context_backend`.

## Testing (TDD, tests first)

- **`FallbackBackend` unit tests** (mock backends, no network):
  - primary-`Ok` uses primary, secondary never called (call-count flag on mock);
  - primary-`Err`-pre-stream uses secondary;
  - primary-`Ok`-then-mid-stream-`Err` propagates AND secondary never called;
  - both-pre-stream-`Err` propagates secondary's error;
  - `name() == primary.name()`;
  - `is_available()` reflects either leg.
- **`plan_main_backend` truth-table tests** — all six rows.
- **`build_main_backend` wiring tests** using stub/unknown backend names for
  observable outcomes:
  - primary=`unknown` + fallback=`stub` ⇒ builds (secondary-alone);
  - primary=`stub` + fallback=`unknown` ⇒ builds anyway, no error (primary-alone);
  - no-fallback path unchanged (primary-alone).
- **CLI flag wiring:** `--fallback-backend cloud` without `--fallback-model`
  defaults to sonnet; `--fallback-backend ollama` without model errors.

## Scope

**In scope (this session):** the `FallbackBackend` decorator, the
`plan_main_backend` pure fn + `build_main_backend` glue, the two CLI flags, the
two `BackendParams` fields, the CLI call-site swap, full unit coverage. No DM
changes, no schema changes, no new dependencies.

**Out of scope (follow-up issue):** GUI mirror — `BackendConfig`/`View`/`Update`
gain `fallback_backend`/`fallback_model` (the `Update` DTO has no
`#[serde(default)]`, so `settings.js::gather()` must send both),
`build_with_strategy` → `build_main_backend`, and a Settings → "Fallback backend"
picker. Purely additive on top of the CLI work.

**Out of scope (later phase):** the N-backend ordered chain / 3B constrained
hop (Phase 1.1 bullet c, second half — needs device data); per-turn
complexity/latency routing (Phase 1.3).

## Non-goals

- No fallback for subsystem backends that build independently (they soft-fail).
- No mid-stream fallback / re-generation.
- No new retry layer; legs keep their own.
- No silent cloud usage — fallback requires explicit configuration.
