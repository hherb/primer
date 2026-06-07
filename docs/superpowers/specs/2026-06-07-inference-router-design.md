# Phase 1.3 Inference Router — Design

**Date:** 2026-06-07
**Status:** approved (brainstorming complete; ready for implementation plan)
**ROADMAP item:** 1.3 — Hybrid inference — "Inference router (local for routine
turns, cloud for complex/knowledge-intensive); latency-aware switching;
config-driven local-only / cloud-preferred / hybrid."

## Summary

A per-turn inference **router** that picks between a primary (typically
local/small) and a secondary (typically cloud/strong) backend based on an
estimate of turn complexity, exposed as three config modes
(`local-only` / `cloud-preferred` / `hybrid`).

The router ships as a **decorator** — `RouterBackend` implements
`InferenceBackend` exactly like the existing `FallbackBackend`, so the
`DialogueManager` is structurally unchanged. The policy receives rich signals
(the `PedagogicalIntent` and the retrieved-passage count) via a new optional
`RoutingSignals` field on `GenerationParams` that every non-router backend
ignores.

**This spec implements:** complexity-based routing + the three config modes.
**This spec designs but defers:** latency-aware switching (a clean extension
point; real thresholds need the owner-gated llama.cpp/QNN bench numbers that
do not yet exist).

## Decisions locked during brainstorming

1. **Architecture — decorator + threaded hint.** `RouterBackend` decorator
   (zero structural `DialogueManager` change) PLUS `GenerationParams` carries an
   optional `RoutingSignals` the dialogue manager populates with intent +
   passage count. Best of both: the proven decorator seam from `FallbackBackend`,
   with rich signals the bare `Prompt` can't carry as structured data.
2. **Scope — complexity + config modes now; latency deferred** as an extension
   point with no guessed thresholds.
3. **Heuristic — composite score + threshold.** A pure scoring function combines
   intent weight + passage term + message-complexity term into one score; route
   to the secondary leg above a tunable named-const threshold.
4. **Failover — router self-fails-over; `FallbackBackend` stays.** The router
   picks an ordered (first, second) leg pair; if `first` fails **pre-stream** it
   transparently tries `second` (same pre-stream-only boundary as
   `FallbackBackend`, never mid-stream). `FallbackBackend` remains the simpler
   resilience-only building block for users who want failover **without**
   per-turn routing.

## Non-goals (YAGNI)

- **Latency-aware switching** — designed as an extension point only (see §6).
- **More than two legs** — the router routes between a primary and a secondary.
  The ordered big-local → small-local → cloud chain (ROADMAP 1.1 bullet c second
  half) is a separate, later concern; `RoutingMode` + the policy generalize, but
  N-leg wiring is out of scope here.
- **Replacing `FallbackBackend`** — explicitly rejected. The router is additive.
- **Per-subsystem routing** (classifier/extractor/comprehension) — they already
  soft-fail and run on the main backend or an explicit override; routing them is
  a non-goal, mirroring the fallback's inherit-only-via-`Arc::clone` stance.

## Architecture

### Components by crate (push the feature gate as deep as possible)

```
primer-core
 ├─ conversation::PedagogicalIntent        (existing — 9 variants)
 ├─ router::RouterMode { LocalOnly, CloudPreferred, Hybrid }   (new)
 ├─ router::RoutingSignals { intent, retrieved_passages }      (new)
 ├─ router::complexity_score(&RoutingSignals, &Prompt) -> f32  (new, PURE)
 ├─ router::order_legs(mode, score) -> LegOrder                (new, PURE)
 ├─ consts::router { weights, threshold, caps }                (new)
 └─ inference::GenerationParams { …, routing: Option<RoutingSignals> }  (field add)

primer-inference
 └─ router::RouterBackend  (decorator; impl InferenceBackend)  (new)

primer-engine::wiring
 └─ build_main_backend(...)  — extended to produce RouterBackend
                               when mode != LocalOnly             (edit)

primer-cli / primer-gui
 └─ --router-mode flag / Settings "Routing mode" picker          (edit)
```

`RouterMode`, `RoutingSignals`, and the two pure functions live in
`primer-core` because both `primer-engine::wiring` (constructs the router) and
the future `primer-pedagogy` paths reference them, and `primer-pedagogy` cannot
depend on `primer-inference` (it only sees `&dyn InferenceBackend`). The pure
score/order functions having no inference dependency is what lets them be
unit-tested on the default `cargo test` with zero I/O.

### The two legs, and unification with `FallbackBackend`

The router is built by the **same** `build_main_backend` path that builds the
fallback today, reusing its primary/secondary two-leg plumbing verbatim:

- **primary leg** = `--backend` / `--model` (typically local/small:
  llamacpp / qnn / ollama).
- **secondary leg** = `--fallback-backend` / `--fallback-model` (typically
  cloud/strong).

| `--router-mode` | secondary configured | result |
| --- | --- | --- |
| unset or `local-only` | yes | **`FallbackBackend`** (today's behavior verbatim — local primary, secondary only on pre-stream failure; no routing) |
| unset or `local-only` | no | primary alone (today's behavior) |
| `hybrid` / `cloud-preferred` | yes | **`RouterBackend`** (per-turn routing + self-failover) |
| `hybrid` / `cloud-preferred` | no | **error** — routing needs a secondary leg to route to |

So the router is **purely additive**: `FallbackBackend` stays as the
resilience-only building block, and the router reuses all of its existing CLI
flags and GUI mirror config. No new `--router-cloud-*` flags.

`RouterBackend::name()` returns the **primary's** name verbatim — load-bearing
for `primer_core::backend::is_small_context_backend` (the per-backend context
budget keys off `name()`), exactly as `FallbackBackend` does. The prompt window
is sized once per turn for the common (primary) case; a secondary handling a
slightly smaller window is fine.

## Routing policy

### Signals

```rust
/// Structured signals the dialogue manager knows but the bare Prompt does
/// not carry as data. Threaded through GenerationParams.routing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingSignals {
    /// The pedagogical intent decided for THIS turn.
    pub intent: PedagogicalIntent,
    /// How many knowledge passages RAG retrieved for this turn.
    pub retrieved_passages: usize,
    // Reserved extension point (see §6): recent_primary_ttft_ms: Option<u64>
}
```

### Composite score (pure, in `primer-core::router`)

```rust
pub fn complexity_score(s: &RoutingSignals, prompt: &Prompt) -> f32 {
    intent_weight(s.intent)                 // table below
        + passage_term(s.retrieved_passages) // min(n, CAP) * W_PASSAGE
        + message_term(prompt)               // last child msg: length + '?'-depth
}
```

- **`intent_weight`** — a `match` over the 9 `PedagogicalIntent` variants. Higher
  weight = more likely to route to the strong secondary leg. Initial values
  (named consts in `consts::router`, flagged tunable; calibration needs real
  usage data like the bench numbers):

  | intent | weight | rationale |
  | --- | --- | --- |
  | `Scaffolding` | 0.45 | child is struggling — the best explanation/analogy matters most |
  | `DirectAnswer` | 0.40 | factual, knowledge-intensive |
  | `AnswerThenPivot` | 0.40 | factual answer + Socratic follow-up |
  | `Extension` | 0.30 | "now what if…?" — can get nuanced |
  | `ComprehensionCheck` | 0.25 | probing genuineness — moderate |
  | `SocraticQuestion` | 0.15 | routine guiding question |
  | `Encouragement` | 0.00 | short, emotional, routine |
  | `SessionClose` | 0.00 | trivial |
  | `SuggestBreak` | 0.00 | trivial, wallclock-driven |

- **`passage_term`** — `min(retrieved_passages, ROUTE_PASSAGE_CAP) * W_PASSAGE`.
  Knowledge-grounded turns lean toward the stronger synthesizer. Capped so a
  large retrieval doesn't dominate the score.
- **`message_term`** — derived from the **last child message** in `prompt.messages`
  (the most recent `Role::User`): a length component (word count above
  `MSG_LONG_WORDS` adds `W_MSG_LONG`) plus a question-depth component (number of
  `?` above 1 adds `W_MSG_QUESTION`, capped). Pure string analysis — no NLP dep.

All weights, caps, and the threshold are named consts in `consts::router`:
`ROUTE_SECONDARY_THRESHOLD = 0.5`, `ROUTE_PASSAGE_CAP = 3`, `W_PASSAGE = 0.15`,
`MSG_LONG_WORDS = 30`, `W_MSG_LONG = 0.20`, `W_MSG_QUESTION = 0.10`,
`MSG_QUESTION_CAP = 2`. (No magic numbers — every value lives here.)

### Mode → ordered leg pair (pure, in `primer-core::router`)

```rust
pub enum Leg { Primary, Secondary }
pub struct LegOrder { pub first: Leg, pub second: Option<Leg> }

pub fn order_legs(mode: RouterMode, score: f32) -> LegOrder {
    match mode {
        RouterMode::LocalOnly =>
            LegOrder { first: Leg::Primary, second: None },        // never secondary
        RouterMode::CloudPreferred =>
            LegOrder { first: Leg::Secondary, second: Some(Leg::Primary) },
        RouterMode::Hybrid if score >= ROUTE_SECONDARY_THRESHOLD =>
            LegOrder { first: Leg::Secondary, second: Some(Leg::Primary) },
        RouterMode::Hybrid =>
            LegOrder { first: Leg::Primary, second: Some(Leg::Secondary) },
    }
}
```

`LocalOnly` yields `second: None` — defensive: even if a `RouterBackend` were
somehow constructed in `LocalOnly` mode it would never reach the secondary.
(In practice `build_main_backend` never builds a `RouterBackend` for
`LocalOnly`; it builds the fallback/primary path instead.)

### Failover at the decorator (`RouterBackend::generate_stream`)

```rust
let signals = params.routing.as_ref();              // None ⇒ score 0.0
let score = signals.map(|s| complexity_score(s, prompt)).unwrap_or(0.0);
let order = order_legs(self.mode, score);
let first = self.leg(order.first);
match first.generate_stream(prompt, params).await {
    Ok(stream) => Ok(stream),                       // mid-stream errors propagate as-is
    Err(e) => match order.second {
        Some(second_leg) => {
            tracing::warn!(target: "primer::router", first = first.name(),
                second = self.leg(second_leg).name(), error = %e,
                "routed leg failed pre-stream; falling back to other leg");
            self.leg(second_leg).generate_stream(prompt, params).await
        }
        None => Err(e),
    },
}
```

Identical pre-stream boundary to `FallbackBackend`: once a 2xx stream begins,
a mid-stream error propagates and the partial Primer turn drops at the
dialogue-manager layer. **No mid-stream re-routing, ever** — the child never
hears two backends answer the same turn.

`is_available()` returns true if **either** leg is available (mirrors
`FallbackBackend`).

When `params.routing` is `None` (e.g. a caller that doesn't populate it), the
score is `0.0`, so `Hybrid` routes to the primary and `CloudPreferred` still
routes to the secondary — sensible defaults that never panic.

## Dialogue-manager integration (the one structural touch)

`respond_to_streaming` already computes `intent` before inference. Two small,
localized changes in `dialogue_manager/turn.rs`:

1. **`build_turn_prompt` surfaces the retrieved-passage count.** Change its
   return type from `Prompt` to `(Prompt, usize)` (the count comes from the
   `retrieve_knowledge` result it already computes internally).
2. **`stream_inference_response` populates `params.routing`.** It takes `intent`
   and `passage_count` and sets
   `params.routing = Some(RoutingSignals { intent, retrieved_passages: passage_count })`.

Honest scope note: this is slightly more than "one line" because the passage
count must be threaded out of `build_turn_prompt`, but it stays confined to
`turn.rs`. Every other backend ignores `params.routing`, so no other call site
changes. `GenerationParams::default()` sets `routing: None`.

## Latency-aware switching — extension point (deferred)

No latency logic ships in this spec. The seam is already shaped for it:

- `RoutingSignals` reserves a future `recent_primary_ttft_ms: Option<u64>`.
- `complexity_score` would add a latency term (e.g. "if recent local TTFT >
  budget, add `W_LATENCY`"), nudging slow-local turns toward the secondary.
- The dialogue manager would track a rolling primary-leg TTFT and pass it in.

The real threshold/budget needs the owner-gated llama.cpp + QNN bench numbers
(p50/p95 TTFT per accelerator) that this repo has not yet collected. Guessing a
magic threshold now would be premature; the extension point keeps the door open
without committing an unvalidated constant.

## CLI surface

- New flag: `--router-mode local-only|cloud-preferred|hybrid` (default
  `local-only`).
- Reuses the existing `--fallback-backend` / `--fallback-model` for the secondary
  leg — no new flags.
- `--router-mode hybrid` (or `cloud-preferred`) with **no** secondary configured
  is a clear startup error: "routing requires a secondary leg; set
  `--fallback-backend` (and `--fallback-model` where required)".
- `BackendParams` gains a `router_mode: RouterMode` field (round-trips through
  the wiring struct like every other backend-affecting flag).

## GUI surface

Standard 3-struct mirror pattern (the same one used for the fallback in #205):
`BackendConfig` / `BackendConfigView` / `BackendConfigUpdate` each gain a
`router_mode` field. **The `Update` DTO has NO `#[serde(default)]`**, so
`settings.js::gather()` must send `router_mode` in every `update_settings`
payload (and every IPC test payload) or the save fails to deserialize. A
Settings → Inference backend "Routing mode" picker (default "local only (no
routing)") drives it; the secondary leg reuses the existing fallback picker.
`wiring.rs::build_with_strategy` passes `router_mode` into `build_main_backend`.

**Scope question for the plan:** this GUI mirror is proposed as the final task
in *this* implementation plan (well-understood pattern). If preferred, it can be
split into a separate follow-up spec — flag at spec review.

## Privacy posture

- **Default is `local-only`** — the router never sends anything to the cloud; the
  runtime works with zero network ([[project_strict_offline_first]]).
- Selecting `hybrid` / `cloud-preferred` **and** configuring a cloud secondary is
  the explicit consent — the same opt-in model as the fallback.
- **Absolute privacy is trivially achievable and must be documented as such:** a
  user who wants zero cloud traffic simply leaves `--router-mode` at `local-only`
  (the default) and/or never provides an API key — with no API key the cloud leg
  cannot build, so even a misconfigured `hybrid` degrades to primary-alone. The
  README/user docs must state this explicitly.
- Worth surfacing in the mode's documentation: in `hybrid` mode the
  *highest-complexity* turns (a struggling child, hard factual questions) are
  exactly the ones routed off-device. That is the intended trade — the strongest
  model for the hardest pedagogical moments — but the mode name and docs should
  make the posture legible so it is a conscious choice.

## Testing (TDD)

**Pure functions (`primer-core::router`, default `cargo test`, no I/O):**
- `intent_weight` — every variant maps to its documented weight (table-driven;
  guards against a new `PedagogicalIntent` variant silently defaulting).
- `message_term` — long message adds weight; multiple `?` add (capped) weight;
  short single-question message scores low; empty/missing user message = 0.
- `passage_term` — monotonic up to the cap, then flat.
- `complexity_score` — representative turns land on the intended side of the
  threshold (e.g. `Scaffolding` + 2 passages + long question ⇒ ≥ threshold;
  `Encouragement` + 0 passages + short ⇒ < threshold).
- `order_legs` — all three modes × above/below threshold; `LocalOnly` ⇒
  `second: None`; `CloudPreferred` ⇒ secondary-first regardless of score.

**`RouterBackend` (mock backends, clone the `fallback.rs` `MockBackend`
harness):**
- routes to secondary when score ≥ threshold (Hybrid); to primary when below.
- `CloudPreferred` always tries secondary first; `LocalOnly` never calls
  secondary.
- pre-stream failure on the chosen leg ⇒ transparently uses the other leg
  (both directions).
- a **mid-stream** error propagates and the other leg is **not** called.
- `name()` returns the primary's name.
- `params.routing == None` ⇒ score 0.0 (no panic; Hybrid ⇒ primary).

**Wiring (`build_main_backend` matrix):**
- `LocalOnly` + secondary ⇒ `FallbackBackend` (unchanged).
- `Hybrid` / `CloudPreferred` + secondary ⇒ `RouterBackend`.
- `Hybrid` + no secondary ⇒ error.
- extend the existing `plan_main_backend`-style tests with the mode dimension.

## Files touched (anticipated)

- `primer-core/src/router.rs` (new) — `RouterMode`, `RoutingSignals`,
  `complexity_score`, `order_legs`, `Leg`/`LegOrder`, unit tests.
- `primer-core/src/consts.rs` — new `pub mod router`.
- `primer-core/src/inference.rs` — `GenerationParams.routing` field + default.
- `primer-core/src/lib.rs` — `pub mod router;`.
- `primer-inference/src/router.rs` (new) — `RouterBackend` + tests.
- `primer-inference/src/lib.rs` — `pub use router::RouterBackend;`.
- `primer-engine/src/wiring.rs` — extend `build_main_backend`; `BackendParams.router_mode`.
- `primer-cli/src/main.rs` — `--router-mode` flag + validation.
- `primer-gui/src/config.rs` + `commands` + `ui/settings.js` — `router_mode` mirror.
- `CLAUDE.md`, `README.md`, `ROADMAP.md` — docs (incl. the absolute-privacy note).

## Open questions for spec review

1. **Intent-weight starting values** — approved in principle; flag any variant
   you'd weight differently.
2. **GUI mirror in this plan vs. a follow-up spec** (see GUI surface §).
3. **`cloud-preferred` semantics** — defined here as "always secondary-first,
   primary on pre-stream failure". Confirm that's the intended meaning (vs.
   "secondary for everything except trivial turns").
