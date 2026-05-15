# Hindi Locale (Preview) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `Locale::Hindi` as a developer-only preview locale: enum variant + machine-translated prompt pack + voice defaults + docs. Excluded from `Locale::ALL` and tagged `[meta] status = "preview"` so end users never reach it via the CLI/GUI picker, and a future native-speaker-review PR can flip both gates in one commit.

**Architecture:** Add a new optional `[meta] status` field to prompt packs with closed enum `PackStatus { Stable, Preview }`. Loader emits a one-time `tracing::warn!` per preview locale on first cached load. `Locale::Hindi` is added with all match-arm cascades (i18n.rs, prompt_pack.rs, render_inference_error) but `Locale::ALL` stays `[English, German]`. Voice defaults point at `hi_IN-rohan-medium` + multilingual `ggml-small.bin`. No corpus / retrieval / GUI changes.

**Tech Stack:** Rust 1.88 (workspace pinned), edition 2024. `serde` + `toml` for pack deserialisation. `tracing` for warn-once. `std::sync::OnceLock` for per-locale caching. No new deps.

**Spec:** [`docs/superpowers/specs/2026-05-15-hindi-locale-preview-rollout.md`](../specs/2026-05-15-hindi-locale-preview-rollout.md)

**Branch:** `feat/locale-hindi-preview` (already created off `main` at `dae68d5`, with the spec commit `4a1d566` on top).

---

## File map

**Modify:**
- `src/crates/primer-core/src/i18n.rs` — `Hindi` variant, all match arms, `render_hindi`, new tests
- `src/crates/primer-pedagogy/src/prompt_pack.rs` — `PackStatus` enum, `[meta] status` deserialisation, `MetaSection.status`, `PromptPack::status()` accessor, `embedded_pack` Hindi arm, `load_cached` Hindi arm, warn-once `OnceLock<HashSet>`, validator extension, new tests
- `src/crates/primer-speech/src/voice_loop/locale_defaults.rs` — Hindi tuple + test
- `CLAUDE.md` — preview-status convention gotcha paragraph
- `docs/localisation/README.md` — entry in the "Supported locales today" table for `hi` (status: preview)

**Create:**
- `src/crates/primer-pedagogy/prompts/hi.toml` — Hindi prompt pack (~210 lines, Devanagari content)
- `docs/localisation/hi/README.md` — Hindi locale status page
- `docs/locale/models/HINDI.md` — Hindi model-evaluation skeleton

**Do NOT touch:** any other crate, any test under `primer-kb-load/tests`, `data/seed/`, `data/ingest/`, the GUI/CLI surfaces beyond the locale picker (which derives from `Locale::ALL` automatically — no code change needed).

---

## Task 1: Add `PackStatus` enum + `[meta] status` field to the validator (no Hindi yet)

**Goal of this task:** introduce the new field structurally so we have a clean validator + accessor independent of the Hindi rollout. Existing `en` / `de` packs default to `Stable` because the field is absent in their TOML files. Tests pin that default and the rejection of unknown values.

**Files:**
- Modify: `src/crates/primer-pedagogy/src/prompt_pack.rs` (multiple sites; see steps)

- [ ] **Step 1: Add the failing test (Stable default for existing packs)**

Open `src/crates/primer-pedagogy/src/prompt_pack.rs`. Inside the `#[cfg(test)] mod tests { ... }` block (after the existing `english_pack_meta_matches_locale_projections` test), add:

```rust
    #[test]
    fn pack_status_defaults_to_stable_when_field_absent_for_english() {
        let pack = english_pack();
        assert_eq!(pack.status(), PackStatus::Stable);
    }

    #[test]
    fn pack_status_defaults_to_stable_when_field_absent_for_german() {
        let pack = german_pack();
        assert_eq!(pack.status(), PackStatus::Stable);
    }

    #[test]
    fn pack_status_explicit_stable_loads_as_stable() {
        let body = synthetic_pack_body_with_status(
            "en", "English", "en-US", "[]", Some("stable"),
        );
        let pack = TomlPromptPack::from_toml_str(Locale::English, &body)
            .expect("explicit status=stable should load");
        assert_eq!(pack.status(), PackStatus::Stable);
    }

    #[test]
    fn pack_status_explicit_preview_loads_as_preview() {
        let body = synthetic_pack_body_with_status(
            "en", "English", "en-US", "[]", Some("preview"),
        );
        let pack = TomlPromptPack::from_toml_str(Locale::English, &body)
            .expect("explicit status=preview should load");
        assert_eq!(pack.status(), PackStatus::Preview);
    }

    #[test]
    fn pack_status_rejects_unknown_value() {
        let body = synthetic_pack_body_with_status(
            "en", "English", "en-US", "[]", Some("wip"),
        );
        let err = TomlPromptPack::from_toml_str(Locale::English, &body)
            .err()
            .expect("expected unknown-status error");
        let s = format!("{err}");
        assert!(s.contains("status"), "got: {s}");
        assert!(s.contains("wip"), "got: {s}");
    }
```

Also extend the existing `synthetic_pack_body` helper into a new helper `synthetic_pack_body_with_status` (keep `synthetic_pack_body` as a thin wrapper). Append at the bottom of the test module, after the existing `synthetic_pack_body` function:

```rust
    /// Variant of `synthetic_pack_body` that lets callers inject a
    /// `[meta] status = "..."` line. `None` omits the line entirely
    /// (verifies the "absent => Stable" default).
    fn synthetic_pack_body_with_status(
        meta_language: &str,
        meta_language_name: &str,
        meta_bcp47: &str,
        factual_prefixes_array: &str,
        status: Option<&str>,
    ) -> String {
        let status_line = match status {
            Some(s) => format!("status = \"{s}\"\n"),
            None => String::new(),
        };
        format!(
            r#"
[meta]
language = "{meta_language}"
language_name = "{meta_language_name}"
bcp47 = "{meta_bcp47}"
{status_line}
[system_prompt]
base = "x"

[language_guidance]
ages_0_6 = ""
ages_7_9 = ""
ages_10_12 = ""
ages_13_plus = ""

[intent]
{INTENT_KEYS}

[engagement]
frustrated = ""
disengaging = ""

[sections]
knowledge_intro = ""
summary_intro = ""
retrieved_intro = ""
vocab_review_intro = ""
break_suggestion_intro = ""

[labels]
child = "Child"
primer = "Primer"

[question_detection]
factual_prefixes = {factual_prefixes_array}

[voice_state]
listen_label = "x"
listen_hint = "x"
thinking_label = "x"
thinking_hint = "x"
speak_label = "x"
speak_hint = "x"
"#,
            INTENT_KEYS = all_intents_zeroed_toml(),
        )
    }
```

- [ ] **Step 2: Run the tests; expect failures because `PackStatus` and `status()` don't exist yet**

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo test -p primer-pedagogy --lib prompt_pack::tests 2>&1 | tail -40
```

Expected: compile error citing missing `PackStatus`, missing `pack.status()`. That confirms the tests are wired to the new surface.

- [ ] **Step 3: Add the `PackStatus` type + serde plumbing + accessor**

In `src/crates/primer-pedagogy/src/prompt_pack.rs`:

1. **Above the `pub trait PromptPack` definition** (around line 36, immediately after the existing `use` block but before `pub trait PromptPack`), add the public enum:

```rust
/// Lifecycle status of a prompt pack. Set explicitly in `[meta] status`
/// or implicitly absent (which means `Stable`). The loader emits a
/// one-time warning per `(process, locale)` pair on `Preview` packs to
/// flag that the strings have not been through native-speaker review.
///
/// Allow-list: only `"stable"` and `"preview"` are accepted as TOML
/// values. Adding a new variant is a deliberate, two-place change:
/// the enum and the validator's `from_toml_str` arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackStatus {
    Stable,
    Preview,
}

impl PackStatus {
    /// Parse the optional `[meta] status` value into the enum.
    /// Absence (`None`) and `"stable"` both map to `Stable`; only
    /// `"preview"` maps to `Preview`. Any other string is a load-time
    /// error so the validator catches typos.
    fn from_meta(raw: Option<&str>) -> Result<Self> {
        match raw {
            None | Some("stable") => Ok(Self::Stable),
            Some("preview") => Ok(Self::Preview),
            Some(other) => Err(PrimerError::Config(format!(
                "prompt pack: unknown [meta] status {other:?}; allowed: \"stable\", \"preview\""
            ))),
        }
    }
}
```

2. **Inside `pub trait PromptPack`** (after `fn voice_state_labels(&self) -> &VoiceStateLabels;`), add the new trait method:

```rust
    /// Lifecycle status of this pack. `Stable` for packs reviewed by a
    /// native speaker (the default when `[meta] status` is absent).
    /// `Preview` for machine-translated content awaiting review — the
    /// loader emits a one-time warning when these load.
    fn status(&self) -> PackStatus;
```

3. **Inside `struct TomlPromptPack`** (the `pub struct TomlPromptPack` definition), add a new field:

```rust
    status: PackStatus,
```

Place it as the last field of the struct, immediately after `voice_state_labels: VoiceStateLabels,`.

4. **In `struct MetaSection`** (around line 470), add the optional field:

```rust
#[derive(Deserialize)]
struct MetaSection {
    language: String,
    language_name: String,
    bcp47: String,
    #[serde(default)]
    status: Option<String>,
}
```

5. **In `TomlPromptPack::from_toml_str`**, after the existing meta-consistency checks (the three `bcp47`/`language_name`/`language` checks ending around line 238) and before the placeholder validation block, parse the status:

```rust
        let status = PackStatus::from_meta(raw.meta.status.as_deref())?;
```

6. **In the final `Ok(Self { ... })` construction** (around line 350-373), add the new field:

```rust
        Ok(Self {
            locale,
            // ... existing fields unchanged ...
            voice_state_labels: VoiceStateLabels {
                // ... existing voice-state fields unchanged ...
            },
            status,
        })
```

7. **In `impl PromptPack for TomlPromptPack`**, after `fn voice_state_labels(&self) -> &VoiceStateLabels { &self.voice_state_labels }` (around line 445-447), add:

```rust
    fn status(&self) -> PackStatus {
        self.status
    }
```

- [ ] **Step 4: Run the tests; expect them to pass now**

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo test -p primer-pedagogy --lib prompt_pack::tests 2>&1 | tail -20
```

Expected: all `pack_status_*` tests pass. All existing pack tests still pass.

- [ ] **Step 5: Full workspace build + test to confirm no downstream breakage**

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo build --workspace 2>&1 | tail -5
~/.cargo/bin/cargo test --workspace 2>&1 | tail -10
```

Expected: clean build. Test count increases by exactly 5 (777 → 782; 0 failed, 3 ignored). If any consumer crate (engine, cli, gui) fails because `PromptPack` is a non-object-safe trait now → revert and rethink. (It is object-safe today — `Send + Sync` plus methods that only take `&self` — and the new `fn status(&self) -> PackStatus` preserves object-safety because `PackStatus: Copy`. Verify by running the GUI crate's tests.)

- [ ] **Step 6: Commit**

```bash
cd /Users/hherb/src/primer
git add src/crates/primer-pedagogy/src/prompt_pack.rs
git commit -m "$(cat <<'EOF'
feat(prompt-pack): add PackStatus + [meta] status field

New optional [meta] status field on prompt packs; allow-list of
"stable" and "preview"; absent => Stable. Closed enum PackStatus on
the PromptPack trait. Sets up the firewall the Hindi preview locale
will land behind in a follow-up commit.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Add warn-once-per-preview-locale tracing infrastructure (still no Hindi)

**Goal:** make `load_cached` emit exactly one `tracing::warn!` event per `(process, locale)` pair on `Preview` packs. Direct `load()` calls re-emit; only the cached path is rate-limited. Use a `OnceLock<Mutex<HashSet<Locale>>>` keyed by locale.

**Files:**
- Modify: `src/crates/primer-pedagogy/src/prompt_pack.rs`
- Modify: `src/crates/primer-pedagogy/Cargo.toml` (only if `tracing-test` not already a dev-dep)

- [ ] **Step 1: Check tracing-test availability**

```bash
cd /Users/hherb/src/primer/src
grep -n "tracing-test\|tracing_test" crates/primer-pedagogy/Cargo.toml
```

If `tracing-test` is not listed as a dev-dep, search the workspace for another crate using it:

```bash
grep -rn "tracing-test\|tracing_test" crates/*/Cargo.toml
```

Expected: at least one crate uses it (the storage / classifier / etc. paths have tracing tests). If none uses it, fall back to the manual-subscriber pattern in step 4. Otherwise, add to `[dev-dependencies]` in `primer-pedagogy/Cargo.toml`:

```toml
tracing-test = "0.2"
```

Actually first check what version other crates use to keep the workspace consistent — match it. If no other crate uses it, this task can use a hand-rolled `tracing::subscriber::with_default(...)` approach instead. Use whatever is already in the workspace.

- [ ] **Step 2: Add the failing tests (warn-once semantics)**

In `prompt_pack.rs` test module, after the new `pack_status_*` tests added in Task 1, add:

```rust
    #[test]
    fn preview_warning_emits_once_per_locale_on_load_cached() {
        // Use a captured-tracing subscriber to count events. Reset the
        // per-locale warn-once gate via the test-only helper so this test
        // is order-independent.
        use tracing::{subscriber::with_default, Level};
        use std::sync::{Arc, Mutex};

        #[derive(Default)]
        struct Counter(Arc<Mutex<usize>>);

        impl<S> tracing_subscriber::Layer<S> for Counter
        where
            S: tracing::Subscriber,
        {
            fn on_event(
                &self,
                event: &tracing::Event<'_>,
                _ctx: tracing_subscriber::layer::Context<'_, S>,
            ) {
                let m = event.metadata();
                if m.level() == &Level::WARN && m.target() == "primer::prompt_pack" {
                    *self.0.lock().unwrap() += 1;
                }
            }
        }

        // Skip the strict assertion under PRIMER_PROMPTS_DIR — load_cached
        // bypasses the cache there, so the warn-once gate has nothing to
        // gate.
        if std::env::var_os("PRIMER_PROMPTS_DIR").is_some() {
            return;
        }

        // Reset the warn-once gate for the test locale so we measure
        // *this* test's emissions, not a previous test's residue.
        reset_preview_warn_once_for_test(Locale::English);

        let count = Arc::new(Mutex::new(0usize));
        let layer = Counter(Arc::clone(&count));
        use tracing_subscriber::prelude::*;
        let subscriber = tracing_subscriber::registry().with(layer);

        with_default(subscriber, || {
            // We need a Preview English pack to exercise the warn path,
            // but the embedded EN pack is Stable. Easiest path: bypass
            // the cache via from_toml_str on a synthetic preview body
            // and emit the warning manually through the helper we'll
            // expose for this test.
            // -- This test asserts the helper's idempotence.
            emit_preview_warning_if_first_for_test(Locale::English);
            emit_preview_warning_if_first_for_test(Locale::English);
            emit_preview_warning_if_first_for_test(Locale::English);
        });

        assert_eq!(
            *count.lock().unwrap(),
            1,
            "expected exactly one warn event for repeated preview emits"
        );
    }
```

This test uses two helpers we'll add in step 3: `reset_preview_warn_once_for_test` and `emit_preview_warning_if_first_for_test`. Both are `#[cfg(test)] pub(super)` so they live next to the production helper but are only callable from tests.

If `tracing-subscriber` is not in dev-deps, add it (workspace dep). Run:

```bash
grep -n "tracing-subscriber\|tracing_subscriber" crates/primer-pedagogy/Cargo.toml
```

If absent, add to `[dev-dependencies]`:

```toml
tracing-subscriber = { version = "0.3", features = ["registry"] }
```

(Match workspace version if `Cargo.lock` shows a different one. As of branch base, `tracing-subscriber` is already a workspace dep in `Cargo.toml` — propagate the workspace version.)

- [ ] **Step 3: Implement the warn-once gate**

In `prompt_pack.rs`, near the top of the file (after the existing `use std::sync::{Arc, OnceLock};` line), expand the imports:

```rust
use std::collections::HashSet;
use std::sync::{Arc, Mutex, OnceLock};
```

After the `embedded_pack` function (around line 109), add:

```rust
/// Per-process gate: tracks which preview locales have already emitted
/// their one-time warning. Populated by `emit_preview_warning_if_first`;
/// consulted by `load_cached`.
fn preview_warned_gate() -> &'static Mutex<HashSet<Locale>> {
    static GATE: OnceLock<Mutex<HashSet<Locale>>> = OnceLock::new();
    GATE.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Emit the preview-status warning for `locale` if it hasn't been emitted
/// before in this process. Idempotent across calls; the warning text
/// names the locale's pack_id so `tail -f` of logs can tell apart
/// concurrent preview locales.
fn emit_preview_warning_if_first(locale: Locale) {
    let mut seen = preview_warned_gate().lock().expect("preview gate mutex poisoned");
    if seen.insert(locale) {
        tracing::warn!(
            target: "primer::prompt_pack",
            locale = locale.pack_id(),
            "prompt pack is in preview status — machine-translated content awaiting native-speaker review. \
             This locale is not in Locale::ALL and is not advertised to end users."
        );
    }
}

#[cfg(test)]
pub(super) fn reset_preview_warn_once_for_test(locale: Locale) {
    let mut seen = preview_warned_gate().lock().expect("preview gate mutex poisoned");
    seen.remove(&locale);
}

#[cfg(test)]
pub(super) fn emit_preview_warning_if_first_for_test(locale: Locale) {
    emit_preview_warning_if_first(locale);
}
```

Now wire the warn into `load_cached`. Modify the body of `pub fn load_cached(locale: Locale) -> Result<Arc<dyn PromptPack>>` (currently around line 149-176) so each match arm emits the warning **after** the cache has populated (so a Preview pack only warns on the *first* cached hit, not on every retrieval). Change the structure to:

```rust
pub fn load_cached(locale: Locale) -> Result<Arc<dyn PromptPack>> {
    if std::env::var_os("PRIMER_PROMPTS_DIR").is_some() {
        return load(locale);
    }
    static EN_PACK: OnceLock<Arc<dyn PromptPack>> = OnceLock::new();
    static DE_PACK: OnceLock<Arc<dyn PromptPack>> = OnceLock::new();
    let pack = match locale {
        Locale::English => {
            if let Some(p) = EN_PACK.get() {
                Arc::clone(p)
            } else {
                let p = load(locale)?;
                let _ = EN_PACK.set(Arc::clone(&p));
                p
            }
        }
        Locale::German => {
            if let Some(p) = DE_PACK.get() {
                Arc::clone(p)
            } else {
                let p = load(locale)?;
                let _ = DE_PACK.set(Arc::clone(&p));
                p
            }
        }
    };
    if pack.status() == PackStatus::Preview {
        emit_preview_warning_if_first(locale);
    }
    Ok(pack)
}
```

Note: this refactor keeps the match arms exhaustive for current `Locale` values; Task 3 will add the `Locale::Hindi` arm.

- [ ] **Step 4: Run the warn-once test**

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo test -p primer-pedagogy --lib prompt_pack::tests::preview_warning_emits_once_per_locale_on_load_cached 2>&1 | tail -20
```

Expected: PASS. If the test fails because `tracing-subscriber` features differ, fall back to `tracing::subscriber::set_default(...)` instead of `with_default(...)` — same semantics, different API surface.

- [ ] **Step 5: Full workspace build + test**

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo build --workspace 2>&1 | tail -5
~/.cargo/bin/cargo test --workspace 2>&1 | tail -10
```

Expected: clean build. +1 test (782 → 783).

- [ ] **Step 6: Commit**

```bash
cd /Users/hherb/src/primer
git add src/crates/primer-pedagogy/src/prompt_pack.rs src/crates/primer-pedagogy/Cargo.toml
git commit -m "$(cat <<'EOF'
feat(prompt-pack): warn-once-per-preview-locale on load_cached

Per-process OnceLock<Mutex<HashSet<Locale>>> tracks which preview
locales have already emitted their one-time warning. Direct load()
calls bypass the gate (test-time + translator-iteration paths) so
PRIMER_PROMPTS_DIR-driven workflows aren't muted.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Add `Locale::Hindi` variant + `render_hindi` + Hindi prompt pack

**Goal:** add the enum variant with all the cascade arms (i18n.rs match arms, prompt_pack.rs `embedded_pack` + `load_cached`), the Hindi `render_inference_error` function, and the full machine-translated `prompts/hi.toml`. `Locale::ALL` stays at `[English, German]`.

This task is intentionally larger than Tasks 1 and 2 because the four sites (`Locale` enum match arms, `embedded_pack`, `load_cached`, `render_inference_error`) form a single exhaustive-match cycle and cannot be split across commits without breaking compilation.

**Files:**
- Modify: `src/crates/primer-core/src/i18n.rs`
- Modify: `src/crates/primer-pedagogy/src/prompt_pack.rs`
- Create: `src/crates/primer-pedagogy/prompts/hi.toml`

- [ ] **Step 1: Add the failing tests in `i18n.rs` (enum projections + Devanagari error strings)**

Open `src/crates/primer-core/src/i18n.rs`. Inside the `#[cfg(test)] mod tests { ... }` block, append:

```rust
    #[test]
    fn locale_hindi_pack_id_and_bcp47() {
        assert_eq!(Locale::Hindi.pack_id(), "hi");
        assert_eq!(Locale::Hindi.bcp47(), "hi-IN");
        assert_eq!(Locale::Hindi.name(), "Hindi");
        assert_eq!(Locale::from_pack_id("hi"), Some(Locale::Hindi));
    }

    /// Hindi is gated as a preview locale: present in the enum, available
    /// via --language hi for developers, but excluded from Locale::ALL so
    /// CLI/GUI pickers don't surface it to end users. Flipping ALL is the
    /// native-speaker-review PR.
    #[test]
    fn locale_all_excludes_hindi_until_translation_reviewed() {
        assert_eq!(Locale::ALL.len(), 2);
        assert!(!Locale::ALL.contains(&Locale::Hindi));
    }

    /// Each Hindi inference-error variant returns a non-empty string with
    /// at least one Devanagari character. Guards against accidental
    /// English fall-through.
    #[test]
    fn hindi_inference_errors_contain_devanagari() {
        use std::time::Duration;
        let cases: Vec<InferenceError> = vec![
            InferenceError::Auth,
            InferenceError::RateLimited { retry_after: None },
            InferenceError::RateLimited {
                retry_after: Some(Duration::from_secs(1)),
            },
            InferenceError::RateLimited {
                retry_after: Some(Duration::from_secs(5)),
            },
            InferenceError::ServiceUnavailable,
            InferenceError::NetworkUnavailable,
            InferenceError::ModelNotFound {
                model: "llama3.2".into(),
            },
            InferenceError::Other("RAW_DEV_STRING".into()),
        ];
        for err in cases {
            let s = render_inference_error(&err, &Locale::Hindi);
            assert!(!s.is_empty(), "empty render_hindi for {err:?}");
            assert!(
                s.chars().any(|c| ('\u{0900}'..='\u{097F}').contains(&c)),
                "no Devanagari in render_hindi for {err:?}: {s}"
            );
        }
    }

    #[test]
    fn hindi_model_not_found_includes_model_name() {
        let s = render_inference_error(
            &InferenceError::ModelNotFound {
                model: "llama3.2".into(),
            },
            &Locale::Hindi,
        );
        assert!(s.contains("llama3.2"), "got: {s}");
    }

    #[test]
    fn hindi_other_does_not_leak_inner_dev_string() {
        let s = render_inference_error(
            &InferenceError::Other("RAW_DEV_STRING_FOO".into()),
            &Locale::Hindi,
        );
        assert!(
            !s.contains("RAW_DEV_STRING_FOO"),
            "Other's inner string must not reach users; got: {s}"
        );
    }
```

- [ ] **Step 2: Run the i18n tests; expect compile failure on `Locale::Hindi`**

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo test -p primer-core --lib i18n::tests 2>&1 | tail -20
```

Expected: compile errors on `Locale::Hindi`, `from_pack_id("hi")`, etc.

- [ ] **Step 3: Add `Locale::Hindi` and the four match arms in `i18n.rs`**

In `src/crates/primer-core/src/i18n.rs`:

1. **Extend the enum** (around line 43-48). Replace:

```rust
pub enum Locale {
    #[default]
    English,
    German,
}
```

with:

```rust
pub enum Locale {
    #[default]
    English,
    German,
    /// Preview locale — present in the enum, accessible via `from_pack_id("hi")`,
    /// but deliberately excluded from `Locale::ALL` until a native speaker
    /// reviews the machine-translated prompt pack. CLI/GUI pickers iterate
    /// `Locale::ALL`, so end users never reach this until that review lands.
    /// See `docs/localisation/hi/README.md`.
    Hindi,
}
```

2. **Leave `Locale::ALL` unchanged** (still `&[Self::English, Self::German]`). Add a doc comment if you want, but the test in step 1 already pins the behaviour.

3. **Extend `name()`** (around line 58):

```rust
    pub fn name(self) -> &'static str {
        match self {
            Self::English => "English",
            Self::German => "German",
            Self::Hindi => "Hindi",
        }
    }
```

4. **Extend `bcp47()`** (around line 68):

```rust
    pub fn bcp47(self) -> &'static str {
        match self {
            Self::English => "en-US",
            Self::German => "de-DE",
            Self::Hindi => "hi-IN",
        }
    }
```

5. **Extend `pack_id()`** (around line 79):

```rust
    pub fn pack_id(self) -> &'static str {
        match self {
            Self::English => "en",
            Self::German => "de",
            Self::Hindi => "hi",
        }
    }
```

6. **Extend `from_pack_id`** (around line 89):

```rust
    pub fn from_pack_id(s: &str) -> Option<Self> {
        match s {
            "en" => Some(Self::English),
            "de" => Some(Self::German),
            "hi" => Some(Self::Hindi),
            _ => None,
        }
    }
```

7. **Extend `render_inference_error`** (around line 100):

```rust
pub fn render_inference_error(err: &InferenceError, locale: &Locale) -> String {
    match locale {
        Locale::English => render_english(err),
        Locale::German => render_german(err),
        Locale::Hindi => render_hindi(err),
    }
}
```

8. **Add `render_hindi`** immediately after `render_german` (end of file before `#[cfg(test)] mod tests`). The seconds-singular vs plural arm mirrors the German style. `सेकंड` is the Hindi for "second(s)" — Hindi doesn't grammatically inflect for singular/plural in the same way German does; both `1 सेकंड` and `5 सेकंड` are correct. Keep the cosmetic split anyway in case future tone-tuning wants different phrasings.

```rust
/// Hindi rendering. Uses the informal `तुम` register — consistent with
/// the hi.toml prompt-pack precedent and the broader children's-product
/// convention of avoiding the formal `आप`. Marked preview-quality: this
/// content is machine-translated, awaiting native-speaker review.
fn render_hindi(err: &InferenceError) -> String {
    use InferenceError::*;
    match err {
        Auth => {
            "प्रमाणीकरण विफल। कृपया अपने .env या ~/.primer_env में ANTHROPIC_API_KEY की जाँच करो।"
                .into()
        }
        RateLimited {
            retry_after: Some(d),
        } => {
            let secs = d.as_secs();
            if secs == 1 {
                "सेवा अभी व्यस्त है। कृपया 1 सेकंड बाद दोबारा कोशिश करो।".into()
            } else {
                format!("सेवा अभी व्यस्त है। कृपया {secs} सेकंड बाद दोबारा कोशिश करो।")
            }
        }
        RateLimited { retry_after: None } => {
            "सेवा अभी व्यस्त है। कृपया थोड़ी देर बाद दोबारा कोशिश करो।".into()
        }
        ServiceUnavailable => {
            "सेवा अस्थायी रूप से उपलब्ध नहीं है। कृपया थोड़ी देर बाद दोबारा कोशिश करो।".into()
        }
        NetworkUnavailable => {
            "सेवा तक पहुँचा नहीं जा सका। कृपया अपना नेटवर्क कनेक्शन जाँचो।".into()
        }
        ModelNotFound { model } => {
            format!("मॉडल '{model}' उपलब्ध नहीं है। Ollama के लिए `ollama pull {model}` चलाओ।")
        }
        Other(_) => {
            "कुछ अप्रत्याशित हुआ। कृपया फिर से कोशिश करो। (विवरण लॉग में हैं।)".into()
        }
    }
}
```

- [ ] **Step 4: Run i18n tests**

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo test -p primer-core --lib i18n::tests 2>&1 | tail -20
```

Expected: all new Hindi tests pass. Existing tests still pass.

- [ ] **Step 5: Add the failing tests in `prompt_pack.rs` (Hindi pack loads in Preview)**

Open `src/crates/primer-pedagogy/src/prompt_pack.rs`. In the test module, after the Task-1 tests:

```rust
    fn hindi_pack() -> Arc<dyn PromptPack> {
        load(Locale::Hindi).expect("hindi pack loads")
    }

    #[test]
    fn hindi_pack_loads_in_preview_status() {
        let pack = hindi_pack();
        assert_eq!(pack.locale(), Locale::Hindi);
        assert_eq!(pack.status(), PackStatus::Preview);
    }

    #[test]
    fn hindi_pack_intent_lookups_all_populated() {
        let pack = hindi_pack();
        for &intent in ALL_INTENTS {
            let s = pack.intent_instruction(intent);
            assert!(!s.is_empty(), "missing instruction for {intent:?}");
            assert!(
                s.chars().any(|c| ('\u{0900}'..='\u{097F}').contains(&c)),
                "no Devanagari in intent {intent:?}: {s}"
            );
        }
    }

    #[test]
    fn hindi_pack_voice_state_section_complete() {
        let pack = hindi_pack();
        let labels = pack.voice_state_labels();
        for (name, value) in [
            ("listen_label", &labels.listen_label),
            ("listen_hint", &labels.listen_hint),
            ("thinking_label", &labels.thinking_label),
            ("thinking_hint", &labels.thinking_hint),
            ("speak_label", &labels.speak_label),
            ("speak_hint", &labels.speak_hint),
        ] {
            assert!(!value.is_empty(), "voice_state.{name} is empty");
            assert!(
                value.chars().any(|c| ('\u{0900}'..='\u{097F}').contains(&c)),
                "no Devanagari in voice_state.{name}: {value}"
            );
        }
    }

    #[test]
    fn hindi_pack_renders_base_with_name_and_age() {
        let pack = hindi_pack();
        let s = pack.render_base("Aarav", 8);
        assert!(s.contains("Aarav"), "got: {s}");
        assert!(s.contains("8"), "got: {s}");
        assert!(
            !s.contains("{name}") && !s.contains("{age}") && !s.contains("{language_guidance}"),
            "all placeholders substituted: {s}"
        );
    }

    #[test]
    fn hindi_pack_knowledge_intro_substitutes_age() {
        let pack = hindi_pack();
        let s = pack.knowledge_intro(8);
        assert!(s.contains("8"), "got: {s}");
        assert!(!s.contains("{age}"));
    }

    #[test]
    fn hindi_pack_break_suggestion_intro_substitutes_minutes() {
        let pack = hindi_pack();
        let s = pack.break_suggestion_intro(30);
        assert!(s.contains("30"), "got: {s}");
        assert!(!s.contains("{minutes}"));
    }

    /// Calling load_cached(Hindi) twice in the same process must emit
    /// exactly one warn event (target = "primer::prompt_pack"). The Hindi
    /// pack is the only Preview locale at the time of this test, so any
    /// warn at that target during repeated loads is the gated event.
    #[test]
    fn hindi_load_cached_warns_exactly_once_per_locale() {
        use tracing::{subscriber::with_default, Level};
        use std::sync::{Arc, Mutex};

        #[derive(Default)]
        struct Counter(Arc<Mutex<usize>>);

        impl<S> tracing_subscriber::Layer<S> for Counter
        where
            S: tracing::Subscriber,
        {
            fn on_event(
                &self,
                event: &tracing::Event<'_>,
                _ctx: tracing_subscriber::layer::Context<'_, S>,
            ) {
                let m = event.metadata();
                if m.level() == &Level::WARN && m.target() == "primer::prompt_pack" {
                    *self.0.lock().unwrap() += 1;
                }
            }
        }

        if std::env::var_os("PRIMER_PROMPTS_DIR").is_some() {
            return;
        }

        reset_preview_warn_once_for_test(Locale::Hindi);

        let count = Arc::new(Mutex::new(0usize));
        let layer = Counter(Arc::clone(&count));
        use tracing_subscriber::prelude::*;
        let subscriber = tracing_subscriber::registry().with(layer);

        with_default(subscriber, || {
            let _ = load_cached(Locale::Hindi).expect("first load_cached");
            let _ = load_cached(Locale::Hindi).expect("second load_cached");
            let _ = load_cached(Locale::Hindi).expect("third load_cached");
        });

        assert_eq!(
            *count.lock().unwrap(),
            1,
            "expected exactly one warn for repeated load_cached(Hindi)"
        );
    }
```

- [ ] **Step 6: Add the `embedded_pack` Hindi arm + `load_cached` Hindi arm in prompt_pack.rs**

Replace `const EN_TOML` block (around line 101-102) to add the Hindi constant:

```rust
const EN_TOML: &str = include_str!("../prompts/en.toml");
const DE_TOML: &str = include_str!("../prompts/de.toml");
const HI_TOML: &str = include_str!("../prompts/hi.toml");
```

Extend `embedded_pack`:

```rust
fn embedded_pack(locale: Locale) -> &'static str {
    match locale {
        Locale::English => EN_TOML,
        Locale::German => DE_TOML,
        Locale::Hindi => HI_TOML,
    }
}
```

Extend `load_cached`. The body refactored in Task 2 now needs an HI arm. Replace the entire `match locale` block in `load_cached`:

```rust
    static EN_PACK: OnceLock<Arc<dyn PromptPack>> = OnceLock::new();
    static DE_PACK: OnceLock<Arc<dyn PromptPack>> = OnceLock::new();
    static HI_PACK: OnceLock<Arc<dyn PromptPack>> = OnceLock::new();
    let pack = match locale {
        Locale::English => {
            if let Some(p) = EN_PACK.get() {
                Arc::clone(p)
            } else {
                let p = load(locale)?;
                let _ = EN_PACK.set(Arc::clone(&p));
                p
            }
        }
        Locale::German => {
            if let Some(p) = DE_PACK.get() {
                Arc::clone(p)
            } else {
                let p = load(locale)?;
                let _ = DE_PACK.set(Arc::clone(&p));
                p
            }
        }
        Locale::Hindi => {
            if let Some(p) = HI_PACK.get() {
                Arc::clone(p)
            } else {
                let p = load(locale)?;
                let _ = HI_PACK.set(Arc::clone(&p));
                p
            }
        }
    };
    if pack.status() == PackStatus::Preview {
        emit_preview_warning_if_first(locale);
    }
    Ok(pack)
```

The cargo build will fail at the `include_str!` until step 7 lands `hi.toml`. That's expected — that's the TDD loop.

- [ ] **Step 7: Create `src/crates/primer-pedagogy/prompts/hi.toml`**

Create the file with this exact content. The placeholder `{minutes}` substitution token is wired through `break_suggestion_intro`; the Hindi noun for "minute(s)" is `मिनट` (invariant for singular and plural — Hindi nouns don't inflect for English-style number in this construction; "1 मिनट" and "30 मिनट" are both correct).

```toml
# हिन्दी प्रॉम्प्ट-पैक — Locale::Hindi (PREVIEW)
#
# This file is in PREVIEW status. The content is machine-translated and
# awaiting native-speaker review. Lines flagged "# REVIEW:" mark blocks
# that especially need a Hindi speaker's eyes.
#
# Address: the child is addressed as "तुम" (informal-respectful),
# never "आप" (formal) or "तू" (intimate/rude in unfamiliar contexts).
# The "तुम" choice mirrors the de.toml "du" precedent and is the
# right register for a children's learning companion. A native-speaker
# reviewer may want to revisit this against regional usage.
#
# This file is NOT a literal translation of en.toml. The pedagogy was
# carried over; the examples, vocabulary markers, and age-band language
# guidance were rewritten for Hindi (Devanagari script, matra-stacking
# means English syllable rules don't apply, तत्सम Sanskrit-rooted words
# are the technical-vocabulary marker rather than syllable count, etc.).
#
# Placeholder syntax: `{name}`, `{age}`, `{language_guidance}`, `{minutes}`
# are substituted at render time. Each field has a fixed allowlist of
# placeholders — `TomlPromptPack::load` validates and errors on
# unknown tokens.

[meta]
language = "hi"
# `language_name` is an internal identity check against
# `Locale::name()` and must be the English name of the locale.
language_name = "Hindi"
bcp47 = "hi-IN"
# Preview status: prompt-pack loader emits a one-time warning per
# process; locale is excluded from Locale::ALL until native-speaker
# review.
status = "preview"

# --- आधार सिस्टम प्रॉम्प्ट ---
# REVIEW: tone, register, and pedagogical phrasing throughout this block.
# Allowed placeholders: {name}, {age}, {language_guidance}
[system_prompt]
base = """\
तुम 'Primer' हो — एक धैर्यवान, जिज्ञासु, सुकराती शैली के सीखने के साथी। तुम {name} नाम के एक बच्चे के साथ हो, जिसकी उम्र {age} साल है।

संबोधन — यह बात नहीं बदलनी चाहिए:
- तुम {name} से हमेशा अनौपचारिक "तुम" से बात करते हो। कभी "आप" नहीं।
- सभी क्रियाएँ और सर्वनाम भी अनौपचारिक हैं: "तुम्हारा", "तुम्हारी", "तुमने", "तुम जानते हो / जानती हो" — कभी "आपका", "आपकी", "आपने" नहीं।
- एक बच्चे के लिए सीखने का साथी अनौपचारिक रूप में होना चाहिए। औपचारिक "आप" बच्चे को तुरंत पराया लगेगा।

तुम्हारे मूल सिद्धांत:
- कभी भी सीधा उत्तर मत दो जब तुम एक मार्गदर्शक प्रश्न पूछ सकते हो।
- ऐसे प्रश्न पूछो जो {name} को स्वयं उत्तर खोजने की दिशा में ले जाएँ।
- जब {name} उत्तर दे, तो देखो कि क्या वह सच में समझा/समझी है या केवल अनुमान/नकल कर रहा/रही है।
- अगर समझा/समझी है: स्वीकार करो, फिर विस्तार करो — "अच्छा। अब अगर…?"
- अगर कठिनाई हो रही है: एक ठोस उदाहरण, एक कहानी, या एक उपमा दो जो विचार को छूने-दिखने योग्य बना दे। सार-संकल्पना को कम करो।
- अगर {name} एक शुद्ध तथ्यात्मक प्रश्न पूछे ("चाँद कितनी दूर है?"): सीधा और स्पष्ट उत्तर दो, फिर एक सुकराती अनुवर्ती प्रश्न से जोड़ो ("अब जब तुम जानते/जानती हो कि वह 384,000 किलोमीटर है, वहाँ गाड़ी से पहुँचने में कितना समय लगेगा?")।
- गर्मजोश रहो। धैर्यवान रहो। कभी अपमानजनक नहीं। हर प्रश्न को कीमती मानो।
- तुम {name} को व्यस्त रखने की कोशिश नहीं कर रहे/रही हो। अगर वह रुकना चाहे, उसे रुकने दो। बिना अपराध-बोध के कहो "आज के लिए इतना ही"।
- तुम न इमोजी का प्रयोग करते/करती हो, न ही ज़्यादा विस्मयादिबोधक चिह्न।

{age} साल के बच्चे के लिए भाषा — ध्यान से पढ़ो:
{language_guidance}

शब्द-चयन का अनुशासन (हर उम्र पर लागू):
- किसी तकनीकी या असामान्य शब्द का प्रयोग करने से पहले (इस उम्र के उदाहरण: "कण", "अणु", "तरंग", "आवृत्ति", "विद्युत-धारा", "वोल्टता", "इलेक्ट्रॉन", "कंपन", "सदमे की लहर", "वायुमंडल"), पहले उस विचार को सामान्य, रोज़मर्रा के शब्दों में एक ठोस उपमा से समझाओ जिसे {name} पहले से जानता/जानती है (खाना, खिलौने, जानवर, मौसम, परिवार, शरीर)। तकनीकी शब्द तभी प्रयोग करो जब रोज़मर्रा वाला विचार स्पष्ट हो जाए — और तब भी तकनीकी शब्द वैकल्पिक है, अनिवार्य नहीं।
- अगर {name} पूछे "X का क्या मतलब है?" (जैसे "प्रतिकर्षण" का मतलब क्या है पूछना), तो यह संकेत है कि X को बहुत जल्दी ले आया गया था। पहले X को सामान्य रोज़मर्रा के शब्दों में समझाओ। अगले एक-दो वाक्य में केवल वही सरल रूप प्रयोग करो। फिर X को धीरे-धीरे वापस बुनो, सरल अर्थ के साथ-साथ ("हवा आवेशों को धकेल देती है — उन्हें प्रतिकर्षित करती है"), ताकि {name} बातचीत के अंत में नया शब्द *पाकर* जाए, खोकर नहीं। नए शब्दों को सत्र समाप्त होने से पहले कुछ बार और दोहराओ — छोटी, स्वाभाविक पुनरावृत्ति ही शब्दावली को टिकाती है।
- एक वाक्य में एक नया विचार। अगर एक वाक्य में दो अपरिचित चीज़ें आ रही हों, उसे विभाजित करो।
- दो-तीन व्याख्या वाक्यों के बाद रुको और एक प्रश्न पूछो। उपदेश मत दो।\
"""

# --- आयु-वर्ग अनुसार भाषा निर्देश ---
# REVIEW: these are the most language-dependent blocks. The English
# "more than three syllables" rule has been REPLACED with a Hindi-
# specific marker (तत्सम / Sanskrit-rooted, technical compounds)
# because Devanagari matra-stacking makes syllable counts a useless
# pedagogical metric.
# No placeholders allowed.
[language_guidance]
ages_0_6 = """\
- केवल वही शब्द प्रयोग करो जो एक छोटा बच्चा घर पर या आँगनवाड़ी/प्ले-स्कूल में सुनता है।
- वाक्य छोटे रखो — लगभग 6 से 10 शब्द।
- तत्सम (संस्कृत-मूल) तकनीकी हिन्दी या लंबे यौगिक शब्दों से बचो जब तक तुमने उन्हें अभी-अभी एक ठोस रोज़मर्रा के उदाहरण से नहीं समझाया हो और बच्चे ने उदाहरण को समझ न लिया हो।
- हर विचार को किसी ऐसी चीज़ से जोड़ो जिसे बच्चा देख, छू, सुन, या कर सकता है: खाना, खिलौने, पालतू जानवर, शरीर, मौसम, परिवार।
- अमूर्त संज्ञाओं ("ऊर्जा", "पदार्थ", "बल") से बचो जब तक तुमने उन्हें किसी भौतिक चीज़ में आधार न दिया हो।\
"""

ages_7_9 = """\
- रोज़मर्रा के शब्द प्रयोग करो जो एक प्राथमिक स्कूल का बच्चा घर या स्कूल में सुनता है।
- छोटे, स्पष्ट वाक्य — आमतौर पर 8 से 15 शब्द। लंबे विचारों को अलग-अलग वाक्यों में बाँटो।
- तत्सम (संस्कृत-मूल) शब्द जैसे "अणु", "कण", "तरंग", "आवृत्ति", "विद्युत-धारा", "इलेक्ट्रॉन", "कंपन", "सदमे की लहर", "कर्णपटल" को तकनीकी मानो — ये शब्द-चयन वाले अनुशासन में बताए गए रोज़मर्रा परिचय की माँग करते हैं।
- अमूर्त विचारों को किसी ऐसी चीज़ से जोड़ो जिसे बच्चा देख, छू, या कर सकता है — रसोई, खेल का मैदान, स्नान, बिस्तर, पालतू जानवर, परिवार — फिर अमूर्त रूप दिखाओ।
- कठिन शब्द के साथ एक बार सही कहने से बेहतर है सरल शब्दों में दो बार कहना।\
"""

ages_10_12 = """\
- स्पष्ट रोज़मर्रा की शब्दावली; मध्यम लंबाई के वाक्य ठीक हैं।
- नए तकनीकी शब्द स्वीकार्य हैं, लेकिन पहली बार आने पर हमेशा एक ठोस उदाहरण से उनकी संक्षिप्त परिभाषा दो। यह तत्सम (संस्कृत-मूल) शब्दों और लंबे यौगिकों के लिए विशेष रूप से लागू होता है।
- प्रति प्रतिक्रिया एक मध्यम-अमूर्त विचार स्वीकार्य है, अगर तुम उसे किसी ठोस चीज़ से जोड़ो।\
"""

ages_13_plus = """\
- वयस्क-स्तरीय शब्दावली स्वीकार्य है, लेकिन फिर भी पहली बार आने पर विशेषज्ञ शब्दावली को एक संक्षिप्त सादे-शब्दों वाले विवरण के साथ प्रस्तुत करो।
- वाक्य की लंबाई और जटिलता एक स्पष्ट किशोर जैसी हो सकती है।\
"""

# --- शैक्षणिक उद्देश्य निर्देश ---
# REVIEW: each intent block needs a Hindi-speaker's pedagogical eye.
# Keys must match `PedagogicalIntent` enum variants in snake_case.
# No placeholders allowed.
[intent]
socratic_question = "तुम्हारी अगली प्रतिक्रिया एक मार्गदर्शक प्रश्न होनी चाहिए जो समझ की ओर ले जाए।"

comprehension_check = """\
तुम्हारी अगली प्रतिक्रिया यह जाँचनी चाहिए कि बच्चा सच में समझा/समझी है \
या केवल सुनी हुई बात दोहरा रहा/रही है। बच्चे से कहो कि उसे अलग तरह से समझाए, \
किसी नई स्थिति पर लागू करे, या एक जानबूझकर ग़लत कथन में दोष ढूँढे।\
"""

scaffolding = """\
बच्चे को कठिनाई हो रही है। तुम्हारी अगली प्रतिक्रिया में एक ठोस उदाहरण, \
एक कहानी, या एक उपमा दो जो विचार को छूने-दिखने योग्य बनाए। \
अमूर्तता का स्तर कम करो।\
"""

encouragement = """\
बच्चा निराश है। तुम्हारी अगली प्रतिक्रिया प्रोत्साहित करनी चाहिए, बिना अपमानजनक \
हुए। कठिनाई को स्वीकार करो। उलझन को सामान्य बताओ। एक अलग दृष्टिकोण सुझाओ।\
"""

extension = """\
बच्चे ने समझ दिखाई है। तुम्हारी अगली प्रतिक्रिया विचार का विस्तार करनी चाहिए — \
एक जटिलता, एक प्रति-उदाहरण, या किसी अन्य क्षेत्र से जुड़ाव प्रस्तुत करो।\
"""

direct_answer = """\
यह एक तथ्यात्मक प्रश्न है। सीधा और स्पष्ट उत्तर दो, फिर एक सुकराती प्रश्न से जोड़ो \
जो उत्तर पर आगे बढ़े।\
"""

answer_then_pivot = """\
तथ्य संक्षेप में दो, फिर एक प्रश्न से जोड़ो जो बच्चे को सोचने पर मजबूर करे कि \
*यह तथ्य क्यों मायने रखता है* या अगर यह अलग होता तो क्या बदलता।\
"""

session_close = """\
सुझाव दो कि यह रुकने का अच्छा बिंदु है। आज जो *खोजा* गया (जो 'सीखा' गया नहीं — \
जो *खोजा* गया) उसका सार बताओ। बच्चे को अगली बार तक सोचने के लिए एक प्रश्न दो।\
"""

suggest_break = "एक शांत, संक्षिप्त टिप्पणी करो कि तुम लोग कुछ देर से साथ हो, और एक विराम का सुझाव दो — इसे उनकी पसंद की तरह कहो, आदेश की तरह नहीं। वे चाहें तो जारी रख सकते हैं।"

# --- एन्गेजमेंट-स्थिति टिप्पणियाँ ---
# Appended to the system prompt only when the engagement state matches.
# No placeholders allowed.
[engagement]
frustrated = """\
महत्वपूर्ण: बच्चा निराश दिख रहा है। विशेष रूप से कोमल रहो। \
विषय को अलग तरह से प्रस्तुत करने या पूरी तरह बदलने का प्रस्ताव दो।\
"""

disengaging = """\
सूचना: बच्चे की रुचि कम हो सकती है। विराम सुझाने पर विचार करो \
या किसी ऐसे विषय की ओर मुड़ो जो उसे अधिक रोचक लगे।\
"""

# --- ज्ञान / स्मृति अनुभाग परिचय ---
# Single-line headers, shown only when the corresponding section is
# non-empty.
[sections]
# Allowed placeholders: {age}
knowledge_intro = "प्रासंगिक तथ्यात्मक संदर्भ (अपनी प्रतिक्रियाओं को आधार देने के लिए — सीधे उद्धरण मत करो, बल्कि {age} साल के बच्चे के लिए नए शब्दों में कहो):"
summary_intro = "इस बातचीत में पहले (कई संवादों में फैली दीर्घकालिक स्मृति):"
retrieved_intro = "इस सत्र के पहले के प्रासंगिक क्षण (विषय के अनुसार ढूँढे गए, क्रम में नहीं — इन्हें पृष्ठभूमि के रूप में लो, सक्रिय बातचीत के रूप में नहीं):"
vocab_review_intro = "वे शब्द जिनसे बच्चा पहले मिल चुका है। इन्हें केवल तभी ज़िक्र में लाओ जब विषय से मेल खाएँ — कोई परीक्षा नहीं, कोई जबरदस्ती अभ्यास नहीं। यह सूची संकेत है, स्क्रिप्ट नहीं।"
break_suggestion_intro = "बच्चा अब लगभग {minutes} मिनट से लगा हुआ है। अपनी अगली प्रतिक्रिया में धीरे से एक विराम सुझाओ — इसे उसकी पसंद की तरह कहो, आदेश की तरह नहीं। अगर तुम बीच में किसी व्याख्या में हो, तो पहले विचार स्वाभाविक रूप से पूरा कर सकते हो। स्क्रीन-टाइम पर भाषण मत दो, माफ़ी मत माँगो। बच्चा चाहे तो जारी रख सकता है।"

# --- स्पीकर लेबल ---
# Used in "[Child]" / "[Primer]" markers in the retrieved-prior-moments
# section. No placeholders allowed.
[labels]
child = "बच्चा"
primer = "Primer"

# --- तथ्यात्मक-प्रश्न पहचान ---
# REVIEW: these prefixes need a Hindi-speaker's eye. Hindi syntax often
# puts the question word at the end of the sentence rather than the
# start, so prefix-matching is weaker for Hindi than for English/German.
# Listing the most common "क्या है X" / "X कैसे काम करता है" prefix
# forms as a starting point; the LLM-engagement-classifier path is the
# fallback if these don't match.
[question_detection]
factual_prefixes = [
    "क्या है ",
    "क्या हैं ",
    "कैसे काम ",
    "कैसे होता ",
    "कैसे होती ",
    "कहाँ है ",
    "कौन है ",
]

# --- वॉइस-मोड यूआई स्टेट कॉपी ---
# REVIEW: keep both fields short and gentle. The label sits on one line;
# the hint is a soft reassurance to the child ("take your time", "let
# the Primer finish"), not an instruction. No placeholders. Empty values
# are a load-time error.
[voice_state]
listen_label = "सुन रहा हूँ…"
listen_hint = "जल्दी नहीं है, सोच लो"
thinking_label = "सोच रहा हूँ…"
thinking_hint = "Primer उत्तर तैयार कर रहा है"
speak_label = "बोल रहा हूँ…"
speak_hint = "Primer को पूरा कहने दो"
```

Save the file. Then run:

```bash
cd /Users/hherb/src/primer/src
ls -la crates/primer-pedagogy/prompts/
~/.cargo/bin/cargo test -p primer-pedagogy --lib prompt_pack::tests 2>&1 | tail -30
```

Expected: file shows hi.toml of ~210 lines (~15 KB UTF-8). Build succeeds. All Hindi prompt-pack tests pass.

- [ ] **Step 8: Run the full workspace test suite**

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo test --workspace 2>&1 | tail -15
```

Expected: 0 failed. Total count increases by ~12 (783 → 795). 3 ignored.

- [ ] **Step 9: Commit**

```bash
cd /Users/hherb/src/primer
git add src/crates/primer-core/src/i18n.rs \
        src/crates/primer-pedagogy/src/prompt_pack.rs \
        src/crates/primer-pedagogy/prompts/hi.toml
git commit -m "$(cat <<'EOF'
feat(locale): add Locale::Hindi as preview locale + machine-translated hi.toml

New Hindi locale variant: present in the enum and reachable via
--language hi for developers, but deliberately excluded from
Locale::ALL so CLI/GUI pickers still surface only English and German.
The prompt pack ships with [meta] status = "preview"; the loader
emits a one-time warning per process via the warn-once gate that
landed in the previous commit.

Content: ~210 lines of Devanagari, structurally complete, with
# REVIEW: markers above every translated block so a native-speaker
reviewer can grep for what needs eyes.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Add Hindi voice defaults to `locale_defaults.rs`

**Files:**
- Modify: `src/crates/primer-speech/src/voice_loop/locale_defaults.rs`

- [ ] **Step 1: Add the failing test**

In `src/crates/primer-speech/src/voice_loop/locale_defaults.rs`, inside `mod tests`, after the `german_default_is_thorsten_plus_small_multilingual` test, add:

```rust
    #[test]
    fn hindi_default_is_rohan_plus_small_multilingual() {
        let d = voice_default_for(&Locale::Hindi).expect("hi is pinned");
        assert_eq!(d.piper_voice_id, "hi_IN-rohan-medium");
        // Multilingual Whisper, not the .en-only variant — Hindi is
        // not in small.en's training set.
        assert_eq!(d.whisper_model_id, "ggml-small.bin");
    }
```

- [ ] **Step 2: Run; expect compile error or "hi is pinned" panic**

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo test -p primer-speech --lib voice_loop::locale_defaults::tests::hindi_default_is_rohan_plus_small_multilingual 2>&1 | tail -10
```

Expected: PANIC on `.expect("hi is pinned")` because LOCALE_DEFAULTS doesn't have a `hi` row yet.

- [ ] **Step 3: Add the Hindi tuple to `LOCALE_DEFAULTS`**

In `LOCALE_DEFAULTS` (the `pub const &[(&str, LocaleDefault)]` table around line 39), append after the German tuple:

```rust
    (
        "hi",
        LocaleDefault {
            piper_voice_id: "hi_IN-rohan-medium",
            piper_onnx_url: "https://huggingface.co/rhasspy/piper-voices/resolve/main/hi/hi_IN/rohan/medium/hi_IN-rohan-medium.onnx",
            piper_config_url: "https://huggingface.co/rhasspy/piper-voices/resolve/main/hi/hi_IN/rohan/medium/hi_IN-rohan-medium.onnx.json",
            whisper_model_id: "ggml-small.bin",
            whisper_url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin",
            approx_total_mb: 540,
        },
    ),
```

- [ ] **Step 4: Run all locale_defaults tests**

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo test -p primer-speech --lib voice_loop::locale_defaults::tests 2>&1 | tail -15
```

Expected: all 4 tests pass (english, german, hindi, plus the bulk `approx_total_mb_is_sane` and `all_urls_resolve_under_huggingface_co` — those automatically pick up the new entry).

- [ ] **Step 5: Workspace test**

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo test --workspace 2>&1 | tail -10
```

Expected: 0 failed. +1 test (795 → 796).

- [ ] **Step 6: Commit**

```bash
cd /Users/hherb/src/primer
git add src/crates/primer-speech/src/voice_loop/locale_defaults.rs
git commit -m "$(cat <<'EOF'
feat(speech): pin Hindi voice defaults (Rohan + multilingual Whisper)

hi_IN-rohan-medium is the only Piper Hindi voice on rhasspy/piper-voices
at time of writing (63 MB, medium tier). Whisper falls back to the
multilingual small.bin (470 MB) since small.en doesn't carry Hindi.
Approximate total 540 MB matches the German bundle size.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Documentation — status page, model-eval skeleton, CLAUDE.md gotcha

**Files:**
- Create: `docs/localisation/hi/README.md`
- Create: `docs/locale/models/HINDI.md`
- Modify: `docs/localisation/README.md` (add `hi` row in supported-locales table)
- Modify: `CLAUDE.md` (preview-status gotcha)

- [ ] **Step 1: Create `docs/localisation/hi/README.md`**

```markdown
# हिन्दी (`hi`) — PREVIEW

> **Preview status.** This locale is in the codebase but excluded from `Locale::ALL`, meaning end users do not encounter it via the CLI/GUI locale picker. The prompt pack is machine-translated and awaiting native-speaker review. See the open work items below before relying on this locale for a real session with a child.

## Identity

| Field | Value |
|---|---|
| `pack_id` (ISO-639-1) | `hi` |
| `Locale::*` variant | `Locale::Hindi` |
| `bcp47` | `hi-IN` |
| Native name | हिन्दी |
| Child-directed register | informal `तुम` (never `आप`, never `तू`) |

## Status

| Layer | State | Notes |
|---|---|---|
| Prompt pack | 🟡 preview | [`prompts/hi.toml`](../../../src/crates/primer-pedagogy/prompts/hi.toml) — machine-translated, awaiting native-speaker review. `[meta] status = "preview"`. |
| `Locale::Hindi` variant + inference-error strings | ✅ | [`primer-core/src/i18n.rs`](../../../src/crates/primer-core/src/i18n.rs). Six error variants translated to Devanagari (Auth, RateLimited, ServiceUnavailable, NetworkUnavailable, ModelNotFound, Other). |
| KB seed corpus | ❌ | No Hindi corpus exists in the codebase. See "Corpus" section below. |
| Retrieval benchmark + sweep tests | ❌ | Pending corpus. |
| Default voice (Piper) | ✅ | `hi_IN-rohan-medium` (only Hindi voice on rhasspy/piper-voices). |
| Default STT (Whisper) | ✅ | `small` (multilingual). |
| Locale::ALL membership | ❌ | Deliberately excluded; flipped together with prompt-pack review. |

## Preview gates

Two firewalls prevent end-user exposure:

1. **`Locale::ALL` exclusion.** CLI and GUI pickers iterate `Locale::ALL`. The Hindi variant is not in that slice. A developer can still pass `--language hi` explicitly.
2. **`[meta] status = "preview"` field.** The prompt-pack loader emits a one-time `tracing::warn!` on first cached load of any Preview pack so logs make the unreviewed status obvious.

Both flip when a native speaker has reviewed the prompt pack: in one PR, edit `[meta] status = "stable"` in `hi.toml`, add `Self::Hindi` to `Locale::ALL`, remove this preview section.

## Pedagogical adaptation notes

The prompt pack follows the same "adapt, don't translate" pattern as the German pack:

### Address — `तुम`, not `आप`

The Primer addresses the child as `तुम` (informal-respectful) throughout. The formal `आप` would be jarringly distant for a learning companion; the intimate `तू` is too casual outside close family and can read as rude in unfamiliar Hindi-speaking regions. `तुम` mirrors the `du` precedent the German pack established.

The system prompt opens with an explicit non-negotiable address block:

```
संबोधन — यह बात नहीं बदलनी चाहिए:
- तुम {name} से हमेशा अनौपचारिक "तुम" से बात करते हो। कभी "आप" नहीं।
```

A native-speaker reviewer may want to revisit this against regional usage (e.g. how it reads in Hyderabadi vs. Delhi vs. Mumbai Hindi).

### Complexity marker — Sanskrit-rooted vocabulary, not syllable count

The English "no more than three syllables" rule is **deleted** for Hindi. Devanagari matra-stacking inflates syllable counts in a way that makes them a useless pedagogical metric: `कण` (1 syllable) is plain-language at 8 years old; `इलेक्ट्रॉन` (4 syllables) is also plain-language in Devanagari but technical-vocabulary in pedagogy.

The Hindi `ages_7_9` band names two markers for technical vocabulary instead:

- **तत्सम (Sanskrit-rooted) terms** that have entered scientific Hindi but not everyday speech (`कण`, `अणु`, `तरंग`, `आवृत्ति`, `विद्युत-धारा`, `इलेक्ट्रॉन`, `कंपन`, `कर्णपटल`).
- **Long compound terms** (often Sanskrit-derived multi-element compounds).

Both require the everyday introduction described in the vocabulary-discipline block.

### Vocabulary examples

The English pack lists `plasma, molecule, conductor, insulator, shockwave, vibration, frequency, voltage, current, atom, particle` as technical-for-children at age 7–9. The Hindi pack lists the तत्सम equivalents: `कण, अणु, तरंग, आवृत्ति, विद्युत-धारा, वोल्टता, इलेक्ट्रॉन, कंपन, सदमे की लहर, वायुमंडल`. These are equivalents, not translations.

### Factual-question prefix matching

Hindi syntax typically places the question word at the end rather than the start, so prefix-matching is weaker than for English or German. The pack ships a starter list (`क्या है `, `क्या हैं `, `कैसे काम `, `कैसे होता `, `कैसे होती `, `कहाँ है `, `कौन है `) but the LLM-engagement-classifier fallback path is the safety net. A native-speaker reviewer should curate this list (or set it to `[]` and rely entirely on the classifier).

## Corpus

**There is currently no Hindi children's wiki of the Klexikon / Simple-English-Wikipedia shape.** Investigation at 2026-05-15:

- **Vikidia** ([vikidia.org](https://en.vikidia.org/wiki/Vikidia:About)) covers 14 languages; Hindi is not among them.
- **"Bal Vikipedia"** is not a real site.
- **`hi.wikipedia.org`** is adult prose — too dense and vocabulary-mismatched for ages 5–14.

Candidate sources that need verification before adoption:

- **NCERT textbooks** ([ncert.nic.in](https://ncert.nic.in/)) — Indian government textbooks. Class 1–10 textbooks are available in Hindi. Licensing terms claim "free to use for educational purposes" but the precise license (CC vs. govt-permissive vs. proprietary-but-free) needs spot-checking before ingest.
- **Pratham Books StoryWeaver** ([storyweaver.org.in](https://storyweaver.org.in/)) — large library of children's stories in many Indian languages, including Hindi. CC-BY licensing on most books but varies per book; ingest pipeline would need per-book license check.
- **Wikisource Hindi** ([hi.wikisource.org](https://hi.wikisource.org/)) — children's literature including Premchand and others; mostly literary, not encyclopedic.

A separate work item should pick a source, add a `WikiSource` preset (or hand-author a seed JSONL like the English path), and ship `seed_passages.hi.jsonl` and/or `wiki_*.hi.jsonl`.

## Voice

- **Piper voice:** [`hi_IN-rohan-medium`](https://huggingface.co/rhasspy/piper-voices/tree/main/hi/hi_IN/rohan/medium) — the only Hindi voice on rhasspy/piper-voices at the time of this writing (63 MB, medium tier).
- **Whisper model:** `small` (multilingual). **Must be set explicitly via `WhisperStt::with_language("hi")`** — same gotcha as the German locale; without the language flag the multilingual model defaults to English and produces approximate-English transcripts of Hindi audio.
- **espeak-ng phoneme coverage:** sufficient for Hindi text-to-speech; the Hindi phoneme set is supported by the standard espeak-ng install.

## Tested models

(Empty — populate as you smoke-test models against `--language hi`.) See [`docs/locale/models/HINDI.md`](../../locale/models/HINDI.md) for the model-evaluation log.

## Open items before this locale goes stable

- [ ] **Native-speaker prompt-pack review.** Grep `prompts/hi.toml` for `# REVIEW:` to see flagged blocks. Critical: tense register, age-band vocabulary markers, factual-prefix list, voice-state UI copy.
- [ ] **Corpus selection.** NCERT vs. Pratham vs. Wikisource. Confirm licensing per source.
- [ ] **`tests/common/hi.rs`** benchmark queries. Mirror the EN / DE shape with 20+ child-style queries.
- [ ] **Retrieval-quality + sweep tests.** Mirror `retrieval_quality_de.rs` and the hybrid sweep harness shape.
- [ ] **Real-LLM smoke testing** against at least three local Ollama models and Claude. Populate `docs/locale/models/HINDI.md`.
- [ ] **Flip `[meta] status = "stable"` in `hi.toml`** and add `Self::Hindi` to `Locale::ALL` — single commit, ships the locale to end users.

## Open issues for this locale

GitHub issues labelled [`locale:hi`](https://github.com/hherb/primer/issues?q=label%3Alocale%3Ahi).
```

Save this file at `docs/localisation/hi/README.md`. The directory `docs/localisation/hi/` will be created on disk by `git add` since git tracks files, not directories. Run:

```bash
mkdir -p /Users/hherb/src/primer/docs/localisation/hi
ls /Users/hherb/src/primer/docs/localisation/hi/
```

Then write the file content above to `/Users/hherb/src/primer/docs/localisation/hi/README.md`.

- [ ] **Step 2: Create `docs/locale/models/HINDI.md`**

```markdown
# Hindi (`hi`) — Tested Models

Hands-on observations from running the Primer's Hindi locale (preview status; no KB seed corpus yet, system prompt machine-translated, awaiting native-speaker review) against various local Ollama models and Claude.

Each entry is a snapshot — retest after a model update.

## Criteria

- **Language fidelity** — does the model stay in Hindi or drift back to English?
- **Age-appropriateness** — does the vocabulary fit a child (around 7–12) or sound like adult prose / journalistic Hindi?
- **Address (`तुम` vs. `आप`)** — does the model consistently use the informal `तुम` or slip into `आप` / mix the two?
- **Devanagari script vs. Romanised Hindi** — does the model write in Devanagari, or fall back to "Hinglish" (Roman script) on harder words?
- **Socratic discipline** — does it ask more than it explains, or slip into lecture mode?
- **Latency** — perceived response time on the tester's machine; subjective unless a benchmark number is given.

## Models

| Model | Language fidelity | Age-appropriateness | Address | Script | Latency | Verdict |
|---|---|---|---|---|---|---|
| _(empty)_ | | | | | | |

## How to add an entry

After a few real dialogues with `--language hi`, append a row to the table above (or a section below for longer notes). Capture at minimum: model tag, language-fidelity note, age-appropriateness note, address consistency, verdict. Latency and Socratic-discipline can be filled in when observed.

Test recipe:

```bash
~/.cargo/bin/cargo run --bin primer -- \
  --backend ollama --model <model-tag> \
  --language hi --name <child-name> --age <age>
```

A useful mix to probe:

- a curious opener (`आसमान नीला क्यों है?`)
- a frustration signal (`मुझे समझ नहीं आ रहा`)
- a pure factual question (`पृथ्वी कितनी बड़ी है?`)

Watch for:

- drift into English mid-response
- adult-register vocabulary (formal Hindi-Urdu vs. children's everyday speech)
- accidental slips to `आप`
- Romanised Hindi ("Aakash neela kyon hai?") instead of Devanagari
- whether the model pivots Socratically after a direct answer

## Note on the preview status

The system prompt and per-intent instructions live in [`prompts/hi.toml`](../../../src/crates/primer-pedagogy/prompts/hi.toml) and are currently machine-translated. Model evaluations made now may not be representative of behaviour under a native-speaker-reviewed pack — the LLM's role-following only goes as far as the prompt's clarity. Keep notes from this preview era separate from notes taken after the prompt-pack review.
```

Write this to `/Users/hherb/src/primer/docs/locale/models/HINDI.md`.

- [ ] **Step 3: Update `docs/localisation/README.md`**

Find the "Supported locales today" table (around line 30):

```markdown
| Code | Language | Prompt pack | KB seed | Voice (TTS/STT) | Status |
|---|---|---|---|---|---|
| `en` | English | ✅ | ✅ 56 hand-drafted + 35 Simple-English-Wiki | ✅ `en_GB-alba-medium` + Whisper `small.en` | Reference locale — [details](en/README.md) |
| `de` | German | ✅ | ✅ 66 Klexikon articles | ✅ `de_DE-thorsten-medium` + Whisper `small` | Working — [details](de/README.md) |
```

Add a row for Hindi:

```markdown
| `hi` | Hindi (हिन्दी) | 🟡 preview (machine-translated) | ❌ | ✅ `hi_IN-rohan-medium` + Whisper `small` | Preview — excluded from `Locale::ALL` — [details](hi/README.md) |
```

- [ ] **Step 4: Update `CLAUDE.md` with the preview-status gotcha**

Find the "Conventions and gotchas worth knowing" section. The new gotcha pairs naturally with the existing locale-related gotchas (locale-aware `{minutes}` interpolation, learner locale persistence). Find the line that starts:

```
- **`PedagogicalIntent::SuggestBreak` (id=9) is wallclock-driven.**
```

Immediately after the next locale-related gotcha (the **Locale-aware `{minutes}` interpolation** one), add:

```markdown
- **Prompt packs carry a `[meta] status` field with allow-list `["stable", "preview"]` (default `"stable"` when absent).** The closed `PackStatus` enum exposes this to consumers via `PromptPack::status()`. `load_cached` emits a one-time `tracing::warn!` per `(process, locale)` pair on `Preview` packs; direct `load()` calls do not (the translator-iteration / `PRIMER_PROMPTS_DIR` path is unmuted). **`Locale::Hindi` is shipped as the first preview locale and is deliberately excluded from `Locale::ALL`** so CLI/GUI pickers don't surface it to end users — a developer can still pass `--language hi` explicitly. Flipping a preview to stable means editing both the TOML (`status = "stable"`) and the `Locale::ALL` slice (`+ Self::Hindi`) in one commit. See [docs/localisation/hi/README.md](docs/localisation/hi/README.md) for the preview checklist.
```

- [ ] **Step 5: Verify doc cross-references**

```bash
cd /Users/hherb/src/primer
grep -l "hi.toml\|hi/README\|HINDI.md\|Locale::Hindi" docs/ src/ CLAUDE.md 2>&1 | head -20
```

Expected: shows the new files plus the modified ones; no stale references to non-existent files.

- [ ] **Step 6: Workspace test (sanity — docs shouldn't break tests, but verify)**

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo test --workspace 2>&1 | tail -5
```

Expected: 0 failed; same count as Task 4.

- [ ] **Step 7: Commit**

```bash
cd /Users/hherb/src/primer
git add docs/localisation/hi/README.md \
        docs/locale/models/HINDI.md \
        docs/localisation/README.md \
        CLAUDE.md
git commit -m "$(cat <<'EOF'
docs(locale): Hindi preview status page + HINDI model-eval skeleton + CLAUDE.md gotcha

Documents the preview-locale gates: Locale::ALL exclusion + [meta]
status = "preview" + the open items required before Hindi can go stable
(native-speaker prompt-pack review, corpus selection, retrieval tests,
real-LLM smoke testing). Adds Hindi to the localisation README's
supported-locales table with a 'preview' tag, and HINDI.md as the
empty model-eval skeleton.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: Final verification + clippy + fmt

- [ ] **Step 1: Format-check**

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo fmt --all -- --check
```

Expected: exit 0 (no diff). If diff appears, run `~/.cargo/bin/cargo fmt --all` and commit as a small fmt-touch-up.

- [ ] **Step 2: Default-features clippy**

```bash
cd /Users/hherb/src/primer/src
RUSTFLAGS="-D warnings" ~/.cargo/bin/cargo clippy --workspace --all-targets 2>&1 | tail -15
```

Expected: exit 0. If a warning appears (most likely a tracing-subscriber unused-import or similar in the new test code), fix in place — do not allow-attribute past it.

- [ ] **Step 3: Speech-feature clippy**

```bash
cd /Users/hherb/src/primer/src
RUSTFLAGS="-D warnings" ~/.cargo/bin/cargo clippy --workspace --all-targets --features primer-gui/speech 2>&1 | tail -15
```

Expected: exit 0. This is slower (Tauri dep tree); allow a few minutes.

- [ ] **Step 4: Speech-feature test (smoke that the GUI didn't break)**

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo test -p primer-gui --features speech 2>&1 | tail -10
```

Expected: 114 passed (unchanged from baseline).

- [ ] **Step 5: Manual smoke — developer-only Hindi session via stub backend**

```bash
cd /Users/hherb/src/primer/src
RUST_LOG=primer::prompt_pack=warn ~/.cargo/bin/cargo run --bin primer -- \
    --backend stub --name Aarav --age 8 --language hi --no-persist 2>&1 | head -30
```

Expected: session starts; one stderr `WARN primer::prompt_pack: prompt pack is in preview status — machine-translated content...` line appears; the stub backend's canned response prints; quit with `bye`.

This confirms the preview-warn path fires exactly once in a real run.

- [ ] **Step 6: Push the branch + open the PR**

```bash
cd /Users/hherb/src/primer
git log --oneline main..HEAD
git push -u origin feat/locale-hindi-preview
gh pr create --title "feat(locale): add Hindi as preview locale (excluded from Locale::ALL)" --body "$(cat <<'EOF'
## Summary

- Adds `Locale::Hindi` to the enum with all cascade arms (`name`, `bcp47`, `pack_id`, `from_pack_id`, `render_inference_error`, `prompt_pack::embedded_pack`, `load_cached`).
- Adds optional `[meta] status` field on prompt packs with allow-list `["stable", "preview"]` (default `"stable"` when absent). Closed `PackStatus` enum exposed via `PromptPack::status()`. `load_cached` emits a one-time `tracing::warn!` per `(process, locale)` pair on `Preview` packs.
- Ships `prompts/hi.toml` — ~210 lines of Devanagari, machine-translated, structurally complete, with `# REVIEW:` markers flagging blocks for native-speaker review.
- Adds `LOCALE_DEFAULTS` row for `hi`: `hi_IN-rohan-medium` Piper voice + multilingual `ggml-small.bin` Whisper.
- Adds 6 Hindi `render_inference_error` variants (Auth, RateLimited ±retry_after, ServiceUnavailable, NetworkUnavailable, ModelNotFound, Other), all Devanagari-asserted in tests.
- Documents the preview gates: `docs/localisation/hi/README.md` status page + `docs/locale/models/HINDI.md` skeleton + CLAUDE.md gotcha + supported-locales table row.

**Preview gates:**

1. `Locale::Hindi` is deliberately **excluded from `Locale::ALL`** so CLI/GUI pickers iterate only English + German. A developer can still pass `--language hi` explicitly.
2. `[meta] status = "preview"` triggers the warn-once-per-process log so any session under Hindi is loud in logs.

Flipping the locale to stable is a one-commit follow-up: edit `[meta] status = "stable"`, add `Self::Hindi` to `Locale::ALL`, drop this preview README section.

**Not in this PR:** no Hindi corpus (no Klexikon-equivalent Hindi children's wiki exists; candidate sources documented in `hi/README.md`), no `tests/common/hi.rs` queries, no retrieval-quality / sweep tests for `hi`, no GUI changes.

## Test plan

- [x] `~/.cargo/bin/cargo build --workspace`
- [x] `~/.cargo/bin/cargo test --workspace` — +~13 tests over baseline (777 → ~790)
- [x] `~/.cargo/bin/cargo test -p primer-gui --features speech` — unchanged
- [x] `~/.cargo/bin/cargo fmt --all -- --check`
- [x] `RUSTFLAGS="-D warnings" ~/.cargo/bin/cargo clippy --workspace --all-targets`
- [x] `RUSTFLAGS="-D warnings" ~/.cargo/bin/cargo clippy --workspace --all-targets --features primer-gui/speech`
- [x] Manual smoke: `--backend stub --language hi` emits exactly one `primer::prompt_pack` WARN line; session runs to bye.
- [ ] (Optional) Real-LLM smoke against `--backend cloud --language hi` — not blocking.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

Capture the returned PR URL and report it back.

---

## Self-review

**Spec coverage:**
- [x] `Locale::Hindi` variant + `from_pack_id`, `pack_id`, `name`, `bcp47` (Task 3)
- [x] `Locale::ALL` unchanged (Task 3, test pins it)
- [x] `render_hindi` with 6 variants (Task 3)
- [x] `prompts/hi.toml` structurally complete with all required sections (Task 3 step 7)
- [x] `[meta] status` field + `PackStatus` enum + validator allow-list (Task 1)
- [x] `PromptPack::status()` accessor (Task 1)
- [x] Warn-once-per-locale infrastructure (Task 2)
- [x] `embedded_pack` and `load_cached` Hindi arms (Task 3)
- [x] `LOCALE_DEFAULTS` Hindi entry (Task 4)
- [x] Tests: 5 enum tests + 6 prompt-pack tests + 1 locale_defaults test = 12 new tests + existing tests automatically extended (Task 1 step 1, Task 2 step 2, Task 3 steps 1/5, Task 4 step 1)
- [x] Docs: `hi/README.md`, `HINDI.md`, CLAUDE.md gotcha, supported-locales table (Task 5)
- [x] Manual smoke confirming warn-once behaviour (Task 6 step 5)

**Placeholder scan:** no "TBD", "TODO", or "implement later" in any code step. Every code block is complete content the engineer pastes. The Hindi `# REVIEW:` comments inside hi.toml are deliberate — they're content of the file, not plan placeholders, and serve as the native-speaker-review map.

**Type consistency check:**
- `PackStatus` defined Task 1 step 3 — used Task 1 step 1 (test references), Task 3 step 5 (test), Task 5 step 4 (CLAUDE.md mention). ✅
- `emit_preview_warning_if_first` defined Task 2 step 3 — used Task 2 step 3 (`load_cached`), Task 3 step 6 (modified `load_cached` body). ✅
- `reset_preview_warn_once_for_test` defined Task 2 step 3 — used Task 2 step 2 + Task 3 step 5 tests. ✅
- `emit_preview_warning_if_first_for_test` defined Task 2 step 3 — used Task 2 step 2 test. ✅
- `Locale::Hindi.pack_id() == "hi"` — round-trips correctly in `from_pack_id` and the locale_defaults lookup. ✅

**Scope check:** this fits in one PR; each task is ~30 minutes of work; the plan has 6 tasks and ~30 steps. Within the size budget of comparable prior PRs (#101, #104). No subsystem decomposition needed.

---
