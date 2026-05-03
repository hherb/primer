# LLM-based Concept Extraction Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Populate `Turn.concepts` and `LearnerModel.concepts` per turn via an LLM-backed concept extractor, mirroring the existing engagement-classifier architecture exactly.

**Architecture:** New `primer-extractor` crate (mirrors `primer-classifier`) with `ConceptExtractor` trait, `LlmConceptExtractor` (wraps `Arc<dyn InferenceBackend>`), `StubConceptExtractor`, `ExtractorSettings`. One LLM call per completed exchange (child + primer concepts together) to halve inference cost and capture concepts the Primer introduced. Spawned async after the per-turn save block; the task self-persists `turn_concepts` for both turns via a new `SessionStore::update_turn_concepts` method, then returns `ConceptExtraction` which the dialogue manager awaits at the start of the next turn (with a bounded timeout) and applies to in-memory `LearnerModel.concepts`. Soft-fail policy throughout — extractor errors never propagate.

**Tech Stack:** Rust 1.85+ edition 2024, async-trait, tokio (sync + macros), serde_json, rusqlite (bundled SQLite), existing `primer-core` traits.

---

## Pre-task: branch setup

Work happens on a feature branch off `main`. Don't commit to `main` directly.

```bash
git checkout -b feature/concept-extraction-llm
```

All cargo commands run from `src/` (workspace root). Always invoke as `~/.cargo/bin/cargo` to bypass any Homebrew rust shadow on `PATH` (rust-toolchain.toml is honoured by rustup proxies only).

---

## File Structure

**New files:**
- `src/crates/primer-core/src/extractor.rs` — shared types (`ExtractionContext`, `ConceptExtraction`).
- `src/crates/primer-extractor/Cargo.toml` — crate manifest.
- `src/crates/primer-extractor/src/lib.rs` — `ConceptExtractor` trait + module exports.
- `src/crates/primer-extractor/src/consts.rs` — every numeric default.
- `src/crates/primer-extractor/src/settings.rs` — `ExtractorSettings`.
- `src/crates/primer-extractor/src/stub.rs` — `StubConceptExtractor`.
- `src/crates/primer-extractor/src/llm.rs` — `LlmConceptExtractor` + prompt + parser.

**Modified files:**
- `src/Cargo.toml` — add `primer-extractor` to workspace members + `[workspace.dependencies]`.
- `src/crates/primer-core/src/lib.rs` — `pub mod extractor;`.
- `src/crates/primer-core/src/storage.rs` — add `SessionStore::update_turn_concepts` trait method.
- `src/crates/primer-storage/src/lib.rs` — implement `update_turn_concepts` on `SqliteSessionStore`.
- `src/crates/primer-pedagogy/Cargo.toml` — add `primer-extractor` dep.
- `src/crates/primer-pedagogy/src/dialogue_manager.rs` — wire extractor field, spawn task, await pending extraction, drain on close, apply to `learner.concepts`. Update mock `CountingStore` impl with the new trait method.
- `src/crates/primer-cli/Cargo.toml` — add `primer-extractor` dep.
- `src/crates/primer-cli/src/main.rs` — add CLI flags, `BackendParams.extractor_*` fields, `build_extractor` helper, dispatch matrix tests, update `--verbose` print path, update `DialogueManager::new` call site.
- `CLAUDE.md` — document the new crate + spawn cadence in the architecture section.
- `ROADMAP.md` — tick the "concept extraction from child responses" bullet.

**Single responsibility per file.** `consts.rs` holds only numbers; `settings.rs` only the struct + Default; `stub.rs` and `llm.rs` are independent implementations of the trait. The new dialogue-manager additions co-locate next to the matching classifier code so future readers can see the parallel.

---

## Task 1: Add core types — `ExtractionContext` and `ConceptExtraction`

**Files:**
- Create: `src/crates/primer-core/src/extractor.rs`
- Modify: `src/crates/primer-core/src/lib.rs`

- [ ] **Step 1: Write the failing test for the soft-fail factory**

Add to a new file `src/crates/primer-core/src/extractor.rs`:

```rust
//! Extractor types — output and input for the concept extractor.
//!
//! `ConceptExtraction` is one extractor output (child + primer concepts
//! for one completed exchange). `ExtractionContext` is its input —
//! newtype-wrapped so future fields can be added without breaking the
//! trait signature.

use serde::{Deserialize, Serialize};

use crate::conversation::Turn;

/// One extractor output. Concepts the child surfaced and concepts the
/// Primer introduced, separated so each turn's `concepts` field can be
/// populated correctly.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConceptExtraction {
    pub child_concepts: Vec<String>,
    pub primer_concepts: Vec<String>,
}

impl ConceptExtraction {
    /// Empty extraction. Used as the soft-fail return on extractor
    /// errors (parse failure, network error, invalid output). Never
    /// shaped as `Err` — failures must not propagate.
    pub fn empty() -> Self {
        Self::default()
    }
}

/// Input to the extractor. Newtype'd so future fields can be added
/// without breaking the trait signature.
pub struct ExtractionContext<'a> {
    /// The child's most recent turn — exactly the turn whose concepts
    /// we want extracted into `child_concepts`.
    pub child_turn: &'a Turn,
    /// The Primer's response to that turn — concepts INTRODUCED by the
    /// Primer end up in `primer_concepts`.
    pub primer_turn: &'a Turn,
    /// A small slice of preceding turns for context (oldest-first).
    /// Helps the LLM disambiguate pronoun references across turns.
    pub recent_turns: &'a [Turn],
    // Reserved for extension. Adding fields here is non-breaking
    // because the trait method takes ExtractionContext by value.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_factory_returns_empty_vecs() {
        let e = ConceptExtraction::empty();
        assert!(e.child_concepts.is_empty());
        assert!(e.primer_concepts.is_empty());
    }
}
```

Then add the module to `src/crates/primer-core/src/lib.rs`:

```rust
pub mod extractor;
```

(Insert in alphabetical order — between `error` and `inference`.)

- [ ] **Step 2: Run test to verify it passes**

Run: `~/.cargo/bin/cargo test -p primer-core extractor::tests::empty_factory_returns_empty_vecs`
Expected: PASS — single test runs and passes.

- [ ] **Step 3: Commit**

```bash
git add src/crates/primer-core/src/extractor.rs src/crates/primer-core/src/lib.rs
git commit -m "feat(core): add ExtractionContext and ConceptExtraction types"
```

---

## Task 2: Add `SessionStore::update_turn_concepts` trait method

**Files:**
- Modify: `src/crates/primer-core/src/storage.rs`

- [ ] **Step 1: Add the trait method (compile-fail until impls follow)**

Add to `SessionStore` trait in `src/crates/primer-core/src/storage.rs`, immediately after `most_recent_session_learner_id`:

```rust
    /// Add concepts to a previously-persisted turn. Resolves
    /// `(session_id, turn_index)` → `turn_id` internally; lazily creates
    /// any new `concepts` rows; idempotently links via `turn_concepts`
    /// (`INSERT OR IGNORE` so calling this twice with overlapping
    /// concepts is a no-op).
    ///
    /// Used by the post-response concept-extractor task to backfill
    /// concepts onto a turn that `save_session` has already persisted.
    /// The append-only `save_session` skip-already-persisted invariant
    /// means concepts cannot be added in the normal save path once a
    /// turn is on disk; this method exists to fill that gap without
    /// breaking that invariant.
    ///
    /// Returns `Err` if `(session_id, turn_index)` does not resolve to
    /// an existing turn, or on genuine I/O failure. An empty `concepts`
    /// slice is a no-op (returns `Ok(())`).
    async fn update_turn_concepts(
        &self,
        session_id: SessionId,
        turn_index: usize,
        concepts: &[String],
    ) -> Result<()>;
```

- [ ] **Step 2: Run build to surface mock-impl gaps**

Run: `~/.cargo/bin/cargo build --workspace`
Expected: FAIL — compile error in `dialogue_manager.rs` test module:
```
error[E0046]: not all trait items implemented, missing: `update_turn_concepts`
```
Plus the same in `SqliteSessionStore`. We'll fix both in subsequent tasks; for now this confirms the trait change reached the impls.

- [ ] **Step 3: Add stub method to `CountingStore` test mock to keep tests compiling for downstream tasks**

In `src/crates/primer-pedagogy/src/dialogue_manager.rs`, inside `impl primer_core::storage::SessionStore for CountingStore`, add:

```rust
        async fn update_turn_concepts(
            &self,
            _session_id: primer_core::conversation::SessionId,
            _turn_index: usize,
            _concepts: &[String],
        ) -> Result<()> {
            Ok(())
        }
```

- [ ] **Step 4: Run build again**

Run: `~/.cargo/bin/cargo build --workspace`
Expected: still FAIL because `SqliteSessionStore` doesn't implement it yet. That's fine — Task 3 fixes it. We commit the trait + mock now.

- [ ] **Step 5: Commit**

```bash
git add src/crates/primer-core/src/storage.rs src/crates/primer-pedagogy/src/dialogue_manager.rs
git commit -m "feat(core): add SessionStore::update_turn_concepts trait method"
```

---

## Task 3: Implement `update_turn_concepts` on `SqliteSessionStore`

**Files:**
- Modify: `src/crates/primer-storage/src/lib.rs`
- Test: `src/crates/primer-storage/src/lib.rs` (existing `#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing tests**

Find the existing tests module in `src/crates/primer-storage/src/lib.rs` (search for `#[cfg(test)]`) and add:

```rust
    #[tokio::test]
    async fn update_turn_concepts_appends_to_persisted_turn() {
        let store = SqliteSessionStore::open(Path::new(":memory:")).unwrap();
        let learner_id = Uuid::new_v4();
        let mut session = primer_core::conversation::Session::new(learner_id);
        session.add_turn(primer_core::conversation::Turn {
            speaker: primer_core::conversation::Speaker::Child,
            text: "what is photosynthesis?".into(),
            timestamp: Utc::now(),
            intent: None,
            concepts: vec![],
        });
        store.save_session(&session).await.unwrap();

        store
            .update_turn_concepts(session.id, 0, &["photosynthesis".into(), "biology".into()])
            .await
            .unwrap();

        let loaded = store.load_session(session.id).await.unwrap().unwrap();
        let mut concepts = loaded.turns[0].concepts.clone();
        concepts.sort();
        assert_eq!(concepts, vec!["biology".to_string(), "photosynthesis".into()]);
    }

    #[tokio::test]
    async fn update_turn_concepts_is_idempotent() {
        let store = SqliteSessionStore::open(Path::new(":memory:")).unwrap();
        let learner_id = Uuid::new_v4();
        let mut session = primer_core::conversation::Session::new(learner_id);
        session.add_turn(primer_core::conversation::Turn {
            speaker: primer_core::conversation::Speaker::Child,
            text: "what is photosynthesis?".into(),
            timestamp: Utc::now(),
            intent: None,
            concepts: vec![],
        });
        store.save_session(&session).await.unwrap();

        // Run twice with overlapping concepts — should not duplicate rows.
        store
            .update_turn_concepts(session.id, 0, &["photosynthesis".into()])
            .await
            .unwrap();
        store
            .update_turn_concepts(session.id, 0, &["photosynthesis".into(), "biology".into()])
            .await
            .unwrap();

        let loaded = store.load_session(session.id).await.unwrap().unwrap();
        assert_eq!(loaded.turns[0].concepts.len(), 2);
    }

    #[tokio::test]
    async fn update_turn_concepts_empty_slice_is_noop() {
        let store = SqliteSessionStore::open(Path::new(":memory:")).unwrap();
        let learner_id = Uuid::new_v4();
        let mut session = primer_core::conversation::Session::new(learner_id);
        session.add_turn(primer_core::conversation::Turn {
            speaker: primer_core::conversation::Speaker::Child,
            text: "hi".into(),
            timestamp: Utc::now(),
            intent: None,
            concepts: vec![],
        });
        store.save_session(&session).await.unwrap();

        store
            .update_turn_concepts(session.id, 0, &[])
            .await
            .unwrap();

        let loaded = store.load_session(session.id).await.unwrap().unwrap();
        assert!(loaded.turns[0].concepts.is_empty());
    }

    #[tokio::test]
    async fn update_turn_concepts_errors_on_missing_turn() {
        let store = SqliteSessionStore::open(Path::new(":memory:")).unwrap();
        let unknown_session = Uuid::new_v4();
        let res = store
            .update_turn_concepts(unknown_session, 0, &["x".into()])
            .await;
        assert!(res.is_err(), "expected Err for unknown (session, turn_index)");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `~/.cargo/bin/cargo test -p primer-storage update_turn_concepts`
Expected: COMPILE FAIL — `SqliteSessionStore` does not implement the trait method yet (E0046 from Task 2).

- [ ] **Step 3: Implement `update_turn_concepts`**

In `src/crates/primer-storage/src/lib.rs`, add the method to the `impl primer_core::storage::SessionStore for SqliteSessionStore` block. Add it directly after `most_recent_session_learner_id`. Insert this code:

```rust
    async fn update_turn_concepts(
        &self,
        session_id: primer_core::conversation::SessionId,
        turn_index: usize,
        concepts: &[String],
    ) -> Result<()> {
        if concepts.is_empty() {
            return Ok(());
        }

        let mut conn = self.conn.lock().unwrap();
        let tx = conn
            .transaction()
            .map_err(|e| PrimerError::Storage(format!("update_turn_concepts begin tx: {e}")))?;

        // Resolve (session_id, turn_index) → turn_id.
        let turn_id: i64 = tx
            .query_row(
                "SELECT id FROM turns WHERE session_id = ?1 AND turn_index = ?2",
                rusqlite::params![session_id.to_string(), turn_index as i64],
                |r| r.get(0),
            )
            .map_err(|e| {
                PrimerError::Storage(format!(
                    "resolve turn (session={session_id}, index={turn_index}): {e}"
                ))
            })?;

        let mut insert_concept = tx
            .prepare("INSERT OR IGNORE INTO concepts (name) VALUES (?1)")
            .map_err(|e| PrimerError::Storage(format!("prepare insert concept: {e}")))?;
        let mut select_concept = tx
            .prepare("SELECT id FROM concepts WHERE name = ?1")
            .map_err(|e| PrimerError::Storage(format!("prepare select concept: {e}")))?;
        let mut link_concept = tx
            .prepare(
                "INSERT OR IGNORE INTO turn_concepts (turn_id, concept_id) VALUES (?1, ?2)",
            )
            .map_err(|e| PrimerError::Storage(format!("prepare link concept: {e}")))?;

        for name in concepts {
            insert_concept
                .execute(rusqlite::params![name])
                .map_err(|e| PrimerError::Storage(format!("upsert concept {name}: {e}")))?;
            let cid: i64 = select_concept
                .query_row(rusqlite::params![name], |r| r.get(0))
                .map_err(|e| PrimerError::Storage(format!("select concept {name}: {e}")))?;
            link_concept
                .execute(rusqlite::params![turn_id, cid])
                .map_err(|e| PrimerError::Storage(format!("link concept {name}: {e}")))?;
        }

        drop(link_concept);
        drop(select_concept);
        drop(insert_concept);

        tx.commit()
            .map_err(|e| PrimerError::Storage(format!("update_turn_concepts commit: {e}")))?;
        Ok(())
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `~/.cargo/bin/cargo test -p primer-storage update_turn_concepts`
Expected: 4 tests passing (`update_turn_concepts_appends_to_persisted_turn`, `update_turn_concepts_is_idempotent`, `update_turn_concepts_empty_slice_is_noop`, `update_turn_concepts_errors_on_missing_turn`).

Run full storage test suite: `~/.cargo/bin/cargo test -p primer-storage`
Expected: all storage tests still pass (no regressions).

- [ ] **Step 5: Commit**

```bash
git add src/crates/primer-storage/src/lib.rs
git commit -m "feat(storage): implement update_turn_concepts on SqliteSessionStore"
```

---

## Task 4: Create `primer-extractor` crate skeleton

**Files:**
- Create: `src/crates/primer-extractor/Cargo.toml`
- Create: `src/crates/primer-extractor/src/lib.rs`
- Modify: `src/Cargo.toml`

- [ ] **Step 1: Create the manifest**

Create `src/crates/primer-extractor/Cargo.toml`:

```toml
[package]
name = "primer-extractor"
version = "0.1.0"
edition = "2024"
description = "Concept extractor trait + implementations for the Primer."

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
```

- [ ] **Step 2: Create the trait file**

Create `src/crates/primer-extractor/src/lib.rs`:

```rust
//! Concept extractor — trait + implementations.
//!
//! Extracts the concepts a child surfaced and the concepts the Primer
//! introduced, given one completed exchange. One LLM-backed
//! implementation (`LlmConceptExtractor`) wraps an
//! `Arc<dyn InferenceBackend>` so the crate stays free of
//! `primer-inference` as a build dependency. A stub implementation
//! supports tests.

use async_trait::async_trait;

use primer_core::error::Result;
use primer_core::extractor::{ConceptExtraction, ExtractionContext};

pub mod consts;
pub mod llm;
pub mod settings;
pub mod stub;

pub use llm::LlmConceptExtractor;
pub use settings::ExtractorSettings;
pub use stub::StubConceptExtractor;

/// Extractor of concepts from one completed exchange.
///
/// Implementations may be LLM-backed (via `Arc<dyn InferenceBackend>`),
/// rule-based (future), or a stub for tests. Soft-fail policy:
/// extractor errors must NEVER propagate up and abort the conversation.
/// Implementations log via `tracing::warn!` and return
/// `ConceptExtraction::empty()` on any failure.
#[async_trait]
pub trait ConceptExtractor: Send + Sync {
    /// Stable identifier carrying impl + model-version. Future schema
    /// may persist this alongside extracted concepts so we can re-extract
    /// the archive with a different model later. Examples: "llm:claude-haiku-4-5",
    /// "stub", "rule-based-v1".
    fn identifier(&self) -> &str;

    async fn extract(&self, ctx: ExtractionContext<'_>) -> Result<ConceptExtraction>;
}
```

- [ ] **Step 3: Add the crate to the workspace**

Edit `src/Cargo.toml`. In `[workspace] members`, add `"crates/primer-extractor"` (alphabetical placement: after `primer-classifier`):

Find:
```toml
members = [
    "crates/primer-core",
    "crates/primer-inference",
    "crates/primer-speech",
    "crates/primer-knowledge",
    "crates/primer-pedagogy",
    "crates/primer-cli",
    "crates/primer-storage",
    "crates/primer-classifier",
]
```
Replace with:
```toml
members = [
    "crates/primer-core",
    "crates/primer-inference",
    "crates/primer-speech",
    "crates/primer-knowledge",
    "crates/primer-pedagogy",
    "crates/primer-cli",
    "crates/primer-storage",
    "crates/primer-classifier",
    "crates/primer-extractor",
]
```

In `[workspace.dependencies]`, add the workspace path entry directly after the `primer-classifier` line:

Find:
```toml
primer-classifier = { path = "crates/primer-classifier" }
```
Replace with:
```toml
primer-classifier = { path = "crates/primer-classifier" }
primer-extractor = { path = "crates/primer-extractor" }
```

- [ ] **Step 4: Create empty placeholder modules so `lib.rs` compiles**

Create `src/crates/primer-extractor/src/consts.rs`:
```rust
//! Default values for `ExtractorSettings`. Per the no-magic-numbers
//! convention, every numeric used by the extractor subsystem is
//! defined here (or in a sibling settings struct field).
```

Create `src/crates/primer-extractor/src/settings.rs`:
```rust
//! Tunable settings for the extractor subsystem.
```

Create `src/crates/primer-extractor/src/stub.rs`:
```rust
//! Stub extractor — deterministic, test-controllable.
```

Create `src/crates/primer-extractor/src/llm.rs`:
```rust
//! LLM-backed concept extractor.
```

(The `pub use` lines in `lib.rs` will fail until Tasks 5–7 fill these in. We'll let those errors surface as the next compile step.)

- [ ] **Step 5: Build to verify the crate skeleton wires up**

Run: `~/.cargo/bin/cargo build -p primer-extractor`
Expected: FAIL with "cannot find type `LlmConceptExtractor` in module `llm`" and similar for `ExtractorSettings`/`StubConceptExtractor`. That's the planned incremental state — Tasks 5–7 fill in those types.

To stop the build error from blocking workspace builds in the meantime, temporarily comment out the three `pub use` lines in `lib.rs`. We'll restore them in Task 7.

Edit `src/crates/primer-extractor/src/lib.rs`, find:
```rust
pub use llm::LlmConceptExtractor;
pub use settings::ExtractorSettings;
pub use stub::StubConceptExtractor;
```
Replace with:
```rust
// Re-exports activated as Tasks 5–7 land:
// pub use llm::LlmConceptExtractor;
// pub use settings::ExtractorSettings;
// pub use stub::StubConceptExtractor;
```

Run again: `~/.cargo/bin/cargo build -p primer-extractor`
Expected: PASS — empty crate compiles.

- [ ] **Step 6: Commit**

```bash
git add src/Cargo.toml src/crates/primer-extractor
git commit -m "feat(extractor): scaffold primer-extractor crate"
```

---

## Task 5: `ExtractorSettings` + `consts.rs`

**Files:**
- Modify: `src/crates/primer-extractor/src/consts.rs`
- Modify: `src/crates/primer-extractor/src/settings.rs`

- [ ] **Step 1: Write the failing test**

Add to `src/crates/primer-extractor/src/settings.rs`:

```rust
//! Tunable settings for the extractor subsystem.

use std::time::Duration;

use crate::consts;

#[derive(Debug, Clone)]
pub struct ExtractorSettings {
    /// Maximum time to block awaiting the previous turn's extraction
    /// before the next intent decision. Defaults to 1500ms — extraction
    /// is allowed to be slower than classification because no in-turn
    /// decision currently depends on extracted concepts. If the timeout
    /// fires, the task is detached (extraction may still complete and
    /// persist asynchronously); we just don't wait for it.
    pub blocking_timeout: Duration,
    /// How many surrounding turns to include as context in the
    /// extractor prompt. Helps disambiguate pronouns ("it", "that").
    pub recent_context_turns: usize,
    /// Hard cap on the LLM's raw output length before parsing, to
    /// guard against runaway responses. Char-boundary-safe.
    pub max_output_chars: usize,
    /// Hard cap on extracted concepts per speaker. Truncated post-parse
    /// so a runaway list doesn't bloat `concepts` table cardinality.
    pub max_concepts_per_speaker: usize,
    pub generation_max_tokens: u32,
    pub generation_temperature: f32,
    pub generation_top_p: f32,
}

impl Default for ExtractorSettings {
    fn default() -> Self {
        Self {
            blocking_timeout: Duration::from_millis(consts::DEFAULT_BLOCKING_TIMEOUT_MS),
            recent_context_turns: consts::DEFAULT_RECENT_CONTEXT_TURNS,
            max_output_chars: consts::DEFAULT_MAX_EXTRACTOR_OUTPUT_CHARS,
            max_concepts_per_speaker: consts::DEFAULT_MAX_CONCEPTS_PER_SPEAKER,
            generation_max_tokens: consts::DEFAULT_EXTRACTOR_MAX_TOKENS,
            generation_temperature: consts::DEFAULT_EXTRACTOR_TEMPERATURE,
            generation_top_p: consts::DEFAULT_EXTRACTOR_TOP_P,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_consts() {
        let s = ExtractorSettings::default();
        assert_eq!(
            s.blocking_timeout,
            Duration::from_millis(consts::DEFAULT_BLOCKING_TIMEOUT_MS)
        );
        assert_eq!(s.recent_context_turns, consts::DEFAULT_RECENT_CONTEXT_TURNS);
        assert_eq!(
            s.max_output_chars,
            consts::DEFAULT_MAX_EXTRACTOR_OUTPUT_CHARS
        );
        assert_eq!(
            s.max_concepts_per_speaker,
            consts::DEFAULT_MAX_CONCEPTS_PER_SPEAKER
        );
        assert_eq!(
            s.generation_max_tokens,
            consts::DEFAULT_EXTRACTOR_MAX_TOKENS
        );
        assert!((s.generation_temperature - consts::DEFAULT_EXTRACTOR_TEMPERATURE).abs() < 1e-6);
        assert!((s.generation_top_p - consts::DEFAULT_EXTRACTOR_TOP_P).abs() < 1e-6);
    }
}
```

- [ ] **Step 2: Add the constants**

Replace the contents of `src/crates/primer-extractor/src/consts.rs` with:

```rust
//! Default values for `ExtractorSettings`. Per the no-magic-numbers
//! convention, every numeric used by the extractor subsystem is
//! defined here (or in a sibling settings struct field).

pub const DEFAULT_BLOCKING_TIMEOUT_MS: u64 = 1500;
pub const DEFAULT_RECENT_CONTEXT_TURNS: usize = 4;
pub const DEFAULT_MAX_EXTRACTOR_OUTPUT_CHARS: usize = 1024;
pub const DEFAULT_MAX_CONCEPTS_PER_SPEAKER: usize = 8;
pub const DEFAULT_EXTRACTOR_MAX_TOKENS: u32 = 384;
pub const DEFAULT_EXTRACTOR_TEMPERATURE: f32 = 0.1;
pub const DEFAULT_EXTRACTOR_TOP_P: f32 = 0.9;
```

- [ ] **Step 3: Re-enable the `pub use settings::ExtractorSettings;` re-export**

In `src/crates/primer-extractor/src/lib.rs`, change:
```rust
// pub use settings::ExtractorSettings;
```
to:
```rust
pub use settings::ExtractorSettings;
```

(Leave `LlmConceptExtractor` and `StubConceptExtractor` re-exports commented out for now — they're enabled in Tasks 6 and 7.)

- [ ] **Step 4: Run tests**

Run: `~/.cargo/bin/cargo test -p primer-extractor settings::tests::defaults_match_consts`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/crates/primer-extractor
git commit -m "feat(extractor): ExtractorSettings + consts"
```

---

## Task 6: `StubConceptExtractor`

**Files:**
- Modify: `src/crates/primer-extractor/src/stub.rs`
- Modify: `src/crates/primer-extractor/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Replace `src/crates/primer-extractor/src/stub.rs` with:

```rust
//! Stub extractor — deterministic, test-controllable.

use std::collections::VecDeque;

use async_trait::async_trait;
use tokio::sync::Mutex;

use primer_core::error::Result;
use primer_core::extractor::{ConceptExtraction, ExtractionContext};

use crate::ConceptExtractor;

pub struct StubConceptExtractor {
    fallback: ConceptExtraction,
    script: Mutex<Option<VecDeque<ConceptExtraction>>>,
}

impl StubConceptExtractor {
    pub fn new() -> Self {
        Self {
            fallback: ConceptExtraction::empty(),
            script: Mutex::new(None),
        }
    }

    pub fn with_response(response: ConceptExtraction) -> Self {
        Self {
            fallback: response,
            script: Mutex::new(None),
        }
    }

    pub fn with_script(script: Vec<ConceptExtraction>) -> Self {
        Self {
            fallback: ConceptExtraction::empty(),
            script: Mutex::new(Some(script.into())),
        }
    }
}

impl Default for StubConceptExtractor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ConceptExtractor for StubConceptExtractor {
    fn identifier(&self) -> &str {
        "stub"
    }

    async fn extract(&self, _ctx: ExtractionContext<'_>) -> Result<ConceptExtraction> {
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
    use primer_core::conversation::{Speaker, Turn};

    fn t(speaker: Speaker, text: &str) -> Turn {
        Turn {
            speaker,
            text: text.into(),
            timestamp: Utc::now(),
            intent: None,
            concepts: vec![],
        }
    }

    fn ctx<'a>(child: &'a Turn, primer: &'a Turn) -> ExtractionContext<'a> {
        ExtractionContext {
            child_turn: child,
            primer_turn: primer,
            recent_turns: &[],
        }
    }

    #[tokio::test]
    async fn default_returns_empty() {
        let e = StubConceptExtractor::new();
        let child = t(Speaker::Child, "hi");
        let primer = t(Speaker::Primer, "hello");
        let r = e.extract(ctx(&child, &primer)).await.unwrap();
        assert!(r.child_concepts.is_empty());
        assert!(r.primer_concepts.is_empty());
    }

    #[tokio::test]
    async fn with_response_returns_configured() {
        let configured = ConceptExtraction {
            child_concepts: vec!["gravity".into()],
            primer_concepts: vec!["mass".into()],
        };
        let e = StubConceptExtractor::with_response(configured.clone());
        let child = t(Speaker::Child, "?");
        let primer = t(Speaker::Primer, "?");
        let r = e.extract(ctx(&child, &primer)).await.unwrap();
        assert_eq!(r.child_concepts, configured.child_concepts);
        assert_eq!(r.primer_concepts, configured.primer_concepts);
    }

    #[tokio::test]
    async fn with_script_returns_in_order_then_falls_back() {
        let e = StubConceptExtractor::with_script(vec![
            ConceptExtraction {
                child_concepts: vec!["a".into()],
                primer_concepts: vec![],
            },
            ConceptExtraction {
                child_concepts: vec!["b".into()],
                primer_concepts: vec![],
            },
        ]);
        let child = t(Speaker::Child, "?");
        let primer = t(Speaker::Primer, "?");
        let r1 = e.extract(ctx(&child, &primer)).await.unwrap();
        assert_eq!(r1.child_concepts, vec!["a".to_string()]);
        let r2 = e.extract(ctx(&child, &primer)).await.unwrap();
        assert_eq!(r2.child_concepts, vec!["b".to_string()]);
        // Script exhausted — falls back to empty.
        let r3 = e.extract(ctx(&child, &primer)).await.unwrap();
        assert!(r3.child_concepts.is_empty());
    }

    #[tokio::test]
    async fn identifier_is_stub() {
        let e = StubConceptExtractor::new();
        assert_eq!(e.identifier(), "stub");
    }
}
```

- [ ] **Step 2: Re-enable the re-export**

In `src/crates/primer-extractor/src/lib.rs`, change:
```rust
// pub use stub::StubConceptExtractor;
```
to:
```rust
pub use stub::StubConceptExtractor;
```

- [ ] **Step 3: Run tests**

Run: `~/.cargo/bin/cargo test -p primer-extractor stub::tests`
Expected: 4 passing tests (`default_returns_empty`, `with_response_returns_configured`, `with_script_returns_in_order_then_falls_back`, `identifier_is_stub`).

- [ ] **Step 4: Commit**

```bash
git add src/crates/primer-extractor
git commit -m "feat(extractor): StubConceptExtractor with script + response variants"
```

---

## Task 7: `LlmConceptExtractor`

**Files:**
- Modify: `src/crates/primer-extractor/src/llm.rs`
- Modify: `src/crates/primer-extractor/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Replace `src/crates/primer-extractor/src/llm.rs` with:

```rust
//! LLM-backed concept extractor. Wraps any `InferenceBackend`
//! (via `Arc<dyn ...>`) so the chat backend can be reused or a
//! cheaper extraction-specific model can be swapped in independently.

use std::sync::Arc;

use async_trait::async_trait;
use tracing::warn;

use primer_core::conversation::{Speaker, Turn};
use primer_core::error::Result;
use primer_core::extractor::{ConceptExtraction, ExtractionContext};
use primer_core::inference::{InferenceBackend, Message, Prompt, Role};

use crate::ConceptExtractor;
use crate::settings::ExtractorSettings;

pub struct LlmConceptExtractor {
    backend: Arc<dyn InferenceBackend>,
    settings: ExtractorSettings,
    identifier: String,
}

impl LlmConceptExtractor {
    pub fn new(
        backend: Arc<dyn InferenceBackend>,
        model: String,
        settings: ExtractorSettings,
    ) -> Self {
        let identifier = format!("llm:{model}");
        Self {
            backend,
            settings,
            identifier,
        }
    }
}

#[async_trait]
impl ConceptExtractor for LlmConceptExtractor {
    fn identifier(&self) -> &str {
        &self.identifier
    }

    async fn extract(&self, ctx: ExtractionContext<'_>) -> Result<ConceptExtraction> {
        let prompt = build_extraction_prompt(&ctx);
        let params = primer_core::inference::GenerationParams {
            max_tokens: self.settings.generation_max_tokens,
            temperature: self.settings.generation_temperature,
            top_p: self.settings.generation_top_p,
            stop_sequences: vec![],
        };
        let raw = match self.backend.generate(&prompt, &params).await {
            Ok(r) => r,
            Err(e) => {
                warn!(extractor = %self.identifier, error = ?e, "extractor backend call failed");
                return Ok(ConceptExtraction::empty());
            }
        };

        let truncated = truncate_to_chars(&raw, self.settings.max_output_chars);
        match parse_extraction_output(truncated) {
            Ok(mut e) => {
                truncate_concept_lists(&mut e, self.settings.max_concepts_per_speaker);
                Ok(e)
            }
            Err(reason) => {
                let snippet: String = truncated.chars().take(200).collect();
                warn!(extractor = %self.identifier, raw = %snippet, %reason, "extractor output unparseable");
                Ok(ConceptExtraction::empty())
            }
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn truncate_to_chars(s: &str, max_chars: usize) -> &str {
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

fn truncate_concept_lists(e: &mut ConceptExtraction, max_per_speaker: usize) {
    if e.child_concepts.len() > max_per_speaker {
        e.child_concepts.truncate(max_per_speaker);
    }
    if e.primer_concepts.len() > max_per_speaker {
        e.primer_concepts.truncate(max_per_speaker);
    }
}

// ── Prompt builder ────────────────────────────────────────────────────────────

fn build_extraction_prompt(ctx: &ExtractionContext) -> Prompt {
    let context_section = if ctx.recent_turns.is_empty() {
        "(no prior context)".to_string()
    } else {
        ctx.recent_turns
            .iter()
            .map(|t| format!("{}: {}", speaker_label(t.speaker), t.text))
            .collect::<Vec<_>>()
            .join("\n")
    };

    let system = "You extract conceptual topics from a Socratic learning \
        conversation between a child and a teaching companion (the Primer). \
        Output ONLY valid JSON of the form: \
        {\"child_concepts\": [\"<topic>\", ...], \"primer_concepts\": [\"<topic>\", ...]} — \
        no other text. Each topic is a short noun phrase (1–3 words, lowercase). \
        For child_concepts, list topics the child surfaced or engaged with — \
        what they brought up, asked about, or attempted to reason about. \
        For primer_concepts, list topics the Primer introduced — what the \
        child was exposed to via the Primer's response. A topic the Primer \
        asks about counts as Primer-introduced even if the child only \
        acknowledged it. Use [] when a speaker introduced no clear topics. \
        Do not invent topics absent from the text.".to_string();

    let user = format!(
        "Prior context (oldest first):\n{context_section}\n\nTurn to analyse:\n{}: {}\n{}: {}\n\nExtract concepts now.",
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

// ── Output parser ─────────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
struct ExtractorOutput {
    #[serde(default)]
    child_concepts: Vec<String>,
    #[serde(default)]
    primer_concepts: Vec<String>,
}

fn parse_extraction_output(raw: &str) -> std::result::Result<ConceptExtraction, String> {
    let json_str = extract_first_json_object(raw)
        .ok_or_else(|| "no JSON object found in output".to_string())?;
    let parsed: ExtractorOutput =
        serde_json::from_str(json_str).map_err(|e| format!("JSON parse error: {e}"))?;

    Ok(ConceptExtraction {
        child_concepts: normalize_concepts(parsed.child_concepts),
        primer_concepts: normalize_concepts(parsed.primer_concepts),
    })
}

/// Lower-case, trim, drop empty/long entries, dedupe (preserving first
/// occurrence). 64-char cap is generous for any reasonable noun phrase
/// and prevents pathological "concept = entire sentence" outputs.
fn normalize_concepts(input: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(input.len());
    for c in input {
        let trimmed = c.trim().to_lowercase();
        if trimmed.is_empty() || trimmed.chars().count() > 64 {
            continue;
        }
        if seen.insert(trimmed.clone()) {
            out.push(trimmed);
        }
    }
    out
}

/// Extract the first balanced JSON object from `raw`. Tolerates wrapper
/// text before/after, and JSON containing nested braces.
fn extract_first_json_object(raw: &str) -> Option<&str> {
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

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use futures::stream;
    use primer_core::conversation::{Speaker, Turn};
    use primer_core::inference::{GenerationParams, Prompt, TokenChunk, TokenStream};

    /// Test backend that returns canned text from `generate_stream` and stubs
    /// `name` / `is_available`.
    struct CannedBackend(String);

    #[async_trait]
    impl InferenceBackend for CannedBackend {
        fn name(&self) -> &str {
            "canned"
        }

        async fn is_available(&self) -> bool {
            true
        }

        async fn generate_stream(
            &self,
            _prompt: &Prompt,
            _params: &GenerationParams,
        ) -> Result<TokenStream> {
            let text = self.0.clone();
            let chunk = TokenChunk { text, done: true };
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

    fn ctx<'a>(child: &'a Turn, primer: &'a Turn) -> ExtractionContext<'a> {
        ExtractionContext {
            child_turn: child,
            primer_turn: primer,
            recent_turns: &[],
        }
    }

    #[test]
    fn identifier_includes_model_name() {
        let backend = Arc::new(CannedBackend("{}".into())) as Arc<dyn InferenceBackend>;
        let e = LlmConceptExtractor::new(
            backend,
            "claude-haiku-4-5".into(),
            ExtractorSettings::default(),
        );
        assert_eq!(e.identifier(), "llm:claude-haiku-4-5");
    }

    #[tokio::test]
    async fn extract_parses_well_formed_json() {
        let backend = Arc::new(CannedBackend(
            r#"{"child_concepts": ["photosynthesis", "Plants"], "primer_concepts": ["chlorophyll"]}"#.into(),
        )) as Arc<dyn InferenceBackend>;
        let e = LlmConceptExtractor::new(
            backend,
            "test-model".into(),
            ExtractorSettings::default(),
        );
        let c = turn(Speaker::Child, "what's photosynthesis?");
        let p = turn(Speaker::Primer, "great question!");
        let r = e.extract(ctx(&c, &p)).await.unwrap();
        assert_eq!(r.child_concepts, vec!["photosynthesis", "plants"]);
        assert_eq!(r.primer_concepts, vec!["chlorophyll"]);
    }

    #[tokio::test]
    async fn extract_handles_wrapper_text() {
        let backend = Arc::new(CannedBackend(
            r#"Here you go: {"child_concepts": ["gravity"], "primer_concepts": []} done"#.into(),
        )) as Arc<dyn InferenceBackend>;
        let e = LlmConceptExtractor::new(
            backend,
            "test-model".into(),
            ExtractorSettings::default(),
        );
        let c = turn(Speaker::Child, "?");
        let p = turn(Speaker::Primer, "?");
        let r = e.extract(ctx(&c, &p)).await.unwrap();
        assert_eq!(r.child_concepts, vec!["gravity"]);
        assert!(r.primer_concepts.is_empty());
    }

    #[tokio::test]
    async fn extract_returns_empty_on_unparseable_output() {
        let backend = Arc::new(CannedBackend("not json at all".into())) as Arc<dyn InferenceBackend>;
        let e = LlmConceptExtractor::new(
            backend,
            "test-model".into(),
            ExtractorSettings::default(),
        );
        let c = turn(Speaker::Child, "?");
        let p = turn(Speaker::Primer, "?");
        let r = e.extract(ctx(&c, &p)).await.unwrap();
        assert!(r.child_concepts.is_empty());
        assert!(r.primer_concepts.is_empty());
    }

    #[tokio::test]
    async fn extract_returns_empty_on_backend_error() {
        struct ErrorBackend;

        #[async_trait]
        impl InferenceBackend for ErrorBackend {
            fn name(&self) -> &str {
                "error-test"
            }
            async fn is_available(&self) -> bool {
                true
            }
            async fn generate_stream(
                &self,
                _prompt: &Prompt,
                _params: &GenerationParams,
            ) -> Result<TokenStream> {
                Err(primer_core::error::PrimerError::Inference(
                    "simulated network error".into(),
                ))
            }
        }

        let backend = Arc::new(ErrorBackend) as Arc<dyn InferenceBackend>;
        let e = LlmConceptExtractor::new(
            backend,
            "test-model".into(),
            ExtractorSettings::default(),
        );
        let c = turn(Speaker::Child, "?");
        let p = turn(Speaker::Primer, "?");
        let r = e.extract(ctx(&c, &p)).await.unwrap();
        assert!(r.child_concepts.is_empty());
        assert!(r.primer_concepts.is_empty());
    }

    #[tokio::test]
    async fn extract_truncates_concept_lists_per_speaker_cap() {
        let mut payload_child: Vec<String> = (0..20).map(|i| format!("child-{i}")).collect();
        let payload_primer: Vec<String> = (0..20).map(|i| format!("primer-{i}")).collect();
        payload_child.dedup();
        let payload = serde_json::json!({
            "child_concepts": payload_child,
            "primer_concepts": payload_primer,
        });
        let backend = Arc::new(CannedBackend(payload.to_string())) as Arc<dyn InferenceBackend>;
        let settings = ExtractorSettings {
            max_concepts_per_speaker: 5,
            ..ExtractorSettings::default()
        };
        let e = LlmConceptExtractor::new(backend, "test-model".into(), settings);
        let c = turn(Speaker::Child, "?");
        let p = turn(Speaker::Primer, "?");
        let r = e.extract(ctx(&c, &p)).await.unwrap();
        assert_eq!(r.child_concepts.len(), 5);
        assert_eq!(r.primer_concepts.len(), 5);
    }

    #[tokio::test]
    async fn extract_normalizes_lowercase_and_dedupes() {
        let backend = Arc::new(CannedBackend(
            r#"{"child_concepts": ["Gravity", "gravity", "  "], "primer_concepts": []}"#.into(),
        )) as Arc<dyn InferenceBackend>;
        let e = LlmConceptExtractor::new(
            backend,
            "test-model".into(),
            ExtractorSettings::default(),
        );
        let c = turn(Speaker::Child, "?");
        let p = turn(Speaker::Primer, "?");
        let r = e.extract(ctx(&c, &p)).await.unwrap();
        assert_eq!(r.child_concepts, vec!["gravity"]);
    }
}
```

- [ ] **Step 2: Re-enable the re-export**

In `src/crates/primer-extractor/src/lib.rs`, change:
```rust
// pub use llm::LlmConceptExtractor;
```
to:
```rust
pub use llm::LlmConceptExtractor;
```

- [ ] **Step 3: Run tests**

Run: `~/.cargo/bin/cargo test -p primer-extractor`
Expected: all extractor tests pass — settings (1), stub (4), llm (7) = 12 tests.

- [ ] **Step 4: Commit**

```bash
git add src/crates/primer-extractor
git commit -m "feat(extractor): LlmConceptExtractor with prompt + parser + soft-fail"
```

---

## Task 8: Wire `extractor` field into `DialogueManager`

**Files:**
- Modify: `src/crates/primer-pedagogy/Cargo.toml`
- Modify: `src/crates/primer-pedagogy/src/dialogue_manager.rs`

- [ ] **Step 1: Add the dependency**

Edit `src/crates/primer-pedagogy/Cargo.toml`. In the `[dependencies]` section, immediately after `primer-classifier.workspace = true`, add:

```toml
primer-extractor.workspace = true
```

- [ ] **Step 2: Add the new fields to `DialogueManager` and update `new()` signature**

In `src/crates/primer-pedagogy/src/dialogue_manager.rs`:

Find the `use primer_classifier::...;` import line and add directly below it:
```rust
use primer_core::extractor::ConceptExtraction;
use primer_extractor::{ConceptExtractor, ExtractorSettings};
```

In the `DialogueManager` struct (search for `pub struct DialogueManager<'a>`), add three new fields directly after `classify_task: Option<JoinHandle<Option<EngagementAssessment>>>,`:

```rust
    /// Concept extractor — called after each Primer response to extract
    /// concepts from the just-completed exchange. Arc for the same
    /// spawn-capture reason as `classifier`.
    extractor: Arc<dyn ConceptExtractor>,
    /// Tunable parameters for the extractor.
    extractor_settings: ExtractorSettings,
    /// Handle to the in-flight extractor task spawned after the previous
    /// turn. `None` when no task is running.
    extract_task: Option<JoinHandle<Option<ConceptExtraction>>>,
```

In `DialogueManager::new`, add two parameters before `config: PedagogyConfig` and store them:

```rust
    pub fn new(
        learner: LearnerModel,
        inference: &'a dyn InferenceBackend,
        knowledge: &'a dyn KnowledgeBase,
        stores: DialogueManagerStores,
        classifier: Arc<dyn EngagementClassifier>,
        classifier_settings: ClassifierSettings,
        extractor: Arc<dyn ConceptExtractor>,
        extractor_settings: ExtractorSettings,
        config: PedagogyConfig,
    ) -> Self {
        let session = Session::new(learner.profile.id);
        Self {
            learner,
            session,
            inference,
            knowledge,
            storage: stores.session,
            learner_store: stores.learner,
            classifier,
            classifier_settings,
            classify_task: None,
            extractor,
            extractor_settings,
            extract_task: None,
            config,
            learner_dirty: false,
        }
    }
```

- [ ] **Step 3: Update existing dialogue_manager tests to pass the extractor**

In the `#[cfg(test)] mod tests` block, find the `fn stub_classifier()` helper and add a stub_extractor helper directly after it:

```rust
    fn stub_extractor() -> Arc<dyn ConceptExtractor> {
        Arc::new(primer_extractor::StubConceptExtractor::new())
    }
```

Add at top of test module (search for `use super::*;`), add:
```rust
    use primer_extractor::ExtractorSettings;
```

Then update every existing call to `DialogueManager::new` in the tests file. Search for `DialogueManager::new(` — there are several call sites. For each one, add `stub_extractor(),` and `ExtractorSettings::default(),` between `classifier_settings.clone()` (or `ClassifierSettings::default()`) and the final `pedagogy_config` argument.

Example of the change pattern — find:
```rust
        let mut dm = DialogueManager::new(
            test_learner(),
            &backend as &dyn InferenceBackend,
            &EmptyKnowledge as &dyn KnowledgeBase,
            DialogueManagerStores::default(),
            stub_classifier(),
            ClassifierSettings::default(),
            PedagogyConfig::default(),
        );
```
Replace with:
```rust
        let mut dm = DialogueManager::new(
            test_learner(),
            &backend as &dyn InferenceBackend,
            &EmptyKnowledge as &dyn KnowledgeBase,
            DialogueManagerStores::default(),
            stub_classifier(),
            ClassifierSettings::default(),
            stub_extractor(),
            ExtractorSettings::default(),
            PedagogyConfig::default(),
        );
```

Apply this transformation to every `DialogueManager::new(` call in the test module.

- [ ] **Step 4: Build to verify the wiring compiles**

Run: `~/.cargo/bin/cargo build --workspace`
Expected: FAIL — the CLI's `DialogueManager::new` call site does NOT yet pass extractor. Task 12 fixes that. For now we expect only the CLI to fail to compile.

Run: `~/.cargo/bin/cargo build -p primer-pedagogy`
Expected: PASS — pedagogy crate builds with new fields.

Run: `~/.cargo/bin/cargo test -p primer-pedagogy`
Expected: PASS — all existing pedagogy tests still pass with the new helpers.

- [ ] **Step 5: Commit**

```bash
git add src/crates/primer-pedagogy
git commit -m "feat(pedagogy): wire ConceptExtractor field into DialogueManager"
```

---

## Task 9: Spawn extraction task after primer turn save

**Files:**
- Modify: `src/crates/primer-pedagogy/src/dialogue_manager.rs`

- [ ] **Step 1: Write the failing test**

Add to `src/crates/primer-pedagogy/src/dialogue_manager.rs` test module (search for the end of `#[cfg(test)] mod tests`):

```rust
    /// Session-store spy that records `update_turn_concepts` calls so
    /// tests can assert the extractor's persistence side effect.
    struct ConceptCapturingStore {
        inner: CountingStore,
        captures: Mutex<Vec<(usize, Vec<String>)>>,
    }

    impl ConceptCapturingStore {
        fn new() -> Self {
            Self {
                inner: CountingStore::new(),
                captures: Mutex::new(vec![]),
            }
        }
        fn captured(&self) -> Vec<(usize, Vec<String>)> {
            self.captures.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl primer_core::storage::SessionStore for ConceptCapturingStore {
        async fn save_session(&self, session: &Session) -> Result<()> {
            self.inner.save_session(session).await
        }
        async fn load_session(&self, id: uuid::Uuid) -> Result<Option<Session>> {
            self.inner.load_session(id).await
        }
        async fn retrieve_session_turns(
            &self,
            session_id: uuid::Uuid,
            query: &str,
            k: usize,
            exclude_indices_at_or_after: usize,
        ) -> Result<Vec<Turn>> {
            self.inner
                .retrieve_session_turns(session_id, query, k, exclude_indices_at_or_after)
                .await
        }
        async fn save_classification(
            &self,
            session_id: primer_core::conversation::SessionId,
            turn_index: usize,
            assessment: &primer_core::classifier::EngagementAssessment,
            classifier_identifier: &str,
        ) -> Result<()> {
            self.inner
                .save_classification(session_id, turn_index, assessment, classifier_identifier)
                .await
        }
        async fn load_recent_assessments(
            &self,
            session_id: primer_core::conversation::SessionId,
            classifier_identifier: &str,
            k: usize,
        ) -> Result<Vec<primer_core::classifier::EngagementAssessment>> {
            self.inner
                .load_recent_assessments(session_id, classifier_identifier, k)
                .await
        }
        async fn most_recent_session_learner_id(&self) -> Result<Option<uuid::Uuid>> {
            self.inner.most_recent_session_learner_id().await
        }
        async fn update_turn_concepts(
            &self,
            _session_id: primer_core::conversation::SessionId,
            turn_index: usize,
            concepts: &[String],
        ) -> Result<()> {
            self.captures
                .lock()
                .unwrap()
                .push((turn_index, concepts.to_vec()));
            Ok(())
        }
    }

    #[tokio::test]
    async fn extract_task_persists_concepts_for_both_turns_after_response() {
        let backend = ScriptedBackend::new(vec![Ok(chunk("Hi there!", true))]);
        let extractor = Arc::new(primer_extractor::StubConceptExtractor::with_response(
            ConceptExtraction {
                child_concepts: vec!["topic-a".into()],
                primer_concepts: vec!["topic-b".into()],
            },
        ));
        let store = Arc::new(ConceptCapturingStore::new());

        let stores = DialogueManagerStores {
            session: Some(store.clone() as Arc<dyn primer_core::storage::SessionStore>),
            learner: None,
        };

        let mut dm = DialogueManager::new(
            test_learner(),
            &backend as &dyn InferenceBackend,
            &EmptyKnowledge as &dyn KnowledgeBase,
            stores,
            stub_classifier(),
            ClassifierSettings::default(),
            extractor as Arc<dyn ConceptExtractor>,
            ExtractorSettings::default(),
            PedagogyConfig::default(),
        );

        dm.respond_to("Hello").await.unwrap();

        // Yield until the spawned extractor task lands its captures.
        for _ in 0..50 {
            if store.captured().len() >= 2 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        let captures = store.captured();
        assert_eq!(captures.len(), 2, "expected child + primer captures");
        // Child turn is at index 0, primer at index 1.
        let child_capture = captures.iter().find(|(i, _)| *i == 0).unwrap();
        let primer_capture = captures.iter().find(|(i, _)| *i == 1).unwrap();
        assert_eq!(child_capture.1, vec!["topic-a".to_string()]);
        assert_eq!(primer_capture.1, vec!["topic-b".to_string()]);
    }

    #[tokio::test]
    async fn extract_task_does_not_spawn_on_inference_error() {
        let backend = ScriptedBackend::new(vec![Err(PrimerError::Inference("boom".into()))]);
        let extractor = Arc::new(primer_extractor::StubConceptExtractor::with_response(
            ConceptExtraction {
                child_concepts: vec!["should-not-persist".into()],
                primer_concepts: vec![],
            },
        ));
        let store = Arc::new(ConceptCapturingStore::new());

        let stores = DialogueManagerStores {
            session: Some(store.clone() as Arc<dyn primer_core::storage::SessionStore>),
            learner: None,
        };

        let mut dm = DialogueManager::new(
            test_learner(),
            &backend as &dyn InferenceBackend,
            &EmptyKnowledge as &dyn KnowledgeBase,
            stores,
            stub_classifier(),
            ClassifierSettings::default(),
            extractor as Arc<dyn ConceptExtractor>,
            ExtractorSettings::default(),
            PedagogyConfig::default(),
        );

        let _ = dm.respond_to("Hello").await;

        // Give the runtime a chance to run any spuriously-spawned task.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(store.captured().is_empty(), "extractor must not run on inference error");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `~/.cargo/bin/cargo test -p primer-pedagogy extract_task_persists_concepts_for_both_turns_after_response extract_task_does_not_spawn_on_inference_error`
Expected: FAIL — `extract_task_persists_concepts_for_both_turns_after_response` fails because no concepts are captured (the spawn site doesn't exist yet).

- [ ] **Step 3: Implement the spawn site**

In `src/crates/primer-pedagogy/src/dialogue_manager.rs`, find the existing `if result.is_ok()` block at step 6 (search for `// 6. Spawn a classification task`). Directly after that block (after the closing brace of `if let Some(child_idx) = child_turn_index { ... }`) but still inside the outer `if result.is_ok()`, add a parallel `extract_task` spawn:

Find:
```rust
        // 6. Spawn a classification task for the child turn that just completed.
        //    Skipped on error paths — without a completed Primer response there
        //    is no exchange to assess, and the partial Primer turn was dropped.
        if result.is_ok() {
            let child_turn_index = self
                .session
                .turns
                .iter()
                .enumerate()
                .rev()
                .find(|(_, t)| t.speaker == Speaker::Child)
                .map(|(i, _)| i);

            if let Some(child_idx) = child_turn_index {
```

Inside this `if result.is_ok()` block, after the existing classifier spawn finishes (the line `self.classify_task = Some(task);`), add a new spawn for the extractor before the outer `}`:

Find this existing fragment (the very end of step 6):
```rust
                self.classify_task = Some(task);
            }
        }

        result
    }
```

Replace it with:
```rust
                self.classify_task = Some(task);
            }

            // 7. Spawn an extraction task for the just-completed exchange.
            //    Same skip-on-error policy as the classifier. The task
            //    self-persists `turn_concepts` for both turns; the JoinHandle
            //    output is consumed by `await_pending_extraction` at the
            //    start of the next turn so the in-memory `learner.concepts`
            //    can be updated.
            let total_turns = self.session.turns.len();
            if total_turns >= 2
                && self.session.turns[total_turns - 1].speaker == Speaker::Primer
                && self.session.turns[total_turns - 2].speaker == Speaker::Child
            {
                let child_idx = total_turns - 2;
                let primer_idx = total_turns - 1;
                let child_turn = self.session.turns[child_idx].clone();
                let primer_turn = self.session.turns[primer_idx].clone();
                let recent_turns: Vec<Turn> = self
                    .session
                    .turns
                    .iter()
                    .rev()
                    .skip(2) // skip the just-added child + primer turns
                    .take(self.extractor_settings.recent_context_turns)
                    .cloned()
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect();
                let extractor = Arc::clone(&self.extractor);
                let store = self.storage.clone();
                let session_id = self.session.id;

                let task = tokio::spawn(async move {
                    let ctx = primer_core::extractor::ExtractionContext {
                        child_turn: &child_turn,
                        primer_turn: &primer_turn,
                        recent_turns: &recent_turns,
                    };
                    match extractor.extract(ctx).await {
                        Ok(extraction) => {
                            if let Some(store) = store {
                                if !extraction.child_concepts.is_empty() {
                                    if let Err(e) = store
                                        .update_turn_concepts(
                                            session_id,
                                            child_idx,
                                            &extraction.child_concepts,
                                        )
                                        .await
                                    {
                                        tracing::warn!(error = ?e, "update_turn_concepts (child) failed");
                                    }
                                }
                                if !extraction.primer_concepts.is_empty() {
                                    if let Err(e) = store
                                        .update_turn_concepts(
                                            session_id,
                                            primer_idx,
                                            &extraction.primer_concepts,
                                        )
                                        .await
                                    {
                                        tracing::warn!(error = ?e, "update_turn_concepts (primer) failed");
                                    }
                                }
                            }
                            Some(extraction)
                        }
                        Err(e) => {
                            tracing::warn!(error = ?e, "extractor returned error");
                            None
                        }
                    }
                });
                self.extract_task = Some(task);
            }
        }

        result
    }
```

- [ ] **Step 4: Run tests**

Run: `~/.cargo/bin/cargo test -p primer-pedagogy extract_task_persists_concepts_for_both_turns_after_response extract_task_does_not_spawn_on_inference_error`
Expected: both tests pass.

Run full pedagogy test suite: `~/.cargo/bin/cargo test -p primer-pedagogy`
Expected: all tests pass (no regressions).

- [ ] **Step 5: Commit**

```bash
git add src/crates/primer-pedagogy/src/dialogue_manager.rs
git commit -m "feat(pedagogy): spawn concept-extraction task after primer turn save"
```

---

## Task 10: `await_pending_extraction` and apply to `learner.concepts`

**Files:**
- Modify: `src/crates/primer-pedagogy/src/dialogue_manager.rs`

- [ ] **Step 1: Write the failing test**

In the `#[cfg(test)] mod tests` block, add this test (after the previous extractor tests):

```rust
    #[tokio::test]
    async fn pending_extraction_applied_to_learner_at_next_turn() {
        let backend = ScriptedBackend::new(vec![
            Ok(chunk("Hi turn 1!", true)),
        ]);
        // Two turns of extraction scripted: turn 1 surfaces "gravity" + "physics",
        // turn 2 surfaces "mass". Only the first one matters for this test —
        // we want to assert that after respond_to(turn 2), the learner has
        // gravity from turn 1's extraction.
        let extractor = Arc::new(primer_extractor::StubConceptExtractor::with_script(vec![
            ConceptExtraction {
                child_concepts: vec!["gravity".into()],
                primer_concepts: vec!["physics".into()],
            },
            ConceptExtraction {
                child_concepts: vec!["mass".into()],
                primer_concepts: vec![],
            },
        ]));

        let mut dm = DialogueManager::new(
            test_learner(),
            &backend as &dyn InferenceBackend,
            &EmptyKnowledge as &dyn KnowledgeBase,
            DialogueManagerStores::default(),
            stub_classifier(),
            ClassifierSettings::default(),
            extractor as Arc<dyn ConceptExtractor>,
            ExtractorSettings::default(),
            PedagogyConfig::default(),
        );

        dm.respond_to("turn 1").await.unwrap();

        // Replace the backend script so respond_to(turn 2) has something to say.
        // We can do this through the script Mutex on ScriptedBackend.
        *backend.script.lock().unwrap() = Some(vec![Ok(chunk("Hi turn 2!", true))]);

        // Allow the previous-turn extractor task to complete before turn 2 starts.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        dm.respond_to("turn 2").await.unwrap();

        let names: std::collections::HashSet<&str> = dm
            .learner
            .concepts
            .iter()
            .map(|c| c.concept_id.as_str())
            .collect();
        assert!(
            names.contains("gravity"),
            "child concept 'gravity' should be applied to learner; got: {:?}",
            names
        );
        assert!(
            names.contains("physics"),
            "primer concept 'physics' should be applied to learner; got: {:?}",
            names
        );
    }
```

(Note: `ScriptedBackend.script` is currently private. We need to make the test mutate it. The cleanest path is to add a `set_script` method on `ScriptedBackend`. Insert this method into the `impl ScriptedBackend` block near `fn summary_call_count`):

```rust
        fn set_script(&self, items: Vec<Result<TokenChunk>>) {
            *self.script.lock().unwrap() = Some(items);
        }
```

Then in the test, use `backend.set_script(vec![Ok(chunk("Hi turn 2!", true))]);` instead of the direct `*backend.script.lock()...` mutation.

- [ ] **Step 2: Run test to verify it fails**

Run: `~/.cargo/bin/cargo test -p primer-pedagogy pending_extraction_applied_to_learner_at_next_turn`
Expected: FAIL — `learner.concepts` is empty because nothing applies the extraction.

- [ ] **Step 3: Implement `await_pending_extraction` and `apply_extraction`**

In `src/crates/primer-pedagogy/src/dialogue_manager.rs`, near the existing `apply_assessment` free function (it's defined right after the `DialogueManager` struct definition, before the `impl<'a> DialogueManager<'a>`), add a sibling free function `apply_extraction`:

```rust
/// Merge a `ConceptExtraction` into the in-memory `LearnerModel.concepts`.
///
/// Adds new `ConceptState` rows (depth = `Aware`, confidence = 0.5) for
/// concepts not yet seen; for concepts already in the learner model,
/// increments `encounter_count` and refreshes `last_encountered`. The
/// updated state is what `LearnerStore::save_learner` will persist on
/// the next save (idempotent upsert into `learner_concepts` — monotonic
/// across the child's lifetime).
///
/// Both `child_concepts` and `primer_concepts` feed into the same
/// `learner.concepts` store. Today the model doesn't distinguish "a
/// concept the child surfaced" from "a concept the Primer introduced";
/// future work could add a per-side `encounter_count_by_speaker`.
pub(crate) fn apply_extraction(
    learner: &mut primer_core::learner::LearnerModel,
    extraction: &primer_core::extractor::ConceptExtraction,
) -> bool {
    use primer_core::learner::{ConceptState, UnderstandingDepth};
    let now = Utc::now();
    let mut changed = false;
    let combined = extraction
        .child_concepts
        .iter()
        .chain(extraction.primer_concepts.iter());
    for name in combined {
        if let Some(existing) = learner
            .concepts
            .iter_mut()
            .find(|c| c.concept_id == *name)
        {
            existing.encounter_count = existing.encounter_count.saturating_add(1);
            existing.last_encountered = Some(now);
            changed = true;
        } else {
            learner.concepts.push(ConceptState {
                concept_id: name.clone(),
                depth: UnderstandingDepth::Aware,
                confidence: 0.5,
                encounter_count: 1,
                last_encountered: Some(now),
                notes: vec![],
            });
            changed = true;
        }
    }
    changed
}
```

In the `impl<'a> DialogueManager<'a>` block, add a method `await_pending_extraction` near the existing `await_pending_classification` (search for `// ─── Classifier helpers ───`). Add directly after that helper:

```rust
    /// Wait (up to `extractor_settings.blocking_timeout`) for the extractor
    /// task spawned after the previous turn, then apply its result to
    /// `self.learner.concepts`.
    ///
    /// On timeout the task is detached (so the DB-persistence side effect
    /// can still complete), but the in-memory learner update for THIS
    /// turn is skipped — preferable to blocking the conversation. The
    /// pending concepts will be visible from `load_learner` on next
    /// resume regardless.
    async fn await_pending_extraction(&mut self) {
        let Some(task) = self.extract_task.take() else {
            return;
        };
        let abort = task.abort_handle();
        let timeout = self.extractor_settings.blocking_timeout;
        match tokio::time::timeout(timeout, task).await {
            Ok(Ok(Some(extraction))) => {
                if apply_extraction(&mut self.learner, &extraction) {
                    self.learner_dirty = true;
                }
            }
            Ok(Ok(None)) => { /* soft failure; nothing to apply */ }
            Ok(Err(e)) => tracing::warn!(error = ?e, "extractor task panicked"),
            Err(_) => {
                // Detach (don't abort) so the DB persistence side effect
                // can still complete; we just skip the in-memory apply.
                let _ = abort;
                tracing::debug!(
                    "extractor exceeded blocking timeout — proceeding without applied concepts"
                );
            }
        }
    }
```

Then call it at the start of `respond_to_streaming`. Find:
```rust
        // 0. Wait for the previous turn's classification (if any) to complete
        //    with a bounded timeout, then apply it so decide_intent sees the
        //    updated engagement state.
        self.await_pending_classification().await;
```
Replace with:
```rust
        // 0. Wait for the previous turn's classification + extraction (if any)
        //    to complete with bounded timeouts, then apply their results so
        //    decide_intent sees the updated engagement state and the system
        //    prompt sees freshly-extracted learner concepts.
        self.await_pending_classification().await;
        self.await_pending_extraction().await;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `~/.cargo/bin/cargo test -p primer-pedagogy pending_extraction_applied_to_learner_at_next_turn`
Expected: PASS.

Run full pedagogy test suite: `~/.cargo/bin/cargo test -p primer-pedagogy`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add src/crates/primer-pedagogy/src/dialogue_manager.rs
git commit -m "feat(pedagogy): await + apply pending extraction at next-turn boundary"
```

---

## Task 11: `close_session` drains `extract_task`

**Files:**
- Modify: `src/crates/primer-pedagogy/src/dialogue_manager.rs`

- [ ] **Step 1: Write the failing test**

Add to the test module:

```rust
    #[tokio::test]
    async fn close_session_drains_extractor_task() {
        let backend = ScriptedBackend::new(vec![Ok(chunk("Hi", true))]);
        let extractor = Arc::new(primer_extractor::StubConceptExtractor::with_response(
            ConceptExtraction {
                child_concepts: vec!["x".into()],
                primer_concepts: vec![],
            },
        ));
        let store = Arc::new(ConceptCapturingStore::new());

        let stores = DialogueManagerStores {
            session: Some(store.clone() as Arc<dyn primer_core::storage::SessionStore>),
            learner: None,
        };

        let mut dm = DialogueManager::new(
            test_learner(),
            &backend as &dyn InferenceBackend,
            &EmptyKnowledge as &dyn KnowledgeBase,
            stores,
            stub_classifier(),
            ClassifierSettings::default(),
            extractor as Arc<dyn ConceptExtractor>,
            ExtractorSettings::default(),
            PedagogyConfig::default(),
        );

        dm.respond_to("hi").await.unwrap();
        // close_session must drain so the extractor's update_turn_concepts
        // call has landed by the time close returns.
        dm.close_session().await;

        let captures = store.captured();
        assert!(!captures.is_empty(), "expected extraction to land before close returns");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `~/.cargo/bin/cargo test -p primer-pedagogy close_session_drains_extractor_task`
Expected: FAIL — captures is empty because the extractor task race shutdown.

- [ ] **Step 3: Implement the drain**

In `src/crates/primer-pedagogy/src/dialogue_manager.rs`, find the `close_session` method. Add a drain for the extractor task before the existing classifier drain:

Find:
```rust
    pub async fn close_session(&mut self) {
        // Drain the post-response classifier task spawned after the most
        // recent turn. Without this, a quick exit ("respond_to_streaming"
        // immediately followed by close_session) races the runtime shutdown
        // and the last turn_classifications row may never be persisted.
        self.await_pending_classification().await;
```
Replace with:
```rust
    pub async fn close_session(&mut self) {
        // Drain the post-response classifier + extractor tasks spawned
        // after the most recent turn. Without this, a quick exit
        // ("respond_to_streaming" immediately followed by close_session")
        // races the runtime shutdown and the last turn_classifications /
        // turn_concepts rows may never be persisted.
        self.await_pending_classification().await;
        self.await_pending_extraction().await;
```

- [ ] **Step 4: Run test**

Run: `~/.cargo/bin/cargo test -p primer-pedagogy close_session_drains_extractor_task`
Expected: PASS.

Full pedagogy run: `~/.cargo/bin/cargo test -p primer-pedagogy`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add src/crates/primer-pedagogy/src/dialogue_manager.rs
git commit -m "feat(pedagogy): drain extractor task in close_session"
```

---

## Task 12: CLI flags + `build_extractor` dispatch matrix

**Files:**
- Modify: `src/crates/primer-cli/Cargo.toml`
- Modify: `src/crates/primer-cli/src/main.rs`

- [ ] **Step 1: Add the dependency**

Edit `src/crates/primer-cli/Cargo.toml`. In `[dependencies]`, add:
```toml
primer-extractor.workspace = true
```
(Place alphabetically — directly after `primer-classifier.workspace = true`.)

- [ ] **Step 2: Add CLI flags**

In `src/crates/primer-cli/src/main.rs`, find the existing classifier-related CLI flags (search for `classifier_backend: Option<String>,`). Add three new flags directly after `classifier_timeout_ms`:

```rust
    /// Backend used for the concept extractor. Defaults to the same
    /// backend used for the chat (`--backend`); `stub` forces deterministic
    /// extraction (empty concepts) regardless of main backend.
    #[arg(long)]
    extractor_backend: Option<String>,

    /// Model used for the concept extractor. Defaults to the same
    /// model used for the chat (`--model`). Useful for haiku-as-extractor
    /// + sonnet-as-chat on bigger machines.
    #[arg(long)]
    extractor_model: Option<String>,

    /// Maximum time to block awaiting the previous turn's extraction
    /// before the next intent decision. Default 1500ms.
    #[arg(long, default_value_t = 1500)]
    extractor_timeout_ms: u64,
```

- [ ] **Step 3: Extend `BackendParams` and the CLI extraction**

Find:
```rust
struct BackendParams {
    api_key: Option<String>,
    ollama_url: String,
    classifier_backend: Option<String>,
    classifier_model: Option<String>,
}
```
Replace with:
```rust
struct BackendParams {
    api_key: Option<String>,
    ollama_url: String,
    classifier_backend: Option<String>,
    classifier_model: Option<String>,
    extractor_backend: Option<String>,
    extractor_model: Option<String>,
}
```

In `async fn async_main`, find:
```rust
    let backend_params = BackendParams {
        api_key: cli.api_key.clone(),
        ollama_url: cli.ollama_url.clone(),
        classifier_backend: cli.classifier_backend.clone(),
        classifier_model: cli.classifier_model.clone(),
    };
```
Replace with:
```rust
    let backend_params = BackendParams {
        api_key: cli.api_key.clone(),
        ollama_url: cli.ollama_url.clone(),
        classifier_backend: cli.classifier_backend.clone(),
        classifier_model: cli.classifier_model.clone(),
        extractor_backend: cli.extractor_backend.clone(),
        extractor_model: cli.extractor_model.clone(),
    };
```

- [ ] **Step 4: Add `build_extractor` helper**

In the imports near the top, find `use primer_classifier::{` and add directly after it:
```rust
use primer_extractor::{
    ConceptExtractor, ExtractorSettings, LlmConceptExtractor, StubConceptExtractor,
};
```

After the existing `build_classifier` function, add a sibling `build_extractor` (the dispatch matrix is identical except it uses extractor types):

```rust
/// Construct the concept extractor according to the same dispatch matrix
/// as `build_classifier`:
///
/// | main backend | --extractor-backend | --extractor-model | outcome                                |
/// |--------------|---------------------|-------------------|----------------------------------------|
/// | stub         | (unset)             | (any)             | StubConceptExtractor                   |
/// | *            | "stub"              | (any)             | StubConceptExtractor                   |
/// | *            | some(X)             | None              | error (model required for non-stub)    |
/// | *            | some(X)             | some(M)           | LlmConceptExtractor(new backend X, model M) |
/// | non-stub     | (unset)             | None              | LlmConceptExtractor(Arc::clone main)   |
/// | non-stub     | (unset)             | some(M)           | LlmConceptExtractor(new backend same type, model M) |
async fn build_extractor(
    main_backend: Arc<dyn InferenceBackend>,
    main_backend_name: &str,
    main_model: &str,
    params: &BackendParams,
    settings: ExtractorSettings,
) -> Result<Arc<dyn ConceptExtractor>> {
    match (main_backend_name, params.extractor_backend.as_deref()) {
        (_, Some("stub")) => Ok(Arc::new(StubConceptExtractor::new())),
        ("stub", None) => Ok(Arc::new(StubConceptExtractor::new())),
        (_, Some(ext_backend_name)) => {
            let model = params.extractor_model.clone().ok_or_else(|| {
                PrimerError::Inference(
                    "--extractor-model is required when --extractor-backend is set to a non-stub backend".into(),
                )
            })?;
            let ext_backend = build_backend(ext_backend_name, model.clone(), params).await?;
            Ok(Arc::new(LlmConceptExtractor::new(
                ext_backend,
                model,
                settings,
            )))
        }
        (_, None) => match params.extractor_model.as_deref() {
            None => Ok(Arc::new(LlmConceptExtractor::new(
                Arc::clone(&main_backend),
                main_model.to_string(),
                settings,
            ))),
            Some(override_model) => {
                let ext_backend =
                    build_backend(main_backend_name, override_model.to_string(), params).await?;
                Ok(Arc::new(LlmConceptExtractor::new(
                    ext_backend,
                    override_model.to_string(),
                    settings,
                )))
            }
        },
    }
}
```

- [ ] **Step 5: Wire the extractor into `DialogueManager::new`**

In `async fn async_main`, find the existing `let classifier: Arc<dyn EngagementClassifier> = match build_classifier(` block. Directly after that block (after the classifier eprintln), add:

```rust
    let extractor_settings = ExtractorSettings {
        blocking_timeout: std::time::Duration::from_millis(cli.extractor_timeout_ms),
        ..ExtractorSettings::default()
    };

    let extractor: Arc<dyn ConceptExtractor> = match build_extractor(
        Arc::clone(&backend),
        &cli.backend,
        &main_model,
        &backend_params,
        extractor_settings.clone(),
    )
    .await
    {
        Ok(e) => {
            eprintln!("Concept extractor: {}", e.identifier());
            e
        }
        Err(e) => {
            eprintln!("Error constructing concept extractor: {e}");
            std::process::exit(1);
        }
    };
```

Then update the `DialogueManager::new` call. Find:
```rust
    let mut dm = DialogueManager::new(
        learner,
        backend.as_ref(),
        &knowledge as &dyn KnowledgeBase,
        stores,
        classifier,
        classifier_settings,
        pedagogy_config,
    );
```
Replace with:
```rust
    let mut dm = DialogueManager::new(
        learner,
        backend.as_ref(),
        &knowledge as &dyn KnowledgeBase,
        stores,
        classifier,
        classifier_settings,
        extractor,
        extractor_settings,
        pedagogy_config,
    );
```

- [ ] **Step 6: Add CLI tests for `build_extractor`**

At the bottom of `main.rs`, find the `mod classifier_construction_tests` block. Below it, add a parallel `mod extractor_construction_tests`:

```rust
#[cfg(test)]
mod extractor_construction_tests {
    use super::*;

    fn params(
        extractor_backend: Option<&str>,
        extractor_model: Option<&str>,
    ) -> BackendParams {
        BackendParams {
            api_key: Some("k".into()),
            ollama_url: "http://localhost:11434".into(),
            classifier_backend: None,
            classifier_model: None,
            extractor_backend: extractor_backend.map(String::from),
            extractor_model: extractor_model.map(String::from),
        }
    }

    fn stub_main_backend() -> Arc<dyn InferenceBackend> {
        Arc::new(primer_inference::stub::StubBackend)
    }

    #[tokio::test]
    async fn stub_main_no_flags_gives_stub_extractor() {
        let e = build_extractor(
            stub_main_backend(),
            "stub",
            "stub",
            &params(None, None),
            ExtractorSettings::default(),
        )
        .await
        .unwrap();
        assert_eq!(e.identifier(), "stub");
    }

    #[tokio::test]
    async fn explicit_stub_override_wins() {
        let e = build_extractor(
            stub_main_backend(),
            "ollama",
            "llama3.2",
            &params(Some("stub"), None),
            ExtractorSettings::default(),
        )
        .await
        .unwrap();
        assert_eq!(e.identifier(), "stub");
    }

    #[tokio::test]
    async fn nonstub_backend_without_model_errors() {
        let r = build_extractor(
            stub_main_backend(),
            "ollama",
            "llama3.2",
            &params(Some("ollama"), None),
            ExtractorSettings::default(),
        )
        .await;
        assert!(r.is_err());
    }

    #[tokio::test]
    async fn no_flags_reuses_main_model_id() {
        let e = build_extractor(
            stub_main_backend(),
            "ollama",
            "llama3.2",
            &params(None, None),
            ExtractorSettings::default(),
        )
        .await
        .unwrap();
        assert_eq!(e.identifier(), "llm:llama3.2");
    }

    #[tokio::test]
    async fn model_only_override_uses_main_backend_type() {
        let e = build_extractor(
            stub_main_backend(),
            "ollama",
            "llama3.2",
            &params(None, Some("haiku")),
            ExtractorSettings::default(),
        )
        .await
        .unwrap();
        assert_eq!(e.identifier(), "llm:haiku");
    }
}
```

- [ ] **Step 7: Build and run tests**

Run: `~/.cargo/bin/cargo build --workspace`
Expected: PASS — all crates compile.

Run: `~/.cargo/bin/cargo test -p primer-cli extractor_construction_tests`
Expected: 5 passing tests.

Run full workspace: `~/.cargo/bin/cargo test --workspace`
Expected: all tests pass.

- [ ] **Step 8: Commit**

```bash
git add src/crates/primer-cli
git commit -m "feat(cli): --extractor-backend/--extractor-model/--extractor-timeout-ms flags"
```

---

## Task 13: Update `--verbose` to print extractions

**Files:**
- Modify: `src/crates/primer-pedagogy/src/dialogue_manager.rs`
- Modify: `src/crates/primer-cli/src/main.rs`

- [ ] **Step 1: Add a `last_extraction()` accessor to `DialogueManager`**

In `src/crates/primer-pedagogy/src/dialogue_manager.rs`, find the `last_assessment` method (search for `pub fn last_assessment`). Add a new field to `DialogueManager` to track the most recent applied extraction. Find the `learner_dirty: bool,` field in the struct and add directly above it:

```rust
    /// Most recent extractor output applied to the learner. Cleared on
    /// session lifecycle events. Used by `--verbose`.
    last_extraction: Option<primer_core::extractor::ConceptExtraction>,
```

In `DialogueManager::new`, set the new field's initial value. Find:
```rust
            extract_task: None,
            config,
            learner_dirty: false,
```
Replace with:
```rust
            extract_task: None,
            last_extraction: None,
            config,
            learner_dirty: false,
```

In `await_pending_extraction`, capture the extraction before applying. Find:
```rust
            Ok(Ok(Some(extraction))) => {
                if apply_extraction(&mut self.learner, &extraction) {
                    self.learner_dirty = true;
                }
            }
```
Replace with:
```rust
            Ok(Ok(Some(extraction))) => {
                if apply_extraction(&mut self.learner, &extraction) {
                    self.learner_dirty = true;
                }
                self.last_extraction = Some(extraction);
            }
```

Then add the accessor method near `last_assessment`:
```rust
    /// Most recent extractor output applied to the learner (used by `--verbose`).
    /// Returns `None` until at least one extraction has completed.
    pub fn last_extraction(&self) -> Option<&primer_core::extractor::ConceptExtraction> {
        self.last_extraction.as_ref()
    }

    /// Stable identifier of the active concept extractor (used by `--verbose`).
    pub fn extractor_identifier(&self) -> &str {
        self.extractor.identifier()
    }
```

- [ ] **Step 2: Update CLI verbose path**

In `src/crates/primer-cli/src/main.rs`, find the existing `if cli.verbose { ... [classifier] ... }` block. Add a new block after the classifier block:

```rust
        if cli.verbose {
            // ... existing intent + classifier blocks ...
            if let Some(e) = dm.last_extraction() {
                eprintln!(
                    "[extractor] child={:?} primer={:?} ({})",
                    e.child_concepts,
                    e.primer_concepts,
                    dm.extractor_identifier()
                );
            }
        }
```

(Position the block as the last item inside the existing `if cli.verbose` — directly after the classifier print, inside the same `if`.)

- [ ] **Step 3: Build + test**

Run: `~/.cargo/bin/cargo build --workspace`
Expected: PASS.

Run: `~/.cargo/bin/cargo test --workspace`
Expected: all tests pass.

- [ ] **Step 4: Smoke-test --verbose path manually**

Run from `src/`: `~/.cargo/bin/cargo run --bin primer -- --verbose --no-persist`
Type: `what is gravity?` then enter, then `quit`. The `[extractor]` line is NOT expected here (stub main → stub extractor → empty concepts). The line should appear with `child=[] primer=[] (stub)` after the second turn.

Expected: clean run, banner mentions concept extractor, no panics.

- [ ] **Step 5: Commit**

```bash
git add src/crates/primer-pedagogy/src/dialogue_manager.rs src/crates/primer-cli/src/main.rs
git commit -m "feat(cli): --verbose prints extractor output alongside classifier"
```

---

## Task 14: Update CLAUDE.md + ROADMAP.md

**Files:**
- Modify: `CLAUDE.md`
- Modify: `ROADMAP.md`

- [ ] **Step 1: Update `CLAUDE.md` architecture section**

In the "Architecture: trait-based hardware abstraction" section, find the diagram:
```
primer-cli  →  primer-pedagogy  →  primer-core  ←  primer-inference, primer-speech, primer-knowledge, primer-storage, primer-classifier
```
Replace with:
```
primer-cli  →  primer-pedagogy  →  primer-core  ←  primer-inference, primer-speech, primer-knowledge, primer-storage, primer-classifier, primer-extractor
```

Find the `**`primer-classifier`** —` paragraph. Add a new paragraph directly after it describing the extractor:

```
- **`primer-extractor`** — concept-extractor trait + implementations. `ConceptExtractor` is the trait; `LlmConceptExtractor` wraps `Arc<dyn InferenceBackend>` (separate dep so the crate stays free of `primer-inference` build dep); `StubConceptExtractor` powers tests with `with_response`/`with_script` constructors. One LLM call per completed exchange returns `{ child_concepts, primer_concepts }` separately so each turn's `concepts` field is populated correctly. Soft-fail policy: extractor errors return `ConceptExtraction::empty()` and `tracing::warn!` — never propagate. Configuration in `ExtractorSettings` (blocking_timeout, recent_context_turns, max_output_chars, max_concepts_per_speaker, generation_max_tokens, generation_temperature, generation_top_p) — every numeric backed by `consts.rs` defaults.
```

In the `primer-pedagogy` paragraph, find the sentence about classifier flow:
```
The `DialogueManager` now owns the classifier flow:
```
Add to that paragraph (or a new sub-paragraph below it):
```
The extractor flow mirrors the classifier exactly: at the end of each successful response the dialogue manager spawns `tokio::spawn(extractor.extract(...))` for the just-completed (child, primer) exchange. The task self-persists via the new `SessionStore::update_turn_concepts(session_id, turn_index, &concepts)` method (idempotent FK link into `turn_concepts`). At the start of the next turn `await_pending_extraction` waits up to `extractor_settings.blocking_timeout` (default 1500ms — looser than the classifier's 500ms because no in-turn decision depends on extracted concepts) and applies the result to in-memory `LearnerModel.concepts` (encounter_count++, depth=Aware on first sight). On timeout the task is detached so DB persistence still completes.
```

In the "Conventions and gotchas" section, add a new bullet:
```
- **Concept extraction lags one turn for in-memory state.** The extractor task spawned at the end of turn N writes to `turn_concepts` directly, but the in-memory `LearnerModel.concepts` is only updated when the next turn starts (the same await-with-bounded-timeout pattern as the classifier). This means the system prompt for turn N does not include concepts surfaced *in* turn N — only those from earlier turns. Acceptable for v1; if it ever surprises a child ("I just told you about photosynthesis and you're acting like I didn't"), the fix is to apply the extraction synchronously before save_session.
- **`update_turn_concepts` exists because `save_session` skips already-persisted turns.** The append-only invariant on `save_session` (skip turns already on disk by index) means concept additions to a previously-saved turn would be silently dropped. `update_turn_concepts` is the explicit backfill path for the post-response extractor task. Do not move concept persistence back into `save_session` — that would require changing the skip rule and breaking the append-only invariant.
```

- [ ] **Step 2: Update `ROADMAP.md`**

Find the Phase 0.3 section, find the "Concept extraction from child responses" bullet (or whatever language is used), and tick it. If the bullet is `[ ]`, change to `[x]`. If the format is "Open:" / "Done:", move it from one to the other. Inspect the file first to match its existing convention.

- [ ] **Step 3: Run final verification**

Run: `~/.cargo/bin/cargo build --workspace`
Expected: PASS.

Run: `~/.cargo/bin/cargo test --workspace`
Expected: all tests pass. Test count should be 249 + new tests (~15-20 new).

Run: `~/.cargo/bin/cargo clippy --workspace --all-targets`
Expected: clean (the existing 3 pre-existing warnings on `LoopBackends.vad` etc. are accepted; no new ones).

Run: `~/.cargo/bin/cargo fmt --check`
Expected: clean.

If `--features primer-cli/speech` builds were green before, verify they still are:
Run: `~/.cargo/bin/cargo test --workspace --features primer-cli/speech`
Expected: 16 additional speech tests pass alongside the workspace tests.

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md ROADMAP.md
git commit -m "docs: document concept extractor architecture and tick Phase 0.3 bullet"
```

---

## Final integration checks

After Task 14, run the full smoke test sequence from `src/`:

```bash
~/.cargo/bin/cargo build --workspace
~/.cargo/bin/cargo test --workspace
~/.cargo/bin/cargo clippy --workspace --all-targets
~/.cargo/bin/cargo fmt --check
```

All four must pass.

Manual REPL smoke (stub backend, no API key needed):
```bash
~/.cargo/bin/cargo run --bin primer -- --verbose --no-persist --name TestKid
# Type: "what is photosynthesis?"
# Wait for response.
# Type: "yes that's right"
# Confirm `[extractor] child=[] primer=[] (stub)` line appears on stderr
# (stub returns empty by design)
# Type: "quit"
```

If you have an `ANTHROPIC_API_KEY`, do the cloud-backend smoke too:
```bash
~/.cargo/bin/cargo run --bin primer -- --verbose --no-persist --backend cloud --model claude-sonnet-4-6 --extractor-model claude-haiku-4-5
# Same conversation. Confirm:
#  - "Concept extractor: llm:claude-haiku-4-5" banner.
#  - On the second turn, [extractor] line shows non-empty concepts for both speakers.
```

---

## Self-Review

**Spec coverage check:**
- ✅ New crate `primer-extractor` mirroring `primer-classifier` (Tasks 4–7).
- ✅ `ConceptExtractor` trait (Task 4).
- ✅ `LlmConceptExtractor` wrapping `Arc<dyn InferenceBackend>` (Task 7).
- ✅ `StubConceptExtractor` with `with_response`/`with_script` (Task 6).
- ✅ `ExtractorSettings` with consts (Task 5).
- ✅ One extraction call per exchange returning `{ child_concepts, primer_concepts }` (Task 7 prompt).
- ✅ Speaker-attribution test cases (Task 7's prompt is explicit; tests assert correct attribution against canned LLM outputs).
- ✅ Spawned async after primer save with skip-on-error (Task 9).
- ✅ Self-persistence via `update_turn_concepts` (Tasks 2, 3, 9).
- ✅ Apply at next-turn boundary with timeout (Task 10).
- ✅ Drained on `close_session` (Task 11).
- ✅ CLI flags `--extractor-backend/--extractor-model/--extractor-timeout-ms` (Task 12).
- ✅ `--verbose` shows extractor output (Task 13).
- ✅ Documentation (Task 14).

**Type consistency:** `ConceptExtractor::extract` returns `Result<ConceptExtraction>`; `ExtractionContext` has `child_turn`, `primer_turn`, `recent_turns` (matched everywhere); `apply_extraction` takes `&ConceptExtraction`; CLI helper named `build_extractor` consistently.

**Placeholder scan:** None. Every step has concrete code or commands.

---

## Open decisions / risks (called out for executor)

1. **Test count regression on cargo features.** The existing tests in `dialogue_manager.rs` will need their `DialogueManager::new` call sites updated (Task 8 step 3). If the executor misses any, the build fails. Search exhaustively with `grep -rn "DialogueManager::new"` to confirm coverage.

2. **`StubConceptExtractor::with_script` exhaustion behaviour.** When the script is exhausted, the stub falls back to the empty `ConceptExtraction`. Match the classifier's behaviour for parity.

3. **`apply_extraction` is currently public-within-crate (`pub(crate)`).** If a future external consumer wants to apply an extraction outside the dialogue manager, raise visibility deliberately — don't change to `pub` reflexively.

4. **`ScriptedBackend.set_script` helper added in Task 10.** This change is isolated to the test file and only used by the new test. If pre-existing tests already expose script mutation differently, prefer the existing path.

5. **`extractor_timeout_ms` defaults to 1500 (looser than classifier's 500).** Rationale: no in-turn decision currently depends on extracted concepts. If profiling later shows the timeout fires often, lower it; if a downstream feature (comprehension classifier) starts depending on concepts being applied before intent decision, raise the default.
