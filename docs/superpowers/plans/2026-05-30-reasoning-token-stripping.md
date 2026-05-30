# Reasoning-token stripping Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Strip per-model chain-of-thought ("reasoning") markers from `OllamaBackend` and `OpenAiCompatBackend` streamed output so they never reach a child, with a localized "thinking problem, try again" fallback when a model reasons but emits no visible answer.

**Architecture:** A pure, heavily-tested streaming state machine in `primer-core` (`ReasoningFilter`) suppresses any text between configured `(open, close)` marker pairs, robust to markers split across stream chunks. Both backends run each parsed chunk's text through the filter before forwarding, and emit a new `InferenceError::ReasoningWithoutAnswer` (rendered via the existing single i18n boundary) when nothing visible survived. A built-in marker table is always on; the CLI can append custom pairs. (GUI custom-marker editor is deferred to ROADMAP 0.3 — the GUI inherits default stripping for free.)

**Tech Stack:** Rust (edition 2024, toolchain 1.88), `async-trait`, `futures` mpsc streaming, `tracing`, clap. All cargo commands run from `src/` using `~/.cargo/bin/cargo`.

**Spec:** `docs/superpowers/specs/2026-05-30-reasoning-token-stripping-design.md`

---

## File Structure

- **Create** `src/crates/primer-core/src/reasoning.rs` — pure `ReasoningMarker`, `ReasoningFilter`, `finalize_visible()` helper, `default_markers()`, plus all unit tests. One clear responsibility: marker suppression over a token stream.
- **Modify** `src/crates/primer-core/src/lib.rs` — register `pub mod reasoning;`.
- **Modify** `src/crates/primer-core/src/consts.rs` — add `pub mod reasoning { pub const DEFAULT_MARKERS … }`.
- **Modify** `src/crates/primer-core/src/error.rs` — add `InferenceError::ReasoningWithoutAnswer` variant.
- **Modify** `src/crates/primer-core/src/i18n.rs` — add a render arm in `render_english`/`render_german`/`render_hindi` + tests.
- **Modify** `src/crates/primer-inference/src/ollama.rs` — `reasoning_markers` field, `with_extra_markers`, filter wiring in `generate_stream`, tests.
- **Modify** `src/crates/primer-inference/src/openai_compat.rs` — same shape as ollama.
- **Modify** `src/crates/primer-engine/src/wiring.rs` — `reasoning_markers` on `BackendParams`; pass into the two backend arms of `build_backend`.
- **Modify** `src/crates/primer-cli/src/main.rs` — `--reasoning-marker` flag; populate `BackendParams.reasoning_markers`.
- **Modify** `src/crates/primer-gui/src/wiring.rs` — set `reasoning_markers: Vec::new()` at the `BackendParams` construction site (keeps the struct construction compiling; GUI editor deferred).

---

## Task 1: Pure `ReasoningFilter` state machine

**Files:**
- Create: `src/crates/primer-core/src/reasoning.rs`
- Modify: `src/crates/primer-core/src/lib.rs` (register module)
- Modify: `src/crates/primer-core/src/consts.rs` (default marker table)

- [ ] **Step 1: Register the module and the consts table**

In `src/crates/primer-core/src/lib.rs`, add the module declaration in alphabetical position (after `pub mod radio`? no — it sits between `pub mod llm_util;` and `pub mod retry;`). Add:

```rust
pub mod reasoning;
```

In `src/crates/primer-core/src/consts.rs`, append this module at the end of the file:

```rust
/// Defaults for reasoning-token stripping (see [`crate::reasoning`]).
pub mod reasoning {
    /// `(open, close)` marker pairs stripped by default on every Ollama /
    /// openai-compat stream. A non-reasoning model never emits these, so the
    /// filter is a no-op when they are absent.
    ///
    /// - `<think>…</think>`: DeepSeek-R1, QwQ, Qwen3, de-facto convention.
    /// - `<|channel>…<channel|>`: Gemma4 thinking channel. Per the ollama
    ///   gemma4 docs the output is `<|channel>thought\n[reasoning]<channel|>`
    ///   with the final answer OUTSIDE the markers (disabled-mode example:
    ///   `<|channel>thought\n<channel|>[Final answer]`). Note the asymmetry:
    ///   open is `<|channel>` (pipe after `<`), close is `<channel|>` (pipe
    ///   before `>`). Stripping the channel removes the `thought\n` label too;
    ///   the visible answer survives because it is outside the pair.
    pub const DEFAULT_MARKERS: &[(&str, &str)] = &[
        ("<think>", "</think>"),
        ("<|channel>", "<channel|>"),
    ];
}
```

- [ ] **Step 2: Write the failing tests for the pure filter**

Create `src/crates/primer-core/src/reasoning.rs` with ONLY the test module and the public type signatures (bodies `todo!()`), so it compiles-to-fail meaningfully. Paste the full file:

```rust
//! Streaming chain-of-thought ("reasoning") suppression.
//!
//! Reasoning-mode models (DeepSeek-R1, QwQ, Qwen3, Gemma4-thinking, …) wrap
//! their internal reasoning in control markers, then emit the visible answer.
//! Those markers arrive token-by-token, so a single block is split across many
//! stream chunks (`<thi` | `nk>Let me` | `</thi` | `nk>answer`). A per-chunk
//! `str::replace` therefore cannot work — this is a stateful filter.
//!
//! # Safety invariant
//! No byte between an open marker and its matching close marker is ever
//! returned to the caller — across split markers, multiple blocks, and an
//! unbalanced block left open at end-of-stream.

use crate::consts;

/// One reasoning-marker pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReasoningMarker {
    /// Switches the filter into the suppressing state.
    pub open: String,
    /// Switches the filter back to passthrough.
    pub close: String,
}

impl ReasoningMarker {
    /// Convenience constructor from string-likes.
    pub fn new(open: impl Into<String>, close: impl Into<String>) -> Self {
        Self { open: open.into(), close: close.into() }
    }
}

/// The built-in default marker set from [`consts::reasoning::DEFAULT_MARKERS`].
pub fn default_markers() -> Vec<ReasoningMarker> {
    consts::reasoning::DEFAULT_MARKERS
        .iter()
        .map(|(o, c)| ReasoningMarker::new(*o, *c))
        .collect()
}

#[derive(Debug)]
enum State {
    Outside,
    Inside { close: String },
}

/// Stateful streaming filter. Feed chunks via [`push`](Self::push); flush at
/// end of stream via [`finish`](Self::finish).
#[derive(Debug)]
pub struct ReasoningFilter {
    markers: Vec<ReasoningMarker>,
    state: State,
    /// Cross-chunk remainder we cannot yet classify (possible split marker).
    buf: String,
    /// Captured reasoning, drained by the caller for logging.
    suppressed: String,
    /// Sticky: true once any reasoning byte has been suppressed this stream.
    did_suppress: bool,
}

impl ReasoningFilter {
    /// Build a filter over a set of marker pairs. Empty ⇒ identity passthrough.
    pub fn new(markers: Vec<ReasoningMarker>) -> Self {
        Self {
            markers,
            state: State::Outside,
            buf: String::new(),
            suppressed: String::new(),
            did_suppress: false,
        }
    }

    /// Feed one streamed chunk; return the visible text to forward (may be empty).
    pub fn push(&mut self, chunk: &str) -> String {
        todo!()
    }

    /// Flush at end of stream. Outside ⇒ emit the held buffer (a partial that
    /// never completed a marker is real text). Inside ⇒ DROP the held buffer
    /// (stream ended mid-reasoning) and capture it; emit nothing.
    pub fn finish(&mut self) -> String {
        todo!()
    }

    /// Take the reasoning captured since the last drain (for tracing).
    pub fn drain_suppressed(&mut self) -> String {
        std::mem::take(&mut self.suppressed)
    }

    /// True once any reasoning byte has been suppressed this stream.
    pub fn did_suppress(&self) -> bool {
        self.did_suppress
    }
}

/// Decide the final (done-chunk) emission for a streaming backend.
///
/// `total_visible` = visible bytes forwarded BEFORE the done chunk.
/// `tail` = text returned by `filter.finish()` (plus any visible text from the
/// done chunk's own content). Returns `Some(tail)` to emit as the final
/// visible done-chunk text (possibly empty, with `done = true`), or `None` to
/// signal the backend should emit `InferenceError::ReasoningWithoutAnswer`
/// instead — i.e. the model reasoned but produced no visible answer.
pub fn finalize_visible(total_visible: usize, tail: &str, did_suppress: bool) -> Option<String> {
    if total_visible == 0 && tail.is_empty() && did_suppress {
        None
    } else {
        Some(tail.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn think() -> Vec<ReasoningMarker> {
        vec![ReasoningMarker::new("<think>", "</think>")]
    }

    /// Drive a whole stream through the filter and return the concatenated
    /// visible output (push over each chunk, then finish()).
    fn run(markers: Vec<ReasoningMarker>, chunks: &[&str]) -> String {
        let mut f = ReasoningFilter::new(markers);
        let mut out = String::new();
        for c in chunks {
            out.push_str(&f.push(c));
        }
        out.push_str(&f.finish());
        out
    }

    #[test]
    fn no_markers_is_identity() {
        assert_eq!(run(vec![], &["hello ", "world"]), "hello world");
    }

    #[test]
    fn text_without_markers_passes_through() {
        assert_eq!(run(think(), &["just plain text"]), "just plain text");
    }

    #[test]
    fn single_block_in_one_chunk_is_stripped() {
        assert_eq!(
            run(think(), &["before <think>secret</think> after"]),
            "before  after"
        );
    }

    #[test]
    fn open_marker_split_across_chunks() {
        // "<thi" | "nk>secret</think>done"
        assert_eq!(run(think(), &["a<thi", "nk>secret</think>b"]), "ab");
    }

    #[test]
    fn open_marker_split_across_three_chunks() {
        assert_eq!(run(think(), &["a<", "thi", "nk>x</think>b"]), "ab");
    }

    #[test]
    fn close_marker_split_across_chunks() {
        assert_eq!(run(think(), &["a<think>x</thi", "nk>b"]), "ab");
    }

    #[test]
    fn multiple_blocks_in_one_stream() {
        assert_eq!(
            run(think(), &["a<think>1</think>b<think>2</think>c"]),
            "abc"
        );
    }

    #[test]
    fn unbalanced_block_leaks_nothing() {
        // Stream ends inside a reasoning block.
        assert_eq!(run(think(), &["answer<think>still thinking..."]), "answer");
    }

    #[test]
    fn false_prefix_then_real_text_is_emitted() {
        // "<thinking out loud>" is NOT "<think>".
        assert_eq!(
            run(think(), &["I was <thinking out loud> today"]),
            "I was <thinking out loud> today"
        );
    }

    #[test]
    fn custom_marker_appended_pair_is_stripped() {
        let markers = vec![
            ReasoningMarker::new("<think>", "</think>"),
            ReasoningMarker::new("[[r]]", "[[/r]]"),
        ];
        assert_eq!(run(markers, &["a[[r]]hidden[[/r]]b"]), "ab");
    }

    #[test]
    fn gemma4_asymmetric_channel_stripped_answer_survives() {
        let markers = vec![ReasoningMarker::new("<|channel>", "<channel|>")];
        assert_eq!(
            run(markers, &["<|channel>thought\nreasoning<channel|>The answer."]),
            "The answer."
        );
    }

    #[test]
    fn drain_suppressed_returns_captured_then_empties() {
        let mut f = ReasoningFilter::new(think());
        let _ = f.push("a<think>secret</think>b");
        let drained = f.drain_suppressed();
        assert!(drained.contains("secret"));
        assert_eq!(f.drain_suppressed(), "");
    }

    #[test]
    fn did_suppress_false_on_clean_passthrough_true_after_block() {
        let mut f = ReasoningFilter::new(think());
        let _ = f.push("plain");
        assert!(!f.did_suppress());
        let _ = f.push("<think>x</think>");
        assert!(f.did_suppress());
    }

    #[test]
    fn finalize_visible_emits_error_only_when_nothing_visible_and_suppressed() {
        assert_eq!(finalize_visible(0, "", true), None);
        assert_eq!(finalize_visible(5, "", true), Some(String::new()));
        assert_eq!(finalize_visible(0, "answer", true), Some("answer".to_string()));
        // No suppression: an empty model response is a different failure.
        assert_eq!(finalize_visible(0, "", false), Some(String::new()));
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `~/.cargo/bin/cargo test -p primer-core reasoning`
Expected: compiles, then panics at runtime in `push`/`finish` with `not yet implemented` (the `todo!()` bodies). This confirms the test harness wires up before implementation.

- [ ] **Step 4: Implement `push` and `finish`**

Replace the `push` and `finish` bodies in `reasoning.rs`. These are the only `todo!()`s:

```rust
    pub fn push(&mut self, chunk: &str) -> String {
        self.buf.push_str(chunk);
        let mut out = String::new();
        loop {
            match &self.state {
                State::Outside => {
                    // Earliest open marker across all configured pairs.
                    let hit = self
                        .markers
                        .iter()
                        .filter_map(|m| self.buf.find(&m.open).map(|i| (i, m.clone())))
                        .min_by_key(|(i, _)| *i);
                    match hit {
                        Some((i, m)) => {
                            out.push_str(&self.buf[..i]);
                            self.buf.drain(..i + m.open.len());
                            self.state = State::Inside { close: m.close.clone() };
                            // continue scanning the remainder
                        }
                        None => {
                            // Hold back the longest suffix that could be the
                            // start of some open marker; emit the rest.
                            let hold = longest_open_prefix_suffix(&self.buf, &self.markers);
                            let emit_to = self.buf.len() - hold;
                            out.push_str(&self.buf[..emit_to]);
                            self.buf.drain(..emit_to);
                            break;
                        }
                    }
                }
                State::Inside { close } => {
                    let close = close.clone();
                    match self.buf.find(&close) {
                        Some(i) => {
                            self.capture(&self.buf[..i].to_string());
                            self.buf.drain(..i + close.len());
                            self.state = State::Outside;
                            // continue scanning the remainder
                        }
                        None => {
                            // Suppress everything except the longest suffix that
                            // could be the start of this close marker.
                            let hold = longest_prefix_suffix(&self.buf, &close);
                            let take_to = self.buf.len() - hold;
                            let captured: String = self.buf[..take_to].to_string();
                            self.capture(&captured);
                            self.buf.drain(..take_to);
                            break;
                        }
                    }
                }
            }
        }
        out
    }

    pub fn finish(&mut self) -> String {
        match &self.state {
            State::Outside => std::mem::take(&mut self.buf),
            State::Inside { .. } => {
                let leftover = std::mem::take(&mut self.buf);
                self.capture(&leftover);
                String::new()
            }
        }
    }
```

Add these private helpers inside `impl ReasoningFilter` (after `did_suppress`):

```rust
    /// Append captured reasoning text and set the sticky flag.
    fn capture(&mut self, s: &str) {
        if !s.is_empty() {
            self.suppressed.push_str(s);
            self.did_suppress = true;
        }
    }
```

Add these free functions at module scope (below `finalize_visible`):

```rust
/// Length of the longest suffix of `buf` that is a proper prefix of `pat`
/// (i.e. `buf` might be the start of `pat`, continued in a later chunk).
/// Returns 0 if no such overlap, capped so a full `pat` match is handled by
/// the caller's `find`, not held back here.
fn longest_prefix_suffix(buf: &str, pat: &str) -> usize {
    let max = buf.len().min(pat.len().saturating_sub(1));
    // Try the longest candidate first.
    for len in (1..=max).rev() {
        let start = buf.len() - len;
        // Respect char boundaries so slicing never panics.
        if !buf.is_char_boundary(start) {
            continue;
        }
        if pat.starts_with(&buf[start..]) {
            return len;
        }
    }
    0
}

/// Longest suffix of `buf` that is a proper prefix of ANY marker's `open`.
fn longest_open_prefix_suffix(buf: &str, markers: &[ReasoningMarker]) -> usize {
    markers
        .iter()
        .map(|m| longest_prefix_suffix(buf, &m.open))
        .max()
        .unwrap_or(0)
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `~/.cargo/bin/cargo test -p primer-core reasoning`
Expected: all `reasoning::tests::*` PASS.

- [ ] **Step 6: Format, lint, commit**

Run: `~/.cargo/bin/cargo fmt -p primer-core && ~/.cargo/bin/cargo clippy -p primer-core --all-targets -- -D warnings`
Expected: clean.

```bash
cd /Users/hherb/src/primer/src
git add crates/primer-core/src/reasoning.rs crates/primer-core/src/lib.rs crates/primer-core/src/consts.rs
git commit -m "feat(core): pure ReasoningFilter streaming CoT-marker stripper

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: `ReasoningWithoutAnswer` error variant + i18n

**Files:**
- Modify: `src/crates/primer-core/src/error.rs` (variant + `is_retryable`)
- Modify: `src/crates/primer-core/src/i18n.rs` (three render arms + tests)

- [ ] **Step 1: Write the failing i18n tests**

In `src/crates/primer-core/src/i18n.rs`, inside the existing `#[cfg(test)] mod tests` block (the one containing `german_other_does_not_leak_inner_dev_string`), add:

```rust
    #[test]
    fn reasoning_without_answer_is_not_retryable() {
        assert!(!InferenceError::ReasoningWithoutAnswer.is_retryable());
    }

    #[test]
    fn reasoning_without_answer_renders_nonempty_per_locale() {
        for &l in &[Locale::English, Locale::German, Locale::Hindi] {
            let s = render_inference_error(&InferenceError::ReasoningWithoutAnswer, &l);
            assert!(!s.trim().is_empty(), "empty render for {l:?}");
        }
    }

    #[test]
    fn reasoning_without_answer_hindi_has_devanagari() {
        let s = render_inference_error(&InferenceError::ReasoningWithoutAnswer, &Locale::Hindi);
        assert!(
            s.chars().any(|c| ('\u{0900}'..='\u{097F}').contains(&c)),
            "expected Devanagari, got: {s}"
        );
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `~/.cargo/bin/cargo test -p primer-core reasoning_without_answer`
Expected: FAIL to compile — `no variant named ReasoningWithoutAnswer`.

- [ ] **Step 3: Add the variant**

In `src/crates/primer-core/src/error.rs`, add to the `InferenceError` enum, immediately before the `Other(String)` variant:

```rust
    /// A reasoning-mode model emitted chain-of-thought but no visible answer
    /// (truncated mid-thought, or a reasoning block followed by empty output).
    /// Dev-facing `Display` only; the user sees a friendly localized message
    /// via `crate::i18n::render_inference_error`. Not retryable — the "try
    /// again" is the child re-asking, not an automatic retry.
    #[error("model produced reasoning but no visible answer")]
    ReasoningWithoutAnswer,
```

`is_retryable()` already returns `false` for any non-listed variant (it matches only `RateLimited | ServiceUnavailable | NetworkUnavailable`), so no change is needed there — the `reasoning_without_answer_is_not_retryable` test pins this.

- [ ] **Step 4: Add the three render arms**

In `src/crates/primer-core/src/i18n.rs`, add an arm to each of `render_english`, `render_german`, `render_hindi`, placed immediately before each function's `Other(_) =>` arm.

`render_english`:

```rust
        ReasoningWithoutAnswer => {
            "Oops — I'm having a thinking problem right now. Could you ask me that again?"
                .into()
        }
```

`render_german`:

```rust
        ReasoningWithoutAnswer => {
            "Hoppla — ich komme gerade beim Nachdenken durcheinander. Kannst du mich das \
             noch einmal fragen?"
                .into()
        }
```

`render_hindi`:

```rust
        ReasoningWithoutAnswer => {
            "अरे — मुझे अभी सोचने में थोड़ी दिक्कत हो रही है। क्या तुम मुझसे यह दोबारा पूछ सकते हो?"
                .into()
        }
```

- [ ] **Step 5: Run to verify pass**

Run: `~/.cargo/bin/cargo test -p primer-core reasoning_without_answer`
Expected: all three PASS.

- [ ] **Step 6: Format, lint, commit**

Run: `~/.cargo/bin/cargo fmt -p primer-core && ~/.cargo/bin/cargo clippy -p primer-core --all-targets -- -D warnings`
Expected: clean (the per-locale `match` is exhaustive, so the compiler already confirmed all three arms exist).

```bash
cd /Users/hherb/src/primer/src
git add crates/primer-core/src/error.rs crates/primer-core/src/i18n.rs
git commit -m "feat(core): ReasoningWithoutAnswer error + EN/DE/HI render

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Wire the filter into `OllamaBackend`

**Files:**
- Modify: `src/crates/primer-inference/src/ollama.rs`

- [ ] **Step 1: Write the failing wiring test**

At the bottom of `ollama.rs`'s `#[cfg(test)] mod tests`, add a test that exercises the filter through the same parse path the stream uses, plus the finalize decision. Add:

```rust
    mod reasoning_wiring {
        use super::*;
        use primer_core::reasoning::{finalize_visible, ReasoningFilter};

        /// Helper mirroring the generate_stream loop's per-chunk handling:
        /// parse NDJSON lines, filter their content, return (visible, error?).
        fn drive(backend: &OllamaBackend, lines: &[&str]) -> (String, bool) {
            let mut filter = ReasoningFilter::new(backend.reasoning_markers.clone());
            let mut visible = String::new();
            let mut total: usize = 0;
            let mut tail = String::new();
            for line in lines {
                let chunk = parse_ollama_line(line).unwrap();
                if chunk.done {
                    let v = filter.push(&chunk.text);
                    total += v.len();
                    visible.push_str(&v);
                    tail = format!("{v}{}", filter.finish());
                    break;
                } else {
                    let v = filter.push(&chunk.text);
                    total += v.len();
                    visible.push_str(&v);
                }
            }
            let emit_error = finalize_visible(total, &tail, filter.did_suppress()).is_none();
            (visible, emit_error)
        }

        #[test]
        fn strips_think_block_end_to_end() {
            let b = OllamaBackend::new("http://x".into(), "m".into());
            let lines = [
                r#"{"message":{"content":"<think>plan</think>"},"done":false}"#,
                r#"{"message":{"content":"Hi there"},"done":false}"#,
                r#"{"message":{"content":""},"done":true}"#,
            ];
            let (visible, err) = drive(&b, &lines);
            assert_eq!(visible, "Hi there");
            assert!(!err);
        }

        #[test]
        fn only_reasoning_yields_error() {
            let b = OllamaBackend::new("http://x".into(), "m".into());
            let lines = [
                r#"{"message":{"content":"<think>only thinking"},"done":false}"#,
                r#"{"message":{"content":""},"done":true}"#,
            ];
            let (visible, err) = drive(&b, &lines);
            assert_eq!(visible, "");
            assert!(err);
        }

        #[test]
        fn custom_marker_extends_defaults() {
            let b = OllamaBackend::new("http://x".into(), "m".into())
                .with_extra_markers(vec![("[[r]]".into(), "[[/r]]".into())]);
            let lines = [
                r#"{"message":{"content":"a[[r]]hidden[[/r]]b"},"done":false}"#,
                r#"{"message":{"content":""},"done":true}"#,
            ];
            let (visible, err) = drive(&b, &lines);
            assert_eq!(visible, "ab");
            assert!(!err);
        }
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `~/.cargo/bin/cargo test -p primer-inference reasoning_wiring`
Expected: FAIL to compile — `OllamaBackend` has no `reasoning_markers` field and no `with_extra_markers`.

- [ ] **Step 3: Add the field + builder**

In `ollama.rs`, modify the struct and `impl` block:

```rust
pub struct OllamaBackend {
    client: reqwest::Client,
    base_url: String,
    model: String,
    retry_settings: primer_core::retry::RetrySettings,
    /// Marker pairs whose enclosed reasoning is stripped from the stream.
    pub(crate) reasoning_markers: Vec<primer_core::reasoning::ReasoningMarker>,
}

impl OllamaBackend {
    pub fn new(base_url: String, model: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url,
            model,
            retry_settings: primer_core::retry::RetrySettings::default(),
            reasoning_markers: primer_core::reasoning::default_markers(),
        }
    }

    /// Append custom `(open, close)` reasoning-marker pairs to the built-in
    /// defaults. Builder style; returns `Self`.
    pub fn with_extra_markers(mut self, extra: Vec<(String, String)>) -> Self {
        self.reasoning_markers.extend(
            extra
                .into_iter()
                .map(|(o, c)| primer_core::reasoning::ReasoningMarker::new(o, c)),
        );
        self
    }
}
```

- [ ] **Step 4: Run the wiring test to verify it passes**

Run: `~/.cargo/bin/cargo test -p primer-inference reasoning_wiring`
Expected: PASS (the test drives the filter directly; the field + builder now exist).

- [ ] **Step 5: Wire the filter into `generate_stream`'s spawned task**

In `ollama.rs::generate_stream`, capture the markers before the spawn and rewrite the spawned task's loop. Immediately before `let (mut tx, rx) = mpsc::unbounded …`, add:

```rust
        let markers = self.reasoning_markers.clone();
```

Replace the entire `tokio::spawn(async move { … });` block with:

```rust
        tokio::spawn(async move {
            use primer_core::reasoning::{finalize_visible, ReasoningFilter};
            let mut buf = NdjsonBuffer::new();
            let mut filter = ReasoningFilter::new(markers);
            let mut total_visible: usize = 0;
            'outer: loop {
                match bytes_stream.next().await {
                    Some(Ok(bytes)) => {
                        buf.extend(&bytes);
                        while let Some(line) = buf.pop_line() {
                            if line.trim().is_empty() {
                                continue;
                            }
                            let chunk = match parse_ollama_line(&line) {
                                Ok(c) => c,
                                Err(e) => {
                                    tracing::warn!("Skipping unparseable Ollama line: {e}");
                                    continue;
                                }
                            };
                            if chunk.done {
                                // Final chunk: push its own content, then flush.
                                let mut visible = filter.push(&chunk.text);
                                total_visible += visible.len();
                                visible.push_str(&filter.finish());
                                log_suppressed(&mut filter);
                                match finalize_visible(
                                    total_visible,
                                    &visible,
                                    filter.did_suppress(),
                                ) {
                                    Some(text) => {
                                        let _ = tx
                                            .send(Ok(TokenChunk { text, done: true }))
                                            .await;
                                    }
                                    None => {
                                        let _ = tx
                                            .send(Err(PrimerError::Inference(
                                                primer_core::error::InferenceError::ReasoningWithoutAnswer,
                                            )))
                                            .await;
                                    }
                                }
                                break 'outer;
                            } else {
                                let visible = filter.push(&chunk.text);
                                log_suppressed(&mut filter);
                                if !visible.is_empty() {
                                    total_visible += visible.len();
                                    if tx
                                        .send(Ok(TokenChunk { text: visible, done: false }))
                                        .await
                                        .is_err()
                                    {
                                        break 'outer;
                                    }
                                }
                            }
                        }
                    }
                    Some(Err(e)) => {
                        let _ = tx
                            .send(Err(PrimerError::Inference(
                                format!("Ollama byte stream error: {e}").into(),
                            )))
                            .await;
                        break 'outer;
                    }
                    None => break 'outer,
                }
            }
        });
```

Add this small free helper near the top of `ollama.rs` (below `parse_ollama_line`), shared by the loop:

```rust
/// Drain any captured reasoning from the filter and emit it at debug level.
fn log_suppressed(filter: &mut primer_core::reasoning::ReasoningFilter) {
    let r = filter.drain_suppressed();
    if !r.is_empty() {
        tracing::debug!(target: "primer::reasoning", backend = "ollama", suppressed = %r);
    }
}
```

Note: the old loop forwarded the done chunk verbatim and relied on `done` to break. The rewrite handles `done` explicitly. The stub-stream/None-path behavior is unchanged.

- [ ] **Step 6: Run the full crate tests**

Run: `~/.cargo/bin/cargo test -p primer-inference`
Expected: all PASS, including the pre-existing `ndjson_buffer_*` and `classify_ollama_*` tests and the new `reasoning_wiring` tests.

- [ ] **Step 7: Format, lint, commit**

Run: `~/.cargo/bin/cargo fmt -p primer-inference && ~/.cargo/bin/cargo clippy -p primer-inference --all-targets -- -D warnings`
Expected: clean.

```bash
cd /Users/hherb/src/primer/src
git add crates/primer-inference/src/ollama.rs
git commit -m "feat(inference): strip reasoning markers in OllamaBackend stream

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Wire the filter into `OpenAiCompatBackend`

**Files:**
- Modify: `src/crates/primer-inference/src/openai_compat.rs`

- [ ] **Step 1: Write the failing wiring test**

At the bottom of `openai_compat.rs`'s `#[cfg(test)] mod tests`, add:

```rust
    mod reasoning_wiring {
        use super::*;
        use primer_core::reasoning::{finalize_visible, ReasoningFilter};

        /// Mirror the generate_stream loop: parse SSE payloads, filter content.
        fn drive(backend: &OpenAiCompatBackend, payloads: &[&str]) -> (String, bool) {
            let mut filter = ReasoningFilter::new(backend.reasoning_markers.clone());
            let mut visible = String::new();
            let mut total: usize = 0;
            let mut tail = String::new();
            for p in payloads {
                let parsed = parse_openai_compat_chunk(p).unwrap();
                let Some(chunk) = parsed else { continue };
                if chunk.done {
                    let v = filter.push(&chunk.text);
                    total += v.len();
                    visible.push_str(&v);
                    tail = format!("{v}{}", filter.finish());
                    break;
                } else {
                    let v = filter.push(&chunk.text);
                    total += v.len();
                    visible.push_str(&v);
                }
            }
            let emit_error = finalize_visible(total, &tail, filter.did_suppress()).is_none();
            (visible, emit_error)
        }

        fn delta(content: &str) -> String {
            format!(
                r#"{{"choices":[{{"delta":{{"content":"{content}"}},"finish_reason":null}}]}}"#
            )
        }

        #[test]
        fn strips_think_block_end_to_end() {
            let b = OpenAiCompatBackend::new("http://x".into(), "m".into(), None);
            let payloads = [
                delta("<think>plan</think>"),
                delta("Hi there"),
                r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#.to_string(),
            ];
            let refs: Vec<&str> = payloads.iter().map(|s| s.as_str()).collect();
            let (visible, err) = drive(&b, &refs);
            assert_eq!(visible, "Hi there");
            assert!(!err);
        }

        #[test]
        fn only_reasoning_yields_error() {
            let b = OpenAiCompatBackend::new("http://x".into(), "m".into(), None);
            let payloads = [
                delta("<think>only thinking"),
                r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#.to_string(),
            ];
            let refs: Vec<&str> = payloads.iter().map(|s| s.as_str()).collect();
            let (visible, err) = drive(&b, &refs);
            assert_eq!(visible, "");
            assert!(err);
        }

        #[test]
        fn custom_marker_extends_defaults() {
            let b = OpenAiCompatBackend::new("http://x".into(), "m".into(), None)
                .with_extra_markers(vec![("[[r]]".into(), "[[/r]]".into())]);
            let payloads = [
                delta("a[[r]]hidden[[/r]]b"),
                r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#.to_string(),
            ];
            let refs: Vec<&str> = payloads.iter().map(|s| s.as_str()).collect();
            let (visible, err) = drive(&b, &refs);
            assert_eq!(visible, "ab");
            assert!(!err);
        }
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `~/.cargo/bin/cargo test -p primer-inference -- openai_compat`
Expected: FAIL to compile — no `reasoning_markers` field, no `with_extra_markers`.

- [ ] **Step 3: Add the field + builder**

In `openai_compat.rs`, modify the struct + `impl`:

```rust
pub struct OpenAiCompatBackend {
    client: reqwest::Client,
    base_url: String,
    model: String,
    api_key: Option<String>,
    retry_settings: primer_core::retry::RetrySettings,
    /// Marker pairs whose enclosed reasoning is stripped from the stream.
    pub(crate) reasoning_markers: Vec<primer_core::reasoning::ReasoningMarker>,
}

impl OpenAiCompatBackend {
    pub fn new(base_url: String, model: String, api_key: Option<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url,
            model,
            api_key,
            retry_settings: primer_core::retry::RetrySettings::default(),
            reasoning_markers: primer_core::reasoning::default_markers(),
        }
    }

    /// Append custom `(open, close)` reasoning-marker pairs to the built-in
    /// defaults. Builder style; returns `Self`.
    pub fn with_extra_markers(mut self, extra: Vec<(String, String)>) -> Self {
        self.reasoning_markers.extend(
            extra
                .into_iter()
                .map(|(o, c)| primer_core::reasoning::ReasoningMarker::new(o, c)),
        );
        self
    }
}
```

- [ ] **Step 4: Run the wiring test to verify it passes**

Run: `~/.cargo/bin/cargo test -p primer-inference -- reasoning_wiring`
Expected: PASS for both backends' `reasoning_wiring` modules.

- [ ] **Step 5: Wire the filter into `generate_stream`'s spawned task**

In `openai_compat.rs::generate_stream`, immediately before `let (mut tx, rx) = mpsc::unbounded …`, add:

```rust
        let markers = self.reasoning_markers.clone();
```

Replace the `tokio::spawn(async move { … });` block with:

```rust
        tokio::spawn(async move {
            use primer_core::reasoning::{finalize_visible, ReasoningFilter};
            let mut buf = OpenAiSseBuffer::new();
            let mut filter = ReasoningFilter::new(markers);
            let mut total_visible: usize = 0;
            'outer: loop {
                match bytes_stream.next().await {
                    Some(Ok(bytes)) => {
                        buf.extend(&bytes);
                        while let Some(data) = buf.pop_data() {
                            let chunk = match parse_openai_compat_chunk(&data) {
                                Ok(Some(c)) => c,
                                Ok(None) => continue,
                                Err(e) => {
                                    tracing::warn!(
                                        "Skipping unparseable OpenAI-compat SSE line: {e}"
                                    );
                                    continue;
                                }
                            };
                            if chunk.done {
                                let mut visible = filter.push(&chunk.text);
                                total_visible += visible.len();
                                visible.push_str(&filter.finish());
                                log_suppressed(&mut filter);
                                match finalize_visible(
                                    total_visible,
                                    &visible,
                                    filter.did_suppress(),
                                ) {
                                    Some(text) => {
                                        let _ = tx
                                            .send(Ok(TokenChunk { text, done: true }))
                                            .await;
                                    }
                                    None => {
                                        let _ = tx
                                            .send(Err(PrimerError::Inference(
                                                primer_core::error::InferenceError::ReasoningWithoutAnswer,
                                            )))
                                            .await;
                                    }
                                }
                                break 'outer;
                            } else {
                                let visible = filter.push(&chunk.text);
                                log_suppressed(&mut filter);
                                if !visible.is_empty() {
                                    total_visible += visible.len();
                                    if tx
                                        .send(Ok(TokenChunk { text: visible, done: false }))
                                        .await
                                        .is_err()
                                    {
                                        break 'outer;
                                    }
                                }
                            }
                        }
                    }
                    Some(Err(e)) => {
                        let _ = tx
                            .send(Err(PrimerError::Inference(
                                format!("OpenAI-compat byte stream error: {e}").into(),
                            )))
                            .await;
                        break 'outer;
                    }
                    None => break 'outer,
                }
            }
        });
```

Add the same helper near the top of `openai_compat.rs` (below `parse_openai_compat_chunk`), with the backend label changed:

```rust
/// Drain any captured reasoning from the filter and emit it at debug level.
fn log_suppressed(filter: &mut primer_core::reasoning::ReasoningFilter) {
    let r = filter.drain_suppressed();
    if !r.is_empty() {
        tracing::debug!(target: "primer::reasoning", backend = "openai-compat", suppressed = %r);
    }
}
```

- [ ] **Step 6: Run the full crate tests**

Run: `~/.cargo/bin/cargo test -p primer-inference`
Expected: all PASS.

- [ ] **Step 7: Format, lint, commit**

Run: `~/.cargo/bin/cargo fmt -p primer-inference && ~/.cargo/bin/cargo clippy -p primer-inference --all-targets -- -D warnings`
Expected: clean.

```bash
cd /Users/hherb/src/primer/src
git add crates/primer-inference/src/openai_compat.rs
git commit -m "feat(inference): strip reasoning markers in OpenAiCompatBackend stream

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: `BackendParams.reasoning_markers` + `build_backend` wiring + GUI construction site

**Files:**
- Modify: `src/crates/primer-engine/src/wiring.rs`
- Modify: `src/crates/primer-gui/src/wiring.rs`

- [ ] **Step 1: Add the field to `BackendParams`**

In `src/crates/primer-engine/src/wiring.rs`, add to the `BackendParams` struct (after `qnn_qairt_lib_dir`):

```rust
    /// Extra `(open, close)` reasoning-marker pairs appended to the built-in
    /// defaults for the Ollama / openai-compat backends. Empty ⇒ defaults
    /// only. Ignored by every other backend arm.
    pub reasoning_markers: Vec<(String, String)>,
```

- [ ] **Step 2: Pass it into the two backend arms**

In the same file's `build_backend`, update the `"ollama"` and `"openai-compat"` arms:

```rust
        "ollama" => Ok(Arc::new(
            primer_inference::ollama::OllamaBackend::new(params.ollama_url.clone(), model)
                .with_extra_markers(params.reasoning_markers.clone()),
        )),
        "openai-compat" => Ok(Arc::new(
            primer_inference::openai_compat::OpenAiCompatBackend::new(
                params.openai_compat_url.clone(),
                model,
                params.openai_compat_api_key.clone(),
            )
            .with_extra_markers(params.reasoning_markers.clone()),
        )),
```

- [ ] **Step 3: Update every `BackendParams { … }` literal in the engine crate's own tests**

Run: `~/.cargo/bin/cargo build -p primer-engine 2>&1 | head -40`
Expected: FAIL — `missing field reasoning_markers` at each `BackendParams { … }` test literal in `wiring.rs`. For EACH reported line, add `reasoning_markers: Vec::new(),` to that struct literal. Re-run until it builds.

- [ ] **Step 4: Set the field at the GUI construction site**

In `src/crates/primer-gui/src/wiring.rs`, at the `BackendParams { … }` literal (around line 76), add the field (GUI custom-marker editor is deferred, so defaults only):

```rust
        reasoning_markers: Vec::new(),
```

- [ ] **Step 5: Build both crates**

Run: `~/.cargo/bin/cargo build -p primer-engine -p primer-gui`
Expected: clean build.

- [ ] **Step 6: Format, lint, commit**

Run: `~/.cargo/bin/cargo fmt -p primer-engine -p primer-gui && ~/.cargo/bin/cargo clippy -p primer-engine -p primer-gui --all-targets -- -D warnings`
Expected: clean.

```bash
cd /Users/hherb/src/primer/src
git add crates/primer-engine/src/wiring.rs crates/primer-gui/src/wiring.rs
git commit -m "feat(engine): thread reasoning_markers through BackendParams

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: CLI `--reasoning-marker` flag

**Files:**
- Modify: `src/crates/primer-cli/src/main.rs`

- [ ] **Step 1: Add the clap flag**

In `src/crates/primer-cli/src/main.rs`, add to the `Cli` struct (place it just after the `comprehension_model` flag, before `vocab_max_per_prompt`):

```rust
    /// Append a custom reasoning-marker pair to strip from model output.
    /// Repeatable: `--reasoning-marker '<think>' '</think>'`. The built-in
    /// defaults (`<think>…</think>`, Gemma4 `<|channel>…<channel|>`) always
    /// apply; this only adds more. Applies to ollama / openai-compat backends.
    #[arg(long, num_args = 2, value_names = ["OPEN", "CLOSE"], action = clap::ArgAction::Append)]
    reasoning_marker: Vec<String>,
```

Note: clap collects repeated 2-arg occurrences into one flat `Vec<String>` of length `2 × N`. We pair them up at construction.

- [ ] **Step 2: Write the failing pairing test**

Add a unit test near the bottom of `main.rs` (in its `#[cfg(test)] mod tests` if present; otherwise add one). First add the pure pairing helper test:

```rust
#[cfg(test)]
mod reasoning_marker_tests {
    use super::pair_reasoning_markers;

    #[test]
    fn pairs_flat_args_into_tuples() {
        let flat = vec![
            "<a>".to_string(),
            "</a>".to_string(),
            "<b>".to_string(),
            "</b>".to_string(),
        ];
        assert_eq!(
            pair_reasoning_markers(flat),
            vec![
                ("<a>".to_string(), "</a>".to_string()),
                ("<b>".to_string(), "</b>".to_string()),
            ]
        );
    }

    #[test]
    fn empty_is_empty() {
        assert_eq!(pair_reasoning_markers(vec![]), Vec::<(String, String)>::new());
    }

    #[test]
    fn odd_trailing_value_is_dropped() {
        // clap's num_args=2 makes odd counts impossible in practice, but the
        // helper must not panic if handed one.
        let flat = vec!["<a>".to_string(), "</a>".to_string(), "<stray>".to_string()];
        assert_eq!(
            pair_reasoning_markers(flat),
            vec![("<a>".to_string(), "</a>".to_string())]
        );
    }
}
```

- [ ] **Step 3: Run to verify it fails**

Run: `~/.cargo/bin/cargo test -p primer-cli reasoning_marker`
Expected: FAIL to compile — `pair_reasoning_markers` not found.

- [ ] **Step 4: Implement the pairing helper**

Add this free function in `main.rs` (above `fn main` or near the other helpers):

```rust
/// Pair clap's flat `--reasoning-marker OPEN CLOSE` values (a `Vec` of length
/// `2 × N`) into `(open, close)` tuples. A trailing unpaired value is dropped
/// (clap's `num_args = 2` makes that impossible in practice, but be defensive).
fn pair_reasoning_markers(flat: Vec<String>) -> Vec<(String, String)> {
    let mut it = flat.into_iter();
    let mut out = Vec::new();
    while let (Some(open), Some(close)) = (it.next(), it.next()) {
        out.push((open, close));
    }
    out
}
```

- [ ] **Step 5: Populate `BackendParams.reasoning_markers`**

In the `let params = BackendParams { … }` literal in `main.rs` (around line 404), add:

```rust
        reasoning_markers: pair_reasoning_markers(cli.reasoning_marker.clone()),
```

- [ ] **Step 6: Run tests + build**

Run: `~/.cargo/bin/cargo test -p primer-cli reasoning_marker && ~/.cargo/bin/cargo build -p primer-cli`
Expected: tests PASS, build clean.

- [ ] **Step 7: Manual smoke of the help text**

Run: `~/.cargo/bin/cargo run -p primer-cli --bin primer -- --help 2>&1 | grep -A2 reasoning-marker`
Expected: the `--reasoning-marker <OPEN> <CLOSE>` entry appears.

- [ ] **Step 8: Format, lint, commit**

Run: `~/.cargo/bin/cargo fmt -p primer-cli && ~/.cargo/bin/cargo clippy -p primer-cli --all-targets -- -D warnings`
Expected: clean.

```bash
cd /Users/hherb/src/primer/src
git add crates/primer-cli/src/main.rs
git commit -m "feat(cli): --reasoning-marker flag to append custom strip pairs

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: Live `gemma4:e4b` confirmation test (`#[ignore]`'d)

**Files:**
- Modify: `src/crates/primer-inference/src/ollama.rs` (one ignored test)

- [ ] **Step 1: Add the ignored live test**

In `ollama.rs`'s `reasoning_wiring` test module, add:

```rust
        /// Live confirmation that the built-in Gemma4 markers match the real
        /// stream. Requires `ollama serve` + `ollama pull gemma4:e4b`. Run on
        /// demand: `cargo test -p primer-inference gemma4_live -- --ignored --nocapture`.
        #[tokio::test]
        #[ignore = "requires a running ollama with gemma4:e4b pulled"]
        async fn gemma4_live_reasoning_is_stripped() {
            let b = OllamaBackend::new("http://localhost:11434".into(), "gemma4:e4b".into());
            let prompt = Prompt {
                system: "You are a helpful tutor. Think first, then answer.".into(),
                messages: vec![Message {
                    role: Role::User,
                    content: "What is 2+2? Explain briefly.".into(),
                }],
            };
            let params = GenerationParams::default();
            let text = b.generate(&prompt, &params).await.expect("generate");
            eprintln!("VISIBLE OUTPUT:\n{text}");
            assert!(!text.contains("<|channel>"), "open channel marker leaked: {text}");
            assert!(!text.contains("<channel|>"), "close channel marker leaked: {text}");
            assert!(!text.contains("<think>"), "think marker leaked: {text}");
            assert!(!text.trim().is_empty(), "no visible answer produced");
        }
```

- [ ] **Step 2: Verify it compiles and is skipped by default**

Run: `~/.cargo/bin/cargo test -p primer-inference gemma4_live`
Expected: shows `1 ignored` (compiles, not run).

- [ ] **Step 3: (Optional, developer-side) run it live**

Only if `ollama serve` is up and `gemma4:e4b` is pulled:
Run: `~/.cargo/bin/cargo test -p primer-inference gemma4_live -- --ignored --nocapture`
Expected: prints the visible output; PASS = no markers leaked, non-empty answer. If a marker leaks, the real Gemma4 bytes differ from the docs — fix `consts::reasoning::DEFAULT_MARKERS` and re-run.

- [ ] **Step 4: Format, lint, commit**

Run: `~/.cargo/bin/cargo fmt -p primer-inference && ~/.cargo/bin/cargo clippy -p primer-inference --all-targets -- -D warnings`
Expected: clean.

```bash
cd /Users/hherb/src/primer/src
git add crates/primer-inference/src/ollama.rs
git commit -m "test(inference): ignored gemma4:e4b live reasoning-strip confirmation

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 8: Workspace verification + CLAUDE.md gotcha

**Files:**
- Modify: `CLAUDE.md` (one bullet documenting the behavior)

- [ ] **Step 1: Full workspace gate**

Run from `src/`:

```bash
~/.cargo/bin/cargo fmt --all -- --check
~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings
~/.cargo/bin/cargo test --workspace --no-fail-fast
```

Expected: fmt clean; clippy clean; tests exit 0. If any fail, fix before continuing.

- [ ] **Step 2: Update the CLAUDE.md reasoning-leak gotcha**

In `CLAUDE.md`, find the existing bullet that begins with "**Reasoning-mode Ollama models (DeepSeek-R1, Gemma-thinking, Qwen QwQ, medgemma1.5, etc.) leak chain-of-thought tokens**" (in the Termux/Android conventions area) and replace it with:

```markdown
- **Reasoning-mode models' chain-of-thought is stripped before it reaches a child.** `primer_core::reasoning::ReasoningFilter` is a stateful streaming filter (robust to markers split across chunks) wired into BOTH `OllamaBackend` and `OpenAiCompatBackend` `generate_stream` (and therefore `generate`, which aggregates the stream). The built-in marker table (`consts::reasoning::DEFAULT_MARKERS`) covers `<think>…</think>` (DeepSeek-R1, QwQ, Qwen3) and Gemma4's asymmetric `<|channel>…<channel|>`; `--reasoning-marker '<OPEN>' '</CLOSE>'` (repeatable) appends custom pairs via `with_extra_markers`. Suppressed reasoning is logged at `tracing::debug!(target: "primer::reasoning")`. If a model reasons but emits NO visible answer, the backend sends `InferenceError::ReasoningWithoutAnswer`, rendered via the i18n boundary as a friendly "thinking problem, try again" (EN/DE/HI) — the partial turn drops at the dialogue-manager layer like any mid-stream error. **GUI custom-marker editing is deferred** (ROADMAP 0.3); the GUI already gets default stripping for free because it builds the same backends. The `#[ignore]`'d `gemma4_live_reasoning_is_stripped` test confirms the real Gemma4 bytes against a running ollama.
```

- [ ] **Step 3: Commit the doc**

```bash
cd /Users/hherb/src/primer
git add CLAUDE.md
git commit -m "docs(claude): reasoning-token stripping is now implemented

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

- [ ] **Step 4: Push the branch and open a PR**

```bash
cd /Users/hherb/src/primer
git push -u origin feat/reasoning-token-stripping
gh pr create --title "feat: strip reasoning-model chain-of-thought from Ollama + openai-compat" \
  --body "$(cat <<'EOF'
## Summary
Stateful streaming filter (`primer-core::reasoning`) strips per-model chain-of-thought markers from `OllamaBackend` and `OpenAiCompatBackend` before they reach a child. Built-in marker table (`<think>…</think>`, Gemma4 `<|channel>…<channel|>`); `--reasoning-marker` appends custom pairs. Reasoning-without-answer falls back to a localized "thinking problem, try again" via the existing i18n boundary. GUI custom-marker editor deferred to ROADMAP 0.3 (GUI gets default stripping for free).

Spec: `docs/superpowers/specs/2026-05-30-reasoning-token-stripping-design.md`
Plan: `docs/superpowers/plans/2026-05-30-reasoning-token-stripping.md`

## Test plan
- Pure-filter unit tests: split markers, multiple blocks, unbalanced block (no leak), false-prefix, custom + Gemma4 markers, drain/did_suppress.
- Per-backend wiring tests: think-block stripped, only-reasoning → error, custom-marker append.
- i18n: ReasoningWithoutAnswer non-empty per locale, Hindi Devanagari, not retryable.
- `#[ignore]`'d live `gemma4:e4b` confirmation.
- `cargo fmt --check`, `cargo clippy --workspace -D warnings`, `cargo test --workspace` all green.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

Expected: PR created.

---

## Self-Review notes (addressed)

- **Spec coverage:** Component 1 → Task 1; Component 2 → Task 1 (consts); Component 3 → Tasks 3–4; Component 4 → Task 2 + the error-emit paths in Tasks 3–4; Component 5/CLI → Tasks 5–6; Component 5/GUI → explicitly deferred (Task 5 sets `Vec::new()` so it compiles); Component 6 → tests in every task + Task 7 live test.
- **Type consistency:** `ReasoningFilter::{new,push,finish,drain_suppressed,did_suppress}`, `ReasoningMarker::new`, `default_markers()`, `finalize_visible(total_visible, tail, did_suppress)`, `with_extra_markers(Vec<(String,String)>)`, `BackendParams.reasoning_markers: Vec<(String,String)>`, `InferenceError::ReasoningWithoutAnswer`, `pair_reasoning_markers(Vec<String>) -> Vec<(String,String)>` — all consistent across tasks.
- **No placeholders:** every code step shows complete code; the only "find each reported line" step (Task 5 Step 3) is a compiler-driven mechanical fix with an exact command and exact field to add.
- **`pub(crate)` visibility** on `reasoning_markers` is what lets the in-crate `reasoning_wiring` tests read `backend.reasoning_markers`.
