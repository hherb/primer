# GUI reasoning-marker editor — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a Primer GUI user add custom reasoning-marker `(open, close)` pairs in Settings, so chain-of-thought from a model whose delimiters aren't in the built-in table is stripped from the child-visible response.

**Architecture:** The textarea's raw text is stored verbatim as a `String` on the GUI config and its View/Update DTOs. A pure, unit-tested Rust function `parse_reasoning_markers(&str) -> Vec<(String, String)>` converts it to the engine's pair list at session-wiring time. The frontend does zero parsing — it sends the textarea value as-is and echoes the stored string back on load. Empty string ⇒ empty Vec ⇒ built-in defaults only (today's behavior).

**Tech Stack:** Rust (edition 2024, toolchain 1.88), `primer-gui` crate (Tauri 2.x), serde, vanilla JS frontend (`ui/settings.js`, `ui/index.html`).

**Spec:** `docs/superpowers/specs/2026-05-31-gui-reasoning-marker-editor-design.md`

**Branch:** `feat/gui-reasoning-marker-editor` (already created; spec already committed here as `3eb165b`).

**IMPORTANT — toolchain:** every cargo command runs from `/Users/hherb/src/primer/src` (where `rust-toolchain.toml` pins 1.88). Use `~/.cargo/bin/cargo` so Homebrew rust doesn't shadow it. When in doubt: `~/.cargo/bin/cargo +1.88 … ` from `src/`.

---

## File Structure

| File | Responsibility | Action |
| --- | --- | --- |
| `src/crates/primer-gui/src/reasoning_markers.rs` | Pure text→pairs parser + its unit tests | Create |
| `src/crates/primer-gui/src/lib.rs` | Register the new module | Modify (1 line) |
| `src/crates/primer-gui/src/config.rs` | `reasoning_markers: String` on config + View + Update; conversions; tests | Modify |
| `src/crates/primer-gui/src/wiring.rs` | Parse the config string into `BackendParams.reasoning_markers` | Modify (1 line + comment) |
| `src/crates/primer-gui/ui/index.html` | The `<textarea>` field markup | Modify |
| `src/crates/primer-gui/ui/settings.js` | DOM ref, populate, gather, show/hide | Modify |

---

## Task 1: Pure parser module

**Files:**
- Create: `src/crates/primer-gui/src/reasoning_markers.rs`
- Modify: `src/crates/primer-gui/src/lib.rs` (add `pub mod reasoning_markers;`)

- [ ] **Step 1: Create the module file with the failing tests AND the function signature stub**

Create `src/crates/primer-gui/src/reasoning_markers.rs` with this exact content (stub returns `vec![]` so the file compiles but the behavioral tests fail):

```rust
//! Parse the GUI Settings "reasoning markers" textarea into the
//! `(open, close)` pairs the inference backends consume.
//!
//! The textarea holds free text: one `open<whitespace>close` pair per
//! line. This is the GUI counterpart to the CLI's `--reasoning-marker`
//! flag (which receives pre-tokenised clap pairs). Keeping the parse in
//! pure Rust — rather than in `settings.js` — means it is exhaustively
//! unit-tested and the frontend stays a verbatim pass-through.

/// Parse free textarea text into `(open, close)` reasoning-marker pairs.
///
/// Rules:
/// - Each line is trimmed, then split on its **first** whitespace run:
///   `open` = the text before it, `close` = the remainder, trimmed.
/// - A line is dropped if it has no whitespace (open only, no close) or
///   if `close` is empty after trimming. This mirrors the CLI dropping
///   an incomplete pair — no error, no warning.
/// - Blank / whitespace-only lines are ignored.
/// - `close` is "the rest of the line", so a close marker may contain
///   internal spaces (e.g. `<a> </a> tail` → `("<a>", "</a> tail")`).
///
/// Empty input yields an empty `Vec`, which means "built-in defaults
/// only" downstream.
pub fn parse_reasoning_markers(text: &str) -> Vec<(String, String)> {
    let _ = text;
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::parse_reasoning_markers;

    fn pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
        v.iter()
            .map(|(o, c)| (o.to_string(), c.to_string()))
            .collect()
    }

    #[test]
    fn empty_input_is_empty() {
        assert_eq!(parse_reasoning_markers(""), Vec::<(String, String)>::new());
    }

    #[test]
    fn single_pair() {
        assert_eq!(
            parse_reasoning_markers("<think> </think>"),
            pairs(&[("<think>", "</think>")])
        );
    }

    #[test]
    fn multiple_lines_in_order() {
        assert_eq!(
            parse_reasoning_markers("<a> </a>\n<b> </b>"),
            pairs(&[("<a>", "</a>"), ("<b>", "</b>")])
        );
    }

    #[test]
    fn leading_and_trailing_whitespace_trimmed() {
        assert_eq!(
            parse_reasoning_markers("   <a>   </a>   "),
            pairs(&[("<a>", "</a>")])
        );
    }

    #[test]
    fn blank_lines_ignored() {
        assert_eq!(
            parse_reasoning_markers("<a> </a>\n\n   \n<b> </b>"),
            pairs(&[("<a>", "</a>"), ("<b>", "</b>")])
        );
    }

    #[test]
    fn crlf_line_endings_handled() {
        assert_eq!(
            parse_reasoning_markers("<a> </a>\r\n<b> </b>"),
            pairs(&[("<a>", "</a>"), ("<b>", "</b>")])
        );
    }

    #[test]
    fn open_only_line_is_dropped() {
        assert_eq!(
            parse_reasoning_markers("<a>"),
            Vec::<(String, String)>::new()
        );
    }

    #[test]
    fn open_with_trailing_whitespace_only_is_dropped() {
        // After trimming, the line is just "<a>" with no whitespace → no close.
        assert_eq!(
            parse_reasoning_markers("<a>    "),
            Vec::<(String, String)>::new()
        );
    }

    #[test]
    fn tab_separator_works() {
        assert_eq!(
            parse_reasoning_markers("<a>\t</a>"),
            pairs(&[("<a>", "</a>")])
        );
    }

    #[test]
    fn close_with_internal_spaces_preserved() {
        assert_eq!(
            parse_reasoning_markers("<a> </a> tail"),
            pairs(&[("<a>", "</a> tail")])
        );
    }
}
```

Register the module: in `src/crates/primer-gui/src/lib.rs`, the module list currently reads (lines 13–22):

```rust
pub mod commands;
pub mod config;
pub mod csp;
pub mod modal_dialog_contract;
pub mod paths;
pub mod state;
```

Insert `pub mod reasoning_markers;` after the `pub mod paths;` line:

```rust
pub mod paths;
pub mod reasoning_markers;
pub mod state;
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd /Users/hherb/src/primer/src && ~/.cargo/bin/cargo test -p primer-gui reasoning_markers`
Expected: compiles, but the behavioral tests FAIL (e.g. `single_pair` — left `[]`, right `[("<think>", "</think>")]`). `empty_input_is_empty` passes (stub returns `[]`).

- [ ] **Step 3: Implement the parser**

Replace the stub body of `parse_reasoning_markers` (the `let _ = text; Vec::new()`) with:

```rust
pub fn parse_reasoning_markers(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            // `split_once` on the first whitespace char; `None` means the
            // line has no whitespace at all (open only) → drop it.
            let (open, close) = line.split_once(char::is_whitespace)?;
            let close = close.trim();
            if open.is_empty() || close.is_empty() {
                return None;
            }
            Some((open.to_string(), close.to_string()))
        })
        .collect()
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd /Users/hherb/src/primer/src && ~/.cargo/bin/cargo test -p primer-gui reasoning_markers`
Expected: all 10 tests PASS.

- [ ] **Step 5: Commit**

```bash
cd /Users/hherb/src/primer
git add src/crates/primer-gui/src/reasoning_markers.rs src/crates/primer-gui/src/lib.rs
git commit -m "feat(gui): pure parse_reasoning_markers textarea→pairs parser

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Add `reasoning_markers` to config + DTOs

**Files:**
- Modify: `src/crates/primer-gui/src/config.rs` (struct fields, Default, From, into_config, three existing test JSONs, new tests)

- [ ] **Step 1: Write the failing config round-trip tests**

In `src/crates/primer-gui/src/config.rs`, inside `mod tests`, add these three tests immediately after the existing `qnn_paths_pass_through_update_verbatim` test (search for the line `fn qnn_paths_pass_through_update_verbatim`; the test ends with a closing `}` near line ~966 — add after it):

```rust
    #[test]
    fn default_reasoning_markers_is_empty() {
        let cfg = GuiConfig::default();
        assert_eq!(cfg.backend.reasoning_markers, "");
    }

    #[test]
    fn reasoning_markers_round_trip_through_disk() {
        let dir = TempDir::new().unwrap();
        let mut cfg = GuiConfig::default();
        cfg.backend.kind = "ollama".to_string();
        cfg.backend.reasoning_markers = "[[r]] [[/r]]\n<x> </x>".to_string();

        save(dir.path(), &cfg).unwrap();
        let round_trip = load(dir.path()).unwrap();
        assert_eq!(round_trip, cfg);
    }

    #[test]
    fn reasoning_markers_pass_through_view_verbatim() {
        // Not a secret — the view must carry the raw textarea text through
        // unredacted so the settings form can re-show what the user typed.
        let mut cfg = GuiConfig::default();
        cfg.backend.reasoning_markers = "[[r]] [[/r]]".to_string();
        let view: GuiConfigView = (&cfg).into();
        assert_eq!(view.backend.reasoning_markers, "[[r]] [[/r]]");
    }

    #[test]
    fn reasoning_markers_pass_through_update_verbatim() {
        let current = GuiConfig::default();
        let update_json = r#"{
            "learner": {"name": "Ada", "age": 7, "locale": "en"},
            "backend": {
                "kind": "ollama",
                "model": null,
                "ollama_url": "http://localhost:11434",
                "openai_compat_url": "http://localhost:8000",
                "api_key_source": {"kind": "keep"},
                "openai_compat_api_key_source": {"kind": "keep"},
                "reasoning_markers": "[[r]] [[/r]]",
                "qnn_bundle_dir": null,
                "qnn_qairt_lib_dir": null
            },
            "classifier": {"match_main": true, "kind": null, "model": null, "timeout_ms": 3000},
            "extractor": {"match_main": true, "kind": null, "model": null, "timeout_ms": 5000},
            "comprehension": {"match_main": true, "kind": null, "model": null, "timeout_ms": 5000},
            "embedder": {"kind": "none", "model": null, "ollama_url": null, "openai_compat_url": null},
            "vocab": {"max_per_prompt": null},
            "breaks": {"after_mins": 30},
            "persistence": {"session_db": null, "knowledge_db": null, "no_persist": false},
            "ui": {"sidebar_open": true, "last_section": "current_turn"},
            "speech": {"voice_mode_enabled": false, "disable_auto_download": false, "mic_silence_ms": 600, "overrides": {}}
        }"#;
        let update: GuiConfigUpdate = serde_json::from_str(update_json).unwrap();
        let resolved = update.into_config(&current);
        assert_eq!(resolved.backend.reasoning_markers, "[[r]] [[/r]]");
    }
```

- [ ] **Step 2: Run the new tests to verify they fail (compile error)**

Run: `cd /Users/hherb/src/primer/src && ~/.cargo/bin/cargo test -p primer-gui --lib config::tests::reasoning_markers 2>&1 | head -30`
Expected: FAIL — compile error, `no field reasoning_markers on type BackendConfig` (and on the View/Update). This is the expected failing state.

- [ ] **Step 3: Add the field to the three structs**

In `src/crates/primer-gui/src/config.rs`:

(a) `BackendConfig` — after the `qnn_qairt_lib_dir: Option<PathBuf>,` field (line ~97, just before the struct's closing `}`):

```rust
    pub qnn_qairt_lib_dir: Option<PathBuf>,
    /// Raw "reasoning markers" textarea text from Settings: one
    /// `open<whitespace>close` pair per line. Parsed into `(open, close)`
    /// pairs by `crate::reasoning_markers::parse_reasoning_markers` at
    /// session-wiring time and appended to the built-in defaults for the
    /// ollama / openai-compat backends. Empty ⇒ defaults only. Stored
    /// verbatim so the textarea round-trips losslessly. Not a secret —
    /// crosses the IPC View/Update DTOs unredacted.
    pub reasoning_markers: String,
}
```

(b) `Default for BackendConfig` — after `qnn_qairt_lib_dir: None,` (line ~110):

```rust
            qnn_qairt_lib_dir: None,
            reasoning_markers: String::new(),
        }
```

(c) `BackendConfigView` — after its `qnn_qairt_lib_dir: Option<PathBuf>,` (line ~554):

```rust
    pub qnn_qairt_lib_dir: Option<PathBuf>,
    /// Raw reasoning-markers textarea text — passes through verbatim
    /// (not a secret), so the settings form can re-show it.
    pub reasoning_markers: String,
}
```

(d) `From<&GuiConfig> for GuiConfigView` — in the `BackendConfigView { … }` literal, after `qnn_qairt_lib_dir: c.backend.qnn_qairt_lib_dir.clone(),` (line ~569):

```rust
                qnn_qairt_lib_dir: c.backend.qnn_qairt_lib_dir.clone(),
                reasoning_markers: c.backend.reasoning_markers.clone(),
            },
```

(e) `BackendConfigUpdate` — after its `qnn_qairt_lib_dir: Option<PathBuf>,` (line ~619). **No `#[serde(default)]`** — mandatory, like its siblings:

```rust
    pub qnn_qairt_lib_dir: Option<PathBuf>,
    /// Raw reasoning-markers textarea text. Like every other
    /// `BackendConfigUpdate` field, this is **mandatory** in the
    /// `update_settings` payload (the struct has no `#[serde(default)]`),
    /// so `settings.js::gather()` must always send it (empty string when
    /// the textarea is blank). Not a secret — no Keep/Env dance.
    pub reasoning_markers: String,
}
```

(f) `GuiConfigUpdate::into_config` — in the `BackendConfig { … }` literal, after `qnn_qairt_lib_dir: self.backend.qnn_qairt_lib_dir,` (line ~643):

```rust
                qnn_qairt_lib_dir: self.backend.qnn_qairt_lib_dir,
                reasoning_markers: self.backend.reasoning_markers,
            },
```

- [ ] **Step 4: Fix the three existing Update-DTO test JSONs**

The new mandatory `reasoning_markers` field on `BackendConfigUpdate` means every existing test that deserialises a `GuiConfigUpdate` from a JSON literal now fails to parse. There are exactly three such `backend` blocks, each containing the line `"openai_compat_api_key_source": {"kind": "keep"},`. Add `"reasoning_markers": "",` after that line in all three.

Use a single `replace_all` edit. Old string (16-space indent, appears 3×):

```
                "openai_compat_api_key_source": {"kind": "keep"},
```

New string:

```
                "openai_compat_api_key_source": {"kind": "keep"},
                "reasoning_markers": "",
```

(Note: the `{"kind": "env"}` occurrence in `older_config_without_qnn_fields_loads_with_defaults` is a `BackendConfig` load test — `BackendConfig` keeps `#[serde(default)]`, so it does NOT need the field and is intentionally left alone.)

- [ ] **Step 5: Run the config tests to verify they pass**

Run: `cd /Users/hherb/src/primer/src && ~/.cargo/bin/cargo test -p primer-gui --lib config`
Expected: all `config::tests` PASS, including the four new `reasoning_markers*` tests and the three previously-edited Update-JSON tests.

- [ ] **Step 6: Commit**

```bash
cd /Users/hherb/src/primer
git add src/crates/primer-gui/src/config.rs
git commit -m "feat(gui): reasoning_markers String field on backend config + DTOs

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Wire the parser into session construction

**Files:**
- Modify: `src/crates/primer-gui/src/wiring.rs` (line ~157–160)

- [ ] **Step 1: Replace the hard-coded empty Vec**

In `src/crates/primer-gui/src/wiring.rs`, inside the `BackendParams { … }` literal in `build_with_strategy`, replace this block (lines ~157–160):

```rust
        // Reasoning-marker custom-extend is CLI-only for now; the GUI editor
        // is deferred (ROADMAP 0.3). The GUI still gets default stripping for
        // free because the backends seed the built-in marker table in `new`.
        reasoning_markers: Vec::new(),
```

with:

```rust
        // Custom reasoning markers from Settings → Inference backend.
        // Parsed from the raw textarea text into `(open, close)` pairs and
        // appended to the built-in defaults by the ollama / openai-compat
        // backends. Empty string ⇒ empty Vec ⇒ defaults only.
        reasoning_markers: crate::reasoning_markers::parse_reasoning_markers(
            &backend_config.reasoning_markers,
        ),
```

- [ ] **Step 2: Verify the crate builds**

Run: `cd /Users/hherb/src/primer/src && ~/.cargo/bin/cargo build -p primer-gui`
Expected: builds clean (the `reasoning_markers` field on `backend_config` now exists from Task 2; the parser exists from Task 1).

- [ ] **Step 3: Commit**

```bash
cd /Users/hherb/src/primer
git add src/crates/primer-gui/src/wiring.rs
git commit -m "feat(gui): wire reasoning_markers config into BackendParams

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Frontend — textarea, populate, gather, show/hide

**Files:**
- Modify: `src/crates/primer-gui/ui/index.html` (add the textarea)
- Modify: `src/crates/primer-gui/ui/settings.js` (DOM refs, populate, gather, reveal)

This task has no Rust test (the frontend is verbatim pass-through and the repo has no JS test harness); verification is a build + manual check folded into Task 5.

- [ ] **Step 1: Add the textarea to `index.html`**

In `src/crates/primer-gui/ui/index.html`, find the QAIRT-lib-dir field — it ends with this exact markup (the `use the conventional layout.` hint is unique):

```html
              <small class="hint muted"
                >Directory containing <code>libGenie.so</code>. Leave blank to
                use the conventional layout.</small
              >
            </label>
```

Replace it with the same markup followed by the new reasoning-markers field (still inside the `settings-grid`):

```html
              <small class="hint muted"
                >Directory containing <code>libGenie.so</code>. Leave blank to
                use the conventional layout.</small
              >
            </label>
            <label class="field" id="f-backend-reasoning-markers-field">
              <span>Reasoning markers</span>
              <textarea
                id="f-backend-reasoning-markers"
                rows="3"
                spellcheck="false"
                placeholder="[[r]] [[/r]]"
              ></textarea>
              <small class="hint muted"
                >One <code>open&nbsp;close</code> pair per line (split on the
                first space). Built-in defaults
                (<code>&lt;think&gt;…&lt;/think&gt;</code> and Gemma4) always
                apply — this only adds more. Literal text, not regex.</small
              >
            </label>
```

- [ ] **Step 2: Add the DOM references in `settings.js`**

In `src/crates/primer-gui/ui/settings.js`, find the QNN QAIRT-lib-dir refs (lines ~51–52):

```javascript
    backendQnnQairtLibDir: document.getElementById("f-backend-qnn-qairt-lib-dir"),
    backendQnnQairtLibDirField: document.getElementById("f-backend-qnn-qairt-lib-dir-field"),
```

Add the two new refs immediately after:

```javascript
    backendQnnQairtLibDir: document.getElementById("f-backend-qnn-qairt-lib-dir"),
    backendQnnQairtLibDirField: document.getElementById("f-backend-qnn-qairt-lib-dir-field"),
    backendReasoningMarkers: document.getElementById("f-backend-reasoning-markers"),
    backendReasoningMarkersField: document.getElementById(
      "f-backend-reasoning-markers-field",
    ),
```

- [ ] **Step 3: Populate the textarea on load**

In `settings.js`, in `populate(view)`, find the QAIRT line (line ~254):

```javascript
  f.backendQnnQairtLibDir.value = view.backend.qnn_qairt_lib_dir ?? "";
  applyBackendKindReveal(view.backend.kind);
```

Insert the reasoning-markers assignment before the `applyBackendKindReveal` call:

```javascript
  f.backendQnnQairtLibDir.value = view.backend.qnn_qairt_lib_dir ?? "";
  f.backendReasoningMarkers.value = view.backend.reasoning_markers ?? "";
  applyBackendKindReveal(view.backend.kind);
```

- [ ] **Step 4: Show/hide the field for ollama + openai-compat only**

In `settings.js`, in `applyBackendKindReveal(kind)`, find the QNN reveal lines (~447–450):

```javascript
  // QNN bundle / QAIRT-lib paths only relevant for the qnn backend.
  const qnn = kind === "qnn";
  dom.fields.backendQnnBundleDirField.hidden = !qnn;
  dom.fields.backendQnnQairtLibDirField.hidden = !qnn;
```

Add the reasoning-markers reveal immediately after:

```javascript
  // QNN bundle / QAIRT-lib paths only relevant for the qnn backend.
  const qnn = kind === "qnn";
  dom.fields.backendQnnBundleDirField.hidden = !qnn;
  dom.fields.backendQnnQairtLibDirField.hidden = !qnn;
  // Reasoning markers only apply to the ollama / openai-compat backends
  // (stub/cloud/qnn ignore them).
  dom.fields.backendReasoningMarkersField.hidden =
    kind !== "ollama" && kind !== "openai-compat";
```

- [ ] **Step 5: Send the textarea value in `gather()`**

In `settings.js`, in `gather()`, find the QNN path fields in the returned `backend` object (lines ~646–651):

```javascript
      // BackendConfigUpdate has no serde(default), so these two QNN path
      // fields are MANDATORY in the IPC payload — always send them (null
      // when blank). They're plain Option<PathBuf> (not secrets), so no
      // Keep/Env dance.
      qnn_bundle_dir: orNull(f.backendQnnBundleDir.value.trim()),
      qnn_qairt_lib_dir: orNull(f.backendQnnQairtLibDir.value.trim()),
    },
```

Add `reasoning_markers` (verbatim — no trimming, so the stored text round-trips exactly; mandatory like the QNN fields):

```javascript
      // BackendConfigUpdate has no serde(default), so these two QNN path
      // fields are MANDATORY in the IPC payload — always send them (null
      // when blank). They're plain Option<PathBuf> (not secrets), so no
      // Keep/Env dance.
      qnn_bundle_dir: orNull(f.backendQnnBundleDir.value.trim()),
      qnn_qairt_lib_dir: orNull(f.backendQnnQairtLibDir.value.trim()),
      // Raw textarea text — also mandatory (no serde default). Sent
      // verbatim (no trim) so the stored text round-trips exactly; the
      // Rust parser handles all whitespace. Empty string when blank.
      reasoning_markers: f.backendReasoningMarkers.value,
    },
```

- [ ] **Step 6: Commit**

```bash
cd /Users/hherb/src/primer
git add src/crates/primer-gui/ui/index.html src/crates/primer-gui/ui/settings.js
git commit -m "feat(gui): reasoning-markers textarea in Settings (populate/gather/reveal)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: Full verification + acceptance

**Files:** none (verification only)

- [ ] **Step 1: Format check**

Run: `cd /Users/hherb/src/primer/src && ~/.cargo/bin/cargo +1.88 fmt --all -- --check`
Expected: clean (no diff).

- [ ] **Step 2: Clippy (workspace, deny warnings)**

Run: `cd /Users/hherb/src/primer/src && ~/.cargo/bin/cargo +1.88 clippy --workspace --all-targets -- -D warnings`
Expected: clean, no warnings.

- [ ] **Step 3: Full workspace test run**

Run: `cd /Users/hherb/src/primer/src && ~/.cargo/bin/cargo +1.88 test --workspace --no-fail-fast`
Expected: all pass (the prior baseline was 942 passed / 0 failed / 4 ignored; this adds 10 parser tests + 4 config tests = expect ~956 passed / 0 failed / 4 ignored). Confirm 0 failures.

- [ ] **Step 4: Manual acceptance (document the result in the commit/PR, not a code change)**

Build and run the GUI: `cd /Users/hherb/src/primer/src && ~/.cargo/bin/cargo run --bin primer-gui`

Confirm each acceptance criterion from the spec:
1. Settings → Inference backend → select **ollama** (or openai-compat): a "Reasoning markers" textarea is **visible**. Select **stub**/**cloud**/**qnn**: it is **hidden**.
2. With ollama selected, type `[[r]] [[/r]]` in the textarea, Save & start a new session. A model that emits `[[r]]…[[/r]]` around its reasoning has that span stripped from the child-visible reply. (If no such model is handy, this can be confirmed at the unit level — the parser test `single_pair` plus the existing `primer-inference` reasoning tests already prove the end-to-end strip; note that in the PR.)
3. Empty textarea ⇒ only built-in defaults strip (no behavior change).
4. Re-open Settings: the textarea shows `[[r]] [[/r]]` again (round-trip).

Note: a `cargo run --bin primer-gui` needs a desktop session; if running headless, rely on the unit coverage and state that explicitly in the PR body.

- [ ] **Step 5: Push and open the PR**

```bash
cd /Users/hherb/src/primer
git push -u origin feat/gui-reasoning-marker-editor
gh pr create --title "feat(gui): custom reasoning-marker editor in Settings" \
  --body "$(cat <<'EOF'
Completes the deferred GUI half of the reasoning-token-stripping feature (PR #187, ROADMAP 0.3).

## What

Settings → Inference backend now has a "Reasoning markers" textarea (shown for ollama / openai-compat). One `open<whitespace>close` pair per line; the built-in defaults always apply, this only adds more. The raw text is stored verbatim in `gui-config.json`; a pure Rust `parse_reasoning_markers` converts it to `(open, close)` pairs at session-wiring time.

## Tests

- 10 unit tests for `parse_reasoning_markers` (split semantics, blank lines, CRLF, incomplete-line drop, internal-space close).
- 4 config tests (default empty, disk round-trip, View/Update pass-through).
- Existing three Update-DTO test JSONs updated for the new mandatory field.

## Acceptance

[fill in manual GUI check results, or note headless + unit coverage]

Spec: docs/superpowers/specs/2026-05-31-gui-reasoning-marker-editor-design.md
Plan: docs/superpowers/plans/2026-05-31-gui-reasoning-marker-editor.md

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

---

## Self-review notes (author)

- **Spec coverage:** parser semantics (Task 1) ✔; config String field + no-serde-default Update (Task 2) ✔; wiring parse (Task 3) ✔; textarea + populate + gather + ollama/openai-compat-only reveal (Task 4) ✔; acceptance (Task 5) ✔.
- **Hidden break caught:** adding a non-`serde(default)` field to `BackendConfigUpdate` breaks 3 existing test JSONs — handled explicitly in Task 2 Step 4 (and the `{"kind":"env"}` `BackendConfig`-load case is correctly left alone).
- **Type/name consistency:** `reasoning_markers` (snake_case Rust + JSON), `backendReasoningMarkers` / `backendReasoningMarkersField` (JS), `f-backend-reasoning-markers` / `f-backend-reasoning-markers-field` (DOM ids), `parse_reasoning_markers` (fn) — consistent across all tasks.
- **No magic numbers introduced.** Empty-string sentinel only.
