# Comprehension Classifier Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add per-concept comprehension assessment chained after the existing concept extractor, producing `ComprehensionAssessment { concept, depth, confidence, evidence }` for each concept the child engaged with in an exchange. Results promote `LearnerModel.concepts[c].depth` via monotonic max (in-memory) and persist to a new `turn_comprehensions` table (schema v5) for offline analysis.

**Architecture:** New `primer-comprehension` crate (sibling of `primer-classifier` and `primer-extractor`) plus a new `primer-core::llm_util` module hosting two helpers (`extract_first_json_object`, `truncate_to_chars`) currently duplicated across the two existing structured-output crates. The dialogue manager's existing post-response extractor task is widened into a chained extractor→comprehension task; one `JoinHandle` covers both, with a combined bounded-await timeout at the start of the next turn. CLI gains three flags (`--comprehension-backend`, `--comprehension-model`, `--comprehension-timeout-ms`) verbatim parallel to the extractor flags. Schema v5 migration adds `comprehension_classifiers` lookup + `turn_comprehensions` rows (one per concept per turn), reusing the v4 `understanding_depths` lookup and the existing `concepts` table. Soft-fail policy throughout — errors log via `tracing::warn!` and return `ComprehensionResult::empty()`.

**Tech Stack:** Rust 2024 edition (workspace under `src/`), tokio, async-trait, serde/serde_json, rusqlite (bundled SQLite + FTS5), tracing, clap. Build commands run as `~/.cargo/bin/cargo …` from `src/` (rustup toolchain pinned via `rust-toolchain.toml`; Homebrew rust shadows the version we need).

---

## Working ground rules

- **Workspace root is `src/`, not the repo root.** Every cargo command runs from `src/`.
- **Always invoke as `~/.cargo/bin/cargo …`** so the rustup proxy is honored (Homebrew rust 1.86 silently shadows the pinned 1.87+ otherwise).
- **TDD throughout.** Write the failing test first, run it, see the failure, then implement to green. Commit at every green checkpoint.
- **No magic numbers.** Every numeric in production code goes to `consts.rs` (if invariant) or settings (if tunable), per CLAUDE.md.
- **Keep files under ~500 lines** where reasonable; refactor when growing too big. New crate files target ≤300 lines.
- **Never silently demote `concept.depth`.** Apply step uses monotonic max only.
- **Soft-fail every async background task.** Log via `tracing::warn!` and return the empty/unknown variant; never propagate `Err` into the dialogue path.
- **Schema migrations are idempotent + transaction-wrapped.** Mirror the v3/v4 templates in `primer-storage::schema`.
- **`update_turn_concepts` exists for a reason.** Persistence of concept additions to already-saved turns goes through `update_turn_concepts` / `update_exchange_concepts`, not by reopening `save_session`. Same rationale will apply to `save_comprehensions` (we never re-classify already-classified turns through `save_session`).

## File inventory

This plan creates new files and modifies existing files in this order:

**New files (12)**
- `src/crates/primer-comprehension/Cargo.toml`
- `src/crates/primer-comprehension/src/lib.rs` — trait + re-exports
- `src/crates/primer-comprehension/src/llm.rs` — `LlmComprehensionClassifier`
- `src/crates/primer-comprehension/src/stub.rs` — `StubComprehensionClassifier`
- `src/crates/primer-comprehension/src/settings.rs` — `ComprehensionSettings`
- `src/crates/primer-comprehension/src/consts.rs` — defaults
- `src/crates/primer-core/src/comprehension.rs` — output + context types
- `src/crates/primer-core/src/llm_util.rs` — shared JSON-extract + char-truncate helpers

**Modified files (10)**
- `src/Cargo.toml` — add new crate to `[workspace] members` + `[workspace.dependencies]`
- `src/crates/primer-core/src/lib.rs` — register new modules
- `src/crates/primer-core/src/learner.rs` — add `UnderstandingDepth::name()` + `Display`
- `src/crates/primer-core/src/storage.rs` — add `save_comprehensions` to `SessionStore` trait
- `src/crates/primer-classifier/src/llm.rs` — replace duplicate helpers with `primer-core::llm_util`
- `src/crates/primer-extractor/src/llm.rs` — replace duplicate helpers with `primer-core::llm_util`
- `src/crates/primer-storage/src/schema.rs` — v5 migration; bump `USER_VERSION`
- `src/crates/primer-storage/src/catalog.rs` — `get_or_create_comprehension_classifier_id`; rewrite `understanding_depth_name` as a delegator
- `src/crates/primer-storage/src/lib.rs` — implement `save_comprehensions`; call `apply_v5_migrations` from `open()`
- `src/crates/primer-pedagogy/src/Cargo.toml` — add `primer-comprehension` dep
- `src/crates/primer-pedagogy/src/dialogue_manager.rs` — extend `DialogueManagerSubsystems`, rename `extract_task`, add `apply_comprehension`, chain inside the spawned task, add `await_pending_post_response`
- `src/crates/primer-cli/Cargo.toml` — add `primer-comprehension` dep
- `src/crates/primer-cli/src/main.rs` — three new CLI flags, `build_comprehension`, wire into `DialogueManagerSubsystems`, `--verbose` extension, `comprehension_construction_tests`
- `CLAUDE.md`, `ROADMAP.md`, `README.md` — documentation updates

## Sequencing rationale

Tasks are ordered so each one compiles cleanly on its own, with tests passing at every commit boundary:

1. Foundations (core types, shared utilities, depth name) — Tasks 1-3
2. Helper consolidation (drop classifier/extractor duplicates) — Task 4
3. Schema v5 + storage method — Tasks 5-7
4. Comprehension crate (settings, stub, llm) — Tasks 8-12
5. Dialogue manager wiring (chain + apply + await) — Tasks 13-15
6. CLI surface — Tasks 16-17
7. Documentation + final verification — Tasks 18-19

After Task 4, the workspace builds clean even though no new feature exists. After Task 12, the comprehension crate is fully testable in isolation. After Task 15, dialogue-manager unit tests prove end-to-end (with stubs); after Task 17, the binary runs end-to-end against a real LLM.

---

## Task 1: Add `UnderstandingDepth::name()` to `primer-core::learner`

**Files:**
- Modify: `src/crates/primer-core/src/learner.rs:64-73`

The depth-name string is currently sourced from `primer-storage::catalog::understanding_depth_name`. We need it accessible from `primer-comprehension::llm` (for prompt + parser) and from `primer-cli::main` (for `--verbose` output) without depending on `primer-storage`. Putting `name()` on the enum (mirroring `EngagementState::name()` exactly) is the cleanest fix.

- [ ] **Step 1: Write the failing test**

Append to `src/crates/primer-core/src/learner.rs` inside `mod tests`:

```rust
    #[test]
    fn understanding_depth_name_matches_canonical_strings() {
        assert_eq!(UnderstandingDepth::Unknown.name(), "Unknown");
        assert_eq!(UnderstandingDepth::Aware.name(), "Aware");
        assert_eq!(UnderstandingDepth::Recall.name(), "Recall");
        assert_eq!(UnderstandingDepth::Comprehension.name(), "Comprehension");
        assert_eq!(UnderstandingDepth::Application.name(), "Application");
        assert_eq!(UnderstandingDepth::Analysis.name(), "Analysis");
    }

    #[test]
    fn understanding_depth_display_uses_name() {
        assert_eq!(format!("{}", UnderstandingDepth::Comprehension), "Comprehension");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cd src && ~/.cargo/bin/cargo test -p primer-core understanding_depth_name 2>&1 | tail -20
```

Expected: compilation fails with `no method named name found for enum UnderstandingDepth` and `Display is not implemented for UnderstandingDepth`.

- [ ] **Step 3: Implement `name()` and `Display`**

Replace the current `impl UnderstandingDepth` block (lines 64-73) with:

```rust
impl UnderstandingDepth {
    pub const ALL: &'static [Self] = &[
        Self::Unknown,
        Self::Aware,
        Self::Recall,
        Self::Comprehension,
        Self::Application,
        Self::Analysis,
    ];

    /// Canonical machine-readable name. Stable identifier used by the
    /// comprehension-classifier JSON schema and the storage
    /// `understanding_depths` lookup table. Don't rename — depth IDs in
    /// the v4 schema are derived from these names and existing DBs
    /// validate against them.
    pub fn name(self) -> &'static str {
        match self {
            Self::Unknown => "Unknown",
            Self::Aware => "Aware",
            Self::Recall => "Recall",
            Self::Comprehension => "Comprehension",
            Self::Application => "Application",
            Self::Analysis => "Analysis",
        }
    }
}

impl std::fmt::Display for UnderstandingDepth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cd src && ~/.cargo/bin/cargo test -p primer-core understanding_depth 2>&1 | tail -10
```

Expected: all `understanding_depth_*` tests pass.

- [ ] **Step 5: Update `primer-storage::catalog::understanding_depth_name` to delegate**

Modify `src/crates/primer-storage/src/catalog.rs:153-161` from:

```rust
pub fn understanding_depth_name(depth: UnderstandingDepth) -> &'static str {
    match depth {
        UnderstandingDepth::Unknown => "Unknown",
        UnderstandingDepth::Aware => "Aware",
        UnderstandingDepth::Recall => "Recall",
        UnderstandingDepth::Comprehension => "Comprehension",
        UnderstandingDepth::Application => "Application",
        UnderstandingDepth::Analysis => "Analysis",
    }
}
```

to:

```rust
/// Canonical name string for an `UnderstandingDepth` variant.
///
/// Delegates to the enum's `name()` method (defined in
/// `primer_core::learner`). Kept as a free function here for parity
/// with `engagement_state_name`/`speaker_name`/`pedagogical_intent_name`
/// — call-sites in this crate read more uniformly that way. The two
/// must agree byte-for-byte; the v4 lookup-validate-and-seed pass
/// would fail loudly if they ever diverged.
pub fn understanding_depth_name(depth: UnderstandingDepth) -> &'static str {
    depth.name()
}
```

- [ ] **Step 6: Verify the workspace still builds and tests pass**

```bash
cd src && ~/.cargo/bin/cargo test --workspace 2>&1 | tail -5
```

Expected: 275 tests pass (no regression). The two new tests bring the count to 277.

- [ ] **Step 7: Commit**

```bash
cd /Users/hherb/src/primer && git add src/crates/primer-core/src/learner.rs src/crates/primer-storage/src/catalog.rs
git commit -m "$(cat <<'EOF'
feat(core): add UnderstandingDepth::name() + Display

Mirrors EngagementState::name(). primer-storage::catalog now delegates
to it, keeping the v4 understanding_depths lookup contents bit-identical.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Add `primer-core::llm_util` module with shared helpers

**Files:**
- Create: `src/crates/primer-core/src/llm_util.rs`
- Modify: `src/crates/primer-core/src/lib.rs`

The two helpers (`extract_first_json_object`, `truncate_to_chars`) currently live in both `primer-classifier::llm` and `primer-extractor::llm` verbatim. With the comprehension crate landing as the third caller, we extract them now (per spec + brief).

- [ ] **Step 1: Create `llm_util.rs` with the helpers and full test coverage**

Write `src/crates/primer-core/src/llm_util.rs`:

```rust
//! Shared helpers for LLM-output processing.
//!
//! Used by every structured-output classifier/extractor in the
//! workspace (`primer-classifier`, `primer-extractor`,
//! `primer-comprehension`). Pulled out of the individual crates to a
//! shared home so a fix to either helper applies everywhere; previously
//! both functions were duplicated verbatim across two crates.

/// Return a prefix of `s` that is at most `max_chars` Unicode scalar
/// values long. Unlike a raw byte-index slice, this never panics on
/// multi-byte character boundaries.
pub fn truncate_to_chars(s: &str, max_chars: usize) -> &str {
    if s.chars().count() <= max_chars {
        return s;
    }
    let end = s
        .char_indices()
        .nth(max_chars)
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    &s[..end]
}

/// Extract the first balanced JSON object from `raw`. Tolerates
/// wrapper text before/after, and JSON containing nested braces.
/// Returns the substring including the outer braces, or `None` if no
/// balanced object is found.
///
/// Walks the bytes once, tracking string state and escape state so a
/// brace inside a string literal does not count toward the depth.
pub fn extract_first_json_object(raw: &str) -> Option<&str> {
    let start = raw.find('{')?;
    let bytes = raw.as_bytes();
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape = false;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        if escape {
            escape = false;
            continue;
        }
        if in_string {
            match b {
                b'\\' => escape = true,
                b'"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&raw[start..=i]);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_returns_input_when_within_cap() {
        assert_eq!(truncate_to_chars("hello", 10), "hello");
    }

    #[test]
    fn truncate_cuts_to_char_count() {
        assert_eq!(truncate_to_chars("hello world", 5), "hello");
    }

    #[test]
    fn truncate_is_safe_on_multibyte_boundaries() {
        // "café" is 5 bytes (4 chars). Cutting at char 3 must stop
        // before the multibyte 'é', not slice through it.
        let s = "café";
        let cut = truncate_to_chars(s, 3);
        assert_eq!(cut, "caf");
    }

    #[test]
    fn extract_returns_simple_object() {
        assert_eq!(
            extract_first_json_object(r#"{"a":1}"#),
            Some(r#"{"a":1}"#)
        );
    }

    #[test]
    fn extract_handles_leading_and_trailing_text() {
        assert_eq!(
            extract_first_json_object(r#"junk before {"a":1} junk after"#),
            Some(r#"{"a":1}"#)
        );
    }

    #[test]
    fn extract_balances_nested_braces() {
        assert_eq!(
            extract_first_json_object(r#"x {"a":{"b":2}} y"#),
            Some(r#"{"a":{"b":2}}"#)
        );
    }

    #[test]
    fn extract_ignores_braces_inside_strings() {
        assert_eq!(
            extract_first_json_object(r#"{"text": "}{"}"#),
            Some(r#"{"text": "}{"}"#)
        );
    }

    #[test]
    fn extract_ignores_escaped_quotes_inside_strings() {
        // The escaped quote does not close the string; the outer
        // brace at the end closes the object.
        assert_eq!(
            extract_first_json_object(r#"{"x":"a\"b"}"#),
            Some(r#"{"x":"a\"b"}"#)
        );
    }

    #[test]
    fn extract_returns_none_when_unbalanced() {
        assert_eq!(extract_first_json_object("{ unclosed"), None);
    }

    #[test]
    fn extract_returns_none_when_no_brace() {
        assert_eq!(extract_first_json_object("plain text"), None);
    }
}
```

- [ ] **Step 2: Register the module in `primer-core::lib.rs`**

Modify `src/crates/primer-core/src/lib.rs`. Find the existing `pub mod ...;` declarations and add `pub mod llm_util;` alongside them (alphabetical placement is fine; e.g. between `learner` and `speech`).

- [ ] **Step 3: Run the tests to verify they pass**

```bash
cd src && ~/.cargo/bin/cargo test -p primer-core llm_util 2>&1 | tail -5
```

Expected: 9 new tests pass.

- [ ] **Step 4: Commit**

```bash
cd /Users/hherb/src/primer && git add src/crates/primer-core/src/llm_util.rs src/crates/primer-core/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(core): add llm_util module with shared JSON-extract + char-truncate

Hosts extract_first_json_object and truncate_to_chars previously
duplicated in primer-classifier::llm and primer-extractor::llm. The
two duplicates are removed in the next commit.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Drop the duplicate helpers from `primer-classifier` and `primer-extractor`

**Files:**
- Modify: `src/crates/primer-classifier/src/llm.rs:78-91, 186-221` (delete) + `:1-15` (add `use`)
- Modify: `src/crates/primer-extractor/src/llm.rs:90-100, 208-242` (delete) + `:1-15` (add `use`)

Replace local definitions with imports from `primer_core::llm_util`. The function bodies are byte-for-byte identical, so this is a pure refactor.

- [ ] **Step 1: Patch `primer-classifier/src/llm.rs`**

Add this import alongside the existing `use primer_core::...` lines (around line 10-13):

```rust
use primer_core::llm_util::{extract_first_json_object, truncate_to_chars};
```

Delete the local `truncate_to_chars` function (lines 78-91 — the comment block plus the body).

Delete the local `extract_first_json_object` function (lines 186-221).

- [ ] **Step 2: Patch `primer-extractor/src/llm.rs`**

Add the same import alongside existing `use primer_core::...` lines:

```rust
use primer_core::llm_util::{extract_first_json_object, truncate_to_chars};
```

Delete the local `truncate_to_chars` function (lines 90-100).

Delete the local `extract_first_json_object` function (lines 208-242).

- [ ] **Step 3: Run the full test suite**

```bash
cd src && ~/.cargo/bin/cargo test --workspace 2>&1 | tail -10
```

Expected: 286 tests pass (275 baseline + 2 from Task 1 + 9 from Task 2). No regressions; all classifier+extractor tests still green because the helpers behave identically.

- [ ] **Step 4: Lint and format**

```bash
cd src && ~/.cargo/bin/cargo clippy --workspace --all-targets 2>&1 | tail -5 && ~/.cargo/bin/cargo fmt --all -- --check
```

Expected: no warnings, format clean.

- [ ] **Step 5: Commit**

```bash
cd /Users/hherb/src/primer && git add src/crates/primer-classifier/src/llm.rs src/crates/primer-extractor/src/llm.rs
git commit -m "$(cat <<'EOF'
refactor(classifier,extractor): use primer-core::llm_util shared helpers

Drop duplicate extract_first_json_object and truncate_to_chars
definitions in favour of primer_core::llm_util. Pure refactor; no
behaviour change. Tests still cover both functions via primer-core's
own test module.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Add `primer-core::comprehension` types

**Files:**
- Create: `src/crates/primer-core/src/comprehension.rs`
- Modify: `src/crates/primer-core/src/lib.rs`

Defines the in-process types the new crate's trait will speak in: assessment, result, context.

- [ ] **Step 1: Write the failing tests**

Write `src/crates/primer-core/src/comprehension.rs`:

```rust
//! Types produced and consumed by the comprehension classifier.
//!
//! `ComprehensionAssessment` is one classifier output for one concept
//! on one turn. `ComprehensionResult` packages the per-concept
//! assessments. `ComprehensionContext` is the input — newtype-wrapped
//! so future fields can be added without breaking the trait signature.

use serde::{Deserialize, Serialize};

use crate::conversation::Turn;
use crate::learner::UnderstandingDepth;

/// One per-concept assessment of comprehension depth.
///
/// `evidence` is optional and populated only by classifier impls that
/// produce one (LLM impls can generate it cheaply; stub does not).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComprehensionAssessment {
    pub concept: String,
    pub depth: UnderstandingDepth,
    /// Confidence in the assessment, in [0.0, 1.0].
    pub confidence: f32,
    pub evidence: Option<String>,
}

/// One classifier output: zero-or-more per-concept assessments.
///
/// Empty when extraction yielded no concepts, when the LLM saw no
/// evidence for any candidate, or when the classifier soft-failed
/// (use `::empty()` for the soft-fail return).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ComprehensionResult {
    pub assessments: Vec<ComprehensionAssessment>,
}

impl ComprehensionResult {
    /// Empty result. Used as the soft-fail return on classifier errors
    /// (parse failure, network error, invalid output). Never shaped as
    /// `Err` — failures must not propagate.
    pub fn empty() -> Self {
        Self::default()
    }
}

/// Input to the comprehension classifier. Newtype'd so future fields
/// can be added without breaking the trait signature.
pub struct ComprehensionContext<'a> {
    pub child_turn: &'a Turn,
    pub primer_turn: &'a Turn,
    /// A small slice of preceding turns for context (oldest-first).
    /// Helps the LLM disambiguate pronoun references across turns.
    pub recent_turns: &'a [Turn],
    /// Concepts to assess: the dedup'd union of `child_concepts ∪
    /// primer_concepts` from the just-completed exchange's extraction,
    /// already capped to `ComprehensionSettings::max_concepts_per_call`.
    pub candidate_concepts: &'a [String],
    // Reserved for extension. Adding fields here is non-breaking
    // because the trait method takes ComprehensionContext by value.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_factory_returns_empty_assessments() {
        let r = ComprehensionResult::empty();
        assert!(r.assessments.is_empty());
    }

    #[test]
    fn assessment_round_trips_through_serde() {
        let a = ComprehensionAssessment {
            concept: "photosynthesis".into(),
            depth: UnderstandingDepth::Comprehension,
            confidence: 0.85,
            evidence: Some("explained in own words".into()),
        };
        let json = serde_json::to_string(&a).unwrap();
        let back: ComprehensionAssessment = serde_json::from_str(&json).unwrap();
        assert_eq!(back.concept, a.concept);
        assert_eq!(back.depth, a.depth);
        assert!((back.confidence - a.confidence).abs() < 1e-6);
        assert_eq!(back.evidence, a.evidence);
    }

    #[test]
    fn result_round_trips_through_serde() {
        let r = ComprehensionResult {
            assessments: vec![ComprehensionAssessment {
                concept: "gravity".into(),
                depth: UnderstandingDepth::Recall,
                confidence: 0.7,
                evidence: None,
            }],
        };
        let json = serde_json::to_string(&r).unwrap();
        let back: ComprehensionResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back.assessments.len(), 1);
        assert_eq!(back.assessments[0].concept, "gravity");
    }
}
```

- [ ] **Step 2: Register the module**

In `src/crates/primer-core/src/lib.rs`, add `pub mod comprehension;` alongside the existing `pub mod classifier;` / `pub mod extractor;` declarations.

- [ ] **Step 3: Run tests**

```bash
cd src && ~/.cargo/bin/cargo test -p primer-core comprehension 2>&1 | tail -10
```

Expected: 3 new tests pass.

- [ ] **Step 4: Commit**

```bash
cd /Users/hherb/src/primer && git add src/crates/primer-core/src/comprehension.rs src/crates/primer-core/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(core): add comprehension types (assessment, result, context)

Types the new primer-comprehension crate will speak in. Mirrors the
existing classifier/extractor type pairs in primer-core.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Schema v5 — `comprehension_classifiers` + `turn_comprehensions`

**Files:**
- Modify: `src/crates/primer-storage/src/schema.rs`

Add the schema-string constants, the `apply_v5_migrations` function, bump `USER_VERSION` to 5. Idempotent + transaction-wrapped, mirroring v3 and v4.

- [ ] **Step 1: Write the failing tests**

In `src/crates/primer-storage/src/schema.rs`'s test module (find the existing `#[cfg(test)] mod tests { ... }`), append:

```rust
    #[test]
    fn v5_migration_creates_tables_and_indices() {
        let conn = Connection::open_in_memory().unwrap();
        // v1
        conn.execute_batch(SCHEMA_SQL).unwrap();
        // v2..v5 in order so the FKs (e.g. understanding_depths from v4) exist
        apply_v2_migrations(&conn).unwrap();
        apply_v3_migrations(&conn).unwrap();
        apply_v4_migrations(&conn).unwrap();
        apply_v5_migrations(&conn).unwrap();
        assert!(table_exists(&conn, "comprehension_classifiers"));
        assert!(table_exists(&conn, "turn_comprehensions"));
        // Indices exist (queryable through sqlite_master with type='index').
        let idx_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name LIKE 'idx_turn_comprehensions_%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(idx_count, 2);
    }

    #[test]
    fn v5_migration_is_idempotent_on_re_run() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA_SQL).unwrap();
        apply_v2_migrations(&conn).unwrap();
        apply_v3_migrations(&conn).unwrap();
        apply_v4_migrations(&conn).unwrap();
        apply_v5_migrations(&conn).unwrap();
        // Running again must not error.
        apply_v5_migrations(&conn).unwrap();
        assert!(table_exists(&conn, "turn_comprehensions"));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cd src && ~/.cargo/bin/cargo test -p primer-storage v5_migration 2>&1 | tail -10
```

Expected: failure with `cannot find function apply_v5_migrations`.

- [ ] **Step 3: Add the schema strings + migration function**

In `src/crates/primer-storage/src/schema.rs`, append after the `apply_v4_migrations` function (around line 308):

```rust
// ─── v5 schema strings ──────────────────────────────────────────────────────
//
// v5 adds per-concept comprehension assessments alongside the existing
// per-turn engagement classifications (v3). One row per (turn, concept,
// classifier_id) — re-classification by a different classifier id lands
// as a parallel row, preserving historical labels.

const CREATE_COMPREHENSION_CLASSIFIERS_TABLE: &str = "
    CREATE TABLE IF NOT EXISTS comprehension_classifiers (
        id         INTEGER PRIMARY KEY AUTOINCREMENT,
        identifier TEXT NOT NULL UNIQUE
    )
";

const CREATE_TURN_COMPREHENSIONS_TABLE: &str = "
    CREATE TABLE IF NOT EXISTS turn_comprehensions (
        id              INTEGER PRIMARY KEY AUTOINCREMENT,
        session_id      TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
        turn_id         INTEGER NOT NULL REFERENCES turns(id),
        concept_id      INTEGER NOT NULL REFERENCES concepts(id),
        depth_id        INTEGER NOT NULL REFERENCES understanding_depths(id),
        confidence      REAL NOT NULL,
        classifier_id   INTEGER NOT NULL REFERENCES comprehension_classifiers(id),
        evidence        TEXT,
        created_at      TIMESTAMP NOT NULL,
        UNIQUE(turn_id, concept_id, classifier_id)
    )
";

const CREATE_TURN_COMPREHENSIONS_TURN_INDEX: &str = "
    CREATE INDEX IF NOT EXISTS idx_turn_comprehensions_turn
        ON turn_comprehensions(turn_id)
";

const CREATE_TURN_COMPREHENSIONS_CONCEPT_INDEX: &str = "
    CREATE INDEX IF NOT EXISTS idx_turn_comprehensions_concept
        ON turn_comprehensions(concept_id)
";

/// Apply v5 migrations idempotently. Safe to run on a fresh DB (after
/// v4 objects exist), on a v4 DB being upgraded, and on a v5 DB being
/// re-opened.
///
/// All steps run inside a single transaction so a partial failure rolls
/// back to the pre-migration state.
///
/// v5 adds:
/// - `comprehension_classifiers` lookup table (lazy population, mirrors
///   the v3 `classifiers` table).
/// - `turn_comprehensions` table — one row per (turn, concept,
///   classifier) recording an `UnderstandingDepth` label with confidence
///   + optional evidence text. FK'd into `turns`, `concepts`,
///   `understanding_depths`, and `comprehension_classifiers`.
/// - Two helper indices: by turn (to load all assessments for a turn)
///   and by concept (to trace a concept's depth trajectory across
///   sessions).
pub(crate) fn apply_v5_migrations(conn: &Connection) -> Result<()> {
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| PrimerError::Storage(format!("v5 migration: failed to begin tx: {e}")))?;
    tx.execute(CREATE_COMPREHENSION_CLASSIFIERS_TABLE, [])
        .map_err(|e| PrimerError::Storage(format!("v5 migration: comprehension_classifiers: {e}")))?;
    tx.execute(CREATE_TURN_COMPREHENSIONS_TABLE, [])
        .map_err(|e| PrimerError::Storage(format!("v5 migration: turn_comprehensions: {e}")))?;
    tx.execute(CREATE_TURN_COMPREHENSIONS_TURN_INDEX, [])
        .map_err(|e| PrimerError::Storage(format!("v5 migration: turn-index: {e}")))?;
    tx.execute(CREATE_TURN_COMPREHENSIONS_CONCEPT_INDEX, [])
        .map_err(|e| PrimerError::Storage(format!("v5 migration: concept-index: {e}")))?;
    tx.commit()
        .map_err(|e| PrimerError::Storage(format!("v5 migration: commit: {e}")))?;
    Ok(())
}
```

Bump `pub const USER_VERSION: i64 = 4;` (line 25) to:

```rust
pub const USER_VERSION: i64 = 5;
```

Update the doc comment block above `USER_VERSION` to add a paragraph for v5:

```rust
/// Bumped to 5 when we added the `comprehension_classifiers` and
/// `turn_comprehensions` tables that back the per-concept
/// comprehension-classifier feature (Phase 0.3).
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cd src && ~/.cargo/bin/cargo test -p primer-storage v5_migration 2>&1 | tail -5
```

Expected: 2 new tests pass.

- [ ] **Step 5: Wire the migration into `open()`**

Find where `apply_v4_migrations(&conn)?;` is called in `src/crates/primer-storage/src/lib.rs` (search for it):

```bash
grep -n "apply_v4_migrations" src/crates/primer-storage/src/lib.rs
```

Immediately after that line, add:

```rust
        schema::apply_v5_migrations(&conn)?;
```

(Use the same call style — `schema::apply_v5_migrations` if v4 uses that, or just `apply_v5_migrations` if it's already imported. Match the existing line.)

- [ ] **Step 6: Re-run the full storage test suite**

```bash
cd src && ~/.cargo/bin/cargo test -p primer-storage 2>&1 | tail -10
```

Expected: previous storage tests still pass + 2 new v5 tests pass. **An existing test that opens a DB and calls `pragma user_version` may fail with `expected 4, got 5`** — fix any such assertion to expect 5.

- [ ] **Step 7: Commit**

```bash
cd /Users/hherb/src/primer && git add src/crates/primer-storage/src/schema.rs src/crates/primer-storage/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(storage): schema v5 — comprehension_classifiers + turn_comprehensions

Idempotent + transaction-wrapped migration adds the two tables that
back per-concept comprehension assessments. UNIQUE(turn_id, concept_id,
classifier_id) makes re-classification by the same model id a no-op
while a different classifier id lands as a parallel row.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: Add `SessionStore::save_comprehensions` trait method

**Files:**
- Modify: `src/crates/primer-core/src/storage.rs`

Adds the new method to the trait and threads `ComprehensionAssessment` through. Implementations land in Task 7.

- [ ] **Step 1: Add the trait method**

Modify `src/crates/primer-core/src/storage.rs`. After the `update_exchange_concepts` method (around line 150), append the new method to the `SessionStore` trait:

```rust
    /// Persist the per-concept comprehension assessments for one
    /// completed exchange. Resolves `(session_id, primer_turn_index)`
    /// → `turn_id` internally; lazily creates any new `concepts` and
    /// `comprehension_classifiers` rows; UNIQUE-constrained on
    /// `(turn_id, concept_id, classifier_id)` so re-saving the same
    /// classifier's output for the same turn is a no-op.
    ///
    /// `primer_turn_index` is used (not the child turn) because the
    /// comprehension assessment describes what the child demonstrated
    /// in the exchange that *concluded* with the Primer's response —
    /// keying off the Primer turn aligns the row with "the most recent
    /// thing the model can have an opinion about."
    ///
    /// An empty `assessments` slice is a no-op (returns `Ok(())`).
    /// Returns `Err` if `(session_id, primer_turn_index)` does not
    /// resolve to an existing turn, or on genuine I/O failure. The
    /// whole call runs inside a single transaction.
    async fn save_comprehensions(
        &self,
        session_id: SessionId,
        primer_turn_index: usize,
        assessments: &[crate::comprehension::ComprehensionAssessment],
        classifier_identifier: &str,
    ) -> Result<()>;
```

Add the `ComprehensionAssessment` import alongside the existing imports at the top of the file:

```rust
use crate::comprehension::ComprehensionAssessment;
```

(Then use `&[ComprehensionAssessment]` instead of the fully qualified name in the signature, for consistency with `&EngagementAssessment` above. Update the actual signature accordingly.)

- [ ] **Step 2: Try to build — expect a compile error in `primer-storage`**

```bash
cd src && ~/.cargo/bin/cargo build -p primer-storage 2>&1 | tail -10
```

Expected: `not all trait items implemented, missing: save_comprehensions` for `SqliteSessionStore`. This drives Task 7.

- [ ] **Step 3: Don't commit yet** — Task 7 lands the impl in the same commit as the trait method to keep the workspace buildable at every commit boundary.

---

## Task 7: Implement `SessionStore::save_comprehensions` in `SqliteSessionStore`

**Files:**
- Modify: `src/crates/primer-storage/src/lib.rs`
- Modify: `src/crates/primer-storage/src/catalog.rs`

- [ ] **Step 1: Add `get_or_create_comprehension_classifier_id` helper to catalog.rs**

Find the existing `get_or_create_classifier_id` helper in `src/crates/primer-storage/src/catalog.rs` (search for it) and add a sibling immediately below it:

```rust
/// Resolve a comprehension-classifier identifier string to its
/// integer id, inserting into `comprehension_classifiers` lazily on
/// first observation. Mirrors `get_or_create_classifier_id` exactly,
/// just against the v5 lookup table.
pub(crate) fn get_or_create_comprehension_classifier_id(
    conn: &Connection,
    identifier: &str,
) -> Result<i64> {
    if let Some(id) = conn
        .query_row(
            "SELECT id FROM comprehension_classifiers WHERE identifier = ?1",
            rusqlite::params![identifier],
            |r| r.get::<_, i64>(0),
        )
        .optional()
        .map_err(|e| PrimerError::Storage(format!("comprehension_classifier_id lookup: {e}")))?
    {
        return Ok(id);
    }
    conn.execute(
        "INSERT INTO comprehension_classifiers (identifier) VALUES (?1)",
        rusqlite::params![identifier],
    )
    .map_err(|e| PrimerError::Storage(format!("comprehension_classifier_id insert: {e}")))?;
    Ok(conn.last_insert_rowid())
}
```

(If the existing `get_or_create_classifier_id` uses a slightly different shape, copy that exactly — match `Connection` vs `Transaction`, `Result` type, error format string conventions etc. Read the existing helper before writing.)

- [ ] **Step 2: Write failing tests for `save_comprehensions`**

In `src/crates/primer-storage/src/lib.rs` test module (find `#[cfg(test)] mod tests` near the bottom of the file, or wherever the existing `save_classification_*` tests live), append:

```rust
    // ─── save_comprehensions ────────────────────────────────────────────

    #[tokio::test]
    async fn save_comprehensions_persists_one_row_per_concept() {
        use primer_core::comprehension::ComprehensionAssessment;
        use primer_core::learner::UnderstandingDepth;

        let store = SqliteSessionStore::open(":memory:").unwrap();
        let session = make_two_turn_session();
        store.save_session(&session).await.unwrap();

        let assessments = vec![
            ComprehensionAssessment {
                concept: "photosynthesis".into(),
                depth: UnderstandingDepth::Comprehension,
                confidence: 0.85,
                evidence: Some("explained in own words".into()),
            },
            ComprehensionAssessment {
                concept: "chlorophyll".into(),
                depth: UnderstandingDepth::Aware,
                confidence: 0.6,
                evidence: None,
            },
        ];
        store
            .save_comprehensions(session.id, 1, &assessments, "llm:test:model")
            .await
            .unwrap();

        let count: i64 = store
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM turn_comprehensions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn save_comprehensions_unique_constraint_makes_resave_a_noop() {
        use primer_core::comprehension::ComprehensionAssessment;
        use primer_core::learner::UnderstandingDepth;

        let store = SqliteSessionStore::open(":memory:").unwrap();
        let session = make_two_turn_session();
        store.save_session(&session).await.unwrap();

        let a = vec![ComprehensionAssessment {
            concept: "gravity".into(),
            depth: UnderstandingDepth::Recall,
            confidence: 0.7,
            evidence: None,
        }];
        store
            .save_comprehensions(session.id, 1, &a, "llm:test:model")
            .await
            .unwrap();
        // Same classifier + same turn + same concept → INSERT OR IGNORE
        // makes this a no-op.
        store
            .save_comprehensions(session.id, 1, &a, "llm:test:model")
            .await
            .unwrap();

        let count: i64 = store
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM turn_comprehensions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn save_comprehensions_empty_slice_is_noop() {
        let store = SqliteSessionStore::open(":memory:").unwrap();
        let session = make_two_turn_session();
        store.save_session(&session).await.unwrap();

        store
            .save_comprehensions(session.id, 1, &[], "llm:test:model")
            .await
            .unwrap();

        let count: i64 = store
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM turn_comprehensions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn save_comprehensions_missing_turn_returns_err() {
        use primer_core::comprehension::ComprehensionAssessment;
        use primer_core::learner::UnderstandingDepth;

        let store = SqliteSessionStore::open(":memory:").unwrap();
        let session = make_two_turn_session();
        store.save_session(&session).await.unwrap();

        let a = vec![ComprehensionAssessment {
            concept: "x".into(),
            depth: UnderstandingDepth::Aware,
            confidence: 0.5,
            evidence: None,
        }];
        let result = store
            .save_comprehensions(session.id, 99, &a, "llm:test:model")
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn save_comprehensions_classifier_lookup_populated_lazily() {
        use primer_core::comprehension::ComprehensionAssessment;
        use primer_core::learner::UnderstandingDepth;

        let store = SqliteSessionStore::open(":memory:").unwrap();
        let session = make_two_turn_session();
        store.save_session(&session).await.unwrap();

        let a = vec![ComprehensionAssessment {
            concept: "x".into(),
            depth: UnderstandingDepth::Aware,
            confidence: 0.5,
            evidence: None,
        }];
        store
            .save_comprehensions(session.id, 1, &a, "llm:newmodel")
            .await
            .unwrap();

        let count: i64 = store
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM comprehension_classifiers WHERE identifier = 'llm:newmodel'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }
```

The helper `make_two_turn_session()` likely already exists in this test module; if not, locate the closest test that builds a small session and copy its setup. The test must produce a `Session` whose turn at index 1 is a Primer turn. Read the test module's existing helpers (search for `make_*_session` or `fn fixture_*`) and reuse them.

- [ ] **Step 2.5: Run the tests to verify they fail**

```bash
cd src && ~/.cargo/bin/cargo test -p primer-storage save_comprehensions 2>&1 | tail -10
```

Expected: compile failure mentioning `save_comprehensions` not in scope on the impl.

- [ ] **Step 3: Implement `save_comprehensions` on `SqliteSessionStore`**

Find the `impl SessionStore for SqliteSessionStore` block in `src/crates/primer-storage/src/lib.rs`. After the existing `update_exchange_concepts` method, append:

```rust
    async fn save_comprehensions(
        &self,
        session_id: primer_core::conversation::SessionId,
        primer_turn_index: usize,
        assessments: &[primer_core::comprehension::ComprehensionAssessment],
        classifier_identifier: &str,
    ) -> Result<()> {
        if assessments.is_empty() {
            return Ok(());
        }

        let conn = self.conn.lock().unwrap();
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| PrimerError::Storage(format!("save_comprehensions begin tx: {e}")))?;

        // Resolve (session_id, primer_turn_index) → turn.id
        let turn_id: i64 = tx
            .query_row(
                "SELECT id FROM turns WHERE session_id = ?1 AND turn_index = ?2",
                rusqlite::params![session_id.to_string(), primer_turn_index as i64],
                |r| r.get(0),
            )
            .map_err(|e| {
                PrimerError::Storage(format!(
                    "save_comprehensions: turn_id lookup ({session_id}, {primer_turn_index}): {e}"
                ))
            })?;

        let classifier_id =
            catalog::get_or_create_comprehension_classifier_id(&tx, classifier_identifier)?;

        let now = Utc::now().to_rfc3339();
        // Per-call cache of concept names → ids so a concept appearing
        // in multiple assessments resolves to the same row without
        // hitting the DB twice. Aligns with the cache pattern used in
        // update_exchange_concepts.
        let mut concept_id_cache: std::collections::HashMap<String, i64> =
            std::collections::HashMap::new();

        for a in assessments {
            let concept_id = if let Some(&id) = concept_id_cache.get(&a.concept) {
                id
            } else {
                let id = get_or_create_concept_id(&tx, &a.concept)?;
                concept_id_cache.insert(a.concept.clone(), id);
                id
            };
            let depth_id = catalog::understanding_depth_id(a.depth);

            tx.execute(
                "INSERT OR IGNORE INTO turn_comprehensions \
                     (session_id, turn_id, concept_id, depth_id, confidence, classifier_id, evidence, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                rusqlite::params![
                    session_id.to_string(),
                    turn_id,
                    concept_id,
                    depth_id,
                    a.confidence,
                    classifier_id,
                    a.evidence.as_deref(),
                    now,
                ],
            )
            .map_err(|e| PrimerError::Storage(format!("save_comprehensions: insert: {e}")))?;
        }

        tx.commit()
            .map_err(|e| PrimerError::Storage(format!("save_comprehensions commit: {e}")))?;
        Ok(())
    }
```

The helper `get_or_create_concept_id` already exists in this file (used by `update_turn_concepts` / `update_exchange_concepts`). If its name differs slightly, match the existing helper.

- [ ] **Step 4: Run the storage tests**

```bash
cd src && ~/.cargo/bin/cargo test -p primer-storage 2>&1 | tail -10
```

Expected: all pre-existing storage tests still pass + 5 new `save_comprehensions_*` tests pass.

- [ ] **Step 5: Lint, format, full test suite**

```bash
cd src && ~/.cargo/bin/cargo clippy --workspace --all-targets 2>&1 | tail -10 && ~/.cargo/bin/cargo fmt --all -- --check && ~/.cargo/bin/cargo test --workspace 2>&1 | tail -5
```

Expected: no warnings, format clean, all tests pass.

- [ ] **Step 6: Commit**

```bash
cd /Users/hherb/src/primer && git add src/crates/primer-core/src/storage.rs src/crates/primer-storage/src/lib.rs src/crates/primer-storage/src/catalog.rs
git commit -m "$(cat <<'EOF'
feat(storage): SessionStore::save_comprehensions

Persists per-concept comprehension assessments for one exchange,
keyed by (turn_id, concept_id, classifier_id). UNIQUE-constrained
so re-saving is a no-op; transaction-wrapped so a partial failure
rolls back. comprehension_classifiers populated lazily.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: Scaffold `primer-comprehension` crate (Cargo.toml + empty lib.rs)

**Files:**
- Create: `src/crates/primer-comprehension/Cargo.toml`
- Create: `src/crates/primer-comprehension/src/lib.rs`
- Modify: `src/Cargo.toml`

- [ ] **Step 1: Create the crate manifest**

Write `src/crates/primer-comprehension/Cargo.toml`:

```toml
[package]
name = "primer-comprehension"
version = "0.1.0"
edition = "2024"
description = "Per-concept comprehension classifier trait + implementations for the Primer."

[dependencies]
primer-core.workspace = true
async-trait.workspace = true
tokio = { workspace = true, features = ["sync"] }
tracing.workspace = true
serde.workspace = true
serde_json.workspace = true

[dev-dependencies]
tokio = { workspace = true, features = ["macros", "rt", "sync", "time"] }
futures.workspace = true
chrono.workspace = true
```

- [ ] **Step 2: Create the trait stub**

Write `src/crates/primer-comprehension/src/lib.rs`:

```rust
//! Comprehension classifier — trait + implementations.
//!
//! Assesses the depth of understanding a child has demonstrated for
//! each concept they engaged with in one completed exchange.
//! `LlmComprehensionClassifier` wraps an `Arc<dyn InferenceBackend>`
//! so the crate stays free of `primer-inference` as a build
//! dependency. A stub implementation supports tests.
//!
//! Soft-fail policy: classifier errors must NEVER propagate up and
//! abort the conversation. Implementations log via `tracing::warn!`
//! and return `ComprehensionResult::empty()` on any failure.

use async_trait::async_trait;

use primer_core::comprehension::{ComprehensionContext, ComprehensionResult};
use primer_core::error::Result;

pub mod consts;
pub mod llm;
pub mod settings;
pub mod stub;

pub use llm::LlmComprehensionClassifier;
pub use settings::ComprehensionSettings;
pub use stub::StubComprehensionClassifier;

/// Per-concept comprehension classifier.
///
/// Implementations may be LLM-backed (via `Arc<dyn InferenceBackend>`),
/// rule-based (future), or a stub for tests. Soft-fail policy applies
/// — never propagate errors.
#[async_trait]
pub trait ComprehensionClassifier: Send + Sync {
    /// Stable identifier carrying impl + model. Persisted in the
    /// `comprehension_classifiers` lookup table; lets us re-classify
    /// the archive with a different classifier later without
    /// overwriting old labels. Examples: `"llm:cloud-anthropic:claude-haiku-4-5"`,
    /// `"stub"`.
    fn identifier(&self) -> &str;

    async fn classify(&self, ctx: ComprehensionContext<'_>) -> Result<ComprehensionResult>;
}
```

- [ ] **Step 3: Register the crate in the workspace**

Modify `src/Cargo.toml`:

In the `[workspace] members` array, add `"crates/primer-comprehension"` alongside the existing entries:

```toml
[workspace]
resolver = "2"
members = [
    "crates/primer-core",
    "crates/primer-inference",
    "crates/primer-speech",
    "crates/primer-knowledge",
    "crates/primer-pedagogy",
    "crates/primer-cli",
    "crates/primer-storage",
    "crates/primer-classifier",
    "crates/primer-comprehension",
    "crates/primer-extractor",
]
```

In the `[workspace.dependencies]` block, alongside the existing `primer-* = { path = ... }` lines, add:

```toml
primer-comprehension = { path = "crates/primer-comprehension" }
```

- [ ] **Step 4: Build to verify the crate compiles (with empty stub/llm/settings/consts modules expected to fail next)**

```bash
cd src && ~/.cargo/bin/cargo build -p primer-comprehension 2>&1 | tail -10
```

Expected: failure mentioning `unresolved module 'consts'` etc. — that's normal; we add the contents next.

- [ ] **Step 5: Don't commit yet** — the crate isn't viable until Tasks 9–11 fill in the modules. Continue.

---

## Task 9: `primer-comprehension::consts` + `settings`

**Files:**
- Create: `src/crates/primer-comprehension/src/consts.rs`
- Create: `src/crates/primer-comprehension/src/settings.rs`

- [ ] **Step 1: Write `consts.rs`**

```rust
//! Default values for `ComprehensionSettings`. Per the no-magic-numbers
//! convention, every numeric used by the comprehension subsystem is
//! defined here (or in a sibling settings struct field).

pub const DEFAULT_BLOCKING_TIMEOUT_MS: u64 = 1500;
pub const DEFAULT_RECENT_CONTEXT_TURNS: usize = 4;

/// Hard cap on the LLM's raw output length before parsing. Larger than
/// the extractor's cap because comprehension's output is per-concept
/// (10 entries × ~60 chars structured ≈ ~600 chars before envelope,
/// plus evidence text up to evidence_max_chars per assessment).
pub const DEFAULT_MAX_COMPREHENSION_OUTPUT_CHARS: usize = 4096;

/// Hard cap on candidate concepts handed to one classifier call.
/// Bounds prompt size on pathological turns; concepts beyond the cap
/// are simply not assessed this turn (they retain their existing
/// learner.concepts state and may be assessed in a future turn).
pub const DEFAULT_MAX_CONCEPTS_PER_CALL: usize = 10;

/// Confidence threshold below which an assessment is persisted to
/// `turn_comprehensions` (raw signal preserved) but NOT applied to
/// in-memory `LearnerModel.concepts.depth`. Same value the engagement
/// classifier uses, intentionally — keeps two LLM-confidence-gated
/// classifiers from getting out of step.
pub const DEFAULT_CONFIDENCE_THRESHOLD: f32 = 0.6;

pub const DEFAULT_COMPREHENSION_MAX_TOKENS: u32 = 512;
pub const DEFAULT_COMPREHENSION_TEMPERATURE: f32 = 0.1;
pub const DEFAULT_COMPREHENSION_TOP_P: f32 = 0.9;

/// Per-assessment evidence-text cap. Truncated char-boundary-safe at
/// the parser layer so a verbose LLM doesn't produce 1KB evidence
/// blobs that bloat `turn_comprehensions.evidence`.
pub const DEFAULT_EVIDENCE_MAX_CHARS: usize = 240;

/// Char cap on the snippet of unparseable LLM output included in the
/// `tracing::warn!` log line. Tracing concern only — not user-tunable.
pub const LLM_DEBUG_SNIPPET_CHARS: usize = 256;
```

- [ ] **Step 2: Write `settings.rs`**

```rust
//! Tunable settings for the comprehension subsystem.

use std::time::Duration;

use crate::consts;

#[derive(Debug, Clone)]
pub struct ComprehensionSettings {
    /// Maximum time to block awaiting the previous turn's
    /// extractor→comprehension chain before the next intent decision.
    /// On timeout, the task is detached so DB persistence still
    /// completes; we just don't wait for it.
    pub blocking_timeout: Duration,
    /// How many surrounding turns to include as context in the
    /// classifier prompt. Helps disambiguate pronoun references.
    pub recent_context_turns: usize,
    /// Hard cap on the LLM's raw output length before parsing.
    /// Char-boundary-safe.
    pub max_output_chars: usize,
    /// Hard cap on candidate concepts handed to one classifier call.
    /// Bounds prompt size; over-cap concepts are simply not assessed
    /// this turn.
    pub max_concepts_per_call: usize,
    /// Below this confidence, an assessment is persisted but NOT
    /// applied to in-memory `LearnerModel.concepts.depth`. Above it
    /// (>=), `apply_comprehension` promotes depth via monotonic max.
    pub confidence_threshold: f32,
    pub generation_max_tokens: u32,
    pub generation_temperature: f32,
    pub generation_top_p: f32,
    /// Per-assessment evidence-text cap (chars). Truncated
    /// char-boundary-safe at the parser layer.
    pub evidence_max_chars: usize,
}

impl Default for ComprehensionSettings {
    fn default() -> Self {
        Self {
            blocking_timeout: Duration::from_millis(consts::DEFAULT_BLOCKING_TIMEOUT_MS),
            recent_context_turns: consts::DEFAULT_RECENT_CONTEXT_TURNS,
            max_output_chars: consts::DEFAULT_MAX_COMPREHENSION_OUTPUT_CHARS,
            max_concepts_per_call: consts::DEFAULT_MAX_CONCEPTS_PER_CALL,
            confidence_threshold: consts::DEFAULT_CONFIDENCE_THRESHOLD,
            generation_max_tokens: consts::DEFAULT_COMPREHENSION_MAX_TOKENS,
            generation_temperature: consts::DEFAULT_COMPREHENSION_TEMPERATURE,
            generation_top_p: consts::DEFAULT_COMPREHENSION_TOP_P,
            evidence_max_chars: consts::DEFAULT_EVIDENCE_MAX_CHARS,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_consts() {
        let s = ComprehensionSettings::default();
        assert_eq!(
            s.blocking_timeout,
            Duration::from_millis(consts::DEFAULT_BLOCKING_TIMEOUT_MS)
        );
        assert_eq!(s.recent_context_turns, consts::DEFAULT_RECENT_CONTEXT_TURNS);
        assert_eq!(
            s.max_output_chars,
            consts::DEFAULT_MAX_COMPREHENSION_OUTPUT_CHARS
        );
        assert_eq!(
            s.max_concepts_per_call,
            consts::DEFAULT_MAX_CONCEPTS_PER_CALL
        );
        assert!(
            (s.confidence_threshold - consts::DEFAULT_CONFIDENCE_THRESHOLD).abs() < 1e-6
        );
        assert_eq!(
            s.generation_max_tokens,
            consts::DEFAULT_COMPREHENSION_MAX_TOKENS
        );
        assert!(
            (s.generation_temperature - consts::DEFAULT_COMPREHENSION_TEMPERATURE).abs() < 1e-6
        );
        assert!((s.generation_top_p - consts::DEFAULT_COMPREHENSION_TOP_P).abs() < 1e-6);
        assert_eq!(s.evidence_max_chars, consts::DEFAULT_EVIDENCE_MAX_CHARS);
    }
}
```

- [ ] **Step 3: Don't run tests yet** — `lib.rs` still references absent `stub` and `llm` modules. Continue.

---

## Task 10: `primer-comprehension::stub`

**Files:**
- Create: `src/crates/primer-comprehension/src/stub.rs`

- [ ] **Step 1: Write `stub.rs` with full test coverage**

```rust
//! Stub comprehension classifier — deterministic, test-controllable.
//!
//! Mirrors the shape of `primer_classifier::StubEngagementClassifier`
//! and `primer_extractor::StubConceptExtractor`.

use std::collections::VecDeque;

use async_trait::async_trait;
use tokio::sync::Mutex;

use primer_core::comprehension::{ComprehensionContext, ComprehensionResult};
use primer_core::error::Result;

use crate::ComprehensionClassifier;

pub struct StubComprehensionClassifier {
    fallback: ComprehensionResult,
    script: Mutex<Option<VecDeque<ComprehensionResult>>>,
}

impl StubComprehensionClassifier {
    pub fn new() -> Self {
        Self {
            fallback: ComprehensionResult::empty(),
            script: Mutex::new(None),
        }
    }

    pub fn with_response(response: ComprehensionResult) -> Self {
        Self {
            fallback: response,
            script: Mutex::new(None),
        }
    }

    pub fn with_script(script: Vec<ComprehensionResult>) -> Self {
        Self {
            fallback: ComprehensionResult::empty(),
            script: Mutex::new(Some(script.into())),
        }
    }
}

impl Default for StubComprehensionClassifier {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ComprehensionClassifier for StubComprehensionClassifier {
    fn identifier(&self) -> &str {
        "stub"
    }

    async fn classify(&self, _ctx: ComprehensionContext<'_>) -> Result<ComprehensionResult> {
        let mut script = self.script.lock().await;
        if let Some(q) = script.as_mut() {
            if let Some(next) = q.pop_front() {
                return Ok(next);
            }
        }
        Ok(self.fallback.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use primer_core::comprehension::ComprehensionAssessment;
    use primer_core::conversation::{Speaker, Turn};
    use primer_core::learner::UnderstandingDepth;

    fn t(speaker: Speaker, text: &str) -> Turn {
        Turn {
            speaker,
            text: text.into(),
            timestamp: Utc::now(),
            intent: None,
            concepts: vec![],
        }
    }

    fn ctx<'a>(child: &'a Turn, primer: &'a Turn, candidates: &'a [String]) -> ComprehensionContext<'a> {
        ComprehensionContext {
            child_turn: child,
            primer_turn: primer,
            recent_turns: &[],
            candidate_concepts: candidates,
        }
    }

    #[tokio::test]
    async fn default_returns_empty() {
        let c = StubComprehensionClassifier::new();
        let child = t(Speaker::Child, "hi");
        let primer = t(Speaker::Primer, "hello");
        let candidates: Vec<String> = vec![];
        let r = c.classify(ctx(&child, &primer, &candidates)).await.unwrap();
        assert!(r.assessments.is_empty());
    }

    #[tokio::test]
    async fn with_response_returns_configured() {
        let configured = ComprehensionResult {
            assessments: vec![ComprehensionAssessment {
                concept: "gravity".into(),
                depth: UnderstandingDepth::Recall,
                confidence: 0.7,
                evidence: None,
            }],
        };
        let c = StubComprehensionClassifier::with_response(configured.clone());
        let child = t(Speaker::Child, "?");
        let primer = t(Speaker::Primer, "?");
        let candidates: Vec<String> = vec!["gravity".into()];
        let r = c.classify(ctx(&child, &primer, &candidates)).await.unwrap();
        assert_eq!(r.assessments.len(), 1);
        assert_eq!(r.assessments[0].concept, "gravity");
    }

    #[tokio::test]
    async fn with_script_returns_in_order_then_falls_back() {
        let mk = |concept: &str| ComprehensionResult {
            assessments: vec![ComprehensionAssessment {
                concept: concept.into(),
                depth: UnderstandingDepth::Aware,
                confidence: 0.8,
                evidence: None,
            }],
        };
        let c = StubComprehensionClassifier::with_script(vec![mk("a"), mk("b")]);
        let child = t(Speaker::Child, "?");
        let primer = t(Speaker::Primer, "?");
        let candidates: Vec<String> = vec![];
        let r1 = c.classify(ctx(&child, &primer, &candidates)).await.unwrap();
        assert_eq!(r1.assessments[0].concept, "a");
        let r2 = c.classify(ctx(&child, &primer, &candidates)).await.unwrap();
        assert_eq!(r2.assessments[0].concept, "b");
        let r3 = c.classify(ctx(&child, &primer, &candidates)).await.unwrap();
        assert!(r3.assessments.is_empty()); // falls back
    }

    #[tokio::test]
    async fn identifier_is_stub() {
        let c = StubComprehensionClassifier::new();
        assert_eq!(c.identifier(), "stub");
    }
}
```

- [ ] **Step 2: Don't run tests yet** — `llm.rs` is still missing.

---

## Task 11: `primer-comprehension::llm`

**Files:**
- Create: `src/crates/primer-comprehension/src/llm.rs`

- [ ] **Step 1: Write `llm.rs` with prompt builder, parser, and full test coverage**

```rust
//! LLM-backed comprehension classifier. Wraps any `InferenceBackend`
//! (via `Arc<dyn ...>`) so the chat backend can be reused or a cheaper
//! comprehension-specific model can be swapped in independently.

use std::sync::Arc;

use async_trait::async_trait;
use tracing::warn;

use primer_core::comprehension::{
    ComprehensionAssessment, ComprehensionContext, ComprehensionResult,
};
use primer_core::conversation::Speaker;
use primer_core::error::Result;
use primer_core::inference::{InferenceBackend, Message, Prompt, Role};
use primer_core::learner::UnderstandingDepth;
use primer_core::llm_util::{extract_first_json_object, truncate_to_chars};

use crate::ComprehensionClassifier;
use crate::consts;
use crate::settings::ComprehensionSettings;

pub struct LlmComprehensionClassifier {
    backend: Arc<dyn InferenceBackend>,
    settings: ComprehensionSettings,
    identifier: String,
}

impl LlmComprehensionClassifier {
    /// Build a new LLM-backed comprehension classifier.
    ///
    /// The stable `identifier` is composed as `llm:{backend}:{model}`,
    /// matching the format used by `LlmConceptExtractor`.
    pub fn new(
        backend: Arc<dyn InferenceBackend>,
        model: String,
        settings: ComprehensionSettings,
    ) -> Self {
        let identifier = format!("llm:{}:{}", backend.name(), model);
        Self {
            backend,
            settings,
            identifier,
        }
    }
}

#[async_trait]
impl ComprehensionClassifier for LlmComprehensionClassifier {
    fn identifier(&self) -> &str {
        &self.identifier
    }

    async fn classify(&self, ctx: ComprehensionContext<'_>) -> Result<ComprehensionResult> {
        if ctx.candidate_concepts.is_empty() {
            return Ok(ComprehensionResult::empty());
        }

        let prompt = build_classification_prompt(&ctx);
        let params = primer_core::inference::GenerationParams {
            max_tokens: self.settings.generation_max_tokens,
            temperature: self.settings.generation_temperature,
            top_p: self.settings.generation_top_p,
            stop_sequences: vec![],
        };
        let raw = match self.backend.generate(&prompt, &params).await {
            Ok(r) => r,
            Err(e) => {
                warn!(classifier = %self.identifier, error = ?e, "comprehension backend call failed");
                return Ok(ComprehensionResult::empty());
            }
        };

        let truncated = truncate_to_chars(&raw, self.settings.max_output_chars);
        match parse_classification_output(
            truncated,
            ctx.candidate_concepts,
            self.settings.evidence_max_chars,
        ) {
            Ok(r) => Ok(r),
            Err(reason) => {
                let snippet: String = truncated
                    .chars()
                    .take(consts::LLM_DEBUG_SNIPPET_CHARS)
                    .collect();
                warn!(classifier = %self.identifier, raw = %snippet, %reason, "comprehension output unparseable");
                Ok(ComprehensionResult::empty())
            }
        }
    }
}

// ─── Prompt builder ───────────────────────────────────────────────────────────

fn build_classification_prompt(ctx: &ComprehensionContext) -> Prompt {
    let context_section = if ctx.recent_turns.is_empty() {
        "(no prior context)".to_string()
    } else {
        ctx.recent_turns
            .iter()
            .map(|t| format!("{}: {}", speaker_label(t.speaker), t.text))
            .collect::<Vec<_>>()
            .join("\n")
    };

    let candidates_section = ctx
        .candidate_concepts
        .iter()
        .map(|c| format!("- {c}"))
        .collect::<Vec<_>>()
        .join("\n");

    let depth_levels = UnderstandingDepth::ALL
        .iter()
        .map(|d| d.name())
        .collect::<Vec<_>>()
        .join(", ");

    let system = format!(
        "You assess a child's depth of understanding for specific concepts in a Socratic \
        learning conversation. Output ONLY valid JSON of the form: \
        {{\"assessments\": [{{\"concept\": \"<from candidates>\", \"depth\": \"<one of: {depth_levels}>\", \
        \"confidence\": 0.0-1.0, \"evidence\": \"<one short sentence>\"}}]}} — no other text. \
        \
        Only emit assessments for concepts where the CHILD's turn shows actual demonstration. \
        Concepts the Primer introduced but the child did not engage with should be OMITTED — not \
        assessed at the lowest level. A bare acknowledgement (\"yeah\", \"ok\", \"I see\") is not \
        evidence; omit those concepts too. \
        \
        Depth meanings: \
        - Aware: child shows the concept registered (named it back, asked about it). \
        - Recall: child stated a fact about it. \
        - Comprehension: child explained it in their own words. \
        - Application: child transferred it to a new context they hadn't seen. \
        - Analysis: child compared, contrasted, or reasoned about it. \
        \
        Use Unknown only if you have no evidence at all (you should normally omit instead)."
    );

    let user = format!(
        "Prior context (oldest first):\n{context_section}\n\nExchange to analyse:\n{}: {}\n{}: {}\n\nCandidate concepts to assess (only these — do not invent others):\n{candidates_section}\n\nAssess now.",
        speaker_label(ctx.child_turn.speaker),
        ctx.child_turn.text,
        speaker_label(ctx.primer_turn.speaker),
        ctx.primer_turn.text,
    );

    Prompt {
        system,
        messages: vec![Message {
            role: Role::User,
            content: user,
        }],
    }
}

fn speaker_label(s: Speaker) -> &'static str {
    match s {
        Speaker::Child => "Child",
        Speaker::Primer => "Primer",
    }
}

// ─── Output parser ─────────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
struct ClassifierOutput {
    #[serde(default)]
    assessments: Vec<RawAssessment>,
}

#[derive(serde::Deserialize)]
struct RawAssessment {
    concept: String,
    depth: String,
    confidence: f32,
    #[serde(default)]
    evidence: Option<String>,
}

fn parse_classification_output(
    raw: &str,
    candidates: &[String],
    evidence_max_chars: usize,
) -> std::result::Result<ComprehensionResult, String> {
    let json_str = extract_first_json_object(raw)
        .ok_or_else(|| "no JSON object found in output".to_string())?;
    let parsed: ClassifierOutput =
        serde_json::from_str(json_str).map_err(|e| format!("JSON parse error: {e}"))?;

    let mut out: Vec<ComprehensionAssessment> = Vec::with_capacity(parsed.assessments.len());
    for a in parsed.assessments {
        // Drop assessments whose concept name isn't in the candidate
        // list (defends against the LLM inventing concepts).
        if !candidates.iter().any(|c| c == &a.concept) {
            continue;
        }
        // Drop assessments with out-of-range confidence.
        if !(0.0..=1.0).contains(&a.confidence) {
            continue;
        }
        // Drop assessments with unknown depth name.
        let depth = match depth_from_name(&a.depth) {
            Some(d) => d,
            None => continue,
        };
        let evidence = a.evidence.map(|e| {
            let trimmed = truncate_to_chars(&e, evidence_max_chars);
            trimmed.to_string()
        });
        out.push(ComprehensionAssessment {
            concept: a.concept,
            depth,
            confidence: a.confidence,
            evidence,
        });
    }

    Ok(ComprehensionResult { assessments: out })
}

fn depth_from_name(name: &str) -> Option<UnderstandingDepth> {
    UnderstandingDepth::ALL
        .iter()
        .copied()
        .find(|d| d.name() == name)
}

// ─── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use futures::stream;
    use primer_core::conversation::{Speaker, Turn};
    use primer_core::inference::{GenerationParams, Prompt, TokenChunk, TokenStream};

    /// Test backend that returns canned text from `generate_stream` and stubs
    /// `name` / `is_available`.
    struct CannedBackend {
        name: &'static str,
        text: String,
    }

    #[async_trait]
    impl InferenceBackend for CannedBackend {
        fn name(&self) -> &str {
            self.name
        }
        async fn is_available(&self) -> bool {
            true
        }
        async fn generate_stream(
            &self,
            _prompt: &Prompt,
            _params: &GenerationParams,
        ) -> Result<TokenStream> {
            let chunk = TokenChunk {
                text: self.text.clone(),
                done: true,
            };
            Ok(Box::pin(stream::once(async move { Ok(chunk) })))
        }
    }

    fn turn(speaker: Speaker, text: &str) -> Turn {
        Turn {
            speaker,
            text: text.into(),
            timestamp: Utc::now(),
            intent: None,
            concepts: vec![],
        }
    }

    fn ctx<'a>(
        child: &'a Turn,
        primer: &'a Turn,
        candidates: &'a [String],
    ) -> ComprehensionContext<'a> {
        ComprehensionContext {
            child_turn: child,
            primer_turn: primer,
            recent_turns: &[],
            candidate_concepts: candidates,
        }
    }

    #[test]
    fn identifier_includes_backend_and_model() {
        let backend = Arc::new(CannedBackend { name: "ollama", text: "{}".into() })
            as Arc<dyn InferenceBackend>;
        let c = LlmComprehensionClassifier::new(
            backend,
            "haiku".into(),
            ComprehensionSettings::default(),
        );
        assert_eq!(c.identifier(), "llm:ollama:haiku");
    }

    #[tokio::test]
    async fn classify_parses_well_formed_json() {
        let backend = Arc::new(CannedBackend {
            name: "x",
            text: r#"{"assessments": [{"concept": "photosynthesis", "depth": "Comprehension", "confidence": 0.85, "evidence": "explained well"}]}"#.into(),
        }) as Arc<dyn InferenceBackend>;
        let c = LlmComprehensionClassifier::new(
            backend,
            "test-model".into(),
            ComprehensionSettings::default(),
        );
        let child = turn(Speaker::Child, "Plants make food from sunlight.");
        let primer = turn(Speaker::Primer, "What do you think they need?");
        let candidates: Vec<String> = vec!["photosynthesis".into()];
        let r = c.classify(ctx(&child, &primer, &candidates)).await.unwrap();
        assert_eq!(r.assessments.len(), 1);
        assert_eq!(r.assessments[0].concept, "photosynthesis");
        assert_eq!(r.assessments[0].depth, UnderstandingDepth::Comprehension);
        assert!((r.assessments[0].confidence - 0.85).abs() < 1e-6);
        assert_eq!(r.assessments[0].evidence.as_deref(), Some("explained well"));
    }

    #[tokio::test]
    async fn classify_handles_wrapper_text() {
        let backend = Arc::new(CannedBackend {
            name: "x",
            text: r#"sure: {"assessments": [{"concept": "gravity", "depth": "Aware", "confidence": 0.6}]} ok?"#.into(),
        }) as Arc<dyn InferenceBackend>;
        let c = LlmComprehensionClassifier::new(
            backend,
            "test".into(),
            ComprehensionSettings::default(),
        );
        let child = turn(Speaker::Child, "?");
        let primer = turn(Speaker::Primer, "?");
        let candidates: Vec<String> = vec!["gravity".into()];
        let r = c.classify(ctx(&child, &primer, &candidates)).await.unwrap();
        assert_eq!(r.assessments.len(), 1);
        assert_eq!(r.assessments[0].depth, UnderstandingDepth::Aware);
    }

    #[tokio::test]
    async fn classify_skips_when_candidates_empty() {
        // No backend call should be made; canned text is irrelevant.
        let backend = Arc::new(CannedBackend { name: "x", text: "{}".into() })
            as Arc<dyn InferenceBackend>;
        let c = LlmComprehensionClassifier::new(
            backend,
            "test".into(),
            ComprehensionSettings::default(),
        );
        let child = turn(Speaker::Child, "?");
        let primer = turn(Speaker::Primer, "?");
        let candidates: Vec<String> = vec![];
        let r = c.classify(ctx(&child, &primer, &candidates)).await.unwrap();
        assert!(r.assessments.is_empty());
    }

    #[tokio::test]
    async fn classify_drops_assessment_with_concept_not_in_candidates() {
        let backend = Arc::new(CannedBackend {
            name: "x",
            text: r#"{"assessments": [{"concept": "invented", "depth": "Aware", "confidence": 0.9}]}"#.into(),
        }) as Arc<dyn InferenceBackend>;
        let c = LlmComprehensionClassifier::new(
            backend,
            "test".into(),
            ComprehensionSettings::default(),
        );
        let child = turn(Speaker::Child, "?");
        let primer = turn(Speaker::Primer, "?");
        let candidates: Vec<String> = vec!["other".into()];
        let r = c.classify(ctx(&child, &primer, &candidates)).await.unwrap();
        assert!(r.assessments.is_empty());
    }

    #[tokio::test]
    async fn classify_drops_assessment_with_unknown_depth_name() {
        let backend = Arc::new(CannedBackend {
            name: "x",
            text: r#"{"assessments": [{"concept": "x", "depth": "MasteryGod", "confidence": 0.9}]}"#.into(),
        }) as Arc<dyn InferenceBackend>;
        let c = LlmComprehensionClassifier::new(
            backend,
            "test".into(),
            ComprehensionSettings::default(),
        );
        let child = turn(Speaker::Child, "?");
        let primer = turn(Speaker::Primer, "?");
        let candidates: Vec<String> = vec!["x".into()];
        let r = c.classify(ctx(&child, &primer, &candidates)).await.unwrap();
        assert!(r.assessments.is_empty());
    }

    #[tokio::test]
    async fn classify_drops_assessment_with_out_of_range_confidence() {
        let backend = Arc::new(CannedBackend {
            name: "x",
            text: r#"{"assessments": [{"concept": "x", "depth": "Aware", "confidence": 1.5}]}"#.into(),
        }) as Arc<dyn InferenceBackend>;
        let c = LlmComprehensionClassifier::new(
            backend,
            "test".into(),
            ComprehensionSettings::default(),
        );
        let child = turn(Speaker::Child, "?");
        let primer = turn(Speaker::Primer, "?");
        let candidates: Vec<String> = vec!["x".into()];
        let r = c.classify(ctx(&child, &primer, &candidates)).await.unwrap();
        assert!(r.assessments.is_empty());
    }

    #[tokio::test]
    async fn classify_truncates_evidence_to_cap() {
        let long_evidence = "z".repeat(500);
        let payload = serde_json::json!({
            "assessments": [{"concept": "x", "depth": "Aware", "confidence": 0.5, "evidence": long_evidence}]
        });
        let backend = Arc::new(CannedBackend { name: "x", text: payload.to_string() })
            as Arc<dyn InferenceBackend>;
        let settings = ComprehensionSettings {
            evidence_max_chars: 50,
            ..ComprehensionSettings::default()
        };
        let c = LlmComprehensionClassifier::new(backend, "test".into(), settings);
        let child = turn(Speaker::Child, "?");
        let primer = turn(Speaker::Primer, "?");
        let candidates: Vec<String> = vec!["x".into()];
        let r = c.classify(ctx(&child, &primer, &candidates)).await.unwrap();
        assert_eq!(r.assessments.len(), 1);
        let ev = r.assessments[0].evidence.as_deref().unwrap();
        assert_eq!(ev.chars().count(), 50);
    }

    #[tokio::test]
    async fn classify_returns_empty_on_unparseable_output() {
        let backend = Arc::new(CannedBackend { name: "x", text: "not json".into() })
            as Arc<dyn InferenceBackend>;
        let c = LlmComprehensionClassifier::new(
            backend,
            "test".into(),
            ComprehensionSettings::default(),
        );
        let child = turn(Speaker::Child, "?");
        let primer = turn(Speaker::Primer, "?");
        let candidates: Vec<String> = vec!["x".into()];
        let r = c.classify(ctx(&child, &primer, &candidates)).await.unwrap();
        assert!(r.assessments.is_empty());
    }

    #[tokio::test]
    async fn classify_returns_empty_on_backend_error() {
        struct ErrorBackend;
        #[async_trait]
        impl InferenceBackend for ErrorBackend {
            fn name(&self) -> &str { "error" }
            async fn is_available(&self) -> bool { true }
            async fn generate_stream(
                &self,
                _prompt: &Prompt,
                _params: &GenerationParams,
            ) -> Result<TokenStream> {
                Err(primer_core::error::PrimerError::Inference("simulated".into()))
            }
        }
        let backend = Arc::new(ErrorBackend) as Arc<dyn InferenceBackend>;
        let c = LlmComprehensionClassifier::new(
            backend,
            "test".into(),
            ComprehensionSettings::default(),
        );
        let child = turn(Speaker::Child, "?");
        let primer = turn(Speaker::Primer, "?");
        let candidates: Vec<String> = vec!["x".into()];
        let r = c.classify(ctx(&child, &primer, &candidates)).await.unwrap();
        assert!(r.assessments.is_empty());
    }
}
```

- [ ] **Step 2: Run the comprehension crate tests**

```bash
cd src && ~/.cargo/bin/cargo test -p primer-comprehension 2>&1 | tail -10
```

Expected: all tests pass. Settings (1 test) + stub (4 tests) + llm (10 tests) = 15 tests.

- [ ] **Step 3: Lint, format, full test suite**

```bash
cd src && ~/.cargo/bin/cargo clippy --workspace --all-targets 2>&1 | tail -10 && ~/.cargo/bin/cargo fmt --all -- --check && ~/.cargo/bin/cargo test --workspace 2>&1 | tail -5
```

Expected: 100% clean.

- [ ] **Step 4: Commit (covers Tasks 8-11 — the whole crate as one viable unit)**

```bash
cd /Users/hherb/src/primer && git add src/Cargo.toml src/crates/primer-comprehension
git commit -m "$(cat <<'EOF'
feat(comprehension): new primer-comprehension crate

Sibling to primer-classifier and primer-extractor:
- ComprehensionClassifier trait with identifier() + classify()
- LlmComprehensionClassifier (Arc<dyn InferenceBackend>)
- StubComprehensionClassifier (with_response/with_script)
- ComprehensionSettings (all numerics tunable, backed by consts)

Soft-fail policy throughout. Output parser drops assessments whose
concept is not in the candidate list, whose depth is unknown, or
whose confidence is out of [0.0, 1.0]. Evidence truncated
char-boundary-safe to evidence_max_chars.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 12: Extend `DialogueManagerSubsystems` and add `apply_comprehension`

**Files:**
- Modify: `src/crates/primer-pedagogy/Cargo.toml`
- Modify: `src/crates/primer-pedagogy/src/dialogue_manager.rs`

- [ ] **Step 1: Add `primer-comprehension` dependency to `primer-pedagogy`**

Modify `src/crates/primer-pedagogy/Cargo.toml`. In `[dependencies]`, alongside the existing `primer-classifier.workspace = true` and `primer-extractor.workspace = true`, add:

```toml
primer-comprehension.workspace = true
```

- [ ] **Step 2: Write the failing test for `apply_comprehension`**

In `src/crates/primer-pedagogy/src/dialogue_manager.rs`, find the existing `mod tests` block. Add a new test:

```rust
    #[test]
    fn apply_comprehension_promotes_depth_via_monotonic_max() {
        use primer_comprehension::ComprehensionSettings;
        use primer_core::comprehension::{ComprehensionAssessment, ComprehensionResult};
        use primer_core::learner::{ConceptState, UnderstandingDepth};

        let mut learner = test_learner();
        learner.concepts.push(ConceptState {
            concept_id: "gravity".into(),
            depth: UnderstandingDepth::Aware,
            confidence: 0.5,
            encounter_count: 1,
            last_encountered: None,
            notes: vec![],
        });

        let result = ComprehensionResult {
            assessments: vec![ComprehensionAssessment {
                concept: "gravity".into(),
                depth: UnderstandingDepth::Comprehension,
                confidence: 0.85,
                evidence: None,
            }],
        };
        let settings = ComprehensionSettings::default();
        let changed = apply_comprehension(&mut learner, &result, &settings);
        assert!(changed);
        assert_eq!(
            learner.concepts.iter().find(|c| c.concept_id == "gravity").unwrap().depth,
            UnderstandingDepth::Comprehension,
        );
    }

    #[test]
    fn apply_comprehension_does_not_demote() {
        use primer_comprehension::ComprehensionSettings;
        use primer_core::comprehension::{ComprehensionAssessment, ComprehensionResult};
        use primer_core::learner::{ConceptState, UnderstandingDepth};

        let mut learner = test_learner();
        learner.concepts.push(ConceptState {
            concept_id: "gravity".into(),
            depth: UnderstandingDepth::Comprehension,
            confidence: 0.85,
            encounter_count: 5,
            last_encountered: None,
            notes: vec![],
        });
        let result = ComprehensionResult {
            assessments: vec![ComprehensionAssessment {
                concept: "gravity".into(),
                depth: UnderstandingDepth::Aware,
                confidence: 0.95,
                evidence: None,
            }],
        };
        let settings = ComprehensionSettings::default();
        let changed = apply_comprehension(&mut learner, &result, &settings);
        assert!(!changed);
        assert_eq!(
            learner.concepts.iter().find(|c| c.concept_id == "gravity").unwrap().depth,
            UnderstandingDepth::Comprehension,
        );
    }

    #[test]
    fn apply_comprehension_skips_below_confidence_threshold() {
        use primer_comprehension::ComprehensionSettings;
        use primer_core::comprehension::{ComprehensionAssessment, ComprehensionResult};
        use primer_core::learner::{ConceptState, UnderstandingDepth};

        let mut learner = test_learner();
        learner.concepts.push(ConceptState {
            concept_id: "gravity".into(),
            depth: UnderstandingDepth::Aware,
            confidence: 0.5,
            encounter_count: 1,
            last_encountered: None,
            notes: vec![],
        });
        let result = ComprehensionResult {
            assessments: vec![ComprehensionAssessment {
                concept: "gravity".into(),
                depth: UnderstandingDepth::Comprehension,
                confidence: 0.3, // below default threshold of 0.6
                evidence: None,
            }],
        };
        let settings = ComprehensionSettings::default();
        let changed = apply_comprehension(&mut learner, &result, &settings);
        assert!(!changed);
        assert_eq!(
            learner.concepts.iter().find(|c| c.concept_id == "gravity").unwrap().depth,
            UnderstandingDepth::Aware,
        );
    }

    #[test]
    fn apply_comprehension_skips_concept_not_in_learner_model() {
        use primer_comprehension::ComprehensionSettings;
        use primer_core::comprehension::{ComprehensionAssessment, ComprehensionResult};
        use primer_core::learner::UnderstandingDepth;

        let mut learner = test_learner(); // empty concepts
        let result = ComprehensionResult {
            assessments: vec![ComprehensionAssessment {
                concept: "missing".into(),
                depth: UnderstandingDepth::Comprehension,
                confidence: 0.9,
                evidence: None,
            }],
        };
        let settings = ComprehensionSettings::default();
        let changed = apply_comprehension(&mut learner, &result, &settings);
        assert!(!changed);
        // No insertion — apply_extraction is the only insertion path.
        assert!(learner.concepts.iter().find(|c| c.concept_id == "missing").is_none());
    }
```

The helper `test_learner()` likely already exists in this test module (used by the existing `apply_extraction_*` tests); locate and reuse it.

- [ ] **Step 3: Run the tests to verify they fail**

```bash
cd src && ~/.cargo/bin/cargo test -p primer-pedagogy apply_comprehension 2>&1 | tail -10
```

Expected: failure with `cannot find function apply_comprehension`.

- [ ] **Step 4: Implement `apply_comprehension`**

In `src/crates/primer-pedagogy/src/dialogue_manager.rs`, near the existing `apply_extraction` function (search for `pub(crate) fn apply_extraction`), append a sibling function:

```rust
/// Apply a `ComprehensionResult` to the in-memory `LearnerModel`.
///
/// For each assessment whose `confidence >= settings.confidence_threshold`,
/// promote `learner.concepts[concept].depth` via monotonic max
/// (never demote — that's an explicit forgetting event handled
/// algorithmically over `turn_comprehensions`, not here).
///
/// Sub-threshold assessments are still persisted to disk by the
/// caller (full longitudinal record) but don't update in-memory state.
///
/// Concepts not already in `learner.concepts` are skipped — insertion
/// is the responsibility of `apply_extraction` (which always runs
/// before this function in the await sequence). The corner case of
/// an assessment for an unknown concept is documented in the spec
/// (Task 12 of the plan-of-record): the parser layer drops them, so
/// this branch is defensive only.
///
/// Returns `true` if any depth or confidence was updated; the caller
/// uses this to set `learner_dirty` so the per-turn save flushes.
pub(crate) fn apply_comprehension(
    learner: &mut primer_core::learner::LearnerModel,
    result: &primer_core::comprehension::ComprehensionResult,
    settings: &primer_comprehension::ComprehensionSettings,
) -> bool {
    let mut changed = false;
    for a in &result.assessments {
        if a.confidence < settings.confidence_threshold {
            continue;
        }
        if let Some(c) = learner
            .concepts
            .iter_mut()
            .find(|c| c.concept_id == a.concept)
        {
            if a.depth > c.depth {
                c.depth = a.depth;
                c.confidence = a.confidence;
                changed = true;
            }
        }
    }
    changed
}
```

- [ ] **Step 5: Extend `DialogueManagerSubsystems`**

Modify the existing `DialogueManagerSubsystems` struct (around `dialogue_manager.rs:68`):

```rust
pub struct DialogueManagerSubsystems {
    pub classifier: Arc<dyn EngagementClassifier>,
    pub classifier_settings: ClassifierSettings,
    pub extractor: Arc<dyn ConceptExtractor>,
    pub extractor_settings: ExtractorSettings,
    pub comprehension: Arc<dyn primer_comprehension::ComprehensionClassifier>,
    pub comprehension_settings: primer_comprehension::ComprehensionSettings,
}
```

- [ ] **Step 6: Run the tests to verify they pass**

```bash
cd src && ~/.cargo/bin/cargo test -p primer-pedagogy apply_comprehension 2>&1 | tail -10
```

Expected: 4 new tests pass. Other pre-existing pedagogy tests will fail to compile because `DialogueManagerSubsystems` now needs two more fields — Tasks 13-14 fix the call sites and helpers.

- [ ] **Step 7: Don't commit yet** — Tasks 13-15 land the rest of the wiring as one viable unit.

---

## Task 13: Update test helpers and `default_subsystems()` to include comprehension

**Files:**
- Modify: `src/crates/primer-pedagogy/src/dialogue_manager.rs` (test helpers)

- [ ] **Step 1: Find existing test helpers**

Search the test module for `default_subsystems` and `subsystems_with_extractor`:

```bash
grep -n "default_subsystems\|stub_extractor\|stub_classifier\|fn subsystems" src/crates/primer-pedagogy/src/dialogue_manager.rs
```

- [ ] **Step 2: Add a `stub_comprehension()` helper**

Alongside `stub_classifier()` and `stub_extractor()`, append:

```rust
    fn stub_comprehension() -> Arc<dyn primer_comprehension::ComprehensionClassifier> {
        Arc::new(primer_comprehension::StubComprehensionClassifier::new())
    }
```

- [ ] **Step 3: Update `default_subsystems()` to include comprehension**

Modify the existing `default_subsystems()` function:

```rust
    fn default_subsystems() -> DialogueManagerSubsystems {
        DialogueManagerSubsystems {
            classifier: stub_classifier(),
            classifier_settings: ClassifierSettings::default(),
            extractor: stub_extractor(),
            extractor_settings: ExtractorSettings::default(),
            comprehension: stub_comprehension(),
            comprehension_settings: primer_comprehension::ComprehensionSettings::default(),
        }
    }
```

- [ ] **Step 4: Update `subsystems_with_extractor()` similarly**

```rust
    fn subsystems_with_extractor(
        extractor: Arc<dyn ConceptExtractor>,
    ) -> DialogueManagerSubsystems {
        DialogueManagerSubsystems {
            classifier: stub_classifier(),
            classifier_settings: ClassifierSettings::default(),
            extractor,
            extractor_settings: ExtractorSettings::default(),
            comprehension: stub_comprehension(),
            comprehension_settings: primer_comprehension::ComprehensionSettings::default(),
        }
    }
```

- [ ] **Step 5: Add a `subsystems_with_comprehension()` helper** (a sibling we'll need in Task 14):

```rust
    fn subsystems_with_comprehension(
        comprehension: Arc<dyn primer_comprehension::ComprehensionClassifier>,
    ) -> DialogueManagerSubsystems {
        DialogueManagerSubsystems {
            classifier: stub_classifier(),
            classifier_settings: ClassifierSettings::default(),
            extractor: stub_extractor(),
            extractor_settings: ExtractorSettings::default(),
            comprehension,
            comprehension_settings: primer_comprehension::ComprehensionSettings::default(),
        }
    }
```

- [ ] **Step 6: Run the existing pedagogy tests**

```bash
cd src && ~/.cargo/bin/cargo test -p primer-pedagogy 2>&1 | tail -10
```

Expected: all pre-existing pedagogy tests pass + the 4 `apply_comprehension_*` tests from Task 12 pass. Some tests may still need their construction sites adjusted — fix any compile errors that surface (likely zero if the helpers were the bottleneck).

- [ ] **Step 7: Don't commit yet** — Tasks 14-15 add the runtime wiring.

---

## Task 14: Chain comprehension into the post-response spawned task

**Files:**
- Modify: `src/crates/primer-pedagogy/src/dialogue_manager.rs`

This is the biggest single change in the plan: rename `extract_task` → `post_response_task`, widen its result, chain comprehension inside the existing spawn body, and expose new `last_comprehension` / `comprehension_identifier` accessors.

- [ ] **Step 1: Add new fields to `DialogueManager`**

Find the `DialogueManager` struct (around `dialogue_manager.rs:85`). Modify the existing fields:

- Rename `extract_task: Option<JoinHandle<Option<ExtractionResult>>>` to `post_response_task: Option<JoinHandle<Option<PostResponseResult>>>`.
- Add new fields below it:
  ```rust
      comprehension: Arc<dyn primer_comprehension::ComprehensionClassifier>,
      comprehension_settings: primer_comprehension::ComprehensionSettings,
      last_comprehension: Option<primer_core::comprehension::ComprehensionResult>,
  ```

- [ ] **Step 2: Replace `ExtractionResult` with `PostResponseResult`**

Find the existing `struct ExtractionResult` definition (around `dialogue_manager.rs:144`). Replace it with:

```rust
/// Output of the spawned post-response task: the extracted concepts
/// (and their turn indices for syncing back into in-memory
/// `Session.turns`) plus the comprehension assessments. Returned
/// through the `JoinHandle` so `await_pending_post_response` can
/// apply both to in-memory state at the next-turn boundary.
struct PostResponseResult {
    extraction: ExtractionPart,
    comprehension: primer_core::comprehension::ComprehensionResult,
}

/// The extraction portion of the post-response result.
struct ExtractionPart {
    child_turn_index: usize,
    primer_turn_index: usize,
    extraction: ConceptExtraction,
}
```

(Keep the original `child_turn_index` / `primer_turn_index` field names so the existing `await_pending_extraction` body's access paths still work — the rename is the only structural change.)

- [ ] **Step 3: Update `DialogueManager::new()` to populate the new fields**

Find the `pub fn new(...)` constructor around `dialogue_manager.rs:258`. Modify the body so the `Self { ... }` block:

- replaces `extract_task: None,` with `post_response_task: None,`
- adds the three new fields:
  ```rust
              comprehension: subsystems.comprehension,
              comprehension_settings: subsystems.comprehension_settings,
              last_comprehension: None,
  ```

- [ ] **Step 4: Modify the spawn body to chain comprehension**

Find the spawn body in `respond_to_streaming` (around `dialogue_manager.rs:603-638`). The current code spawns the extractor task and stores the JoinHandle in `self.extract_task`. Replace the spawn body with one that chains comprehension after extraction.

Replace the `let task = tokio::spawn(async move { ... }); self.extract_task = Some(task);` block with:

```rust
                let extractor = Arc::clone(&self.extractor);
                let comprehension = Arc::clone(&self.comprehension);
                let comp_settings = self.comprehension_settings.clone();
                let store = self.storage.clone();
                let session_id = self.session.id;
                let comp_classifier_id = comprehension.identifier().to_string();

                let task = tokio::spawn(async move {
                    // ── Step 1: Extract concepts ──
                    let extraction_ctx = primer_core::extractor::ExtractionContext {
                        child_turn: &child_turn,
                        primer_turn: &primer_turn,
                        recent_turns: &recent_turns,
                    };
                    let extraction = match extractor.extract(extraction_ctx).await {
                        Ok(e) => e,
                        Err(e) => {
                            tracing::warn!(error = ?e, "extractor returned error");
                            return None;
                        }
                    };

                    // ── Step 2: Persist concepts ──
                    if let Some(ref store) = store {
                        if let Err(e) = store
                            .update_exchange_concepts(
                                session_id,
                                child_idx,
                                &extraction.child_concepts,
                                primer_idx,
                                &extraction.primer_concepts,
                            )
                            .await
                        {
                            tracing::warn!(error = ?e, "update_exchange_concepts failed");
                        }
                    }

                    // ── Step 3: Build candidate concepts (child ∪ primer, dedup, capped) ──
                    let mut candidates: Vec<String> = Vec::with_capacity(
                        extraction.child_concepts.len() + extraction.primer_concepts.len(),
                    );
                    let mut seen = std::collections::HashSet::new();
                    for c in extraction
                        .child_concepts
                        .iter()
                        .chain(extraction.primer_concepts.iter())
                    {
                        if seen.insert(c.clone()) {
                            candidates.push(c.clone());
                            if candidates.len() >= comp_settings.max_concepts_per_call {
                                break;
                            }
                        }
                    }

                    // ── Step 4: Run comprehension ──
                    let comp_result = if candidates.is_empty() {
                        primer_core::comprehension::ComprehensionResult::empty()
                    } else {
                        let comp_ctx = primer_core::comprehension::ComprehensionContext {
                            child_turn: &child_turn,
                            primer_turn: &primer_turn,
                            recent_turns: &recent_turns,
                            candidate_concepts: &candidates,
                        };
                        match comprehension.classify(comp_ctx).await {
                            Ok(r) => r,
                            Err(e) => {
                                tracing::warn!(error = ?e, "comprehension returned error");
                                primer_core::comprehension::ComprehensionResult::empty()
                            }
                        }
                    };

                    // ── Step 5: Persist comprehensions ──
                    if !comp_result.assessments.is_empty() {
                        if let Some(ref store) = store {
                            if let Err(e) = store
                                .save_comprehensions(
                                    session_id,
                                    primer_idx,
                                    &comp_result.assessments,
                                    &comp_classifier_id,
                                )
                                .await
                            {
                                tracing::warn!(error = ?e, "save_comprehensions failed");
                            }
                        }
                    }

                    Some(PostResponseResult {
                        extraction: ExtractionPart {
                            child_turn_index: child_idx,
                            primer_turn_index: primer_idx,
                            extraction,
                        },
                        comprehension: comp_result,
                    })
                });
                self.post_response_task = Some(task);
```

(Keep the surrounding `if total_turns >= 2 && ...` guard exactly as it was; only the inner spawn-and-handle code changes.)

- [ ] **Step 5: Update accessors**

Find `pub fn last_extraction(&self)` and update its return — it should still return `Option<&ConceptExtraction>` because the field semantics haven't changed. The internal field is still `last_extraction`. (Verify by reading the existing accessor; no change should be needed if the field stayed.)

Add new accessors after `last_extraction`:

```rust
    /// Most recent comprehension result applied to the learner (used by `--verbose`).
    /// Cleared on session lifecycle events. Returns `None` until the first
    /// completed exchange whose comprehension has been awaited.
    pub fn last_comprehension(&self) -> Option<&primer_core::comprehension::ComprehensionResult> {
        self.last_comprehension.as_ref()
    }

    /// Stable identifier of the active comprehension classifier (used by `--verbose`).
    pub fn comprehension_identifier(&self) -> &str {
        self.comprehension.identifier()
    }
```

- [ ] **Step 6: Don't run tests yet** — Task 15 fixes `await_pending_extraction` and `close_session`.

---

## Task 15: Rename `await_pending_extraction` → `await_pending_post_response`; chain in `close_session`

**Files:**
- Modify: `src/crates/primer-pedagogy/src/dialogue_manager.rs`

- [ ] **Step 1: Update the call site at the start of `respond_to_streaming`**

Find the existing call (around `dialogue_manager.rs:399-404`):

```rust
        self.await_pending_classification().await;
        self.await_pending_extraction().await;
```

Change the second line to:

```rust
        self.await_pending_post_response().await;
```

- [ ] **Step 2: Rewrite the `await_pending_extraction` function as `await_pending_post_response`**

Find the existing `await_pending_extraction` function (around `dialogue_manager.rs:769`). Rename it and update the body to apply both extraction and comprehension. Replace the entire function:

```rust
    /// Wait (up to `extractor_settings.blocking_timeout +
    /// comprehension_settings.blocking_timeout`) for the post-response
    /// chained task (extraction → comprehension) to complete.
    ///
    /// On success: applies extraction to in-memory state via
    /// `apply_extraction` (inserts new concepts at `Aware`, increments
    /// encounter counts) THEN applies comprehension via
    /// `apply_comprehension` (monotonic-max promotion of depths, gated
    /// on `confidence_threshold`).
    ///
    /// On timeout: detaches the task (does NOT abort), so DB
    /// persistence still completes asynchronously. The next-turn intent
    /// decision proceeds with stale in-memory data — same behaviour as
    /// the old extractor await.
    async fn await_pending_post_response(&mut self) {
        let Some(task) = self.post_response_task.take() else {
            return;
        };
        let timeout =
            self.extractor_settings.blocking_timeout + self.comprehension_settings.blocking_timeout;

        match tokio::time::timeout(timeout, task).await {
            Ok(Ok(Some(result))) => {
                // Apply extraction first so any new concepts are in
                // learner.concepts before comprehension promotes their
                // depths.
                if apply_extraction(&mut self.learner, &result.extraction.extraction) {
                    self.learner_dirty = true;
                }
                merge_concepts_into_turn(
                    &mut self.session.turns,
                    result.extraction.child_turn_index,
                    &result.extraction.extraction.child_concepts,
                );
                merge_concepts_into_turn(
                    &mut self.session.turns,
                    result.extraction.primer_turn_index,
                    &result.extraction.extraction.primer_concepts,
                );
                self.last_extraction = Some(result.extraction.extraction);

                // Apply comprehension — promotes depths via monotonic
                // max for assessments meeting the confidence threshold.
                if apply_comprehension(
                    &mut self.learner,
                    &result.comprehension,
                    &self.comprehension_settings,
                ) {
                    self.learner_dirty = true;
                }
                self.last_comprehension = Some(result.comprehension);
            }
            Ok(Ok(None)) => {
                // Task completed but returned None (extractor errored).
                // No state to apply.
            }
            Ok(Err(e)) => tracing::warn!(error = ?e, "post-response task panicked"),
            Err(_) => {
                tracing::warn!(
                    timeout_ms = timeout.as_millis() as u64,
                    "post-response chain exceeded blocking timeout — proceeding with stale state"
                );
                // task is dropped here, but tokio::spawn'd futures
                // continue to run to completion in the background.
            }
        }
    }
```

- [ ] **Step 3: Update `close_session` to drain the renamed task**

Find `close_session` (around `dialogue_manager.rs:692`). The body currently drains `extract_task`; rename to `post_response_task`:

```bash
grep -n "extract_task\|post_response_task" src/crates/primer-pedagogy/src/dialogue_manager.rs
```

Wherever `extract_task` is referenced in `close_session`, change it to `post_response_task`. The drain pattern (`.take()` + `.await`) stays.

- [ ] **Step 4: Add a streaming-flow integration test**

In the test module, add tests that verify the chained behaviour end-to-end with stubs:

```rust
    #[tokio::test]
    async fn post_response_chain_persists_extraction_and_comprehension() {
        use primer_comprehension::StubComprehensionClassifier;
        use primer_core::comprehension::{ComprehensionAssessment, ComprehensionResult};
        use primer_core::extractor::ConceptExtraction;
        use primer_core::learner::UnderstandingDepth;

        // Set up a "concept-capturing" session-store mock that records
        // what was passed to update_exchange_concepts and
        // save_comprehensions. The plan reuses the existing ConceptCapturingStore
        // from primer-pedagogy's test fixtures (a parallel will exist
        // because the extractor PR added one).
        // <If no such mock exists, add one with two Mutex<Vec<...>>> fields
        // tracking the two methods.>

        let extractor = Arc::new(primer_extractor::StubConceptExtractor::with_response(
            ConceptExtraction {
                child_concepts: vec!["gravity".into()],
                primer_concepts: vec![],
            },
        )) as Arc<dyn ConceptExtractor>;

        let comprehension = Arc::new(StubComprehensionClassifier::with_response(
            ComprehensionResult {
                assessments: vec![ComprehensionAssessment {
                    concept: "gravity".into(),
                    depth: UnderstandingDepth::Recall,
                    confidence: 0.8,
                    evidence: Some("named the concept".into()),
                }],
            },
        )) as Arc<dyn primer_comprehension::ComprehensionClassifier>;

        // Drive a full respond_to_streaming → await_pending_post_response cycle
        // with these stubs and assert:
        //   - update_exchange_concepts was called once with the extracted concepts
        //   - save_comprehensions was called once with the comprehension assessment
        //   - learner.concepts contains "gravity" at depth Recall
        // <Detailed test body to mirror the existing extractor test that
        // exercises this same path. The exact mock-store wiring is project-
        // specific; reproduce the pattern from the existing extractor test.>
    }

    #[tokio::test]
    async fn post_response_chain_skips_comprehension_on_empty_extraction() {
        // Stub extractor returns empty → candidate_concepts is empty →
        // comprehension is not invoked.
        let extractor = Arc::new(primer_extractor::StubConceptExtractor::new())
            as Arc<dyn ConceptExtractor>;

        // A "spy" comprehension that fails the test if classify() is called.
        // Implement inline:
        struct PanicOnCall;
        #[async_trait::async_trait]
        impl primer_comprehension::ComprehensionClassifier for PanicOnCall {
            fn identifier(&self) -> &str { "panic" }
            async fn classify(
                &self,
                _ctx: primer_core::comprehension::ComprehensionContext<'_>,
            ) -> primer_core::error::Result<primer_core::comprehension::ComprehensionResult> {
                panic!("comprehension must not be called when extractor returned empty");
            }
        }
        let comprehension = Arc::new(PanicOnCall)
            as Arc<dyn primer_comprehension::ComprehensionClassifier>;

        // Drive a full respond_to_streaming → await_pending_post_response cycle
        // with these stubs. The spy will fail the test if comprehension was
        // called. <Detailed body parallel to the test above.>
    }
```

The existing test infrastructure for the extractor flow includes mocks like `ConceptCapturingStore` (see the brief). Mirror those exactly; the storage trait now has `save_comprehensions` so add a `comprehensions: Mutex<Vec<...>>` field to the existing capturing store rather than building a parallel one.

**If you cannot find an existing `ConceptCapturingStore`-style mock,** the test must be adjusted: reduce both tests to direct calls into a small helper that drives one turn through the dialogue manager with stub stores. The acceptance criterion is "both turn-flow tests run, and one fails fast (panic) if comprehension is invoked when it shouldn't be."

- [ ] **Step 5: Run the pedagogy tests**

```bash
cd src && ~/.cargo/bin/cargo test -p primer-pedagogy 2>&1 | tail -10
```

Expected: all existing pedagogy tests pass (they used `default_subsystems()` and `subsystems_with_extractor()` which now produce a 6-field struct via Task 13's helpers), the 4 `apply_comprehension_*` tests pass, and the 2 new chain tests pass.

- [ ] **Step 6: Workspace lint, format, full test suite**

```bash
cd src && ~/.cargo/bin/cargo clippy --workspace --all-targets 2>&1 | tail -10 && ~/.cargo/bin/cargo fmt --all -- --check && ~/.cargo/bin/cargo test --workspace 2>&1 | tail -5
```

Expected: 100% clean. Test count target so far: 275 baseline + 2 (Task 1) + 9 (Task 2) + 3 (Task 4) + 2 (Task 5) + 5 (Task 7) + 1 (Task 9 settings) + 4 (Task 10 stub) + 10 (Task 11 llm) + 4 (Task 12 apply_comprehension) + 2 (Task 15 chain) = 317 tests total.

- [ ] **Step 7: Commit Tasks 12–15 together**

```bash
cd /Users/hherb/src/primer && git add src/crates/primer-pedagogy
git commit -m "$(cat <<'EOF'
feat(pedagogy): chain comprehension after extraction in post-response task

DialogueManagerSubsystems gains comprehension + comprehension_settings.
The existing extractor task is widened to a chained
extractor → comprehension task; one JoinHandle covers both, with a
combined bounded-await timeout (default 1500 + 1500 = 3000 ms) at the
start of the next turn.

apply_comprehension promotes learner.concepts.depth via monotonic max,
gated on settings.confidence_threshold. Sub-threshold assessments are
persisted to disk but not applied to in-memory state.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 16: CLI flags + `build_comprehension`

**Files:**
- Modify: `src/crates/primer-cli/Cargo.toml`
- Modify: `src/crates/primer-cli/src/main.rs`

- [ ] **Step 1: Add the dependency**

In `src/crates/primer-cli/Cargo.toml`, alongside the existing `primer-classifier.workspace = true` and `primer-extractor.workspace = true`, add:

```toml
primer-comprehension.workspace = true
```

- [ ] **Step 2: Add the three new CLI flags to the `Cli` struct**

Modify `src/crates/primer-cli/src/main.rs`. Find the existing `extractor_*` flags (around lines 122-138) and append three new flags immediately after `extractor_timeout_ms`:

```rust
    /// Backend used for the comprehension classifier. Defaults to the same
    /// backend used for the chat (`--backend`); `stub` forces deterministic
    /// comprehension (empty assessments) regardless of main backend.
    #[arg(long)]
    comprehension_backend: Option<String>,

    /// Model used for the comprehension classifier. Defaults to the same
    /// model used for the chat (`--model`). Useful for haiku-as-comprehension
    /// + sonnet-as-chat configurations on bigger machines.
    #[arg(long)]
    comprehension_model: Option<String>,

    /// Maximum time to block awaiting the previous turn's
    /// extractor → comprehension chain before the next intent decision.
    /// Combined with `--extractor-timeout-ms` this caps the total
    /// background-work budget per turn. Defaults to
    /// `primer_comprehension::consts::DEFAULT_BLOCKING_TIMEOUT_MS`.
    #[arg(long, default_value_t = primer_comprehension::consts::DEFAULT_BLOCKING_TIMEOUT_MS)]
    comprehension_timeout_ms: u64,
```

- [ ] **Step 3: Update the `--verbose` flag's help text to mention comprehension**

Find the existing `verbose` flag (around line 144) and update its doc comment:

```rust
    /// Print pedagogical decisions (intent chosen, classifier output,
    /// extractor output, comprehension output) alongside the conversation,
    /// on stderr. Stdout stays clean.
    #[arg(long)]
    verbose: bool,
```

- [ ] **Step 4: Extend `BackendParams`**

Find the `BackendParams` struct (around line 266). Add two fields:

```rust
    pub(crate) comprehension_backend: Option<String>,
    pub(crate) comprehension_model: Option<String>,
```

(Match the field-visibility pattern used by the existing `extractor_backend` / `extractor_model` lines.)

- [ ] **Step 5: Wire BackendParams construction**

Find where `BackendParams { ... }` is built from `cli.*` (around line 660-668):

```bash
grep -n "extractor_backend: cli.extractor_backend" src/crates/primer-cli/src/main.rs
```

Add two corresponding lines:

```rust
        comprehension_backend: cli.comprehension_backend.clone(),
        comprehension_model: cli.comprehension_model.clone(),
```

- [ ] **Step 6: Add `build_comprehension` (verbatim parallel to `build_extractor`)**

After the existing `build_extractor` function (ends around line 453), append:

```rust
/// Construct the comprehension classifier according to the same dispatch matrix
/// as `build_extractor`:
///
/// | main backend | --comprehension-backend | --comprehension-model | outcome                                    |
/// |--------------|-------------------------|-----------------------|--------------------------------------------|
/// | stub         | (unset)                 | (any)                 | StubComprehensionClassifier                |
/// | *            | "stub"                  | (any)                 | StubComprehensionClassifier                |
/// | *            | some(X)                 | None                  | error (model required for non-stub)        |
/// | *            | some(X)                 | some(M)               | LlmComprehensionClassifier(new backend X, model M) |
/// | non-stub     | (unset)                 | None                  | LlmComprehensionClassifier(Arc::clone main) |
/// | non-stub     | (unset)                 | some(M)               | LlmComprehensionClassifier(new backend same type, model M) |
async fn build_comprehension(
    main_backend: Arc<dyn InferenceBackend>,
    main_backend_name: &str,
    main_model: &str,
    params: &BackendParams,
    settings: primer_comprehension::ComprehensionSettings,
) -> Result<Arc<dyn primer_comprehension::ComprehensionClassifier>> {
    if matches!(params.comprehension_backend.as_deref(), Some("stub"))
        && params.comprehension_model.is_some()
    {
        tracing::warn!(
            model = ?params.comprehension_model,
            "--comprehension-model ignored: --comprehension-backend is stub (deterministic, no model)"
        );
    }

    match (main_backend_name, params.comprehension_backend.as_deref()) {
        (_, Some("stub")) => Ok(Arc::new(primer_comprehension::StubComprehensionClassifier::new())),
        ("stub", None) => Ok(Arc::new(primer_comprehension::StubComprehensionClassifier::new())),
        (_, Some(comp_backend_name)) => {
            let model = params.comprehension_model.clone().ok_or_else(|| {
                PrimerError::Inference(
                    "--comprehension-model is required when --comprehension-backend is set to a non-stub backend".into(),
                )
            })?;
            let comp_backend = build_backend(comp_backend_name, model.clone(), params).await?;
            Ok(Arc::new(primer_comprehension::LlmComprehensionClassifier::new(
                comp_backend,
                model,
                settings,
            )))
        }
        (_, None) => match params.comprehension_model.as_deref() {
            None => Ok(Arc::new(primer_comprehension::LlmComprehensionClassifier::new(
                Arc::clone(&main_backend),
                main_model.to_string(),
                settings,
            ))),
            Some(override_model) => {
                let comp_backend =
                    build_backend(main_backend_name, override_model.to_string(), params).await?;
                Ok(Arc::new(primer_comprehension::LlmComprehensionClassifier::new(
                    comp_backend,
                    override_model.to_string(),
                    settings,
                )))
            }
        },
    }
}
```

- [ ] **Step 7: Wire the comprehension classifier into the launch path**

After the existing `let extractor: Arc<dyn ConceptExtractor> = match build_extractor(...) {...};` block (around lines 843-865), append:

```rust
    let comprehension_settings = primer_comprehension::ComprehensionSettings {
        blocking_timeout: std::time::Duration::from_millis(cli.comprehension_timeout_ms),
        ..primer_comprehension::ComprehensionSettings::default()
    };

    let comprehension: Arc<dyn primer_comprehension::ComprehensionClassifier> =
        match build_comprehension(
            Arc::clone(&backend),
            &cli.backend,
            &main_model,
            &backend_params,
            comprehension_settings.clone(),
        )
        .await
        {
            Ok(c) => {
                eprintln!("Comprehension classifier: {}", c.identifier());
                c
            }
            Err(e) => {
                eprintln!("Error constructing comprehension classifier: {e}");
                std::process::exit(1);
            }
        };
```

- [ ] **Step 8: Wire into `DialogueManagerSubsystems`**

Find the `DialogueManagerSubsystems { ... }` construction (around lines 873-878) and add the two new fields:

```rust
    let subsystems = primer_pedagogy::DialogueManagerSubsystems {
        classifier,
        classifier_settings,
        extractor,
        extractor_settings,
        comprehension,
        comprehension_settings,
    };
```

- [ ] **Step 9: Extend the `--verbose` block**

Find the verbose block (around lines 985-1013). After the existing `if let Some(e) = dm.last_extraction() { ... }` block, append:

```rust
            if let Some(c) = dm.last_comprehension() {
                if c.assessments.is_empty() {
                    eprintln!("[comprehension] (none) ({})", dm.comprehension_identifier());
                } else {
                    let pairs: Vec<String> = c
                        .assessments
                        .iter()
                        .map(|a| format!("{}={}({:.2})", a.concept, a.depth, a.confidence))
                        .collect();
                    eprintln!(
                        "[comprehension] {} ({})",
                        pairs.join(" "),
                        dm.comprehension_identifier()
                    );
                }
            }
```

- [ ] **Step 10: Build to verify no regressions**

```bash
cd src && ~/.cargo/bin/cargo build --workspace 2>&1 | tail -5
```

Expected: clean build. (Tests next task.)

- [ ] **Step 11: Don't commit yet** — Task 17 lands the construction tests.

---

## Task 17: `comprehension_construction_tests`

**Files:**
- Modify: `src/crates/primer-cli/src/main.rs`

- [ ] **Step 1: Update the existing `params()` helpers in `classifier_construction_tests` and `extractor_construction_tests`**

Find both test modules' `params(...)` helper. They currently build `BackendParams` with `classifier_backend: None, classifier_model: None` (or `extractor_*: ...`). Add two new lines to each helper to default the new fields:

```rust
            comprehension_backend: None,
            comprehension_model: None,
```

(Both helpers — search for `BackendParams {` inside both test modules.)

- [ ] **Step 2: Add `comprehension_construction_tests` module**

Append after `extractor_construction_tests` (around line 1276):

```rust
#[cfg(test)]
mod comprehension_construction_tests {
    use super::*;

    fn params(comprehension_backend: Option<&str>, comprehension_model: Option<&str>) -> BackendParams {
        BackendParams {
            api_key: Some("k".into()),
            ollama_url: "http://localhost:11434".into(),
            classifier_backend: None,
            classifier_model: None,
            extractor_backend: None,
            extractor_model: None,
            comprehension_backend: comprehension_backend.map(String::from),
            comprehension_model: comprehension_model.map(String::from),
        }
    }

    fn stub_main_backend() -> Arc<dyn InferenceBackend> {
        Arc::new(primer_inference::stub::StubBackend)
    }

    #[tokio::test]
    async fn stub_main_no_flags_gives_stub_comprehension() {
        let c = build_comprehension(
            stub_main_backend(),
            "stub",
            "stub",
            &params(None, None),
            primer_comprehension::ComprehensionSettings::default(),
        )
        .await
        .unwrap();
        assert_eq!(c.identifier(), "stub");
    }

    #[tokio::test]
    async fn explicit_stub_override_wins() {
        let c = build_comprehension(
            stub_main_backend(),
            "ollama",
            "llama3.2",
            &params(Some("stub"), None),
            primer_comprehension::ComprehensionSettings::default(),
        )
        .await
        .unwrap();
        assert_eq!(c.identifier(), "stub");
    }

    #[tokio::test]
    async fn nonstub_backend_without_model_errors() {
        let r = build_comprehension(
            stub_main_backend(),
            "ollama",
            "llama3.2",
            &params(Some("ollama"), None),
            primer_comprehension::ComprehensionSettings::default(),
        )
        .await;
        assert!(r.is_err());
    }

    #[tokio::test]
    async fn no_flags_reuses_main_model_id() {
        let c = build_comprehension(
            stub_main_backend(),
            "ollama",
            "llama3.2",
            &params(None, None),
            primer_comprehension::ComprehensionSettings::default(),
        )
        .await
        .unwrap();
        assert_eq!(c.identifier(), "llm:stub:llama3.2");
    }

    #[tokio::test]
    async fn model_only_override_uses_main_backend_type() {
        let c = build_comprehension(
            stub_main_backend(),
            "ollama",
            "llama3.2",
            &params(None, Some("haiku")),
            primer_comprehension::ComprehensionSettings::default(),
        )
        .await
        .unwrap();
        assert_eq!(c.identifier(), "llm:ollama:haiku");
    }
}
```

- [ ] **Step 3: Run the CLI tests**

```bash
cd src && ~/.cargo/bin/cargo test -p primer-cli 2>&1 | tail -10
```

Expected: existing tests still pass + 5 new comprehension tests pass.

- [ ] **Step 4: Workspace verification**

```bash
cd src && ~/.cargo/bin/cargo clippy --workspace --all-targets 2>&1 | tail -10 && ~/.cargo/bin/cargo fmt --all -- --check && ~/.cargo/bin/cargo test --workspace 2>&1 | tail -5
```

Expected: 100% clean. Test count target: 322 tests (317 from Task 15 + 5 from Task 17).

- [ ] **Step 5: Speech-feature build sanity-check**

```bash
cd src && ~/.cargo/bin/cargo build --workspace --features primer-cli/speech 2>&1 | tail -5
```

Expected: clean. (No speech-path code touched, so this should always pass; checking guards against an accidental import-path break.)

- [ ] **Step 6: Commit Tasks 16-17 together**

```bash
cd /Users/hherb/src/primer && git add src/crates/primer-cli
git commit -m "$(cat <<'EOF'
feat(cli): --comprehension-backend/-model/-timeout-ms + verbose line

build_comprehension dispatch matrix verbatim parallel to build_extractor.
--verbose now prints a [comprehension] line per turn alongside [intent],
[classifier], [extractor].

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 18: Documentation updates (CLAUDE.md, ROADMAP.md, README.md)

**Files:**
- Modify: `CLAUDE.md`
- Modify: `ROADMAP.md`
- Modify: `README.md`

- [ ] **Step 1: Update `CLAUDE.md`**

Find these specific sections in `CLAUDE.md` and update them:

1. **Dependency-graph diagram** (around line 44):

   Change:
   ```
   primer-cli  →  primer-pedagogy  →  primer-core  ←  primer-inference, primer-speech, primer-knowledge, primer-storage, primer-classifier, primer-extractor
   ```

   to:

   ```
   primer-cli  →  primer-pedagogy  →  primer-core  ←  primer-inference, primer-speech, primer-knowledge, primer-storage, primer-classifier, primer-comprehension, primer-extractor
   ```

2. **Add a `primer-comprehension` paragraph** after the `primer-extractor` paragraph. Use this verbatim:

   ```
   - **`primer-comprehension`** — comprehension-classifier trait + implementations. `ComprehensionClassifier` is the trait; `LlmComprehensionClassifier` wraps `Arc<dyn InferenceBackend>` (a separate dep so the crate stays free of `primer-inference` build dep); `StubComprehensionClassifier` powers tests with `with_response`/`with_script` constructors. One LLM call per completed exchange returns a JSON array of per-concept assessments `{concept, depth, confidence, evidence}` against the candidate set the extractor surfaced (deduped union of `child_concepts ∪ primer_concepts`, capped at `max_concepts_per_call`). Soft-fail policy throughout — comprehension errors return `ComprehensionResult::empty()` and `tracing::warn!`, never propagate. Configuration in `ComprehensionSettings` (blocking_timeout, recent_context_turns, max_output_chars, max_concepts_per_call, confidence_threshold, generation_max_tokens, generation_temperature, generation_top_p, evidence_max_chars) — every numeric backed by `consts.rs` defaults. Schema v5 in `primer-storage` persists every assessment to `turn_comprehensions` for offline depth-trajectory analysis without involving the LLM.
   ```

3. **Update the `primer-pedagogy` paragraph** — find the dialogue-manager paragraph (around line 67-70) and append at the end of the paragraph that ends with "...applied to in-memory `LearnerModel.concepts`":

   ```
   The post-response background path is now `(classifier task) || (extractor → comprehension chain)`: extraction runs first inside the same spawn; comprehension runs only if extraction surfaced any concepts. The await at the start of the next turn applies extraction first (so any new concepts are inserted at `Aware` in `LearnerModel.concepts`) and then comprehension (so `apply_comprehension` can promote depths via monotonic max). Combined timeout is `extractor_settings.blocking_timeout + comprehension_settings.blocking_timeout` (default 1500 + 1500 = 3000 ms); detach-on-timeout so DB persistence still completes.
   ```

4. **Update the existing "Concept extraction lags one turn for in-memory state" gotcha bullet** — append to the same gotcha bullet block:

   ```
   - **Comprehension assessments lag one turn just like concepts.** `apply_comprehension` runs in the same `await_pending_post_response` step as `apply_extraction`, so depth promotions for turn N's exchange are visible to the system prompt at turn N+1, not turn N. Same combined timeout applies.
   - **`apply_comprehension` is monotonic-max.** A weak exchange never demotes `concept.depth`; explicit forgetting is a Phase 0.3+ concern handled algorithmically over `turn_comprehensions`, not via another LLM call.
   - **Per-concept comprehension assessments are persisted even when sub-threshold.** `confidence_threshold` (default 0.6) only gates the in-memory apply step; the longitudinal `turn_comprehensions` record gets every assessment, with confidence scores intact for offline filtering.
   - **`extract_first_json_object` and `truncate_to_chars` live in `primer-core::llm_util`.** All three structured-output crates (`primer-classifier`, `primer-extractor`, `primer-comprehension`) import from there. Don't re-introduce per-crate copies.
   ```

5. **CLI flags paragraph** — find the long sentence enumerating `--classifier-backend`, `--extractor-backend`, etc. (around line 31). Append:

   ```
   `--comprehension-backend stub|cloud|ollama` (default: same as `--backend`), `--comprehension-model <name>` (default: same as `--model`), `--comprehension-timeout-ms <ms>` (default: 1500),
   ```

   immediately before the existing `--verbose` mention. Update the `--verbose` description in the same paragraph to add `[comprehension]` to the list of debug lines printed.

- [ ] **Step 2: Update `ROADMAP.md`**

Find the Phase 0.3 bullet for the LLM-based comprehension classifier (search for "LLM-based comprehension"). Tick it:

```bash
grep -n "comprehension" /Users/hherb/src/primer/ROADMAP.md
```

Change `- [ ] LLM-based comprehension classifier...` to `- [x] LLM-based comprehension classifier...`. Append a one-line landing note matching the style used for other ticked items.

- [ ] **Step 3: Update `README.md`**

Read the current state and assess:

```bash
cd /Users/hherb/src/primer && head -150 README.md
```

Update:
- The crate-list section (lines around 53-62) to mention `primer-comprehension/` alongside `primer-classifier/` and `primer-extractor/`.
- Any test-count number that still says 275 → bump to current (run `~/.cargo/bin/cargo test --workspace` from `src/` to get the live count).
- The development-stage description: third LLM-side analysis (engagement, concept extraction, comprehension) now landed; per-concept depth tracking with monotonic-max promotion now in place.

Don't add new sections beyond the spec's documentation list; this is a status bump.

- [ ] **Step 4: Commit**

```bash
cd /Users/hherb/src/primer && git add CLAUDE.md ROADMAP.md README.md
git commit -m "$(cat <<'EOF'
docs: comprehension classifier (Phase 0.3) — CLAUDE.md + ROADMAP + README

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 19: Final verification + smoke test

**Files:** none (verification only)

- [ ] **Step 1: Full verification gate**

Run all four required cargo invocations from `src/`:

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo build --workspace 2>&1 | tail -5
~/.cargo/bin/cargo test --workspace 2>&1 | tail -5
~/.cargo/bin/cargo clippy --workspace --all-targets 2>&1 | tail -10
~/.cargo/bin/cargo fmt --all -- --check 2>&1 | tail -5
~/.cargo/bin/cargo build --workspace --features primer-cli/speech 2>&1 | tail -5
```

Expected:
- Build: clean
- Tests: ~322 passed, 0 failed
- Clippy: 100% clean (no warnings on any target)
- Fmt: clean
- Speech build: clean

- [ ] **Step 2: Smoke test against a real LLM**

Either Anthropic (key in `ANTHROPIC_API_KEY` or `~/.primer_env`) or local ollama with the user's preferred model:

```bash
# Anthropic path
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo run --bin primer -- --backend cloud --name SmokeTester --age 9 --verbose 2>&1
```

OR

```bash
# Local ollama (note: this model may have high latency on the user's machine right now)
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo run --bin primer -- --backend ollama --model qwen3.6:35b-a3b-q8_0 --name SmokeTester --age 9 --verbose 2>&1
```

Drive a few exchanges where the child demonstrates mixed depths (some at recall, some at comprehension, some at bare acknowledgement). Verify:

- After the second exchange, `[comprehension]` line appears alongside `[intent]`, `[classifier]`, `[extractor]`.
- Format is `[comprehension] <concept>=<Depth>(<confidence>) ... (<classifier_identifier>)`.
- The depths are plausible (a "yeah ok" gets either no assessment or low confidence; a clear explanation gets `Comprehension` or higher).
- No backend errors propagate; if anything fails it shows up as a `tracing::warn!` and `[comprehension] (none)` continues.
- Quitting (`exit` / `bye` / `quit`) drains tasks cleanly with no lingering output.

- [ ] **Step 3: Inspect the on-disk record**

After the smoke run, the session DB should be at `~/.primer/smoketester.db`. Verify the new tables hold rows:

```bash
sqlite3 ~/.primer/smoketester.db "SELECT COUNT(*) FROM turn_comprehensions; SELECT identifier FROM comprehension_classifiers; SELECT pragma_user_version_value FROM (SELECT user_version AS pragma_user_version_value FROM pragma_user_version);"
```

Or a simpler equivalent:

```bash
sqlite3 ~/.primer/smoketester.db <<EOF
SELECT COUNT(*) AS comprehensions FROM turn_comprehensions;
SELECT identifier FROM comprehension_classifiers;
PRAGMA user_version;
EOF
```

Expected: at least one comprehension row (assuming the conversation produced a non-trivial exchange), the classifier identifier matches `[comprehension] ...` from the verbose run, `user_version = 5`.

If running ollama with significant latency the smoke may complete with zero comprehensions because the timeout fired — check the storage row count rather than the verbose output as the source of truth (the spawned task continues running after timeout and persists to disk independently).

- [ ] **Step 4: Optional — clean up the smoke DB**

```bash
rm ~/.primer/smoketester.db
```

- [ ] **Step 5: No commit needed for this task** — it's verification only.

---

## Self-review

(Plan-author self-review pass — fix issues inline rather than re-running.)

**Spec coverage:**

- ✅ New `primer-comprehension` crate with trait + LLM impl + stub + settings + consts → Tasks 8–11.
- ✅ New `primer-core::llm_util` with shared helpers → Task 2; classifier+extractor consolidate → Task 3.
- ✅ New `primer-core::comprehension` types → Task 4.
- ✅ `UnderstandingDepth::name()` + Display → Task 1.
- ✅ Schema v5 migration → Task 5; `save_comprehensions` → Tasks 6–7.
- ✅ DialogueManager wiring (chained spawn, await rename, accessors) → Tasks 12–15.
- ✅ CLI flags + build dispatch + verbose extension + construction tests → Tasks 16–17.
- ✅ Doc updates (CLAUDE.md, ROADMAP, README) → Task 18.
- ✅ Verification gates (5 cargo commands + smoke) → Task 19.
- ✅ Soft-fail policy preserved at every layer (LLM, await, persistence).
- ✅ Monotonic-max apply step matches the spec.
- ✅ Sub-threshold persistence-but-no-apply per spec.
- ✅ Identifier format `llm:{backend}:{model}` matches extractor precedent.

**Placeholder scan:** the integration tests in Task 15 use a placeholder phrase ("If you cannot find an existing `ConceptCapturingStore`-style mock") because the existing test-fixture file structure can vary — this is documented as conditional rather than left "TBD" and the engineer has a concrete fallback. Acceptable.

**Type consistency:** `apply_comprehension` signature in Task 12 (`learner: &mut LearnerModel, result: &ComprehensionResult, settings: &ComprehensionSettings`) matches the call site in Task 15 (`apply_comprehension(&mut self.learner, &result.comprehension, &self.comprehension_settings)`). `PostResponseResult` shape used in Task 14 spawn matches consumption in Task 15 await. `ComprehensionAssessment` field names (`concept`, `depth`, `confidence`, `evidence`) are consistent across `primer-core::comprehension`, the `LlmComprehensionClassifier` parser, the `apply_comprehension` body, and the `save_comprehensions` impl. CLI flag names (`comprehension_backend`, `comprehension_model`, `comprehension_timeout_ms`) are consistent across `Cli` struct, `BackendParams`, and the launch-path wiring.

**Scope check:** single-feature plan with clear task boundaries. Each task can be reverted independently except for the chained Tasks 12–15 (committed together because intermediate states have a half-renamed task field). That bundling is called out explicitly in the relevant task headers.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-05-03-comprehension-classifier.md`. Two execution options:

**1. Subagent-Driven (recommended)** — Dispatch a fresh subagent per task, review between tasks, fast iteration. The atomic-commit boundaries match the natural review checkpoints exactly (e.g. one review after Task 3 covers the helper consolidation; one after Task 7 covers the schema; one after Task 11 covers the new crate; etc).

**2. Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints for review.

Which approach?
