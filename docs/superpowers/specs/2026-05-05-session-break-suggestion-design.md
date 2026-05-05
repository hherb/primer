# Session-Time-Based Break Suggestion (Phase 0.3 close-out)

**Status:** approved 2026-05-05.
**Branch:** `feature/session-break-suggestion` (to be created from `main`).
**Predecessors:** vocab spaced-repetition (`d55c41b`), dialogue-manager refactor (`9733a16`).

## Goal

After the child has been in active conversation for a configurable number of minutes, the Primer's next utterance is phrased — by the LLM, in-character — as a gentle suggestion that they could take a break, with the option to keep going. Subsequent suggestions fire on the same cadence.

## Non-goals

- **Never a forced halt.** The Primer never refuses to continue; the child decides. Parents set a framework with `--session-break-after-mins`, but an actively engaged child is never cut off.
- **No persistence of "I already suggested a break" across `--resume`.** A resumed session's break timer starts fresh; the worst-case extra suggestion soon after a resume is a tolerable outcome (the LLM will phrase it differently anyway).
- **No new schema version.** Adding `PedagogicalIntent::SuggestBreak` only requires a one-line addition to the `pedagogical_intents` lookup-table seed; the `validate_and_seed_lookup` pass on every open INSERTs the new row.
- **No turn-counting state.** Pure wallclock cadence; no "3 turns since last suggestion" mechanism.
- **Not a "back off when child says no" feature.** If field testing shows the cadence is too pushy after a decline, that's a follow-up.

## Architecture

A pure scheduling function in `primer-core` decides whether to fire; `decide_intent_at_with_pack` in `primer-pedagogy` consults it via a small `BreakGate` carrier; the dialogue manager records the last firing in-memory so the cadence resets; the prompt builder injects a locale-keyed guidance section so the LLM voices the suggestion in-character.

```
primer-core
  session_timing::should_suggest_break_now(now, started_at, last_suggested_at, interval_minutes)
  session_timing::BreakGate { interval_minutes, last_suggested_at }
  conversation::PedagogicalIntent::SuggestBreak (new variant)
  consts::break_suggest::DEFAULT_INTERVAL_MINUTES = 30
  config::PedagogyConfig::break_suggest_after_minutes (renamed from max_session_minutes)

primer-storage
  catalog::intent_id  +=  SuggestBreak => 9
  catalog::intent_name +=  SuggestBreak => "SuggestBreak"
  (validate_and_seed_lookup auto-INSERTs id=9 on next open)

primer-pedagogy
  prompt_pack::PromptPack::break_suggestion_intro(&self, minutes: u32) -> String  (new trait method, locale-aware)
  prompts/en.toml + prompts/de.toml gain break_suggestion_intro key (with {minutes} placeholder)
  prompt_builder::decide_intent_at_with_pack(..., break_gate: BreakGate)  (new param)
  prompt_builder::build_system_prompt_*  (adds break_suggestion_intro section when intent==SuggestBreak)
  dialogue_manager::DialogueManager.last_break_suggested_at: Option<DateTime<Utc>>
  dialogue_manager::DialogueManager.now_provider: TestClock           (test seam)

primer-cli
  --session-break-after-mins N  (default DEFAULT_INTERVAL_MINUTES, value_parser 1..)
  Delete hardcoded println! at main.rs:1252-1254
  Delete should_suggest_break() accessor in lifecycle.rs:208 (now dead code)
```

## Per-turn data flow

```
respond_to_streaming(input):
  await pending classifier + extractor/comprehension     [unchanged]
  add child turn                                         [unchanged]
  break_gate = BreakGate {
      interval_minutes: cfg.break_suggest_after_minutes,
      last_suggested_at: self.last_break_suggested_at,
  }
  intent = decide_intent_at_with_pack(pack, learner, session, now, break_gate)
  if intent == SuggestBreak:
      self.last_break_suggested_at = Some(now)           [reset cadence]
  build prompt (prompt-pack adds break_suggestion_intro section
                when intent == SuggestBreak)
  stream LLM output                                      [unchanged]
  add primer turn (with intent recorded)                 [unchanged — intent
                                                          persists to turns
                                                          via storage layer]
  spawn post-response chain                              [unchanged]
```

## Decide-intent precedence

After this change, `decide_intent_at_with_pack`'s ordering becomes:

1. `FrustratedStuck` → `Scaffolding`
2. `FrustratedTrying` → `Encouragement`
3. `Disengaging` → `Encouragement` (early) / `SessionClose` (sustained)
4. **NEW:** `should_suggest_break_now(now, session.started_at, break_gate.last_suggested_at, break_gate.interval_minutes)` → `SuggestBreak`
5. Turn analysis: factual question pattern → `DirectAnswer` / `AnswerThenPivot`; short response → `ComprehensionCheck`; active-concept-understood → `Extension`
6. Default → `SocraticQuestion`

Engagement-state overrides win over the timer: a frustrated child past 30 minutes still gets `Scaffolding`, not `SuggestBreak`. Fix the frustration first.

## The pure scheduling function

```rust
//! src/crates/primer-core/src/session_timing.rs

use chrono::{DateTime, Utc};

/// Carries break-gate state into intent decisions. `disabled()` is the
/// no-op gate: tests and call sites that don't care pass it; `decide_intent`
/// and `decide_intent_at` (the no-pack/no-time wrappers) default to it so
/// the existing characterization tests continue to pass verbatim.
#[derive(Debug, Clone, Copy)]
pub struct BreakGate {
    /// Minutes between break suggestions. 0 disables the gate.
    pub interval_minutes: u32,
    /// In-memory timestamp of the last SuggestBreak; None if none yet.
    pub last_suggested_at: Option<DateTime<Utc>>,
}

impl BreakGate {
    pub fn disabled() -> Self {
        Self { interval_minutes: 0, last_suggested_at: None }
    }
}

/// Returns true when a SuggestBreak should fire on the next turn.
///
/// Reference point is `last_suggested_at` if set, otherwise `started_at`.
/// Returns false when:
///   - `interval_minutes == 0` (gate disabled)
///   - elapsed time < interval
///   - clock went backwards (`now < reference`)
pub fn should_suggest_break_now(
    now: DateTime<Utc>,
    started_at: DateTime<Utc>,
    last_suggested_at: Option<DateTime<Utc>>,
    interval_minutes: u32,
) -> bool {
    if interval_minutes == 0 { return false; }
    let reference = last_suggested_at.unwrap_or(started_at);
    let elapsed = now.signed_duration_since(reference);
    let elapsed_mins = elapsed.num_minutes();
    elapsed_mins >= 0 && (elapsed_mins as u32) >= interval_minutes
}
```

## CLI surface

New flag:

```
--session-break-after-mins N    Minutes between break-suggestion nudges.
                                Must be >= 1. Default: 30 (from
                                consts::break_suggest::DEFAULT_INTERVAL_MINUTES).
```

`clap`'s `value_parser = clap::value_parser!(u32).range(1..)` rejects 0 at parse so it never reaches `BreakGate`. Wired into `PedagogyConfig.break_suggest_after_minutes`.

Removed:

- The hardcoded `println!("Primer: We've been talking for a while…")` at `main.rs:1252-1254`. The LLM's response *is* the break suggestion now.
- `DialogueManager::should_suggest_break()` in `lifecycle.rs:208`. Dead code after this PR; deleted to keep the API surface honest.

## Locale strings (multilingual is a first-class citizen)

Both `prompts/en.toml` and `prompts/de.toml` gain a `break_suggestion_intro` key whose value is a **template string** with a `{minutes}` placeholder. The trait method takes the actual interval in minutes as a parameter and returns the rendered string with the placeholder substituted; the unit word ("minutes" vs "Minuten") lives **inside** each locale's template, not in shared Rust code, so adding a new locale is purely additive (new TOML file, no code change).

```rust
// In trait PromptPack:
fn break_suggestion_intro(&self, minutes: u32) -> String;
```

English template (final wording subject to last-mile editing during implementation):

> `"The child has been working for ~{minutes} minutes. In your next response, gently suggest they take a break — phrase it as their choice, not a directive. You can finish a thought naturally first if you're mid-explanation. Don't lecture about screen time, don't be apologetic. The child can keep going if they want to."`

German template — translator's pass over the same content; the unit word becomes "Minuten" with appropriate phrasing.

**Substitution.** The default `TomlPromptPack` implementation uses `template.replace("{minutes}", &minutes.to_string())`. Templates without the placeholder render unchanged (graceful — useful for stub packs in tests). No `format!`-style escaping subtleties; `{minutes}` is the only variable.

The intro is injected into the system prompt only when `intent == SuggestBreak`. Both packs ship the key in their `[sections]` table; loading is the same `prompt_pack::load_cached` path used for `vocab_review_intro`.

**Why parameterise rather than say "for a while".** Multilingual is a first-class concern in this codebase, and the locale-aware-duration formatting question is going to come up again (vocab "last seen N days ago" in the prompt, future natural-language schedule confirmations). Solving it now — even at the simplest possible level (a placeholder substitution per template) — establishes the pattern: locale-specific *templates* carry locale-specific *formatting choices*. Shared Rust code just hands locales a number; each locale formats it its own way. No `chrono::format` localization machinery needed.

## Test seam for `Utc::now()` in `DialogueManager`

The dialogue manager currently reads the wallclock directly. End-to-end tests need to fast-forward beyond the 30-minute threshold without sleeping; production needs to read the real clock.

Approach: a `#[cfg(test)]`-friendly clock-injection field with a default that resolves to `Utc::now()`.

```rust
// In DialogueManager:
#[cfg(test)]
clock_override: Option<DateTime<Utc>>,

fn now(&self) -> DateTime<Utc> {
    #[cfg(test)]
    if let Some(t) = self.clock_override { return t; }
    Utc::now()
}
```

Test helpers set `clock_override` between turns to advance the simulated clock. Production has zero overhead (the field doesn't exist). Tests get deterministic timing.

This is a smaller blast radius than threading `now` through every call site. Documented at the field with a `// Test-only clock injection.` comment.

## Tests

| Layer | Test count | Cases |
|---|---:|---|
| `primer-core::session_timing` | 6 | gate disabled (interval=0), pre-threshold, exactly-at-threshold (boundary), post-threshold-no-prior, post-threshold-with-prior-not-yet-due, post-threshold-with-prior-due-again, clock-goes-backwards |
| `primer-core::conversation` | 1 (update) | `pedagogical_intent_all_lists_every_variant` count: 8 → 9 |
| `primer-storage::lib` | 2 | open-then-seed has SuggestBreak at id=9 with the expected name; round-trip a `Turn` with `intent=Some(SuggestBreak)` |
| `primer-pedagogy::prompt_builder` | 7 | (a) pre-threshold+Engaged → no SuggestBreak; (b) post-threshold+Engaged → SuggestBreak; (c) post-threshold+FrustratedStuck → Scaffolding (engagement wins); (d) post-threshold+Disengaging-sustained → SessionClose (engagement wins); (e) gate disabled → never SuggestBreak; (f) post-threshold-with-prior-not-yet-due → falls through to natural intent; (g) `build_system_prompt_with_pack_and_vocab` includes `break_suggestion_intro` when intent==SuggestBreak |
| `primer-pedagogy::prompt_pack` | 4 | en + de packs expose non-empty `break_suggestion_intro`; en pack substitutes `{minutes}` with the supplied number; de pack substitutes `{minutes}` and uses "Minuten" in its rendered output |
| `primer-pedagogy::dialogue_manager` | 4 | (a) end-to-end via `respond_to_streaming` with `clock_override` advanced past threshold yields intent=SuggestBreak and records `last_break_suggested_at`; (b) immediate next turn with same wallclock returns natural intent (cadence reset works); (c) `resume_session` clears `last_break_suggested_at`; (d) `new()` initialises it to None |
| `primer-cli` | 3 | `--session-break-after-mins 0` rejected at parse; default flows into `PedagogyConfig`; explicit value overrides default |

**Total new/changed tests: ~27.** Existing 474 tests must stay green; the 18 prompt-builder characterization tests use `decide_intent` (no-pack) and `decide_intent_at` (no-pack) — both default to `BreakGate::disabled()`, so they pass verbatim.

## Decisions and risks

1. **Field rename `max_session_minutes` → `break_suggest_after_minutes`.** Touches `PedagogyConfig`, the deleted `should_suggest_break` in `lifecycle.rs`, the deleted `println!` in `main.rs`, plus test files that build `PedagogyConfig`. Mechanical search-replace; no persistence impact (`PedagogyConfig` is not serialised to disk).
2. **Existing `should_suggest_break()` accessor in `lifecycle.rs:208` is deleted.** Its only caller (the hardcoded `println!`) is also deleted. Anyone re-introducing time-of-session logic should use `BreakGate` + `should_suggest_break_now`.
3. **`PedagogicalIntent::SuggestBreak` lookup ID = 9.** First addition since v3 schema; no collision risk. Pre-existing DBs auto-seed on next open.
4. **Test seam for `Utc::now()`.** A small surface area; alternative (thread `now` through all call sites) is more invasive. The `#[cfg(test)]` field has zero production cost.
5. **What if the child declines?** Out of scope. Cadence keeps ticking; next nudge fires `interval_minutes` later. If pushy in field testing, a future PR can layer "back off on decline" using the comprehension classifier's read of the child's response.
6. **What if SuggestBreak interrupts a coherent topic?** The prompt-pack guidance tells the LLM "you can finish a thought naturally first" — phrasing is the LLM's call.
7. **No anti-spam if the child asks "what?"** A `ComprehensionCheck` follow-up after `SuggestBreak` is fine. Cadence is cooldown-only on `SuggestBreak` itself; turns at any other intent don't reset the timer.
8. **`BreakGate` is `Copy`** so it threads through `decide_intent_at_with_pack` without lifetimes; cheap (two integers + an Option<DateTime>).

## Implementation order

(Plan-skill will produce the per-task TDD breakdown; this is the rough sequence.)

1. Pure-function landing: `session_timing.rs` + `BreakGate` + tests.
2. Add `PedagogicalIntent::SuggestBreak` variant + storage catalog row + variant-count test.
3. Field rename `max_session_minutes` → `break_suggest_after_minutes`; delete `should_suggest_break()` accessor; delete hardcoded `println!`.
4. New consts + `DEFAULT_INTERVAL_MINUTES`.
5. Wire `BreakGate` into `decide_intent_at_with_pack`; preserve no-pack/no-time wrappers via `BreakGate::disabled()`.
6. Prompt-pack: trait method, en.toml + de.toml keys, prompt-builder section injection.
7. `DialogueManager`: new field, test-clock seam, set-on-fire, threading.
8. CLI flag.
9. Verification gauntlet (`cargo test --workspace`, `cargo clippy --workspace --all-targets`, `cargo fmt --check`, `cargo build --workspace --features primer-cli/speech`).
10. Docs (README, ROADMAP, CLAUDE.md if needed).

## Out of scope (deferred)

- Back-off-on-decline behaviour.
- Persisting `last_break_suggested_at` across `--resume`.
- Per-child `break_suggest_after_minutes` overriding the global flag (parents may want this; deferred until field-tested).
- Locale-aware *unit selection* (e.g. switching from "minutes" to "hours" when interval ≥ 60). For now templates hard-code the unit word; if intervals routinely exceed 60 minutes, this can be revisited per-locale.
