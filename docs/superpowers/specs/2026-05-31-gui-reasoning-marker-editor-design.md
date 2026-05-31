# GUI reasoning-marker editor — design

**Date:** 2026-05-31
**Status:** approved (brainstorming → spec)
**Scope:** the deferred GUI half of the reasoning-token-stripping feature shipped in PR #187 (ROADMAP 0.3).

## Problem

PR #187 added chain-of-thought stripping for the `ollama` and `openai-compat`
inference backends: any text between configured `(open, close)` marker pairs is
suppressed before it reaches a child. The built-in defaults
(`<think>…</think>`, Gemma4 `<|channel>…<channel|>`) always apply. The **CLI**
can append custom pairs via the repeatable `--reasoning-marker '<OPEN>'
'</CLOSE>'` flag. The **GUI** gets default stripping for free (it builds the
same backends) but has **no way to add custom pairs** — its
`BackendParams.reasoning_markers` is hard-coded to `Vec::new()`.

This spec covers exposing custom reasoning markers in GUI Settings so a user can
add pairs for a model whose reasoning delimiters are not in the default table.

## Non-goals

- No change to the stripping engine, the default marker table, or the CLI flag.
- No new validation UI (malformed lines are silently dropped, matching the CLI).
- No regex markers — literal text only, same as today.
- Not unifying the CLI's `pair_reasoning_markers` with the GUI parser; their
  input shapes differ (pre-tokenized clap pairs vs. free text).

## Decision: raw string in config + pure Rust parser

The textarea holds free text (`open<whitespace>close`, one pair per line). The
engine needs structured `Vec<(String, String)>` pairs. **The conversion lives in
a pure, unit-tested Rust function**, not in the frontend:

- The raw textarea text is stored verbatim as a `String` on the GUI config and
  its View/Update DTOs.
- `parse_reasoning_markers(&str) -> Vec<(String, String)>` converts it at
  session-wiring time.
- The frontend does **zero parsing** — `gather()` sends the textarea value
  as-is; `populate()` echoes the stored string back into the textarea.

Rationale: this is the best fit for the repo's TDD + pure-function ethos. The
parser gets exhaustive Rust unit tests; the frontend is a verbatim pass-through
with nothing to test (and the repo has no JS test harness). The persisted
`gui-config.json` holds the exact text the user typed, so the field round-trips
losslessly. The rejected alternative — storing a structured `Vec<{open,close}>`
and parsing in `settings.js::gather()` — would put non-trivial parsing logic in
untested JavaScript.

## Parser semantics

`parse_reasoning_markers(text: &str) -> Vec<(String, String)>`:

1. Split `text` into lines (Rust `str::lines()` — CRLF-safe).
2. For each line: trim leading/trailing whitespace.
3. Split the trimmed line on the **first** whitespace run:
   - `open` = the first token (text before the first whitespace).
   - `close` = the remainder, trimmed.
4. Drop the line if `open` is empty (no whitespace on the line → no close) or
   `close` is empty (whitespace but nothing after it).
5. Collect surviving `(open, close)` pairs in document order.

Consequences, made explicit:

- `close` is "the rest of the line after the first whitespace", so a close
  marker **may contain internal spaces** (e.g. `<a> </a> tail` →
  `("<a>", "</a> tail")`). This is more permissive than the CLI's single-arg
  close, and is the natural reading for a line-based textarea.
- Blank lines and whitespace-only lines are ignored.
- An incomplete line (open with no close) is silently dropped — mirrors the
  CLI's `pair_reasoning_markers` dropping a trailing unpaired value. No error,
  no UI warning.
- Empty input ⇒ empty `Vec` ⇒ built-in defaults only (today's behavior).

## Components / changes

1. **`primer-gui/src/config.rs`** — add `reasoning_markers: String` (default
   `""`) to `BackendConfig`, `BackendConfigView`, and `BackendConfigUpdate`.
   Thread it through:
   - the `Default for BackendConfig` impl (`String::new()`),
   - `From<&GuiConfig> for GuiConfigView` (clone into the View),
   - `GuiConfigUpdate::into_config` (move into the resolved config).
   **No `#[serde(default)]` on the `BackendConfigUpdate` field** — it is
   mandatory in the IPC payload, consistent with every sibling field (the
   documented `BackendConfigUpdate`-has-no-`serde(default)` gotcha).
   `BackendConfig` itself keeps its struct-level `#[serde(default)]`, so an
   existing `gui-config.json` without the key loads as `""`.

2. **`primer-gui/src/reasoning_markers.rs`** *(new)* — the pure
   `parse_reasoning_markers` function plus its unit tests. Keeping it in its own
   module keeps `wiring.rs` lean and the parser independently testable.

3. **`primer-gui/src/wiring.rs`** — in `build_with_strategy`, replace
   `reasoning_markers: Vec::new()` with
   `parse_reasoning_markers(&backend_config.reasoning_markers)`.

4. **`primer-gui/ui/index.html`** — add a `<textarea
   id="f-backend-reasoning-markers">` inside the "Inference backend"
   `<details>` group, wrapped in a `<label id="f-backend-reasoning-markers-field">`
   (so the show/hide logic can target it), with a hint: one `open close` pair
   per line, built-in defaults always apply, literal text not regex.

5. **`primer-gui/ui/settings.js`**:
   - DOM refs for the textarea (`backendReasoningMarkers`) and its field wrapper
     (`backendReasoningMarkersField`).
   - `populate()` sets `.value` from `view.backend.reasoning_markers`.
   - `gather()` adds `reasoning_markers: f.backendReasoningMarkers.value`
     (verbatim — no trimming, so the stored text round-trips exactly).
   - Show the field **only for `ollama` + `openai-compat`** (markers are ignored
     by stub/cloud/qnn), mirroring the existing `*-field` show/hide pattern keyed
     off `backendKind`.

## Testing (TDD)

Write the parser tests first, then the parser. Rust unit tests for
`parse_reasoning_markers`:

- empty string → `[]`
- single `open close` pair
- multiple lines → multiple pairs in order
- leading/trailing whitespace on a line trimmed
- blank lines interspersed are ignored
- CRLF line endings handled
- a line with no whitespace (open only) is dropped
- a line with open + trailing whitespace but empty close is dropped
- tab as the separator works
- close with internal spaces preserved (`<a> </a> tail` → `("<a>", "</a> tail")`)

Plus:

- a config round-trip test: a `BackendConfigUpdate` carrying `reasoning_markers`
  resolves into a `BackendConfig` with the same string; the `GuiConfigView`
  carries it back out.
- a wiring test: a `BackendConfig` whose `reasoning_markers` string parses to N
  pairs yields a `BackendParams.reasoning_markers` of those N pairs.

Frontend: verbatim pass-through, no JS harness in the repo, nothing to unit-test.

## Acceptance criteria

On a GUI build (`cargo run --bin primer-gui`):

- Settings → Inference backend → with `ollama` (or `openai-compat`) selected, a
  "Reasoning markers" textarea is visible; with `stub`/`cloud`/`qnn` selected it
  is hidden.
- Enter `[[r]] [[/r]]`, Save & start a new session. A model that emits
  `[[r]]…[[/r]]` around its reasoning has that span stripped from the
  child-visible response.
- An empty textarea ⇒ only the built-in defaults strip (no change from today).
- The entered text round-trips: reopen Settings and the textarea shows
  `[[r]] [[/r]]` again.

## Risks / open points

- **Custom markers propagate to subsystem backends** (classifier, extractor,
  comprehension) — same as the CLI, because they share `BackendParams` via
  `build_backend`. Intentional and already documented on the
  `BackendParams.reasoning_markers` field; the GUI inherits this for free.
- The permissive "close = remainder of line" rule means a user who types three
  tokens on a line gets a two-token close. Acceptable: markers are literal and
  users type exactly what the model emits; the hint documents the format.
