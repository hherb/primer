# Context-Limit Graceful Recovery Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When an inference backend fills its context window mid-reply, the Primer visibly acknowledges it to the child and auto-retries with a progressively smaller prompt until it produces a complete answer (or gracefully soft-stops).

**Architecture:** A general `FinishReason` signal rides on the terminal `TokenChunk`. The QNN backend is the first producer (`Length` on context-limit). The dialogue manager reads it and runs a bounded progressive-shrink retry loop (`Full → NoKnowledge → Minimal`), streaming locale-aware apology / soft-stop strings from the prompt pack between attempts. Only the final answer is persisted.

**Tech Stack:** Rust (edition 2024, toolchain 1.88), `futures` streams, `async_trait`, TOML prompt packs.

**Spec:** [docs/superpowers/specs/2026-06-15-context-limit-graceful-recovery-design.md](../specs/2026-06-15-context-limit-graceful-recovery-design.md)

**Run all cargo commands from `/Users/hherb/src/primer/src` using `~/.cargo/bin/cargo +1.88`.**

---

## File Structure

| File | Responsibility | Change |
| --- | --- | --- |
| `crates/primer-core/src/inference.rs` | `FinishReason` enum + `TokenChunk` field + `Default` | Modify |
| `crates/primer-inference/**` (~30 literal sites) | construct `TokenChunk` | Modify (mechanical) |
| `crates/primer-inference/src/qnn/genie/mod.rs` | `emit_query_outcome` maps `ContextLimit → Length` | Modify |
| `crates/primer-pedagogy/src/dialogue_manager/budget_tier.rs` | `PromptBudgetTier` pure ladder + predicates | Create |
| `crates/primer-pedagogy/src/consts.rs` | retry/window/separator consts | Modify |
| `crates/primer-pedagogy/src/dialogue_manager/turn.rs` | tier-aware prompt build + retry loop + `StreamOutcome` | Modify |
| `crates/primer-pedagogy/src/prompt_pack.rs` | two new section keys + accessors + validation | Modify |
| `crates/primer-pedagogy/prompts/{en,de,hi}.toml` | apology + soft-stop copy | Modify |

---

## Task 1: `FinishReason` signal on `TokenChunk` (primer-core)

**Files:**
- Modify: `crates/primer-core/src/inference.rs:73-79`
- Test: same file's `#[cfg(test)]` module (add one if absent).

- [ ] **Step 1: Write the failing test**

Add to a `#[cfg(test)] mod tests` at the bottom of `crates/primer-core/src/inference.rs`:

```rust
#[cfg(test)]
mod finish_reason_tests {
    use super::*;

    #[test]
    fn finish_reason_defaults_to_stop() {
        assert_eq!(FinishReason::default(), FinishReason::Stop);
    }

    #[test]
    fn token_chunk_default_is_empty_not_done_stop() {
        let c = TokenChunk::default();
        assert!(c.text.is_empty());
        assert!(!c.done);
        assert_eq!(c.finish_reason, FinishReason::Stop);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `~/.cargo/bin/cargo +1.88 test -p primer-core finish_reason -- --nocapture`
Expected: FAIL — `FinishReason` not found / `TokenChunk: Default` not satisfied.

- [ ] **Step 3: Add the enum and field**

Replace the `TokenChunk` definition (currently lines 73-79) with:

```rust
/// Why a streaming generation ended. Meaningful only on the terminal
/// chunk (`done == true`); every non-terminal chunk carries the
/// default `Stop`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FinishReason {
    /// The model finished cleanly.
    #[default]
    Stop,
    /// Generation was cut off because the context window filled. The
    /// reply already streamed in full via the token callback but may
    /// stop mid-thought — the dialogue manager reacts by notifying the
    /// child and retrying with a smaller prompt.
    Length,
}

/// A single token (or chunk) emitted during streaming generation.
#[derive(Debug, Clone, Default)]
pub struct TokenChunk {
    pub text: String,
    /// True when the model has finished generating.
    pub done: bool,
    /// Why generation ended. Only meaningful when `done == true`.
    pub finish_reason: FinishReason,
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `~/.cargo/bin/cargo +1.88 test -p primer-core finish_reason -- --nocapture`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/primer-core/src/inference.rs
git commit -m "feat(core): add FinishReason signal to TokenChunk"
```

---

## Task 2: Make the workspace compile with the new field (mechanical migration)

Adding a third field breaks every `TokenChunk { text, done }` struct literal (~30 sites). Each is fixed by appending `..Default::default()` — `finish_reason` defaults to `Stop`, preserving today's behaviour everywhere. The QNN behavioural change is a **separate** task (Task 3).

**Files (all under `crates/`):** `primer-inference/src/{reasoning_stream.rs, cloud.rs, fallback.rs, stub.rs, bench/run.rs, llamacpp/backend.rs, ollama.rs, openai_compat/mod.rs}`, `primer-inference/src/qnn/{metrics.rs, genie/real.rs, genie/mock.rs, genie/mod.rs}`, `primer-inference/src/router/tests.rs`, `primer-classifier/src/llm.rs`, `primer-comprehension/src/llm.rs`, `primer-extractor/src/llm.rs`, `primer-pedagogy/src/dialogue_manager/test_support.rs`, `primer-pedagogy/src/dialogue_manager/tests/turn_tests.rs`.

- [ ] **Step 1: Find every literal site**

Run: `cd /Users/hherb/src/primer/src && grep -rn --include='*.rs' "TokenChunk {" crates/ | grep -v "pub struct TokenChunk"`

- [ ] **Step 2: Append `..Default::default()` to each literal**

For each struct-literal site, add `..Default::default()` as the last field. Example — `crates/primer-inference/src/stub.rs:46`:

```rust
let chunk = TokenChunk {
    text: token.to_string(),
    done: is_last,
    ..Default::default()
};
```

Two-field one-liners become, e.g. `crates/primer-classifier/src/llm.rs:200`:

```rust
let chunk = TokenChunk { text, done: true, ..Default::default() };
```

Apply the same edit to **every** site from Step 1. Do **not** change any `done`/`text` values. Leave the QNN `emit_query_outcome` site (`genie/mod.rs:245`) as `..Default::default()` for now — Task 3 changes its behaviour.

- [ ] **Step 3: Build the whole workspace**

Run: `~/.cargo/bin/cargo +1.88 build --workspace`
Expected: SUCCESS, no "missing field `finish_reason`" errors.

- [ ] **Step 4: Run the full default test suite (regression guard)**

Run: `~/.cargo/bin/cargo +1.88 test --workspace`
Expected: PASS — byte-identical behaviour (all `Stop`).

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "refactor: thread FinishReason::Stop default through all TokenChunk sites"
```

---

## Task 3: QNN backend emits `Length` on context-limit (primer-inference)

**Files:**
- Modify: `crates/primer-inference/src/qnn/genie/mod.rs:232-258` (`emit_query_outcome`)
- Test: same file's existing `#[cfg(test)] mod tests` (around line 260).

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `crates/primer-inference/src/qnn/genie/mod.rs`:

```rust
#[tokio::test]
async fn context_limit_emits_terminal_length_chunk() {
    use futures::StreamExt;
    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    emit_query_outcome(
        QueryOutcome::ContextLimit,
        GENIE_STATUS_CONTEXT_LIMIT_EXCEEDED,
        &tx,
    );
    drop(tx);
    let chunk = rx.next().await.unwrap().unwrap();
    assert!(chunk.done);
    assert_eq!(
        chunk.finish_reason,
        primer_core::inference::FinishReason::Length
    );
}

#[tokio::test]
async fn complete_emits_terminal_stop_chunk() {
    use futures::StreamExt;
    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    emit_query_outcome(QueryOutcome::Complete, GENIE_STATUS_SUCCESS, &tx);
    drop(tx);
    let chunk = rx.next().await.unwrap().unwrap();
    assert!(chunk.done);
    assert_eq!(
        chunk.finish_reason,
        primer_core::inference::FinishReason::Stop
    );
}
```

(If the test module is not `tokio`-aware, these can instead poll the unbounded receiver synchronously with `rx.try_next()` after the sync `emit_query_outcome` call — pick whichever matches the existing tests in that module.)

- [ ] **Step 2: Run to verify failure**

Run: `~/.cargo/bin/cargo +1.88 test -p primer-inference -- emits_terminal`
Expected: FAIL — `ContextLimit` currently emits `Stop` (the default).

- [ ] **Step 3: Set the finish reason per outcome**

In `emit_query_outcome`, replace the `QueryOutcome::Complete | QueryOutcome::ContextLimit` arm's `unbounded_send` so the terminal chunk carries the right reason:

```rust
QueryOutcome::Complete | QueryOutcome::ContextLimit => {
    let finish_reason = if outcome == QueryOutcome::ContextLimit {
        tracing::warn!(
            target: "primer::qnn",
            "GenieDialog_query hit the context limit (status {status}); completing the turn with the streamed reply"
        );
        primer_core::inference::FinishReason::Length
    } else {
        primer_core::inference::FinishReason::Stop
    };
    let _ = sender.unbounded_send(Ok(TokenChunk {
        text: String::new(),
        done: true,
        finish_reason,
    }));
}
```

- [ ] **Step 4: Run to verify pass**

Run: `~/.cargo/bin/cargo +1.88 test -p primer-inference -- emits_terminal`
Expected: PASS (2 tests). Also re-run the existing `classify_*` tests: `~/.cargo/bin/cargo +1.88 test -p primer-inference classify`.

- [ ] **Step 5: Commit**

```bash
git add crates/primer-inference/src/qnn/genie/mod.rs
git commit -m "feat(qnn): emit FinishReason::Length on context-limit truncation (#224)"
```

---

## Task 4: `PromptBudgetTier` pure ladder (primer-pedagogy)

**Files:**
- Create: `crates/primer-pedagogy/src/dialogue_manager/budget_tier.rs`
- Modify: `crates/primer-pedagogy/src/dialogue_manager/mod.rs` (add `mod budget_tier; pub(crate) use budget_tier::PromptBudgetTier;` near the other submodule declarations)
- Modify: `crates/primer-pedagogy/src/consts.rs`

- [ ] **Step 1: Add consts**

Append to `crates/primer-pedagogy/src/consts.rs`:

```rust
/// Context-limit recovery: how many times the dialogue manager retries a
/// truncated turn with a progressively smaller prompt before accepting the
/// partial reply. 2 retries ⇒ at most 3 inference calls
/// (Full → NoKnowledge → Minimal). See
/// docs/superpowers/specs/2026-06-15-context-limit-graceful-recovery-design.md.
pub const MAX_TRUNCATION_RETRIES: usize = 2;

/// Recent-turn window (in turns) for the `Minimal` recovery tier — the
/// tightest prompt, used on the second retry. Capped well below the
/// small-context default so the prompt leaves maximum room for the reply.
pub const MINIMAL_TIER_CONTEXT_WINDOW_TURNS: usize = 4;

/// Separator streamed around an in-turn notice (apology / soft-stop cue)
/// so it reads as a distinct beat rather than gluing onto the partial reply.
pub const TURN_NOTICE_SEPARATOR: &str = "\n\n";
```

- [ ] **Step 2: Write the failing test**

Create `crates/primer-pedagogy/src/dialogue_manager/budget_tier.rs`:

```rust
//! The progressive-shrink prompt-budget ladder for context-limit recovery.
//!
//! Each tier selects which optional prompt sections are assembled. The
//! ladder is a pure, backend-agnostic state machine so the dialogue
//! manager's retry loop is unit-testable without an inference backend.

/// Which optional prompt sections to include on a given attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PromptBudgetTier {
    /// Today's behaviour: KB passages + long-term memory + full window.
    Full,
    /// First retry: drop KB passages, keep long-term memory.
    NoKnowledge,
    /// Second retry: drop KB + long-term memory, shrink the recent-turn
    /// window to a floor.
    Minimal,
}

impl PromptBudgetTier {
    /// The full ladder, tightest-last. Length − 1 == number of retries.
    pub(crate) const LADDER: [PromptBudgetTier; 3] =
        [Self::Full, Self::NoKnowledge, Self::Minimal];

    /// The next tighter tier, or `None` if already at the tightest.
    pub(crate) fn next_tighter(self) -> Option<Self> {
        match self {
            Self::Full => Some(Self::NoKnowledge),
            Self::NoKnowledge => Some(Self::Minimal),
            Self::Minimal => None,
        }
    }

    /// Whether KB passages are retrieved at this tier.
    pub(crate) fn includes_knowledge(self) -> bool {
        matches!(self, Self::Full)
    }

    /// Whether long-term memory (summary + retrieved older turns) is
    /// retrieved at this tier.
    pub(crate) fn includes_long_term_memory(self) -> bool {
        matches!(self, Self::Full | Self::NoKnowledge)
    }

    /// Effective recent-turn window: `base` for Full/NoKnowledge; capped
    /// at the Minimal floor for Minimal.
    pub(crate) fn context_window_turns(self, base: usize) -> usize {
        match self {
            Self::Full | Self::NoKnowledge => base,
            Self::Minimal => base.min(crate::consts::MINIMAL_TIER_CONTEXT_WINDOW_TURNS),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ladder_length_matches_retry_budget() {
        assert_eq!(
            PromptBudgetTier::LADDER.len(),
            crate::consts::MAX_TRUNCATION_RETRIES + 1
        );
    }

    #[test]
    fn next_tighter_walks_the_ladder_then_stops() {
        assert_eq!(PromptBudgetTier::Full.next_tighter(), Some(PromptBudgetTier::NoKnowledge));
        assert_eq!(PromptBudgetTier::NoKnowledge.next_tighter(), Some(PromptBudgetTier::Minimal));
        assert_eq!(PromptBudgetTier::Minimal.next_tighter(), None);
    }

    #[test]
    fn full_includes_everything_minimal_includes_nothing_optional() {
        assert!(PromptBudgetTier::Full.includes_knowledge());
        assert!(PromptBudgetTier::Full.includes_long_term_memory());
        assert!(!PromptBudgetTier::NoKnowledge.includes_knowledge());
        assert!(PromptBudgetTier::NoKnowledge.includes_long_term_memory());
        assert!(!PromptBudgetTier::Minimal.includes_knowledge());
        assert!(!PromptBudgetTier::Minimal.includes_long_term_memory());
    }

    #[test]
    fn minimal_caps_window_at_floor_others_pass_through() {
        let base = 12;
        assert_eq!(PromptBudgetTier::Full.context_window_turns(base), base);
        assert_eq!(PromptBudgetTier::NoKnowledge.context_window_turns(base), base);
        assert_eq!(
            PromptBudgetTier::Minimal.context_window_turns(base),
            crate::consts::MINIMAL_TIER_CONTEXT_WINDOW_TURNS
        );
    }
}
```

- [ ] **Step 3: Wire the module**

In `crates/primer-pedagogy/src/dialogue_manager/mod.rs`, add alongside the existing `mod turn;` / `mod retrieval;` declarations:

```rust
mod budget_tier;
pub(crate) use budget_tier::PromptBudgetTier;
```

- [ ] **Step 4: Run to verify pass**

Run: `~/.cargo/bin/cargo +1.88 test -p primer-pedagogy budget_tier`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/primer-pedagogy/src/dialogue_manager/budget_tier.rs crates/primer-pedagogy/src/dialogue_manager/mod.rs crates/primer-pedagogy/src/consts.rs
git commit -m "feat(pedagogy): add PromptBudgetTier progressive-shrink ladder"
```

---

## Task 5: Prompt-pack apology / soft-stop strings (primer-pedagogy)

**Files:**
- Modify: `crates/primer-pedagogy/src/prompt_pack.rs` (trait, struct, validation, construction, accessors, `SectionsSection`)
- Modify: `crates/primer-pedagogy/prompts/{en,de,hi}.toml`

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` in `crates/primer-pedagogy/src/prompt_pack.rs` (mirroring `english_pack_exposes_vocab_review_intro`):

```rust
#[test]
fn english_pack_exposes_memory_limit_strings() {
    let pack = load_pack(Locale::English);
    assert!(!pack.memory_limit_retry().is_empty());
    assert!(!pack.memory_limit_soft_stop().is_empty());
}

#[test]
fn german_pack_exposes_memory_limit_strings() {
    let pack = load_pack(Locale::German);
    assert!(!pack.memory_limit_retry().is_empty());
    assert!(!pack.memory_limit_soft_stop().is_empty());
}
```

(Use whatever helper the existing tests use to load a shipped pack — e.g. the same call `english_pack_exposes_vocab_review_intro` at line ~1670 uses. Match it exactly.)

- [ ] **Step 2: Run to verify failure**

Run: `~/.cargo/bin/cargo +1.88 test -p primer-pedagogy memory_limit`
Expected: FAIL — `memory_limit_retry` method not found.

- [ ] **Step 3: Add trait methods**

In the `PromptPack` trait (after `break_suggestion_intro`, ~line 105):

```rust
/// Child-facing apology streamed when a turn was truncated by the
/// context limit and the Primer is about to retry with a smaller
/// prompt. Locale-keyed; no placeholders.
fn memory_limit_retry(&self) -> &str;
/// Child-facing cue streamed when context-limit retries are exhausted
/// and the Primer accepts the partial reply. Locale-keyed; no
/// placeholders.
fn memory_limit_soft_stop(&self) -> &str;
```

- [ ] **Step 4: Add struct fields**

In `struct TomlPromptPack` (after `break_suggestion_intro_template`, ~line 299):

```rust
memory_limit_retry: String,
memory_limit_soft_stop: String,
```

- [ ] **Step 5: Add `SectionsSection` fields (serde-default so fixtures don't break)**

In `struct SectionsSection` (~line 627), add:

```rust
#[serde(default)]
memory_limit_retry: String,
#[serde(default)]
memory_limit_soft_stop: String,
```

- [ ] **Step 6: Validate + construct**

In `from_toml_str`, after the `sections.break_suggestion_intro` validation block (~line 401):

```rust
validate_placeholders(
    "sections.memory_limit_retry",
    &raw.sections.memory_limit_retry,
    &[],
)?;
validate_placeholders(
    "sections.memory_limit_soft_stop",
    &raw.sections.memory_limit_soft_stop,
    &[],
)?;
```

In the `Ok(Self { ... })` construction (~after line 478):

```rust
memory_limit_retry: unescape_braces(&raw.sections.memory_limit_retry),
memory_limit_soft_stop: unescape_braces(&raw.sections.memory_limit_soft_stop),
```

- [ ] **Step 7: Add accessor impls**

In `impl PromptPack for TomlPromptPack` (after `break_suggestion_intro`, ~line 560):

```rust
fn memory_limit_retry(&self) -> &str {
    &self.memory_limit_retry
}
fn memory_limit_soft_stop(&self) -> &str {
    &self.memory_limit_soft_stop
}
```

- [ ] **Step 8: Add shipped copy to the three TOMLs**

In `crates/primer-pedagogy/prompts/en.toml`, under `[sections]`:

```toml
memory_limit_retry = "Oh — I'm sure up to there, but something just happened to my memory. Let me try that again."
memory_limit_soft_stop = "Let's pause there for now — ask me to keep going whenever you're ready."
```

In `crates/primer-pedagogy/prompts/de.toml`, under `[sections]`:

```toml
memory_limit_retry = "Oh — bis hierhin bin ich mir sicher, aber mit meinem Gedächtnis ist gerade etwas passiert. Ich versuche es noch einmal."
memory_limit_soft_stop = "Machen wir hier erst mal eine Pause — sag mir Bescheid, wenn ich weitermachen soll."
```

In `crates/primer-pedagogy/prompts/hi.toml`, under `[sections]` (preview, best-effort — flag for translator pass):

```toml
memory_limit_retry = "अरे — यहाँ तक मुझे पक्का पता है, पर मेरी याददाश्त के साथ अभी कुछ हो गया। मैं फिर से कोशिश करता हूँ।"
memory_limit_soft_stop = "अभी यहीं रुकते हैं — जब तैयार हो, मुझसे आगे बढ़ने को कहना।"
```

- [ ] **Step 9: Run to verify pass**

Run: `~/.cargo/bin/cargo +1.88 test -p primer-pedagogy memory_limit`
Expected: PASS (2 tests). Then full pack tests: `~/.cargo/bin/cargo +1.88 test -p primer-pedagogy prompt_pack`.

- [ ] **Step 10: Commit**

```bash
git add crates/primer-pedagogy/src/prompt_pack.rs crates/primer-pedagogy/prompts/
git commit -m "feat(pedagogy): locale-aware context-limit apology + soft-stop strings"
```

---

## Task 6: Tier-aware `build_turn_prompt` (primer-pedagogy)

**Files:**
- Modify: `crates/primer-pedagogy/src/dialogue_manager/turn.rs:94` (production call) + `:156-221` (`build_turn_prompt`)
- Modify: `crates/primer-pedagogy/src/dialogue_manager/tests/turn_tests.rs` (4 call sites: lines ~579, 628, 662, 699)

- [ ] **Step 1: Add the `tier` parameter and gate retrieval**

Change `build_turn_prompt`'s signature to accept the tier:

```rust
pub(super) async fn build_turn_prompt(
    &self,
    child_input: &str,
    intent: PedagogicalIntent,
    tier: super::PromptBudgetTier,
) -> (Prompt, usize) {
```

Inside, gate KB + LTM retrieval and the window on the tier. Replace the
`retrieve_knowledge` / `retrieve_long_term_memory` / `context_turns` lines:

```rust
let knowledge_context = if tier.includes_knowledge() {
    self.retrieve_knowledge(child_input).await
} else {
    Vec::new()
};
let passage_count = knowledge_context.len();
let (summary, retrieved_older) = if tier.includes_long_term_memory() {
    self.retrieve_long_term_memory(child_input).await
} else {
    (String::new(), Vec::new())
};
// ... due_vocab unchanged ...
let backend_name = self.inference.name();
let base_window = self.config.effective_context_window_turns(backend_name);
let context_turns = tier.context_window_turns(base_window);
```

Leave the rest of the function (the small-context-vs-large branch) unchanged — it already consumes `knowledge_context`, `summary`, `retrieved_older`, `context_turns`.

- [ ] **Step 2: Update the production call site (line 94)**

For now, keep production behaviour identical by passing `Full` (the retry loop lands in Task 7):

```rust
let (prompt, passage_count) = self
    .build_turn_prompt(child_input, intent, super::PromptBudgetTier::Full)
    .await;
```

- [ ] **Step 3: Update the 4 test call sites**

In `crates/primer-pedagogy/src/dialogue_manager/tests/turn_tests.rs`, add `crate::dialogue_manager::PromptBudgetTier::Full` as the third arg to each `.build_turn_prompt(...)` call (lines ~579, 628, 662, 699). Example:

```rust
.build_turn_prompt(
    "tell me a story",
    PedagogicalIntent::SocraticQuestion,
    crate::dialogue_manager::PromptBudgetTier::Full,
)
```

- [ ] **Step 4: Add a tier-behaviour test**

Add to `turn_tests.rs` a test that `NoKnowledge` drops the KB section. Model it on `build_turn_prompt_budgets_system_prompt_for_small_context_backend` (line 639) but assert the rendered system prompt does **not** contain the knowledge-intro marker when the tier is `NoKnowledge`, and **does** when `Full`. Use the existing test scaffolding (seeded KB + small-context backend) from that test; the only delta is the tier arg and the assertion:

```rust
#[tokio::test]
async fn build_turn_prompt_no_knowledge_tier_omits_kb_section() {
    // (reuse the harness from build_turn_prompt_budgets_system_prompt_for_small_context_backend:
    //  a DM with a seeded KB and a small-context backend, queried with a topic the KB covers)
    let (prompt_full, _) = dm
        .build_turn_prompt("why is the sky blue", PedagogicalIntent::SocraticQuestion,
            crate::dialogue_manager::PromptBudgetTier::Full)
        .await;
    let (prompt_nokb, _) = dm
        .build_turn_prompt("why is the sky blue", PedagogicalIntent::SocraticQuestion,
            crate::dialogue_manager::PromptBudgetTier::NoKnowledge)
        .await;
    let kb_marker = dm.prompt_pack().knowledge_intro(8); // the section's intro text
    assert!(prompt_full.system.contains(kb_marker.trim()));
    assert!(!prompt_nokb.system.contains(kb_marker.trim()));
}
```

(If `prompt_pack()`/`knowledge_intro` aren't reachable from the test, assert instead on a distinctive substring of a seeded passage that only appears via the KB section — match whatever the line-639 test already asserts on.)

- [ ] **Step 5: Run to verify pass**

Run: `~/.cargo/bin/cargo +1.88 test -p primer-pedagogy -- build_turn_prompt`
Expected: PASS (existing 4 + new 1).

- [ ] **Step 6: Commit**

```bash
git add crates/primer-pedagogy/src/dialogue_manager/turn.rs crates/primer-pedagogy/src/dialogue_manager/tests/turn_tests.rs
git commit -m "feat(pedagogy): tier-aware build_turn_prompt (KB/LTM/window gating)"
```

---

## Task 7: The recovery retry loop (primer-pedagogy)

**Files:**
- Modify: `crates/primer-pedagogy/src/dialogue_manager/turn.rs` (`respond_to_streaming` ~58-130, `stream_inference_response` ~232-265)
- Test: `crates/primer-pedagogy/src/dialogue_manager/tests/turn_tests.rs`

- [ ] **Step 1: Make `stream_inference_response` return the finish reason**

Add a small struct (top of `turn.rs`, below imports):

```rust
/// Outcome of one streaming attempt: the accumulated text plus why the
/// stream ended (drives the context-limit retry decision).
pub(super) struct StreamOutcome {
    pub text: String,
    pub finish_reason: primer_core::inference::FinishReason,
}
```

Change `stream_inference_response` to borrow the callback and return the outcome:

```rust
async fn stream_inference_response<F>(
    &self,
    prompt: &Prompt,
    intent: PedagogicalIntent,
    passage_count: usize,
    on_chunk: &mut F,
) -> Result<StreamOutcome>
where
    F: FnMut(&str),
{
    let params = GenerationParams {
        routing: Some(primer_core::router::RoutingSignals {
            intent,
            retrieved_passages: passage_count,
        }),
        ..GenerationParams::default()
    };
    let mut stream = self.inference.generate_stream(prompt, &params).await?;

    let mut accumulated = String::new();
    let mut finish_reason = primer_core::inference::FinishReason::Stop;
    while let Some(item) = stream.next().await {
        let chunk = item.inspect_err(|e| {
            tracing::warn!("Stream error mid-generation: {e}");
        })?;
        if !chunk.text.is_empty() {
            on_chunk(&chunk.text);
            accumulated.push_str(&chunk.text);
        }
        if chunk.done {
            finish_reason = chunk.finish_reason;
            break;
        }
    }
    Ok(StreamOutcome { text: accumulated, finish_reason })
}
```

- [ ] **Step 2: Write the failing retry-loop tests**

Add to `turn_tests.rs`. These need a stub backend that can be **scripted to finish with `Length` then `Stop`**. Check `test_support.rs` for the existing scripted-backend helper; if it can only emit `Stop`-terminal streams, extend it with a constructor that takes a per-call `FinishReason` script (e.g. `ScriptedBackend::with_finish_reasons(vec![Length, Length, Stop])`) — the stream's terminal chunk uses the scripted reason. Keep that extension minimal and in `test_support.rs`.

```rust
#[tokio::test]
async fn truncated_turn_retries_then_records_only_final_answer() {
    // Backend scripted: attempt 1 + 2 finish Length, attempt 3 finishes Stop.
    // Each attempt emits a recognizable body ("A1", "A2", "A3").
    let mut seen = String::new();
    let dm = /* DM with the scripted backend + a shipped English pack */;
    let final_text = dm
        .respond_to_streaming("teach me about volcanoes", |c| seen.push_str(c))
        .await
        .unwrap();

    // Streamed to the child: both partials, two apologies, final answer.
    let apology = dm.prompt_pack().memory_limit_retry();
    assert_eq!(seen.matches(apology).count(), 2);
    assert!(seen.contains("A1") && seen.contains("A2") && seen.contains("A3"));

    // Persisted Primer turn = ONLY the final clean answer.
    assert_eq!(final_text, "A3");
    let last = dm.session().turns.last().unwrap();
    assert_eq!(last.text, "A3");
    assert!(!last.text.contains(apology));
}

#[tokio::test]
async fn exhausted_retries_soft_stop_and_record_partial() {
    // Backend scripted: all three attempts finish Length.
    let mut seen = String::new();
    let dm = /* DM with the scripted backend (Length x3) + English pack */;
    let final_text = dm
        .respond_to_streaming("teach me about volcanoes", |c| seen.push_str(c))
        .await
        .unwrap();
    let soft_stop = dm.prompt_pack().memory_limit_soft_stop();
    assert!(seen.contains(soft_stop));
    assert_eq!(final_text, "A3"); // the last partial is accepted
    assert_eq!(dm.session().turns.last().unwrap().text, "A3");
}

#[tokio::test]
async fn clean_first_try_has_no_retries_or_apologies() {
    let mut seen = String::new();
    let dm = /* DM with a normal Stop-terminal stub */;
    dm.respond_to_streaming("hello", |c| seen.push_str(c)).await.unwrap();
    let apology = dm.prompt_pack().memory_limit_retry();
    assert!(!seen.contains(apology));
}
```

(Adapt `dm.session()` / `dm.prompt_pack()` to the actual accessors used elsewhere in `turn_tests.rs`. If no public `prompt_pack()` accessor exists on `DialogueManager`, hardcode the expected English string from the pack in the assertion, or add a `pub(crate) fn prompt_pack(&self) -> &dyn PromptPack` accessor.)

- [ ] **Step 3: Run to verify failure**

Run: `~/.cargo/bin/cargo +1.88 test -p primer-pedagogy -- retries`
Expected: FAIL — no retry loop yet (only one attempt; no apologies).

- [ ] **Step 4: Implement the retry loop in `respond_to_streaming`**

Replace step 2's single prompt-build + stream call (current lines ~94-99) with a loop. The child turn is already recorded once (step 1 of the method, unchanged). Make `on_chunk` mutable (`mut on_chunk: F` is already the binding) and pass `&mut on_chunk`:

```rust
// 2+3. Progressive-shrink recovery loop: build the prompt at the
// current budget tier, stream the reply, and on a context-limit
// truncation notify the child and retry with a smaller prompt.
use primer_core::inference::FinishReason;
use super::PromptBudgetTier;
let sep = crate::consts::TURN_NOTICE_SEPARATOR;

let mut tier = PromptBudgetTier::Full;
let result: Result<String> = loop {
    let (prompt, passage_count) =
        self.build_turn_prompt(child_input, intent, tier).await;
    match self
        .stream_inference_response(&prompt, intent, passage_count, &mut on_chunk)
        .await
    {
        Err(e) => break Err(e),
        Ok(outcome) => match (outcome.finish_reason, tier.next_tighter()) {
            (FinishReason::Stop, _) => break Ok(outcome.text),
            (FinishReason::Length, Some(next)) => {
                on_chunk(sep);
                on_chunk(self.prompt_pack.memory_limit_retry());
                on_chunk(sep);
                tier = next;
            }
            (FinishReason::Length, None) => {
                on_chunk(sep);
                on_chunk(self.prompt_pack.memory_limit_soft_stop());
                break Ok(outcome.text);
            }
        },
    }
};
```

Everything after (steps 4-6 of the method: record the Primer turn from `result`, update learner, persist, spawn background tasks) stays unchanged — it already keys off `result`.

Note: `intent` is computed once before the loop (lines 77-92, unchanged) — the retry reuses the same intent; only the prompt budget shrinks.

- [ ] **Step 5: Run to verify pass**

Run: `~/.cargo/bin/cargo +1.88 test -p primer-pedagogy -- retries`
Expected: PASS (3 tests).

- [ ] **Step 6: Full crate test + clippy**

Run: `~/.cargo/bin/cargo +1.88 test -p primer-pedagogy`
Run: `~/.cargo/bin/cargo +1.88 clippy -p primer-pedagogy --all-targets`
Expected: PASS, no warnings.

- [ ] **Step 7: Commit**

```bash
git add crates/primer-pedagogy/src/dialogue_manager/turn.rs crates/primer-pedagogy/src/dialogue_manager/tests/turn_tests.rs crates/primer-pedagogy/src/dialogue_manager/test_support.rs
git commit -m "feat(pedagogy): context-limit recovery retry loop with child notification (#224)"
```

---

## Task 8: Whole-workspace verification + docs

**Files:**
- Modify: `README.md`, `ROADMAP.md` (if the session changed user-facing status)

- [ ] **Step 1: Format + lint + full test**

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo +1.88 fmt --all -- --check
~/.cargo/bin/cargo +1.88 clippy --workspace --all-targets
~/.cargo/bin/cargo +1.88 test --workspace
```

Expected: all green.

- [ ] **Step 2: Update README / ROADMAP if warranted**

Note the context-limit graceful-recovery behaviour (apology + auto-retry) where the QNN/Phase-1.2 status is described. Keep it factual and brief.

- [ ] **Step 3: Commit**

```bash
git add README.md ROADMAP.md
git commit -m "docs: note context-limit graceful recovery (#224)"
```

- [ ] **Step 4: Push + open PR**

```bash
git push -u origin qnn-context-limit-recovery
gh pr create --base main --title "feat: context-limit graceful recovery — notify + auto-retry (#224)" --body "<summary + test evidence>"
```

---

## Self-Review

- **Spec coverage:** signal (Task 1) ✓; QNN producer (Task 3) ✓; general migration keeping all other backends `Stop` (Task 2) ✓; `PromptBudgetTier` Full/NoKnowledge/Minimal (Task 4) ✓; apology + soft-stop i18n strings (Task 5) ✓; tier-aware prompt build (Task 6) ✓; retry loop, record-only-final, max-3-calls, partial-on-exhaustion (Task 7) ✓; host-only verification (Task 8) ✓. On-device spot-check stays deferred (out of scope, per spec).
- **No magic numbers:** `MAX_TRUNCATION_RETRIES`, `MINIMAL_TIER_CONTEXT_WINDOW_TURNS`, `TURN_NOTICE_SEPARATOR` are named consts; the ladder length is tied to `MAX_TRUNCATION_RETRIES` by a test.
- **Type consistency:** `FinishReason{Stop,Length}`, `TokenChunk.finish_reason`, `StreamOutcome{text,finish_reason}`, `PromptBudgetTier{Full,NoKnowledge,Minimal}` with `next_tighter/includes_knowledge/includes_long_term_memory/context_window_turns`, and pack methods `memory_limit_retry/memory_limit_soft_stop` are used identically across tasks.
- **Known adaptation points (flagged in-task, not placeholders):** the scripted-backend `FinishReason` extension in `test_support.rs` and the exact `dm.session()/prompt_pack()` accessors must match existing test conventions — Task 7 Step 2 calls this out explicitly.
