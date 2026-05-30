# Reasoning-token stripping — design

**Date:** 2026-05-30
**Status:** approved, pre-implementation
**Scope:** strip per-model chain-of-thought ("reasoning") markers from `OllamaBackend` and `OpenAiCompatBackend` streamed output so they never reach a child.

## Problem

Reasoning-mode models (DeepSeek-R1, QwQ, Qwen3, Gemma4-thinking, medgemma, …) emit
their internal chain-of-thought wrapped in control markers, then the visible answer.
`OllamaBackend` and `OpenAiCompatBackend` currently forward every byte of the stream
verbatim, so the reasoning trace leaks into the child-visible response. The same models
are reachable through *both* backends (Ollama and any OpenAI-compatible server — oMLX,
LM Studio, vLLM, llama.cpp `--server`), so the leak is not Ollama-specific.

This violates the product's core constraint: the Primer's visible output is read by a
child, and a raw reasoning dump is confusing at best and pedagogically harmful at worst.

The markers stream **token-by-token**, so a single block arrives split across many
`TokenChunk`s, e.g. `<thi` | `nk>Let me` | ` think…` | `</thi` | `nk>answer`. A
per-chunk or per-line `str::replace` therefore cannot work — the open and close markers
get split across chunk boundaries. Stripping must be a **stateful streaming filter**.

## Decisions (locked)

1. **Scope:** one shared, pure, well-tested filter in `primer-core`, wired into **both**
   `OllamaBackend` and `OpenAiCompatBackend`. (`CloudBackend`/Anthropic uses a separate
   top-level thinking field, not inline markers, so it is out of scope. `QnnBackend` is
   device-unverified and Android-only — out of scope for now; it can adopt the same
   shared filter later for free.)
2. **Behavior:** reasoning content is **dropped** from the visible/returned text, but
   **captured** and emitted via `tracing::debug!(target: "primer::reasoning", …)` for
   developer visibility. **If the model produced reasoning but no visible answer**, the
   child gets a friendly localized "I'm having a thinking problem right now, please try
   again" message instead of a blank turn (see Component 4).
3. **Config — built-in marker table, always-on.** Stripping is always active against a
   curated built-in set of `(open, close)` marker pairs in `consts.rs`. There is no flag
   to forget; a non-reasoning model simply never emits these markers, so stripping is a
   no-op when they are absent.
4. **Config surface — built-in defaults + custom-extend in CLI *and* GUI.** Built-in
   defaults are always active; both the CLI and the GUI expose a way to **append** custom
   marker pairs at runtime for exotic models.

## Why `generate()` is covered for free

`InferenceBackend::generate()` (the non-streaming path used by the classifier, extractor,
and comprehension crates to get JSON back) has a default impl that **aggregates
`generate_stream()`**. Fixing the stream therefore also cleans the non-streaming path —
reasoning markers stop polluting the JSON those structured-output crates parse. No
separate work needed.

## Component 1 — pure filter (`primer-core/src/reasoning.rs`)

A self-contained state machine. No I/O, no logging, no `async`. This is where the
testing rigor concentrates.

```rust
/// One reasoning-marker pair. `open` switches the filter into the
/// suppressing state; `close` switches it back to passthrough.
pub struct ReasoningMarker { pub open: String, pub close: String }

enum State {
    /// Passing text through, watching for any marker's `open`.
    Outside,
    /// Suppressing text, watching for this specific `close`.
    Inside { close: String },
}

pub struct ReasoningFilter {
    markers: Vec<ReasoningMarker>,
    state: State,
    /// Cross-chunk remainder: text we have not yet been able to classify
    /// because it might be the start of a split marker.
    buf: String,
    /// Captured reasoning text, drained by the caller for tracing.
    suppressed: String,
    /// Sticky "ever suppressed any reasoning byte" flag for this stream.
    did_suppress: bool,
}

impl ReasoningFilter {
    /// Build a filter over a set of marker pairs. Empty markers ⇒ identity passthrough.
    pub fn new(markers: Vec<ReasoningMarker>) -> Self;

    /// Feed one streamed chunk; return the visible text to forward (may be empty).
    pub fn push(&mut self, chunk: &str) -> String;

    /// Flush at end of stream. Outside ⇒ emit the remaining held buffer
    /// (a partial that never completed a marker is real text). Inside ⇒
    /// DROP the remaining buffer (stream ended mid-reasoning) and capture it.
    pub fn finish(&mut self) -> String;

    /// Take the captured reasoning accumulated since the last drain, for logging.
    pub fn drain_suppressed(&mut self) -> String;

    /// True once any reasoning byte has been suppressed this stream.
    /// Independent of `drain_suppressed` (which only empties the log buffer).
    pub fn did_suppress(&self) -> bool;
}
```

### Algorithm

State `Outside`:
- Append the incoming chunk to `buf`.
- Find the earliest occurrence of any marker's `open` in `buf`.
  - **Found** at index `i`: emit `buf[..i]`, drop the matched `open`, switch to
    `Inside { close }` for that marker, and continue scanning the remainder.
  - **Not found:** emit everything in `buf` *except* the longest suffix that is a proper
    prefix of some marker's `open` (held back in case the marker is split across the next
    chunk). Keep that suffix in `buf`.

State `Inside { close }`:
- Append the incoming chunk to `buf`.
- Find `close` in `buf`.
  - **Found** at index `i`: append `buf[..i]` to `suppressed`, drop the matched `close`,
    switch to `Outside`, and continue scanning the remainder.
  - **Not found:** move everything except the longest suffix that is a proper prefix of
    `close` into `suppressed`; keep that suffix in `buf`. Emit nothing.

`finish()`:
- `Outside`: the held `buf` is real text that never turned out to be a marker → emit it.
- `Inside`: the stream ended inside a reasoning block (unbalanced/truncated). **Drop**
  `buf` into `suppressed` and emit nothing. Never leak a partial CoT.

`did_suppress()` is set true the first time any byte is appended to `suppressed`. The
backend uses it to decide whether the reasoning-without-answer fallback applies
(Component 4).

### Safety invariant

**No byte between an open marker and its matching close marker is ever returned to the
caller** — including when markers are split across chunks, when multiple blocks appear in
one stream, and when a block is left unbalanced at end-of-stream.

### Edge case: false-prefix then real text

`<thinking out loud>` must NOT be treated as `<think>`. The "longest suffix that is a
proper prefix of a marker" hold-back resolves this: once enough bytes arrive to prove the
buffer is not an exact `open`, the non-matching bytes are emitted as ordinary text.
(`<think>` requires exactly `>` after `<think`; `<thinki…` diverges at `i` vs `>` and is
released.) This is a required test case.

## Component 2 — built-in marker table (`primer-core/src/consts.rs`)

```rust
/// Defaults for reasoning-token stripping (see `crate::reasoning`).
pub mod reasoning {
    /// `(open, close)` marker pairs stripped by default on every
    /// Ollama / openai-compat stream. A non-reasoning model never emits
    /// these, so the filter is a no-op when they are absent.
    pub const DEFAULT_MARKERS: &[(&str, &str)] = &[
        // DeepSeek-R1, QwQ, Qwen3, and the de-facto community convention.
        ("<think>", "</think>"),
        // Gemma4 thinking channel. Output is wrapped as
        //   `<|channel>thought\n[reasoning]<channel|>`
        // with the final answer OUTSIDE the markers (confirmed by the
        // ollama gemma4 docs' disabled-mode example
        //   `<|channel>thought\n<channel|>[Final answer]`).
        // Note the deliberate asymmetry: open is `<|channel>` (pipe after
        // `<`), close is `<channel|>` (pipe before `>`). Stripping the
        // whole channel removes the `thought\n` label too; the visible
        // answer survives because it is outside the pair.
        ("<|channel>", "<channel|>"),
    ];
}
```

The Gemma4 markers are taken from the ollama gemma4 model docs (Thinking Mode
Configuration section). A `#[ignore]`'d live-model wiring test (Component 6) running
`gemma4:e4b` via ollama empirically confirms the exact bytes; if the real stream diverges
from the docs, that test surfaces it and the cure is a one-line edit to this table.

## Component 3 — backend wiring (Ollama + OpenAI-compat)

Each backend gains a `reasoning_markers: Vec<ReasoningMarker>` field:

- `new(...)` populates it from `consts::reasoning::DEFAULT_MARKERS`.
- `with_extra_markers(self, extra: Vec<(String, String)>) -> Self` **appends** custom
  pairs to the defaults (builder style; returns `Self`).

Inside the spawned streaming task (both backends follow the identical fire-and-forget
`mpsc` pattern today):

1. Construct a `ReasoningFilter::new(self.reasoning_markers.clone())` before the loop, and
   a running `visible_bytes_forwarded: usize = 0` counter.
2. For each parsed `TokenChunk`, run `chunk.text` through `filter.push(&text)`. Add the
   returned length to the counter.
3. Forward a `TokenChunk { text: visible, done }` **only when** `visible` is non-empty
   **or** `done` is set.
4. After each step, `let r = filter.drain_suppressed(); if !r.is_empty() {
   tracing::debug!(target: "primer::reasoning", backend = self.name(), suppressed = %r); }`.
5. On the `done` chunk, call `filter.finish()` first and include any flushed text in the
   final emission. Then apply the zero-answer check below.

**Reasoning-without-answer check (at done/finish):** if `visible_bytes_forwarded == 0`
AND `filter.did_suppress()` is true, send
`Err(PrimerError::Inference(InferenceError::ReasoningWithoutAnswer))` into the channel
instead of a final `done` chunk (Component 4). Zero visible bytes with *no* suppression is
a different failure (an empty model response) and is left to existing behavior — this
fallback fires only when reasoning was the cause.

## Component 4 — reasoning-without-answer fallback (i18n)

When a reasoning model thinks but never produces a visible answer (truncated mid-thought,
or a complete reasoning block followed by empty output), the child must not see a blank
turn. They get a friendly, localized message inviting them to try again.

**This is user-facing text, so per [[project_multilingual_intent]] it must not be a string
literal in the backend or in the pure filter.** It routes through the existing single i18n
boundary instead:

- New locale-neutral variant `InferenceError::ReasoningWithoutAnswer` (no fields) in
  `primer-core/src/error.rs`. Its `#[error(...)]` (dev-facing `Display`) is a plain
  English diagnostic; `is_retryable()` returns **false** — it is not a transient HTTP
  condition, and the "try again" is the child re-asking, not an automatic retry (and
  mid-stream errors are not auto-retried anyway).
- A new arm in each of `render_english` / `render_german` / `render_hindi` inside
  `primer_core::i18n` (each `match` is exhaustive per locale, so the compiler forces all
  three — no locale can silently fall through). English draft: *"Oops — I'm having a
  thinking problem right now. Could you ask me that again?"*
- The backend streaming task emits the variant on the zero-visible-but-suppressed
  condition (Component 3).

**Why this reuses everything:** a mid-stream error already flows through the established
graceful-error path — `DialogueManager` drops the partial Primer turn (the child turn
stays), and the CLI/GUI render the message via `render_inference_error(err, locale)`. No
new rendering, dropping, or display code is required; only the new variant + its
translations + the one emit site per backend.

Tests: `render_inference_error(&ReasoningWithoutAnswer, &Locale::{English,German,Hindi})`
returns a non-empty, locale-appropriate string (Hindi contains Devanagari, matching the
existing guard tests); `is_retryable()` is false; a backend wiring test that a stream of
only-reasoning yields the `ReasoningWithoutAnswer` error rather than an empty `done` chunk.

## Component 5 — config plumbing

### CLI (`primer-cli`)

- New repeatable flag: `--reasoning-marker <OPEN> <CLOSE>` (clap `num_args = 2`,
  `action = Append`), collected as `Vec<(String, String)>`. Each occurrence appends one
  pair to the built-in defaults. Documented in the flags help and the `main.rs` arg doc
  comment.
- New `reasoning_markers: Vec<(String, String)>` field on `BackendParams`
  (`primer-engine/src/wiring.rs`), always present (not feature-gated).
- `build_backend`'s `"ollama"` and `"openai-compat"` arms call
  `.with_extra_markers(params.reasoning_markers.clone())` on the constructed backend.

### GUI (`primer-gui`)

Mirrors the CLI through the existing settings/IPC machinery:

- New `reasoning_markers: Vec<ReasoningMarkerDto>` field (`{ open: String, close: String }`)
  on the backend config struct, the `BackendConfigView` read DTO, and the
  `BackendConfigUpdate` write DTO.
- **It is NOT a secret**, so it passes through the View/Update DTOs verbatim — no
  Env/Inline/Keep redaction dance (unlike the API-key fields).
- **`BackendConfigUpdate` has no `#[serde(default)]`**, so every field is mandatory in the
  `update_settings` IPC payload. `settings.js::gather()` MUST send `reasoning_markers`
  (an empty array when the user has entered nothing) or the save fails to deserialize.
  This is the documented GUI gotcha and the single highest-risk wiring point.
- Settings UI: a minimal `<textarea>`, one `open<sep>close` pair per line. **Separator is
  the first run of whitespace on the line** (so a tab or any spaces both work); the open
  marker is everything before it, the close marker everything after (both trimmed). Lines
  that are blank or have no whitespace separator are skipped. Markers never contain
  leading/trailing whitespace, so this is unambiguous. Parsed into the array on
  `gather()`; empty textarea ⇒ empty array ⇒ defaults only. Deliberately a textarea, not a
  dynamic add-a-row builder (YAGNI for a power-user escape hatch).

## Component 6 — testing (TDD)

Pure-filter unit tests (`reasoning.rs`, the bulk of the rigor — written first):

- passthrough with no markers configured;
- passthrough of text containing no markers;
- single block stripped within one chunk;
- **open marker split across two/three chunks**;
- **close marker split across two/three chunks**;
- multiple distinct blocks in one stream;
- **unbalanced/truncated block** — `Inside` at `finish()` leaks nothing;
- **false-prefix then real text** (`<thinking out loud>` ≠ `<think>` → emitted);
- custom-marker append (a non-default pair is stripped);
- the asymmetric Gemma4 pair `<|channel>…<channel|>` stripped, final answer survives;
- `drain_suppressed()` returns the captured reasoning and empties on re-drain;
- `did_suppress()` is false on a clean passthrough, true after any block.

Backend wiring tests:

- a deterministic in-process test per backend that a `<think>…</think>` block fed through
  the parse+filter path is stripped end-to-end (reuse the existing buffer/parse test
  harness shape; no network);
- a per-backend test that an only-reasoning stream yields `ReasoningWithoutAnswer`;
- a `#[ignore]`'d live-model test (`gemma4:e4b` via ollama) that confirms the real Gemma4
  marker bytes match the table — run on demand, not in CI.

i18n tests: as listed in Component 4.

## Out of scope / YAGNI

- Routing reasoning to a separate stream/field for a GUI "thinking" panel (would touch
  `TokenChunk` and every consumer; revisit if a UI need appears).
- `CloudBackend` / `QnnBackend` wiring (Cloud uses a different mechanism; QNN is
  device-unverified — both can adopt the shared filter later with no filter changes).
- A dynamic add-a-row GUI marker editor (textarea suffices).

## File / line-count notes

- `ollama.rs` (377) and `openai_compat.rs` (656) both stay under or near the 500-line
  guideline after a small field + a few lines in the stream loop. `openai_compat.rs` is
  already over; this change adds little, but if it pushes meaningfully further, factor the
  shared "filter-and-forward" step into a tiny helper (the two stream loops are already
  near-identical and a shared helper would serve a future split). Decide during
  implementation based on the actual delta.
- `reasoning.rs` is a new focused module, well under 500 lines.
