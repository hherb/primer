# Session-Time-Based Break Suggestion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Phase 0.3 close-out. After a configurable wallclock interval, the Primer's next utterance is phrased by the LLM (in-character) as a gentle break suggestion. Cadence resets each time it fires; locale-aware `{minutes}` interpolation; never a forced halt.

**Architecture:** Pure scheduling function in `primer-core::session_timing` decides when to fire; `decide_intent_at_with_pack` consults it via a `BreakGate` carrier; `DialogueManager` records `last_break_suggested_at` in-memory after each fire so the cadence resets; the prompt builder injects a locale-keyed system-prompt section so the LLM voices the suggestion. The new `PedagogicalIntent::SuggestBreak` variant flows through the existing intent pipeline (turn metadata, lookup-table seeding, prompt-pack instruction).

**Tech Stack:** Rust 2024 edition (1.87+ via rustup), workspace under `src/`, `cargo` invoked as `~/.cargo/bin/cargo`, `chrono` for time, `serde` for TOML pack deserialisation, `clap` 4.x for CLI parsing, `rusqlite` (with bundled SQLite) for storage.

**Spec:** [docs/superpowers/specs/2026-05-05-session-break-suggestion-design.md](../specs/2026-05-05-session-break-suggestion-design.md) — read this first.

**Branch:** `feature/session-break-suggestion` (already created from `main`; first commit `81dde54` is the spec, second `ce88e30` is the locale clarification).

**Workspace root:** Every `cargo` command runs from `src/`. Always invoke as `~/.cargo/bin/cargo` (Homebrew rust shadows on PATH would silently use Rust 1.86 and silero would fail to compile).

**Conventions inherited from prior session work (see CLAUDE.md):**
- TDD: write the failing test first, watch it fail, implement to green, commit.
- No magic numbers — every numeric goes to `consts.rs` (invariant) or a settings struct (tunable).
- Pure functions in `primer-core` for algorithmic cores (no async, no I/O); take `now` as a parameter so tests can pass deterministic values.
- File-size hygiene: keep new files well under 500 lines.
- `tracing::warn!` for soft-fail diagnostics — never `eprintln!` for production code.
- One commit per task. Push to remote (origin/feature/session-break-suggestion) at end.

---

## File Structure

| Action | Path | Responsibility |
|---|---|---|
| Create | `src/crates/primer-core/src/session_timing.rs` | Pure `should_suggest_break_now` + `BreakGate` carrier struct |
| Modify | `src/crates/primer-core/src/lib.rs` | Add `pub mod session_timing;` |
| Modify | `src/crates/primer-core/src/consts.rs` | Add `break_suggest::DEFAULT_INTERVAL_MINUTES` |
| Modify | `src/crates/primer-core/src/conversation.rs` | Add `SuggestBreak` variant + extend `ALL` + update count test |
| Modify | `src/crates/primer-core/src/config.rs` | Rename `max_session_minutes` → `break_suggest_after_minutes` |
| Modify | `src/crates/primer-storage/src/catalog.rs` | Add `SuggestBreak => 9` arms in `intent_id` and `intent_name` |
| Modify | `src/crates/primer-storage/src/lib.rs` | New round-trip test for `intent=Some(SuggestBreak)` |
| Modify | `src/crates/primer-pedagogy/prompts/en.toml` | New `[intent.suggest_break]` + `[sections].break_suggestion_intro` |
| Modify | `src/crates/primer-pedagogy/prompts/de.toml` | German equivalents |
| Modify | `src/crates/primer-pedagogy/src/prompt_pack.rs` | New trait method + raw struct field + ALL_INTENTS extension + intent_key arm + validator + stub-pack arms (4) |
| Modify | `src/crates/primer-pedagogy/src/prompt_builder.rs` | Add `BreakGate` parameter to `decide_intent_at_with_pack` + leading override + add `break_minutes` parameter to `build_system_prompt_with_pack_and_vocab` (+ wrappers) + section injection |
| Modify | `src/crates/primer-pedagogy/src/dialogue_manager/mod.rs` | New field `last_break_suggested_at`, test-only `clock_override`, `now()` helper |
| Modify | `src/crates/primer-pedagogy/src/dialogue_manager/lifecycle.rs` | Init/reset field; **delete** `should_suggest_break()` accessor (lines 206-213) |
| Modify | `src/crates/primer-pedagogy/src/dialogue_manager/turn.rs` | Build `BreakGate` from config+state, pass into `decide_intent_at_with_pack`, record `last_break_suggested_at` on fire, pass `break_minutes` into `build_system_prompt_with_pack_and_vocab` |
| Modify | `src/crates/primer-pedagogy/src/dialogue_manager/tests/lifecycle_tests.rs` | Add tests for new-init, resume-clears |
| Modify | `src/crates/primer-pedagogy/src/dialogue_manager/tests/turn_tests.rs` | End-to-end SuggestBreak test using `clock_override` |
| Modify | `src/crates/primer-cli/src/main.rs` | Add `--session-break-after-mins` flag, wire to PedagogyConfig, **delete** hardcoded `println!` (lines 1252-1254) |
| Modify | `README.md` | One-line note on the new flag |
| Modify | `ROADMAP.md` | Mark Phase 0.3 break-suggestion item complete |
| Modify | `CLAUDE.md` | Update the Phase 0.3 status statement and add the field-rename gotcha |

---

## Task 1: Add consts for break-suggestion

**Files:**
- Modify: `src/crates/primer-core/src/consts.rs`

- [ ] **Step 1: Add `break_suggest` submodule to consts.rs**

Append to the file (after the `vocab` module, line 61):

```rust
/// Defaults for the session-time-based break-suggestion feature.
///
/// See [`crate::session_timing`] and the design spec at
/// `docs/superpowers/specs/2026-05-05-session-break-suggestion-design.md`.
pub mod break_suggest {

    /// Minutes between break-suggestion nudges. After this many minutes
    /// of session time (or this many minutes since the last suggestion,
    /// whichever is more recent), the next pedagogical intent decision
    /// returns `SuggestBreak`. Configurable via `PedagogyConfig` and the
    /// `--session-break-after-mins` CLI flag. Must be `>= 1` when enabled
    /// (a value of 0 disables the gate entirely).
    pub const DEFAULT_INTERVAL_MINUTES: u32 = 30;
}
```

- [ ] **Step 2: Build to confirm**

Run: `cd src && ~/.cargo/bin/cargo build -p primer-core`
Expected: compiles cleanly (no tests yet).

- [ ] **Step 3: Commit**

```bash
git add src/crates/primer-core/src/consts.rs
git commit -m "$(cat <<'EOF'
core: add consts::break_suggest::DEFAULT_INTERVAL_MINUTES

Foundational constant for the session-time-based break-suggestion
feature. 30 minutes matches the existing PedagogyConfig.max_session_minutes
default that this constant will replace.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Pure function `should_suggest_break_now` + `BreakGate`

**Files:**
- Create: `src/crates/primer-core/src/session_timing.rs`
- Modify: `src/crates/primer-core/src/lib.rs`

- [ ] **Step 1: Write the failing tests + minimal scaffold**

Create `src/crates/primer-core/src/session_timing.rs`:

```rust
//! Pure scheduling helpers for the session-time-based break suggestion.
//!
//! [`should_suggest_break_now`] takes `now` as a parameter so it's
//! deterministic and testable; the dialogue manager calls it from
//! `respond_to_streaming` once per turn to decide whether the next
//! pedagogical intent should be `SuggestBreak`.
//!
//! Constants come from [`crate::consts::break_suggest`].

use chrono::{DateTime, Utc};

/// Carries break-gate state into intent decisions. The carrier exists so
/// `decide_intent_at_with_pack` can take a single parameter rather than
/// two scalars; `disabled()` is the no-op gate that the no-pack/no-time
/// `decide_intent` and `decide_intent_at` wrappers default to so the
/// pre-existing characterization tests pass verbatim.
#[derive(Debug, Clone, Copy)]
pub struct BreakGate {
    /// Minutes between break suggestions. `0` disables the gate.
    pub interval_minutes: u32,
    /// Wallclock timestamp of the last `SuggestBreak` fire in this session;
    /// `None` if none has fired yet.
    pub last_suggested_at: Option<DateTime<Utc>>,
}

impl BreakGate {
    /// The disabled gate — `should_suggest_break_now` always returns false.
    pub fn disabled() -> Self {
        Self {
            interval_minutes: 0,
            last_suggested_at: None,
        }
    }
}

/// Returns true when a `SuggestBreak` intent should fire on the next turn.
///
/// Reference point is `last_suggested_at` if set, otherwise `started_at`.
/// Returns false when:
///   - `interval_minutes == 0` (gate disabled),
///   - elapsed time since the reference point is less than the interval,
///   - the clock went backwards (`now < reference`).
pub fn should_suggest_break_now(
    now: DateTime<Utc>,
    started_at: DateTime<Utc>,
    last_suggested_at: Option<DateTime<Utc>>,
    interval_minutes: u32,
) -> bool {
    if interval_minutes == 0 {
        return false;
    }
    let reference = last_suggested_at.unwrap_or(started_at);
    let elapsed_mins = now.signed_duration_since(reference).num_minutes();
    elapsed_mins >= 0 && (elapsed_mins as u32) >= interval_minutes
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    /// Fixed reference point so tests don't depend on the wallclock.
    fn t(min: i64) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 5, 5, 12, 0, 0).unwrap()
            + chrono::Duration::minutes(min)
    }

    #[test]
    fn break_gate_disabled_has_zero_interval_and_no_prior() {
        let g = BreakGate::disabled();
        assert_eq!(g.interval_minutes, 0);
        assert!(g.last_suggested_at.is_none());
    }

    #[test]
    fn gate_disabled_never_fires() {
        // interval=0 means disabled — should always be false even past threshold.
        assert!(!should_suggest_break_now(t(120), t(0), None, 0));
    }

    #[test]
    fn pre_threshold_no_prior_does_not_fire() {
        // 5 minutes elapsed, 30 min interval — too early.
        assert!(!should_suggest_break_now(t(5), t(0), None, 30));
    }

    #[test]
    fn exactly_at_threshold_fires() {
        // Boundary: elapsed == interval should fire (>=).
        assert!(should_suggest_break_now(t(30), t(0), None, 30));
    }

    #[test]
    fn post_threshold_no_prior_fires() {
        assert!(should_suggest_break_now(t(35), t(0), None, 30));
    }

    #[test]
    fn post_threshold_with_prior_not_yet_due_does_not_fire() {
        // Started at 0, suggested at 30, now at 45 — only 15 min
        // since last suggestion, interval is 30, so not due.
        assert!(!should_suggest_break_now(t(45), t(0), Some(t(30)), 30));
    }

    #[test]
    fn post_threshold_with_prior_due_again_fires() {
        // Started at 0, suggested at 30, now at 65 — 35 min since
        // last suggestion (>= 30), so due again.
        assert!(should_suggest_break_now(t(65), t(0), Some(t(30)), 30));
    }

    #[test]
    fn clock_going_backwards_does_not_fire() {
        // `now` is before `started_at`. signed_duration is negative;
        // we should return false rather than treating wraparound as overdue.
        assert!(!should_suggest_break_now(t(0), t(60), None, 30));
    }
}
```

- [ ] **Step 2: Wire the new module into `lib.rs`**

Edit `src/crates/primer-core/src/lib.rs`. Insert `pub mod session_timing;` in alphabetical order (between `retry` and `speech`). The result should be:

```rust
pub mod retry;
pub mod session_timing;
pub mod speech;
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `cd src && ~/.cargo/bin/cargo test -p primer-core session_timing`
Expected: 8 tests passed (7 cases + the `break_gate_disabled` shape test).

- [ ] **Step 4: Commit**

```bash
git add src/crates/primer-core/src/session_timing.rs src/crates/primer-core/src/lib.rs
git commit -m "$(cat <<'EOF'
core: add session_timing module — should_suggest_break_now + BreakGate

Pure scheduling function for the break-suggestion feature, plus the
small carrier struct that decide_intent_at_with_pack will consume.
8 tests covering: gate-disabled, pre-threshold, boundary (=interval),
post-threshold-no-prior, post-threshold-with-prior-not-yet-due,
post-threshold-with-prior-due-again, clock-goes-backwards, plus the
BreakGate::disabled() shape.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Add `PedagogicalIntent::SuggestBreak` variant + storage catalog

**Files:**
- Modify: `src/crates/primer-core/src/conversation.rs` (lines 47-79)
- Modify: `src/crates/primer-storage/src/catalog.rs` (lines 30-55)
- Modify: `src/crates/primer-pedagogy/src/prompt_pack.rs` (lines 435-457)
- Modify: `src/crates/primer-storage/src/lib.rs` (test addition)

**Note:** Storage's `intent_id` and `intent_name` use exhaustive matches — adding a variant in conversation.rs without updating catalog.rs causes a primer-storage compile error. Same for prompt_pack.rs's `ALL_INTENTS` and `intent_key`. All three must move together.

- [ ] **Step 1: Add the variant to `conversation.rs`**

Edit `src/crates/primer-core/src/conversation.rs:62`:

Change:
```rust
    /// Suggesting the session end (the Primer never tries to maximise engagement).
    SessionClose,
}
```

to:

```rust
    /// Suggesting the session end (the Primer never tries to maximise engagement).
    SessionClose,
    /// Gentle nudge to take a break — the child can keep going. Fired
    /// by the wallclock-based break-suggestion gate; never a forced halt.
    SuggestBreak,
}
```

Edit the `ALL` array at lines 69-78 to extend with the new variant:

```rust
    pub const ALL: &'static [Self] = &[
        Self::SocraticQuestion,
        Self::ComprehensionCheck,
        Self::Scaffolding,
        Self::Encouragement,
        Self::Extension,
        Self::DirectAnswer,
        Self::AnswerThenPivot,
        Self::SessionClose,
        Self::SuggestBreak,
    ];
```

Update the count assertion at line 142:

```rust
    fn pedagogical_intent_all_lists_every_variant() {
        assert_eq!(PedagogicalIntent::ALL.len(), 9);
        // Spot-check a few representatives.
        assert!(PedagogicalIntent::ALL.contains(&PedagogicalIntent::SocraticQuestion));
        assert!(PedagogicalIntent::ALL.contains(&PedagogicalIntent::SessionClose));
        assert!(PedagogicalIntent::ALL.contains(&PedagogicalIntent::SuggestBreak));
    }
```

- [ ] **Step 2: Add the catalog arms in `primer-storage/src/catalog.rs`**

Edit `src/crates/primer-storage/src/catalog.rs:41` (the `intent_id` match):

```rust
        PedagogicalIntent::SessionClose => 8,
        PedagogicalIntent::SuggestBreak => 9,
    }
```

Edit lines 45-55 (the `intent_name` match):

```rust
        PedagogicalIntent::SessionClose => "SessionClose",
        PedagogicalIntent::SuggestBreak => "SuggestBreak",
    }
```

- [ ] **Step 3: Add prompt-pack arms in `prompt_pack.rs`**

Edit `src/crates/primer-pedagogy/src/prompt_pack.rs:435-444` (the `ALL_INTENTS` constant):

```rust
const ALL_INTENTS: &[PedagogicalIntent] = &[
    PedagogicalIntent::SocraticQuestion,
    PedagogicalIntent::ComprehensionCheck,
    PedagogicalIntent::Scaffolding,
    PedagogicalIntent::Encouragement,
    PedagogicalIntent::Extension,
    PedagogicalIntent::DirectAnswer,
    PedagogicalIntent::AnswerThenPivot,
    PedagogicalIntent::SessionClose,
    PedagogicalIntent::SuggestBreak,
];
```

Edit lines 446-457 (the `intent_key` match):

```rust
fn intent_key(intent: PedagogicalIntent) -> &'static str {
    match intent {
        PedagogicalIntent::SocraticQuestion => "socratic_question",
        PedagogicalIntent::ComprehensionCheck => "comprehension_check",
        PedagogicalIntent::Scaffolding => "scaffolding",
        PedagogicalIntent::Encouragement => "encouragement",
        PedagogicalIntent::Extension => "extension",
        PedagogicalIntent::DirectAnswer => "direct_answer",
        PedagogicalIntent::AnswerThenPivot => "answer_then_pivot",
        PedagogicalIntent::SessionClose => "session_close",
        PedagogicalIntent::SuggestBreak => "suggest_break",
    }
}
```

- [ ] **Step 4: Build to find missing TOML keys**

Run: `cd src && ~/.cargo/bin/cargo build -p primer-pedagogy 2>&1 | head -30`
Expected: builds, but at runtime the prompt-pack load will fail with "missing intent 'suggest_break'". We'll address this in Task 5 along with the rest of the pack changes.

For now run the prompt-pack tests and confirm they fail as expected:

Run: `cd src && ~/.cargo/bin/cargo test -p primer-pedagogy --lib prompt_pack 2>&1 | tail -20`
Expected: tests that load en/de packs FAIL with "missing intent 'suggest_break'". This is the red state.

- [ ] **Step 5: Add a placeholder `[intent.suggest_break]` to en.toml and de.toml so the pack loads**

We need the keys to exist now so primer-pedagogy still builds and tests pass. Final wording lands in Task 5.

Edit `src/crates/primer-pedagogy/prompts/en.toml`. Find the `[intent]` section. Add at the end of the intent block:

```toml
suggest_break = "Make a calm, brief acknowledgement that you've been together a while, and offer them a pause — phrase it as their choice, not a directive. They can keep going if they want to."
```

Edit `src/crates/primer-pedagogy/prompts/de.toml`. Add the German equivalent in the same position:

```toml
suggest_break = "Mach eine ruhige, kurze Bemerkung dazu, dass ihr schon eine Weile zusammen seid, und biete eine Pause an — als Vorschlag, nicht als Anweisung. Sie können weitermachen, wenn sie möchten."
```

- [ ] **Step 6: Add storage round-trip test in primer-storage/src/lib.rs**

Find the existing intent round-trip test (search the file for `intent: Some(PedagogicalIntent::SocraticQuestion)`). Most relevant section is around line 1846 (the `for &variant in PedagogicalIntent::ALL` block). The variant-iterator test will already cover SuggestBreak as part of its loop — so it will pass automatically once we extend `PedagogicalIntent::ALL`.

For an explicit single-variant round-trip test, append in the same `mod tests` block in `src/crates/primer-storage/src/lib.rs`:

```rust
    #[tokio::test]
    async fn round_trip_turn_with_intent_suggest_break() {
        // Pinned single-variant test for the SuggestBreak intent.
        // Catches a future regression where the catalog id mapping
        // drifts away from the variant order.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rt.db");
        let store = SqliteSessionStore::open(&path).unwrap();

        let learner_id = LearnerId::new_v4();
        let mut session = Session::new(learner_id);
        session.add_turn(Turn {
            speaker: Speaker::Primer,
            text: "Want to take a break?".into(),
            timestamp: Utc::now(),
            intent: Some(PedagogicalIntent::SuggestBreak),
            concepts: vec![],
        });

        store.save_session(&session).await.unwrap();
        let loaded = store.load_session(session.id).await.unwrap().unwrap();

        assert_eq!(loaded.turns.len(), 1);
        assert_eq!(
            loaded.turns[0].intent,
            Some(PedagogicalIntent::SuggestBreak),
        );
    }
```

(Imports: `Speaker`, `Turn`, `PedagogicalIntent`, `Session`, `LearnerId`, `Utc`, `SqliteSessionStore`. Most are already imported in the test module — verify and adjust by reading the existing imports at the top of the test mod.)

- [ ] **Step 7: Run the full storage + pedagogy test suite**

Run: `cd src && ~/.cargo/bin/cargo test -p primer-core -p primer-storage -p primer-pedagogy 2>&1 | tail -30`
Expected: all green. The variant-iterator test in primer-storage now seeds id=9 → "SuggestBreak"; the prompt-pack tests pass with the new placeholder TOML key.

- [ ] **Step 8: Commit**

```bash
git add src/crates/primer-core/src/conversation.rs \
        src/crates/primer-storage/src/catalog.rs \
        src/crates/primer-storage/src/lib.rs \
        src/crates/primer-pedagogy/src/prompt_pack.rs \
        src/crates/primer-pedagogy/prompts/en.toml \
        src/crates/primer-pedagogy/prompts/de.toml
git commit -m "$(cat <<'EOF'
feat: add PedagogicalIntent::SuggestBreak variant (id=9)

Lookup-table seeding auto-INSERTs id=9 with name=SuggestBreak on next
open of any DB; no schema migration needed. Prompt-pack TOML files get
placeholder intent.suggest_break entries (final wording lands with the
locale-aware {minutes} template in a follow-up task).

Adds an explicit single-variant round-trip storage test alongside the
variant-iterator test that already covers it.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Field rename + delete dead `should_suggest_break` accessor + delete hardcoded println

**Files:**
- Modify: `src/crates/primer-core/src/config.rs` (lines 65-83)
- Modify: `src/crates/primer-pedagogy/src/dialogue_manager/lifecycle.rs` (delete lines 206-213)
- Modify: `src/crates/primer-cli/src/main.rs` (delete lines 1251-1254)

- [ ] **Step 1: Rename in `config.rs`**

Edit `src/crates/primer-core/src/config.rs:65-83`:

Change:
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PedagogyConfig {
    /// How many conversation turns to include in the LLM context window.
    pub context_window_turns: usize,
    /// Maximum session length in minutes before the Primer suggests a break.
    pub max_session_minutes: u32,
    /// How aggressively to probe for comprehension (0.0 = gentle, 1.0 = rigorous).
    /// Adapts based on learner profile, but this sets the baseline.
    pub socratic_pressure: f32,
}

impl Default for PedagogyConfig {
    fn default() -> Self {
        Self {
            context_window_turns: 20,
            max_session_minutes: 30,
            socratic_pressure: 0.5,
        }
    }
}
```

to:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PedagogyConfig {
    /// How many conversation turns to include in the LLM context window.
    pub context_window_turns: usize,
    /// Minutes between break-suggestion nudges. After this many minutes
    /// of session time (or this many minutes since the last suggestion,
    /// whichever is more recent), the next pedagogical intent is forced
    /// to `SuggestBreak`. The Primer never enforces a session halt — the
    /// child can keep going past any number of suggestions. Set to `0`
    /// to disable the gate entirely.
    pub break_suggest_after_minutes: u32,
    /// How aggressively to probe for comprehension (0.0 = gentle, 1.0 = rigorous).
    /// Adapts based on learner profile, but this sets the baseline.
    pub socratic_pressure: f32,
}

impl Default for PedagogyConfig {
    fn default() -> Self {
        Self {
            context_window_turns: 20,
            break_suggest_after_minutes:
                crate::consts::break_suggest::DEFAULT_INTERVAL_MINUTES,
            socratic_pressure: 0.5,
        }
    }
}
```

- [ ] **Step 2: Delete `should_suggest_break` accessor in lifecycle.rs**

Edit `src/crates/primer-pedagogy/src/dialogue_manager/lifecycle.rs`. Delete lines 206-213 (the entire `should_suggest_break` method including its doc comment). Result — the section becomes:

```rust
    /// Stable identifier of the active comprehension classifier (used by `--verbose`).
    pub fn comprehension_identifier(&self) -> &str {
        self.comprehension.identifier()
    }

    /// End the session gracefully. Drains any in-flight classifier task so
    /// the final turn's assessment lands on disk, records `ended_at`, and
```

Also remove the now-stale module-level comment about `should_suggest_break`. Edit lines 9-12:

Change:
```rust
//! `should_suggest_break` is a thin elapsed-time check that lives here
//! because it's a public read-only accessor on session state, not part
//! of the per-turn hot path.
```

to:
```rust
//! Break-suggestion timing decisions live in `decide_intent_at_with_pack`
//! — see `primer_core::session_timing` for the pure helper.
```

- [ ] **Step 3: Delete the hardcoded println in main.rs**

Edit `src/crates/primer-cli/src/main.rs:1250-1255`. Delete:

```rust
        }


        // Check if the session has run long.
        if dm.should_suggest_break() {
            println!("Primer: We've been talking for a while. Want to take a break?\n");
        }

        print!("{}: ", dm.learner.profile.name);
```

Replace with:

```rust
        }


        print!("{}: ", dm.learner.profile.name);
```

- [ ] **Step 4: Build the workspace and run tests**

Run: `cd src && ~/.cargo/bin/cargo build --workspace 2>&1 | tail -10`
Expected: clean build. The rename only had three call sites in source (`config.rs`, `lifecycle.rs:212` deleted, no other callers). All test files build PedagogyConfig with `::default()` so they're unaffected by the field rename.

Run: `cd src && ~/.cargo/bin/cargo test --workspace 2>&1 | tail -10`
Expected: all green.

- [ ] **Step 5: Commit**

```bash
git add src/crates/primer-core/src/config.rs \
        src/crates/primer-pedagogy/src/dialogue_manager/lifecycle.rs \
        src/crates/primer-cli/src/main.rs
git commit -m "$(cat <<'EOF'
refactor: rename max_session_minutes → break_suggest_after_minutes

The field's semantic shifted when we adopted intent-based break
suggestion: it controls the cadence of suggestions, not a hard
session ceiling. The new name reflects what it actually does.

Also deletes the dead should_suggest_break() accessor in
lifecycle.rs and the hardcoded println! banner in main.rs that
fired every turn after the threshold (its only caller).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Add `break_suggestion_intro` template + trait method + locale strings

**Files:**
- Modify: `src/crates/primer-pedagogy/src/prompt_pack.rs`
- Modify: `src/crates/primer-pedagogy/prompts/en.toml`
- Modify: `src/crates/primer-pedagogy/prompts/de.toml`

This task is bigger than the others — it touches the trait, raw struct, validator, four stub-pack inline TOMLs in the test code, two real TOML files, and adds tests. Subdivided into careful steps.

- [ ] **Step 1: Write the failing tests**

Append to `src/crates/primer-pedagogy/src/prompt_pack.rs`'s `mod tests` block (find the existing `english_pack_exposes_vocab_review_intro` test and append after it):

```rust
    #[test]
    fn english_pack_exposes_break_suggestion_intro() {
        let pack = load(Locale::English).unwrap();
        let rendered = pack.break_suggestion_intro(30);
        assert!(
            !rendered.is_empty(),
            "break_suggestion_intro must not be empty"
        );
        assert!(
            rendered.contains("30"),
            "rendered intro should contain the minutes value: {rendered:?}"
        );
        assert!(
            !rendered.contains("{minutes}"),
            "{{minutes}} placeholder must be substituted: {rendered:?}"
        );
    }

    #[test]
    fn german_pack_exposes_break_suggestion_intro() {
        let pack = load(Locale::German).unwrap();
        let rendered = pack.break_suggestion_intro(30);
        assert!(
            !rendered.is_empty(),
            "German break_suggestion_intro must not be empty"
        );
        assert!(
            rendered.contains("30"),
            "rendered intro should contain the minutes value: {rendered:?}"
        );
        assert!(
            rendered.contains("Minuten"),
            "German rendered intro should contain 'Minuten': {rendered:?}"
        );
        assert!(
            !rendered.contains("{minutes}"),
            "{{minutes}} placeholder must be substituted: {rendered:?}"
        );
    }

    #[test]
    fn break_suggestion_intro_substitutes_arbitrary_minutes() {
        let pack = load(Locale::English).unwrap();
        let rendered = pack.break_suggestion_intro(45);
        assert!(rendered.contains("45"), "{rendered:?}");
    }

    #[test]
    fn break_suggestion_intro_with_zero_minutes_renders_zero() {
        // Even though zero is the "disabled" sentinel at the gate level,
        // the trait method itself should faithfully substitute whatever
        // it's given. The dialogue manager's gate prevents zero from
        // ever reaching the trait method in production.
        let pack = load(Locale::English).unwrap();
        let rendered = pack.break_suggestion_intro(0);
        assert!(rendered.contains('0'), "{rendered:?}");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd src && ~/.cargo/bin/cargo test -p primer-pedagogy --lib prompt_pack::tests::english_pack_exposes_break_suggestion_intro 2>&1 | tail -10`
Expected: FAIL with "no method named `break_suggestion_intro` found".

- [ ] **Step 3: Add the trait method to `PromptPack`**

Edit `src/crates/primer-pedagogy/src/prompt_pack.rs:67`. Insert after the `vocab_review_intro` method:

```rust
    /// Render the break-suggestion guidance section with `{minutes}`
    /// substituted. Renders only when the per-turn intent is `SuggestBreak`.
    /// Locale-keyed: each pack's TOML template owns its unit word
    /// ("minutes" / "Minuten") so adding a new locale is purely additive.
    fn break_suggestion_intro(&self, minutes: u32) -> String;
```

- [ ] **Step 4: Add the field to the `TomlPromptPack` struct + raw struct**

Edit lines 165-168 — add after `vocab_review_intro: String,`:

```rust
    vocab_review_intro: String,
    break_suggestion_intro_template: String,
    child_label: String,
```

Edit the raw `SectionsSection` struct (around line 415-420):

```rust
#[derive(Deserialize)]
struct SectionsSection {
    knowledge_intro: String,
    summary_intro: String,
    retrieved_intro: String,
    vocab_review_intro: String,
    break_suggestion_intro: String,
}
```

- [ ] **Step 5: Add validator and constructor wiring**

Find the existing validator block in `from_toml_str` (around line 248-253). Add the new validation right after `vocab_review_intro`'s validator (search the function for similar lines on `sections.*`):

```rust
        validate_placeholders(
            "sections.vocab_review_intro",
            &raw.sections.vocab_review_intro,
            &[],
        )?;
        validate_placeholders(
            "sections.break_suggestion_intro",
            &raw.sections.break_suggestion_intro,
            &["minutes"],
        )?;
```

Edit the constructor at line 286-300. Add the field assignment:

```rust
        Ok(Self {
            locale,
            base_template: raw.system_prompt.base,
            language_guidance: raw.language_guidance,
            intents,
            engagement_frustrated: raw.engagement.frustrated,
            engagement_disengaging: raw.engagement.disengaging,
            knowledge_intro_template: raw.sections.knowledge_intro,
            summary_intro: raw.sections.summary_intro,
            retrieved_intro: raw.sections.retrieved_intro,
            vocab_review_intro: raw.sections.vocab_review_intro,
            break_suggestion_intro_template: raw.sections.break_suggestion_intro,
            child_label: raw.labels.child,
            primer_label: raw.labels.primer,
            factual_prefixes: raw.question_detection.factual_prefixes,
        })
```

- [ ] **Step 6: Implement the method on `TomlPromptPack`**

Edit the `impl PromptPack for TomlPromptPack` block. Add after `vocab_review_intro`:

```rust
    fn vocab_review_intro(&self) -> &str {
        &self.vocab_review_intro
    }
    fn break_suggestion_intro(&self, minutes: u32) -> String {
        self.break_suggestion_intro_template
            .replace("{minutes}", &minutes.to_string())
    }
```

- [ ] **Step 7: Update the four stub TOMLs in test code**

Search `src/crates/primer-pedagogy/src/prompt_pack.rs` for `vocab_review_intro = ""`. There are four occurrences (lines 662, 807, 855, 913). Each is in a test fixture stub TOML. Add a `break_suggestion_intro = ""` line right after each `vocab_review_intro = ""`. Example:

Before:
```toml
vocab_review_intro = ""
```

After:
```toml
vocab_review_intro = ""
break_suggestion_intro = ""
```

Apply this change to **all four** occurrences. Without it the test stubs fail to deserialise.

- [ ] **Step 8: Add the real key to en.toml**

Edit `src/crates/primer-pedagogy/prompts/en.toml`. Find the `[sections]` block (search for `vocab_review_intro = "Concepts the child has`). Add a new key on the next line:

```toml
break_suggestion_intro = "The child has been working for ~{minutes} minutes. In your next response, gently suggest they take a break — phrase it as their choice, not a directive. You can finish a thought naturally first if you're mid-explanation. Don't lecture about screen time, don't be apologetic. The child can keep going if they want to."
```

- [ ] **Step 9: Add the real key to de.toml**

Edit `src/crates/primer-pedagogy/prompts/de.toml`. Find the `[sections]` block. Add:

```toml
break_suggestion_intro = "Das Kind ist jetzt seit etwa {minutes} Minuten dabei. Schlage in deiner nächsten Antwort behutsam eine Pause vor — formuliere es als ihre Wahl, nicht als Anweisung. Wenn du mitten in einer Erklärung bist, kannst du den Gedanken erst zu Ende führen. Halte keinen Vortrag über Bildschirmzeit, sei nicht entschuldigend. Das Kind kann weitermachen, wenn es möchte."
```

- [ ] **Step 10: Run tests to verify they pass**

Run: `cd src && ~/.cargo/bin/cargo test -p primer-pedagogy --lib prompt_pack 2>&1 | tail -15`
Expected: all prompt_pack tests pass, including the four new ones.

Run: `cd src && ~/.cargo/bin/cargo test --workspace 2>&1 | tail -15`
Expected: full workspace green.

- [ ] **Step 11: Commit**

```bash
git add src/crates/primer-pedagogy/src/prompt_pack.rs \
        src/crates/primer-pedagogy/prompts/en.toml \
        src/crates/primer-pedagogy/prompts/de.toml
git commit -m "$(cat <<'EOF'
feat: prompt-pack: break_suggestion_intro(minutes) + locale templates

New trait method renders a locale-aware template with the {minutes}
interval substituted. Each pack's TOML template owns its unit word
('minutes' / 'Minuten') so adding a locale is purely additive — no
shared Rust formatter, no chrono::format localization dependency.

Validator allows {minutes} only in sections.break_suggestion_intro;
typos elsewhere still fail loudly at load time.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: Wire `BreakGate` into `decide_intent_at_with_pack`

**Files:**
- Modify: `src/crates/primer-pedagogy/src/prompt_builder.rs`

- [ ] **Step 1: Write the failing tests**

Open `src/crates/primer-pedagogy/src/prompt_builder.rs`. Find the `mod tests` block (line 461-ish, the characterization tests for `decide_intent`). Append at the end of `mod tests`:

```rust
    // ─── Break-gate tests for decide_intent_at_with_pack ──────────────
    //
    // These tests reuse the existing `learner_with(engagement, concepts)`
    // helper at `prompt_builder.rs:478` and the existing `english_pack()`
    // helper. New helper for break-gate tests:

    fn session_started_at(when: chrono::DateTime<chrono::Utc>) -> Session {
        let mut s = Session::new(Uuid::new_v4());
        s.started_at = when;
        s
    }

    fn fixed_now(min: i64) -> chrono::DateTime<chrono::Utc> {
        use chrono::TimeZone;
        chrono::Utc.with_ymd_and_hms(2026, 5, 5, 12, 0, 0).unwrap()
            + chrono::Duration::minutes(min)
    }

    #[test]
    fn pre_threshold_engaged_does_not_fire_suggest_break() {
        // 5 min after start, 30 min interval — too early.
        let session = session_started_at(fixed_now(0));
        let learner = learner_with(EngagementState::Engaged, vec![]);
        let gate = primer_core::session_timing::BreakGate {
            interval_minutes: 30,
            last_suggested_at: None,
        };
        let intent = decide_intent_at_with_pack(
            english_pack(),
            &learner,
            &session,
            fixed_now(5),
            gate,
        );
        assert_ne!(intent, PedagogicalIntent::SuggestBreak);
    }

    #[test]
    fn post_threshold_engaged_fires_suggest_break() {
        let session = session_started_at(fixed_now(0));
        let learner = learner_with(EngagementState::Engaged, vec![]);
        let gate = primer_core::session_timing::BreakGate {
            interval_minutes: 30,
            last_suggested_at: None,
        };
        let intent = decide_intent_at_with_pack(
            english_pack(),
            &learner,
            &session,
            fixed_now(31),
            gate,
        );
        assert_eq!(intent, PedagogicalIntent::SuggestBreak);
    }

    #[test]
    fn frustrated_stuck_wins_over_break_gate() {
        // Engagement-state overrides fire BEFORE the break gate.
        // Past threshold AND FrustratedStuck → Scaffolding (fix
        // the frustration first), not SuggestBreak.
        let session = session_started_at(fixed_now(0));
        let learner = learner_with(EngagementState::FrustratedStuck, vec![]);
        let gate = primer_core::session_timing::BreakGate {
            interval_minutes: 30,
            last_suggested_at: None,
        };
        let intent = decide_intent_at_with_pack(
            english_pack(),
            &learner,
            &session,
            fixed_now(45),
            gate,
        );
        assert_eq!(intent, PedagogicalIntent::Scaffolding);
    }

    #[test]
    fn disengaging_sustained_wins_over_break_gate() {
        // Past threshold AND Disengaging-sustained → SessionClose, not SuggestBreak.
        let session = session_started_at(fixed_now(0));
        let mut learner = learner_with(EngagementState::Disengaging, vec![]);
        // Force the "sustained" branch by putting the disengagement
        // threshold below the wallclock elapsed.
        learner.preferences.early_disengagement_threshold =
            std::time::Duration::from_secs(60);
        let gate = primer_core::session_timing::BreakGate {
            interval_minutes: 30,
            last_suggested_at: None,
        };
        let intent = decide_intent_at_with_pack(
            english_pack(),
            &learner,
            &session,
            fixed_now(45),
            gate,
        );
        assert_eq!(intent, PedagogicalIntent::SessionClose);
    }

    #[test]
    fn disabled_gate_never_fires_suggest_break() {
        let session = session_started_at(fixed_now(0));
        let learner = learner_with(EngagementState::Engaged, vec![]);
        let gate = primer_core::session_timing::BreakGate::disabled();
        let intent = decide_intent_at_with_pack(
            english_pack(),
            &learner,
            &session,
            fixed_now(120),
            gate,
        );
        assert_ne!(intent, PedagogicalIntent::SuggestBreak);
    }

    #[test]
    fn post_threshold_with_recent_prior_falls_through_to_natural_intent() {
        // Started 60 min ago, suggested 5 min ago, 30 min interval —
        // not yet due again; should fall through to natural intent
        // (which for an empty session is SocraticQuestion).
        let mut session = session_started_at(fixed_now(0));
        // Add a child turn so the natural-intent branch has something
        // to chew on (otherwise default is SocraticQuestion either way,
        // but explicitly populating makes the test's assertion sharper).
        session.add_turn(Turn {
            speaker: Speaker::Child,
            text: "what's gravity".into(),
            timestamp: fixed_now(60),
            intent: None,
            concepts: vec![],
        });
        let learner = learner_with(EngagementState::Engaged, vec![]);
        let gate = primer_core::session_timing::BreakGate {
            interval_minutes: 30,
            last_suggested_at: Some(fixed_now(55)),
        };
        let intent = decide_intent_at_with_pack(
            english_pack(),
            &learner,
            &session,
            fixed_now(60),
            gate,
        );
        assert_ne!(intent, PedagogicalIntent::SuggestBreak);
    }
```

The new tests reference these symbols already imported in the existing `mod tests`: `Session`, `Speaker`, `Turn`, `EngagementState`, `PedagogicalIntent`, `english_pack`, `decide_intent_at_with_pack`, `learner_with`, `Uuid`. `chrono::TimeZone` may need adding (used by `fixed_now`). Verify by reading the test mod's imports before adding anything.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd src && ~/.cargo/bin/cargo test -p primer-pedagogy --lib decide_intent_at_with_pack 2>&1 | tail -20`
Expected: compile error — `decide_intent_at_with_pack` doesn't accept a 5th parameter, and the new tests reference `BreakGate`.

- [ ] **Step 3: Add the `BreakGate` parameter and the override clause**

Edit `src/crates/primer-pedagogy/src/prompt_builder.rs`. The current function signature (line 386-391):

```rust
pub fn decide_intent_at_with_pack(
    pack: &dyn PromptPack,
    learner: &LearnerModel,
    session: &Session,
    now: chrono::DateTime<chrono::Utc>,
) -> PedagogicalIntent {
```

becomes:

```rust
pub fn decide_intent_at_with_pack(
    pack: &dyn PromptPack,
    learner: &LearnerModel,
    session: &Session,
    now: chrono::DateTime<chrono::Utc>,
    break_gate: primer_core::session_timing::BreakGate,
) -> PedagogicalIntent {
```

Insert the leading override after the engagement-state arms — right after the closing brace of the outer `match learner.current_engagement` (line 409) and before the `// Look at the last turn …` comment:

```rust
        EngagementState::Engaged | EngagementState::Reflecting | EngagementState::Unknown => { /* fall through to turn analysis */
        }
    }

    // Break-suggestion gate: fires after engagement-state overrides
    // (a frustrated child past 30 minutes still gets Scaffolding,
    // not SuggestBreak — fix the frustration first) but before turn
    // analysis so it overrides the natural Socratic flow.
    if primer_core::session_timing::should_suggest_break_now(
        now,
        session.started_at,
        break_gate.last_suggested_at,
        break_gate.interval_minutes,
    ) {
        return PedagogicalIntent::SuggestBreak;
    }

    // Look at the last turn — if it was a child's response, decide
    // whether to probe comprehension or extend.
```

- [ ] **Step 4: Update the no-pack/no-time wrappers to default to `BreakGate::disabled()`**

The four wrappers at the top of the file all currently call into `decide_intent_at_with_pack` without a break gate. Update each:

`decide_intent` (line 359):
```rust
pub fn decide_intent(learner: &LearnerModel, session: &Session) -> PedagogicalIntent {
    decide_intent_at(learner, session, chrono::Utc::now())
}
```
(no change needed at this layer — recurses to `decide_intent_at`)

`decide_intent_with_pack` (line 364-370):
```rust
pub fn decide_intent_with_pack(
    pack: &dyn PromptPack,
    learner: &LearnerModel,
    session: &Session,
) -> PedagogicalIntent {
    decide_intent_at_with_pack(
        pack,
        learner,
        session,
        chrono::Utc::now(),
        primer_core::session_timing::BreakGate::disabled(),
    )
}
```

`decide_intent_at` (line 372-379):
```rust
pub fn decide_intent_at(
    learner: &LearnerModel,
    session: &Session,
    now: chrono::DateTime<chrono::Utc>,
) -> PedagogicalIntent {
    decide_intent_at_with_pack(
        english_pack(),
        learner,
        session,
        now,
        primer_core::session_timing::BreakGate::disabled(),
    )
}
```

- [ ] **Step 5: Update production callers of `decide_intent_at_with_pack` in dialogue_manager**

Run: `grep -n "decide_intent_at_with_pack" src/crates/primer-pedagogy/src/dialogue_manager/*.rs`

For now, every caller still wants `BreakGate::disabled()` — Task 7 will swap in the real gate. So for compilation:

In `src/crates/primer-pedagogy/src/dialogue_manager/turn.rs`, find the `decide_intent_at_with_pack(...)` call and append `, primer_core::session_timing::BreakGate::disabled()` as the final argument. (We'll fix this in Task 7.)

- [ ] **Step 6: Run tests to verify they pass**

Run: `cd src && ~/.cargo/bin/cargo test -p primer-pedagogy 2>&1 | tail -20`
Expected: all green, including the 6 new break-gate tests.

- [ ] **Step 7: Commit**

```bash
git add src/crates/primer-pedagogy/src/prompt_builder.rs \
        src/crates/primer-pedagogy/src/dialogue_manager/turn.rs
git commit -m "$(cat <<'EOF'
feat: decide_intent — wire BreakGate into post-engagement override

Adds a leading override after the engagement-state arms in
decide_intent_at_with_pack: when should_suggest_break_now returns
true, return PedagogicalIntent::SuggestBreak. Engagement-state
overrides win over the timer (a frustrated child past 30m gets
Scaffolding, not SuggestBreak — fix the frustration first).

The no-pack/no-time wrappers default to BreakGate::disabled() so the
18 existing characterization tests continue to pass verbatim.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: Inject `break_suggestion_intro` section into the system prompt

**Files:**
- Modify: `src/crates/primer-pedagogy/src/prompt_builder.rs`

- [ ] **Step 1: Write the failing test**

Append to `mod tests` in prompt_builder.rs:

```rust
    #[test]
    fn build_system_prompt_includes_break_suggestion_section_when_intent_is_suggest_break() {
        let learner = LearnerModel::new("Ada".into(), 9);
        let prompt = build_system_prompt_with_pack_and_vocab(
            english_pack(),
            &learner,
            PedagogicalIntent::SuggestBreak,
            &[], // no knowledge passages
            "",  // no summary
            &[], // no retrieved older turns
            &[], // no due vocab
            30,  // break_minutes — substituted into the intro
        );
        assert!(
            prompt.contains("30"),
            "rendered prompt should contain the substituted minutes value: {prompt:?}"
        );
        assert!(
            prompt.to_lowercase().contains("break"),
            "rendered prompt should contain a break-related word: {prompt:?}"
        );
        assert!(
            !prompt.contains("{minutes}"),
            "{{minutes}} placeholder must be substituted: {prompt:?}"
        );
    }

    #[test]
    fn build_system_prompt_omits_break_suggestion_section_for_other_intents() {
        let learner = LearnerModel::new("Ada".into(), 9);
        let prompt = build_system_prompt_with_pack_and_vocab(
            english_pack(),
            &learner,
            PedagogicalIntent::SocraticQuestion,
            &[], "", &[], &[],
            30,
        );
        // The break-suggestion intro contains a distinctive phrase.
        // Pin to a substring that appears only in the break section.
        assert!(
            !prompt.contains("phrase it as their choice"),
            "non-SuggestBreak intent should NOT include the break section: {prompt:?}"
        );
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd src && ~/.cargo/bin/cargo test -p primer-pedagogy --lib build_system_prompt_includes_break_suggestion 2>&1 | tail -10`
Expected: compile error — `build_system_prompt_with_pack_and_vocab` doesn't accept an 8th argument.

- [ ] **Step 3: Add `break_minutes` parameter and section injection**

Edit `src/crates/primer-pedagogy/src/prompt_builder.rs:85-93`. Update signature:

```rust
pub fn build_system_prompt_with_pack_and_vocab(
    pack: &dyn PromptPack,
    learner: &LearnerModel,
    intent: PedagogicalIntent,
    knowledge_context: &[Passage],
    summary: &str,
    retrieved_older: &[Turn],
    due_vocab: &[&ConceptState],
    break_minutes: u32,
) -> String {
```

Inside the function body, after the `let engagement_note` block (line 105), add:

```rust
    let break_suggestion_section = if intent == PedagogicalIntent::SuggestBreak {
        let intro = pack.break_suggestion_intro(break_minutes);
        format!("\n\n{intro}")
    } else {
        String::new()
    };
```

Update the format! at lines 169-172 to include the new section. Place it right after `engagement_note` so all "what should you do" guidance groups together at the top:

```rust
    format!(
        "{base}\n\n{intent_instruction}{engagement_note}{break_suggestion_section}\
         {summary_section}{retrieved_section}{vocab_section}{knowledge_section}"
    )
```

- [ ] **Step 4: Update wrappers and callers of `build_system_prompt_with_pack_and_vocab`**

Find every caller. Likely:
- `build_system_prompt_with_pack` (line 53-66) — the higher wrapper. Add `break_minutes: u32` parameter and pass through.
- `build_system_prompt` (line 183-191) — the no-pack wrapper. Same.
- The dialogue_manager `build_turn_prompt` site (around line 264).
- Existing prompt_builder tests calling `build_system_prompt_with_pack_and_vocab` directly (lines 1418, 1444, 1481).

For `build_system_prompt_with_pack` at line 53:

```rust
pub fn build_system_prompt_with_pack(
    pack: &dyn PromptPack,
    learner: &LearnerModel,
    intent: PedagogicalIntent,
    knowledge_context: &[Passage],
    summary: &str,
    retrieved_older: &[Turn],
) -> String {
    build_system_prompt_with_pack_and_vocab(
        pack,
        learner,
        intent,
        knowledge_context,
        summary,
        retrieved_older,
        &[],
        0, // break_minutes ignored unless intent == SuggestBreak
    )
}
```

For `build_system_prompt` at line 183:

```rust
pub fn build_system_prompt(
    learner: &LearnerModel,
    intent: PedagogicalIntent,
    knowledge_context: &[Passage],
    summary: &str,
    retrieved_older: &[Turn],
) -> String {
    build_system_prompt_with_pack(
        english_pack(),
        learner,
        intent,
        knowledge_context,
        summary,
        retrieved_older,
    )
}
```

(no signature change — wraps the with_pack variant which already passes 0)

For the dialogue_manager site in `build_turn_prompt`, find the call and add `0` as the final argument for now (Task 8 will pass the real value):

```rust
        system: build_system_prompt_with_pack_and_vocab(
            // ... existing args ...
            &due_vocab,
            0, // TODO Task 8: pass config.break_suggest_after_minutes
        ),
```

For each existing test caller of `build_system_prompt_with_pack_and_vocab` at lines 1418, 1444, 1481 — add `0,` as the final argument before the closing paren.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cd src && ~/.cargo/bin/cargo test -p primer-pedagogy 2>&1 | tail -20`
Expected: full pedagogy crate green, including the two new section-injection tests.

- [ ] **Step 6: Commit**

```bash
git add src/crates/primer-pedagogy/src/prompt_builder.rs \
        src/crates/primer-pedagogy/src/dialogue_manager/turn.rs
git commit -m "$(cat <<'EOF'
feat: prompt-builder: inject break_suggestion_intro on SuggestBreak

Adds a break_minutes parameter to build_system_prompt_with_pack_and_vocab
and renders an additional system-prompt section (locale-aware via the
pack's break_suggestion_intro template, with {minutes} substituted)
when intent==SuggestBreak. For all other intents the section is empty.

Wrappers default break_minutes to 0; the dialogue manager will pass
the real config.break_suggest_after_minutes in the next task.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: DialogueManager state — `last_break_suggested_at`, clock seam, init/reset

**Files:**
- Modify: `src/crates/primer-pedagogy/src/dialogue_manager/mod.rs`
- Modify: `src/crates/primer-pedagogy/src/dialogue_manager/lifecycle.rs`
- Modify: `src/crates/primer-pedagogy/src/dialogue_manager/tests/lifecycle_tests.rs`

- [ ] **Step 1: Write the failing tests**

Append to `src/crates/primer-pedagogy/src/dialogue_manager/tests/lifecycle_tests.rs` (inside the existing `mod tests` or at the file scope — match the pattern of the existing test fns in this file). The construction pattern mirrors what existing tests at `tests/turn_tests.rs:25-32` do:

```rust
#[tokio::test]
async fn new_initialises_last_break_suggested_at_to_none() {
    let backend = ScriptedBackend::new(vec![Ok(chunk("", true))]);
    let knowledge = EmptyKnowledge;
    let dm = DialogueManager::new(
        test_learner(),
        &backend,
        &knowledge,
        DialogueManagerStores::default(),
        default_subsystems(),
        primer_core::config::PedagogyConfig::default(),
    );
    assert!(dm.last_break_suggested_at_for_test().is_none());
}

#[tokio::test]
async fn resume_session_clears_last_break_suggested_at() {
    let backend = ScriptedBackend::new(vec![Ok(chunk("", true))]);
    let knowledge = EmptyKnowledge;
    let mut dm = DialogueManager::new(
        test_learner(),
        &backend,
        &knowledge,
        DialogueManagerStores::default(),
        default_subsystems(),
        primer_core::config::PedagogyConfig::default(),
    );
    dm.set_last_break_suggested_at_for_test(Some(chrono::Utc::now()));
    assert!(dm.last_break_suggested_at_for_test().is_some());

    // Build a fresh session to resume — its specifics don't matter
    // for this test, only that resume_session clears the field.
    let session_to_resume = primer_core::conversation::Session::new(
        dm.learner.profile.id,
    );
    dm.resume_session(session_to_resume).await.unwrap();

    assert!(
        dm.last_break_suggested_at_for_test().is_none(),
        "resume_session should clear last_break_suggested_at"
    );
}
```

Both `dm.learner` and `dm.session` are public fields on `DialogueManager` (lines 100-103 of `dialogue_manager/mod.rs`). If imports for `ScriptedBackend`, `EmptyKnowledge`, `chunk`, `test_learner`, `default_subsystems`, `DialogueManagerStores` are missing, copy them from the existing `use super::super::test_support::*; use super::super::*;` lines at the top of this test file.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd src && ~/.cargo/bin/cargo test -p primer-pedagogy lifecycle_tests::new_initialises_last_break 2>&1 | tail -10`
Expected: compile error — `last_break_suggested_at_for_test` does not exist.

- [ ] **Step 3: Add the field to `DialogueManager` struct**

Edit `src/crates/primer-pedagogy/src/dialogue_manager/mod.rs`. Find the struct definition. Add the new fields right after the existing comprehension state field (search for `last_comprehension`):

```rust
pub struct DialogueManager<'a> {
    // ... existing fields up through last_comprehension ...
    pub(super) last_comprehension: Option<ComprehensionResult>,
    /// In-memory timestamp of the last `SuggestBreak` fire. Reset on
    /// `new` and `resume_session`. Not persisted across `--resume`
    /// — see the design spec's non-goals for the rationale.
    pub(super) last_break_suggested_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Test-only clock override. When `Some`, `now()` returns this value
    /// instead of `chrono::Utc::now()`. Production paths never set this.
    #[cfg(test)]
    pub(super) clock_override: Option<chrono::DateTime<chrono::Utc>>,
    // ... rest of existing fields ...
}
```

(Read the existing struct definition to insert in the right place — fields are visibility-tagged.)

- [ ] **Step 4: Add the `now()` helper and the test seam accessors**

In `src/crates/primer-pedagogy/src/dialogue_manager/mod.rs`, find or create an `impl<'a> DialogueManager<'a>` block at the end of the file and add:

```rust
impl<'a> DialogueManager<'a> {
    /// Returns the current wallclock for break-gate decisions. Tests
    /// can override via `clock_override`; production always reads
    /// `chrono::Utc::now()`.
    pub(super) fn now(&self) -> chrono::DateTime<chrono::Utc> {
        #[cfg(test)]
        if let Some(t) = self.clock_override {
            return t;
        }
        chrono::Utc::now()
    }

    #[cfg(test)]
    pub fn last_break_suggested_at_for_test(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.last_break_suggested_at
    }

    #[cfg(test)]
    pub fn set_last_break_suggested_at_for_test(
        &mut self,
        v: Option<chrono::DateTime<chrono::Utc>>,
    ) {
        self.last_break_suggested_at = v;
    }

    #[cfg(test)]
    pub fn set_clock_for_test(&mut self, t: chrono::DateTime<chrono::Utc>) {
        self.clock_override = Some(t);
    }
}
```

- [ ] **Step 5: Initialise the new fields in `new()` and clear them in `resume_session()`**

Edit `src/crates/primer-pedagogy/src/dialogue_manager/lifecycle.rs`.

In `new()` (around line 50), add to the struct literal at the end (just before the closing `}`):

```rust
            last_comprehension: None,
            last_break_suggested_at: None,
            #[cfg(test)]
            clock_override: None,
            config,
```

(Insert order doesn't have to match the struct declaration order, but readability suggests grouping near `last_comprehension`.)

In `resume_session()` (around line 122-159), at the start of the method body:

```rust
    pub async fn resume_session(&mut self, loaded: Session) -> Result<()> {
        self.session = loaded;
        self.session.ended_at = None;
        self.last_break_suggested_at = None;
        // ... rest unchanged ...
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cd src && ~/.cargo/bin/cargo test -p primer-pedagogy lifecycle_tests 2>&1 | tail -15`
Expected: the two new tests pass; existing lifecycle tests still pass.

- [ ] **Step 7: Commit**

```bash
git add src/crates/primer-pedagogy/src/dialogue_manager/mod.rs \
        src/crates/primer-pedagogy/src/dialogue_manager/lifecycle.rs \
        src/crates/primer-pedagogy/src/dialogue_manager/tests/lifecycle_tests.rs
git commit -m "$(cat <<'EOF'
feat: dialogue-manager: in-memory last_break_suggested_at + clock seam

New field tracks when SuggestBreak last fired; reset on new() and
resume_session(). Test-only clock_override field with #[cfg(test)]
visibility lets end-to-end tests fast-forward past the 30-min
threshold without sleeping. Zero production cost.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 9: Wire `BreakGate` into `respond_to_streaming` + record on fire + end-to-end test

**Files:**
- Modify: `src/crates/primer-pedagogy/src/dialogue_manager/turn.rs`
- Modify: `src/crates/primer-pedagogy/src/dialogue_manager/tests/turn_tests.rs`

- [ ] **Step 1: Write the failing end-to-end tests**

Append to `src/crates/primer-pedagogy/src/dialogue_manager/tests/turn_tests.rs`. The construction pattern matches the existing tests in this file at lines 17-32; backend script provides "Hello" so the LLM stream completes:

```rust
#[tokio::test]
async fn respond_after_threshold_yields_suggest_break_intent() {
    let backend = ScriptedBackend::new(vec![
        Ok(chunk("Hello", false)),
        Ok(chunk("", true)),
    ]);
    let knowledge = EmptyKnowledge;
    let mut dm = DialogueManager::new(
        test_learner(),
        &backend,
        &knowledge,
        DialogueManagerStores::default(),
        default_subsystems(),
        primer_core::config::PedagogyConfig::default(),
    );

    // Fast-forward 31 minutes from session start (default
    // break_suggest_after_minutes is 30).
    let advanced = dm.session.started_at + chrono::Duration::minutes(31);
    dm.set_clock_for_test(advanced);

    let _response = dm.respond_to("hello").await.unwrap();

    assert_eq!(
        dm.last_intent(),
        Some(primer_core::conversation::PedagogicalIntent::SuggestBreak),
        "intent should be SuggestBreak after 31 minutes"
    );
    assert!(
        dm.last_break_suggested_at_for_test().is_some(),
        "last_break_suggested_at should be recorded"
    );
}

#[tokio::test]
async fn cadence_resets_after_suggest_break_fires() {
    let backend = ScriptedBackend::new(vec![
        // Two turns' worth of stream chunks.
        Ok(chunk("Hello", false)),
        Ok(chunk("", true)),
        Ok(chunk("Sure", false)),
        Ok(chunk("", true)),
    ]);
    let knowledge = EmptyKnowledge;
    let mut dm = DialogueManager::new(
        test_learner(),
        &backend,
        &knowledge,
        DialogueManagerStores::default(),
        default_subsystems(),
        primer_core::config::PedagogyConfig::default(),
    );

    let advanced = dm.session.started_at + chrono::Duration::minutes(31);
    dm.set_clock_for_test(advanced);
    dm.respond_to("hello").await.unwrap();

    // Second turn at the same wallclock — cadence should still be
    // active (last_suggested_at == now), so should NOT re-fire SuggestBreak.
    dm.respond_to("ok").await.unwrap();

    assert_ne!(
        dm.last_intent(),
        Some(primer_core::conversation::PedagogicalIntent::SuggestBreak),
        "second turn at the same wallclock should fall through to natural intent"
    );
}
```

`ScriptedBackend`, `chunk`, `EmptyKnowledge`, `test_learner`, `default_subsystems`, `DialogueManagerStores` are already imported via `use super::super::test_support::*;` and `use super::super::*;` at the top of the test file.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd src && ~/.cargo/bin/cargo test -p primer-pedagogy turn_tests::respond_after_threshold 2>&1 | tail -15`
Expected: compile or runtime fail — `set_clock_for_test` exists from Task 8 but the production path of `respond_to_streaming` still passes `BreakGate::disabled()`.

- [ ] **Step 3: Replace the disabled BreakGate in `turn.rs` with the real one**

Edit `src/crates/primer-pedagogy/src/dialogue_manager/turn.rs`. Find the `decide_intent_at_with_pack(...)` call site you added a placeholder at in Task 6. Replace `primer_core::session_timing::BreakGate::disabled()` with the real gate built from config + state.

Locate where `now` is obtained for `decide_intent_at_with_pack`. Replace any `chrono::Utc::now()` calls in this function with `self.now()` so the test seam works.

Around the intent decision:

```rust
        let now = self.now();
        let break_gate = primer_core::session_timing::BreakGate {
            interval_minutes: self.config.break_suggest_after_minutes,
            last_suggested_at: self.last_break_suggested_at,
        };
        let intent = primer_pedagogy_internal_decide(
            // ^ adjust to match the actual call site name
            self.prompt_pack.as_ref(),
            &self.learner,
            &self.session,
            now,
            break_gate,
        );

        if intent == primer_core::conversation::PedagogicalIntent::SuggestBreak {
            self.last_break_suggested_at = Some(now);
        }
```

(The exact function name is `prompt_builder::decide_intent_at_with_pack` — match the existing import. Adjust per the actual code.)

- [ ] **Step 4: Pass `break_minutes` into `build_system_prompt_with_pack_and_vocab`**

Find the `build_turn_prompt` (or similar) function in `turn.rs` (or wherever the system prompt is constructed for this turn). Replace the placeholder `0` from Task 7 with the real value:

```rust
            // ... build args ...
            &due_vocab,
            self.config.break_suggest_after_minutes,
        );
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cd src && ~/.cargo/bin/cargo test -p primer-pedagogy 2>&1 | tail -20`
Expected: full pedagogy green; the two new turn tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/crates/primer-pedagogy/src/dialogue_manager/turn.rs \
        src/crates/primer-pedagogy/src/dialogue_manager/tests/turn_tests.rs
git commit -m "$(cat <<'EOF'
feat: dialogue-manager: real BreakGate + record on fire

respond_to_streaming now builds a real BreakGate from config + the
in-memory last_break_suggested_at, threads it into decide_intent,
and records last_break_suggested_at = now whenever SuggestBreak
fires so the cadence resets. Now uses self.now() instead of
chrono::Utc::now() so test-clock overrides work.

Two end-to-end tests pin the behaviour: (a) past-threshold yields
SuggestBreak; (b) immediate next turn at the same wallclock falls
through to natural intent.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 10: CLI `--session-break-after-mins` flag

**Files:**
- Modify: `src/crates/primer-cli/src/main.rs`

- [ ] **Step 1: Find the existing CLI struct**

Run: `grep -n "Args\|#\[clap\|struct.*Cli\|vocab.max.per.prompt\|context_window_turns" src/crates/primer-cli/src/main.rs | head -10`

Locate the args struct. It will be near the top of `main.rs` — look for the existing `--vocab-max-per-prompt` flag definition; the new flag will sit alongside it.

- [ ] **Step 2: Write the failing tests**

Find the existing CLI test mod in main.rs (search for `mod classifier_construction_tests` and look for adjacent test mods, or add a new one). Append a new `mod break_suggest_flag_tests` near the end of the file:

```rust
#[cfg(test)]
mod break_suggest_flag_tests {
    use super::Cli; // <- adjust to actual struct name; could be `Args` or `Opts`
    use clap::Parser;

    #[test]
    fn explicit_value_overrides_default() {
        let cli = Cli::try_parse_from([
            "primer",
            "--name", "Ada",
            "--age", "9",
            "--session-break-after-mins", "15",
        ]).unwrap();
        assert_eq!(cli.session_break_after_mins, 15);
    }

    #[test]
    fn default_is_30() {
        let cli = Cli::try_parse_from([
            "primer",
            "--name", "Ada",
            "--age", "9",
        ]).unwrap();
        assert_eq!(
            cli.session_break_after_mins,
            primer_core::consts::break_suggest::DEFAULT_INTERVAL_MINUTES,
        );
    }

    #[test]
    fn zero_is_rejected() {
        let result = Cli::try_parse_from([
            "primer",
            "--name", "Ada",
            "--age", "9",
            "--session-break-after-mins", "0",
        ]);
        assert!(result.is_err(), "0 should be rejected by the value parser");
    }
}
```

(Adjust the struct name and the field name to match what you find in Step 1. If the existing struct uses `Args`, use `Args` here. If `--vocab-max-per-prompt` lives on a struct field with a known name, follow the same convention.)

- [ ] **Step 3: Run tests to verify they fail**

Run: `cd src && ~/.cargo/bin/cargo test -p primer-cli break_suggest_flag_tests 2>&1 | tail -10`
Expected: compile error — `session_break_after_mins` field doesn't exist on the args struct.

- [ ] **Step 4: Add the flag to the args struct**

Edit `src/crates/primer-cli/src/main.rs`. Add to the args struct (in alphabetical / proximity-to-vocab order):

```rust
    /// Minutes between break-suggestion nudges. The Primer phrases the
    /// suggestion in-character; the child can keep going. Default 30.
    /// Must be >= 1.
    #[arg(
        long,
        default_value_t = primer_core::consts::break_suggest::DEFAULT_INTERVAL_MINUTES,
        value_parser = clap::value_parser!(u32).range(1..),
    )]
    pub session_break_after_mins: u32,
```

- [ ] **Step 5: Wire the flag into `PedagogyConfig`**

Find where `PedagogyConfig` is constructed in main.rs (search for `PedagogyConfig {` or `PedagogyConfig::default`). Apply the CLI value:

```rust
    let pedagogy_config = PedagogyConfig {
        break_suggest_after_minutes: cli.session_break_after_mins,
        ..PedagogyConfig::default()
    };
```

(If the existing code uses `PedagogyConfig::default()` and no override site exists yet, add one. The dialogue manager constructor takes `PedagogyConfig` — make sure this populated config is what gets passed in.)

- [ ] **Step 6: Run tests to verify they pass**

Run: `cd src && ~/.cargo/bin/cargo test -p primer-cli 2>&1 | tail -15`
Expected: green.

- [ ] **Step 7: Commit**

```bash
git add src/crates/primer-cli/src/main.rs
git commit -m "$(cat <<'EOF'
feat: cli: --session-break-after-mins flag (default 30, min 1)

Surfaces the break-suggestion cadence as a parental tunable. Wired
into PedagogyConfig.break_suggest_after_minutes; clap rejects 0 at
parse so the disabled-gate path is reachable only via direct config
construction (not via a typo on the command line).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 11: Verification gauntlet

**No file modifications. Just running everything.**

- [ ] **Step 1: Full test sweep**

Run: `cd src && ~/.cargo/bin/cargo test --workspace 2>&1 | tail -20`
Expected: workspace fully green. Count should be roughly **474 + 27 ≈ 501 passed** (the spec budgets ~27 new/changed tests).

- [ ] **Step 2: Clippy clean**

Run: `cd src && ~/.cargo/bin/cargo clippy --workspace --all-targets 2>&1 | tail -20`
Expected: zero warnings.

- [ ] **Step 3: Format check**

Run: `cd src && ~/.cargo/bin/cargo fmt --all -- --check 2>&1 | tail -10`
Expected: zero diff. If any output, run `~/.cargo/bin/cargo fmt --all` and stage the changes as a `chore: cargo fmt` commit.

- [ ] **Step 4: Speech-feature build**

Run: `cd src && ~/.cargo/bin/cargo build --workspace --features primer-cli/speech 2>&1 | tail -10`
Expected: clean build. Speech feature is unaffected by this change but the build catches accidental breakage.

- [ ] **Step 5: Smoke run with the stub backend**

Run: `cd src && ~/.cargo/bin/cargo run --bin primer -- --backend stub --name SmokeTester --age 9 --no-persist --session-break-after-mins 15 --verbose 2>&1 | head -5`
Then type `quit` to exit. Expected: REPL starts, `--session-break-after-mins 15` is accepted without complaint, no panics.

- [ ] **Step 6: If any step fails, fix and re-run.** No commit step here — verification doesn't change source. If a fix is needed it lands as a separate commit.

---

## Task 12: Documentation updates

**Files:**
- Modify: `README.md`
- Modify: `ROADMAP.md`
- Modify: `CLAUDE.md`

- [ ] **Step 1: Update README.md**

Find the section listing CLI flags or Phase 0.x progress. Add a one-line mention of the new flag:

```markdown
- `--session-break-after-mins N` — minutes between break-suggestion nudges (default 30; must be ≥1).
```

If the README has a Phase 0.3 status block, mark the break-suggestion item complete.

- [ ] **Step 2: Update ROADMAP.md**

Find the Phase 0.3 entries. Mark the break-suggestion item ✓ / done. If the ROADMAP tracks issues by row, update the corresponding row.

- [ ] **Step 3: Update CLAUDE.md**

Find the Phase 0.3 status statement (early in the file). Update to reflect that break-suggestion is now done and Phase 0.3 is complete.

Add a gotcha about the field rename, near the existing per-feature gotchas:

```markdown
- **`PedagogyConfig.max_session_minutes` was renamed to `break_suggest_after_minutes`.** It controls the cadence of break-suggestion nudges, not a hard session ceiling. The Primer never enforces a session halt — children can keep going through any number of nudges.
- **Break-suggestion gate state is in-memory.** `DialogueManager.last_break_suggested_at: Option<DateTime<Utc>>` resets on `new` and `resume_session`. A resumed session might cause one extra suggestion if timing aligns badly; that's intentional (see spec).
```

- [ ] **Step 4: Build the docs (no command needed — markdown is plain text)**

- [ ] **Step 5: Commit**

```bash
git add README.md ROADMAP.md CLAUDE.md
git commit -m "$(cat <<'EOF'
docs: README + ROADMAP + CLAUDE.md for break-suggestion (Phase 0.3 close)

README gains the --session-break-after-mins flag note. ROADMAP marks
the Phase 0.3 break-suggestion item complete. CLAUDE.md documents the
field rename gotcha (max_session_minutes → break_suggest_after_minutes)
and the in-memory state policy.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Self-Review Checklist (run before declaring complete)

- [ ] **Spec coverage:** Every section of [docs/superpowers/specs/2026-05-05-session-break-suggestion-design.md](../specs/2026-05-05-session-break-suggestion-design.md) is implemented. Cross-reference: pure function (Task 2), variant + storage (Task 3), field rename (Task 4), prompt-pack (Task 5), decide_intent (Task 6), system-prompt section (Task 7), DialogueManager state (Task 8), wallclock plumbing (Task 9), CLI flag (Task 10).
- [ ] **Test count:** ~27 new/changed tests landed across tasks 2, 3, 5, 6, 7, 8, 9, 10. Existing 474 must stay green.
- [ ] **No unused code:** `should_suggest_break()` accessor is gone; hardcoded `println!` is gone; old field name is gone.
- [ ] **Locale awareness:** Both en.toml and de.toml have the new key with the `{minutes}` placeholder; substitution works in both.
- [ ] **No magic numbers:** `30` lives in `consts::break_suggest::DEFAULT_INTERVAL_MINUTES`; everywhere else either reads from config or from this const.
- [ ] **File-size hygiene:** New `session_timing.rs` ≪ 500 lines; modifications to existing files don't push them past 500 (lifecycle.rs, turn.rs, prompt_builder.rs were already large but unchanged in shape).
- [ ] **Final verification gauntlet (Task 11) ran clean.**
