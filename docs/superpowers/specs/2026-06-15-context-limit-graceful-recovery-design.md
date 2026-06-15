# Context-limit graceful recovery — design

**Date:** 2026-06-15
**Issues:** [#224](https://github.com/hherb/primer/issues/224) (mid-sentence truncation) + owner proviso (notify + auto-retry). Builds on [#223](https://github.com/hherb/primer/issues/223) (the context-limit status code) and PR #222 (graceful completion path).
**Status:** approved (brainstorming gate passed 2026-06-15).

## Problem

When the Qualcomm NPU (`QnnBackend`, 2048-token Genie context) fills the context
window mid-generation, `GenieDialog_query` returns
`GENIE_STATUS_CONTEXT_LIMIT_EXCEEDED`. PR #222 made that path complete the turn
gracefully — emitting a terminal `done` chunk with whatever already streamed —
rather than dropping the turn. But:

1. The reply can stop **mid-word/mid-thought**, which is confusing for a child.
2. The terminal chunk is **byte-identical to a clean finish**
   (`TokenChunk { text: "", done: true }`), so the dialogue manager cannot tell a
   truncated turn from a complete one and therefore cannot react.

## Goal

A child never ends a turn on a mid-thought. When the context limit fires, the
Primer **visibly acknowledges** it ("something happened to my memory — let me try
again") and **automatically retries** with a smaller prompt until it produces a
complete answer (or gracefully soft-stops after a bounded number of attempts).

### Pedagogical rationale (owner)

The visible "something went wrong, let me fix it" moment is **itself
pedagogically valuable**: it demonstrates to the child that the unexpected
happened and the Primer is working to fix it. This matches the Primer's overall
goal of inspiring independent thinking and the triangulation of answer sources —
reinforcing the principle that **no source of answers is invariably and
consistently right**, the Primer included. An apology-then-retry models honest
self-correction rather than presenting the Primer as an infallible oracle.

## Design decisions (settled in brainstorming)

| Decision | Choice | Rationale |
| --- | --- | --- |
| Child-facing UX | **Partial + apology + clean retry** | Text streams live and cannot be un-displayed; honest, preserves streaming/TTFT on the slow on-device path. |
| What is persisted | **Only the clean retry answer** | Keeps `turn_comprehensions` / summary / learner-model record coherent; the child's takeaway is the complete answer. |
| Retry strategy | **Progressive shrink, up to 2 retries, then soft-stop** | Drop KB → drop LTM + shrink window → accept partial with a gentle cue. Max 3 inference calls. |
| Signal scope | **General `finish_reason`, QNN is first producer** | Backend-agnostic, fully host-testable, no QNN coupling in `primer-pedagogy`. |

### Reframing of issue #224's literal "trim to last sentence"

Because text streams to the screen token-by-token, the already-displayed mid-word
fragment **cannot be retroactively trimmed** (a terminal cannot un-print).
Issue #224's literal "trim the reply to its last complete sentence" is therefore
**replaced** by *closure via apology + clean retry*: the turn's final
displayed-and-persisted state is always a complete answer, so a child never ends
on a mid-thought. The `primer_core::prompt_budget::truncate_to_tokens` sentence
helper ends up **unused** in this path. The owner approved this reframing as an
improvement (see pedagogical rationale above). No GUI-only retroactive trim is in
scope.

## Architecture

Three layers, one new signal threaded through them.

### Layer 1 — the signal (`primer-core`)

```rust
// primer-core/src/inference.rs

/// Why a streaming generation ended. Meaningful only on the terminal
/// chunk (`done == true`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FinishReason {
    /// The model finished cleanly (default).
    #[default]
    Stop,
    /// Generation was cut off because the context window filled. The
    /// reply already streamed in full but may stop mid-thought.
    Length,
}

pub struct TokenChunk {
    pub text: String,
    pub done: bool,
    pub finish_reason: FinishReason, // Stop on every non-terminal chunk
}

impl TokenChunk {
    /// Mid-stream delta (not done, Stop).
    pub fn delta(text: impl Into<String>) -> Self { /* done: false, Stop */ }
    /// Terminal clean finish (text: "", done, Stop).
    pub fn stop() -> Self { /* … */ }
    /// Terminal context-limit finish (text: "", done, Length).
    pub fn length() -> Self { /* … */ }
}
```

`done` is retained (every non-terminal chunk is `Stop`); the dialogue manager's
retry trigger is `chunk.done && chunk.finish_reason == FinishReason::Length`.
The ergonomic constructors replace the ~12 existing `TokenChunk { text, done }`
struct literals across `primer-inference` (cloud, ollama, openai-compat,
llamacpp, stub, fallback, bench, reasoning_stream, qnn) — a readability win, and
it keeps the new field from being a painful literal-churn.

### Layer 2 — the QNN producer (`primer-inference`)

Only `qnn/genie/mod.rs::emit_query_outcome` changes behaviourally:

- `QueryOutcome::Complete` → `TokenChunk::stop()`
- `QueryOutcome::ContextLimit` → `TokenChunk::length()` (keeps the existing
  `tracing::warn!(target: "primer::qnn", …)`)
- `QueryOutcome::Error` → unchanged (emits `Err`, drops the turn)

`emit_query_outcome` / `classify_query_status` are already pure and host-tested
(they run on `cargo test` without an NPU), so the new behaviour is CI-covered.

Every **other** backend emits `Stop` via the constructors — no behavioural
change. Cloud/ollama mapping their *own* native length finish-reason
(`stop_reason: "max_tokens"` / `done_reason: "length"`) to `FinishReason::Length`
is **opt-in future work**, explicitly out of scope here.

### Layer 3 — the recovery state machine (`primer-pedagogy`)

Two changes in `dialogue_manager/turn.rs`.

**(a) `build_turn_prompt` gains a budget tier** — a pure enum selecting which
optional sections are assembled:

```rust
pub(super) enum PromptBudgetTier {
    Full,        // today's behaviour (KB + LTM summary + retrieved-older + full window)
    NoKnowledge, // drop KB passages, keep LTM
    Minimal,     // drop KB + LTM, shrink the recent-turn window to a floor
}
```

The tier→behaviour mapping is a pure function, unit-testable without a backend.
`Full` is byte-identical to today, so non-truncating turns are unaffected.

**(b) `stream_inference_response` returns the finish reason** (e.g. a small
`StreamOutcome { text: String, finish_reason: FinishReason }`), and
`respond_to_streaming` wraps it in a bounded retry loop:

```
tier = Full
loop:
    (text, reason) = stream_inference_response(prompt@tier, …)   // streams live via on_chunk
    match (reason, tier):
        (Stop, _)               => break                          // success
        (Length, Full)          => emit apology; tier = NoKnowledge; rebuild; continue
        (Length, NoKnowledge)   => emit apology; tier = Minimal;   rebuild; continue
        (Length, Minimal)       => emit soft-stop cue; break       // accept partial
record_primer_turn(final_text_only)   // partial attempts are discarded
```

- The apology / soft-stop text is streamed through the **existing** `on_chunk`
  callback — no signature change (CLI prints it, GUI emits `primer://chunk`,
  voice mode speaks it).
- The child turn is recorded **once**, before the loop. Only the final answer
  becomes the Primer turn, so the classifier/extractor/comprehension turn-pair
  invariant holds and the longitudinal record stays clean.
- Max **3 inference calls**. Retry count and the window floor are named consts in
  `primer_core::consts` — no magic numbers.

### Layer 4 — locale-aware strings (`primer-pedagogy` prompt packs)

Both strings are Primer *speech* (streamed, TTS'd in voice mode), so they live in
the prompt pack like `break_suggestion_intro` — never hard-coded English in Rust
(multilingual principle). Two new keys per pack:

- `memory_limit_retry` — the apology.
- `memory_limit_soft_stop` — the exhausted-retries cue.

Surfaced via `PromptPack` accessors; validated for non-emptiness at pack-load
time (same `validate_*` pattern as `voice_state`) so consumers render them
unconditionally without `Option` plumbing.

Draft copy (final copy reviewed with owner):

| Locale | `memory_limit_retry` | `memory_limit_soft_stop` |
| --- | --- | --- |
| `en` | "Oh — I'm sure up to there, but something just happened to my memory. Let me try that again." | "Let's pause there for now — ask me to keep going whenever you're ready." |
| `de` | "Oh — bis hierhin bin ich mir sicher, aber mit meinem Gedächtnis ist gerade etwas passiert. Ich versuche es noch einmal." | "Machen wir hier erst mal eine Pause — sag mir Bescheid, wenn ich weitermachen soll." |
| `hi` (preview) | best-effort draft, flagged for the translator pass | best-effort draft, flagged for the translator pass |

## Testing (TDD, all host-runnable)

1. **`primer-core`** — `FinishReason` default; `TokenChunk::{delta,stop,length}`
   set the right fields.
2. **`primer-inference`** — extend the existing `emit_query_outcome` tests:
   `ContextLimit` emits a terminal chunk with `finish_reason == Length`;
   `Complete` with `Stop`. (Run on `cargo test`.)
3. **`primer-pedagogy`** — the core coverage:
   - Pure `PromptBudgetTier` mapping (Full/NoKnowledge/Minimal drop the right
     sections).
   - Recovery state machine via a stub backend scripted `Length → Length → Stop`:
     assert 3 calls, two apologies streamed via `on_chunk`, the prompt shrank each
     retry, and only the final `Stop` answer is the recorded Primer turn.
   - `Length → Length → Length` (exhausted): soft-stop cue streamed, partial
     accepted, exactly one Primer turn recorded.
   - `Stop` first try: zero retries, zero apologies, byte-identical to today
     (regression guard).
4. **Prompt packs** — load-time validation rejects an empty `memory_limit_*`;
   accessor returns the right per-locale string.

The only thing that **cannot** be host-verified is the real on-device
context-limit firing — that stays a deferred manual spot-check against the cl2048
bundle (owner-gated), exactly as #224 already scoped.

## Out of scope

- Cloud/ollama/openai-compat mapping their native length finish-reasons to
  `FinishReason::Length` (opt-in follow-up).
- Retroactive trimming of already-displayed partial text (impossible on CLI;
  superseded by apology + retry closure).
- On-device throughput / thermal re-measurement (unchanged by this work).

## Consts introduced (no magic numbers)

- Max retry attempts (= 2 retries ⇒ 3 total inference calls).
- `Minimal`-tier recent-turn window floor.

Both in `primer_core::consts` (new `prompt_recovery` sub-module or alongside the
existing small-context budget consts).
