# GUI Speech-Backend Selector Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a Settings → Speech dropdown to choose the speech backend (Whisper+Piper vs macOS Native), feature-gated with a disabled-hint when not compiled in, and fix the settings-save bug that silently resets `speech.backend` / `download_timeout_secs`.

**Architecture:** Frontend-only behavior change plus one tiny Tauri capability command. The Rust IPC already round-trips the full `SpeechSettings` (incl. `backend`) through `GuiConfigView`/`GuiConfigUpdate`, so no DTO changes are needed. A new `macos_native_speech_available()` command (mirroring `voice_mode_available()`) tells the frontend whether to enable the macOS-native `<option>`. The settings modal's `populate()` reflects the stored backend and `gather()` now round-trips `backend` + `download_timeout_secs`.

**Tech Stack:** Rust (Tauri 2.x commands, `cfg!` capability flags), vanilla JS (settings modal), HTML.

> **Build note:** All cargo commands run from `src/`. Use `~/.cargo/bin/cargo` (rustup), never Homebrew rust. The default `cargo build`/`cargo test` for `primer-gui` does NOT compile the `macos-native` feature, so `macos_native_speech_available()` returns `false` in CI/default test runs — the tests below are written against the same `cfg!(...)` expression so they hold on both build flavors.

---

### Task 1: Add the `macos_native_speech_available` capability command

**Files:**
- Modify: `src/crates/primer-gui/src/commands/voice.rs` (add command next to `voice_mode_available`, ~line 464; add test in the existing `#[cfg(test)] mod tests`)
- Modify: `src/crates/primer-gui/src/commands/mod.rs:40-41` (register in `generate_handler!`)

- [ ] **Step 1: Write the failing test**

Add to the existing `#[cfg(test)] mod tests` block in `src/crates/primer-gui/src/commands/voice.rs`:

```rust
    /// The macOS-native speech capability reflects the compiled feature
    /// set exactly. Written against the same `cfg!` expression as the
    /// command so it holds on both a default build (false) and a
    /// `--features macos-native` build (true on macOS).
    #[tokio::test]
    async fn macos_native_speech_available_matches_cfg() {
        let expected = cfg!(all(
            target_os = "macos",
            any(feature = "macos-native", feature = "macos-native-26"),
        ));
        assert_eq!(
            macos_native_speech_available().await.unwrap(),
            expected
        );
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd src && ~/.cargo/bin/cargo test -p primer-gui macos_native_speech_available_matches_cfg`
Expected: FAIL — compile error `cannot find function \`macos_native_speech_available\``.

- [ ] **Step 3: Write minimal implementation**

Add immediately after the `voice_mode_available` function (after line 464) in `src/crates/primer-gui/src/commands/voice.rs`:

```rust
/// Whether this binary was compiled with a macOS-native speech stack
/// (`macos-native` or `macos-native-26`). The settings modal uses this
/// to enable/disable the "macOS Native" speech-backend option: selecting
/// it on a build without the feature would silently fall through to
/// whisper/piper (see `voice::backends::build_loop_backends`), so the
/// option is shown-but-disabled with a hint instead. Mirrors
/// `voice_mode_available` — a pure compile-time flag, no session state.
#[tauri::command]
pub async fn macos_native_speech_available() -> Result<bool, String> {
    Ok(cfg!(all(
        target_os = "macos",
        any(feature = "macos-native", feature = "macos-native-26"),
    )))
}
```

Then register it in `src/crates/primer-gui/src/commands/mod.rs` by adding a line after `voice::voice_mode_available,` (line 40):

```rust
        voice::voice_mode_available,
        voice::macos_native_speech_available,
    ])
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd src && ~/.cargo/bin/cargo test -p primer-gui macos_native_speech_available_matches_cfg`
Expected: PASS.

- [ ] **Step 5: Verify the workspace still builds**

Run: `cd src && ~/.cargo/bin/cargo build -p primer-gui`
Expected: Compiles cleanly (registration line type-checks against `generate_handler!`).

- [ ] **Step 6: Commit**

```bash
cd /Users/hherb/src/primer
git add src/crates/primer-gui/src/commands/voice.rs src/crates/primer-gui/src/commands/mod.rs
git commit -m "feat(gui): macos_native_speech_available capability command

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Guard the speech-backend IPC round-trip (Rust regression test)

This task adds a Rust test pinning that a `GuiConfigUpdate` carrying
`"backend":"macos-native"` survives `into_config` — the invariant the frontend
relies on once `gather()` sends the field (Task 4). No production code changes.

**Files:**
- Modify: `src/crates/primer-gui/src/config.rs` (add test in the existing `#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `src/crates/primer-gui/src/config.rs`. The `update_json` mirrors the shape `settings.js::gather()` sends (full config); the assertion targets the speech backend specifically:

```rust
    /// A settings update carrying `speech.backend = macos-native` must
    /// survive `into_config` unchanged. This is the invariant the
    /// frontend relies on once gather() round-trips the field — if a
    /// future refactor drops `speech.backend` from `GuiConfigUpdate` or
    /// `into_config`, the GUI toggle silently reverts to whisper-piper.
    #[test]
    fn update_preserves_macos_native_speech_backend() {
        let current = GuiConfig::default();
        let update_json = r#"{
            "learner": {"name": "Bo", "age": 7, "locale": "en"},
            "backend": {"kind": "stub", "model": null, "ollama_url": "http://localhost:11434",
                "openai_compat_url": "http://localhost:8000",
                "api_key_source": {"kind": "env"},
                "openai_compat_api_key_source": {"kind": "env"},
                "qnn_bundle_dir": null, "qnn_qairt_lib_dir": null, "reasoning_markers": ""},
            "classifier": {"match_main": true, "kind": null, "model": null, "timeout_ms": 3000},
            "extractor": {"match_main": true, "kind": null, "model": null, "timeout_ms": 3000},
            "comprehension": {"match_main": true, "kind": null, "model": null, "timeout_ms": 3000},
            "embedder": {"kind": "none", "model": null, "ollama_url": null, "openai_compat_url": null},
            "vocab": {"max_per_prompt": null},
            "breaks": {"after_mins": 30},
            "persistence": {"session_db": null, "knowledge_db": null, "no_persist": false},
            "ui": {"sidebar_open": true, "last_section": "current_turn"},
            "speech": {"voice_mode_enabled": false, "disable_auto_download": false,
                "backend": "macos-native", "mic_silence_ms": 600,
                "download_timeout_secs": 1800, "overrides": {}}
        }"#;
        let update: GuiConfigUpdate = serde_json::from_str(update_json).unwrap();
        let resolved = update.into_config(&current);
        assert_eq!(resolved.speech.backend, SpeechBackend::MacosNative);
        assert_eq!(resolved.speech.download_timeout_secs, 1800);
    }
```

- [ ] **Step 2: Run test to verify it passes (it should pass immediately)**

Run: `cd src && ~/.cargo/bin/cargo test -p primer-gui update_preserves_macos_native_speech_backend`
Expected: PASS. (The Rust IPC already round-trips `speech` verbatim — this test documents and guards that. If it FAILS, the JSON shape drifted from `GuiConfigUpdate`; reconcile the JSON with the current struct fields before proceeding.)

> Note: this is a characterization test for already-correct behavior, so it passes on first run rather than going red-first. That is intentional — its job is to lock the round-trip the frontend depends on.

- [ ] **Step 3: Commit**

```bash
cd /Users/hherb/src/primer
git add src/crates/primer-gui/src/config.rs
git commit -m "test(gui): pin speech.backend IPC round-trip through into_config

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Add the speech-backend selector to the settings HTML

**Files:**
- Modify: `src/crates/primer-gui/ui/index.html:658-675` (inside `#speech-settings-fields`, before the mic-silence field)

- [ ] **Step 1: Add the dropdown + unavailable hint**

In `src/crates/primer-gui/ui/index.html`, locate `<div id="speech-settings-fields">` (line 658) and `<div class="settings-grid">` (line 659). Insert the backend selector as the first child of the `settings-grid`, immediately before the existing `<label class="field"><span>Mic silence (ms)</span>` block (line 660):

```html
            <div class="settings-grid">
              <label class="field field-full">
                <span>Speech backend</span>
                <select id="f-speech-backend">
                  <option value="whisper-piper">
                    Whisper + Piper (cross-platform)
                  </option>
                  <option value="macos-native">
                    macOS Native (Apple speech)
                  </option>
                </select>
              </label>
              <p
                class="hint muted field-full"
                id="f-speech-backend-unavailable-hint"
                hidden
              >
                macOS Native needs a build with
                <code>--features primer-gui/macos-native</code>.
              </p>
              <label class="field">
                <span>Mic silence (ms)</span>
```

(The existing mic-silence `<label>` and the rest of the grid stay unchanged — you are only inserting the two new blocks above it.)

- [ ] **Step 2: Verify the HTML is well-formed**

Run: `cd src && python3 -c "import xml.dom.minidom,sys; xml.dom.minidom.parse('crates/primer-gui/ui/index.html')" 2>&1 | head -3 || echo "minidom is strict about HTML5; if it errors on void elements that is expected — instead eyeball the diff"`
Expected: Either parses, or fails only on HTML5 void elements (not on your new markup). Primary check: `git diff src/crates/primer-gui/ui/index.html` shows a balanced `<select>...</select>` and the new `<p>` with `hidden`.

- [ ] **Step 3: Commit**

```bash
cd /Users/hherb/src/primer
git add src/crates/primer-gui/ui/index.html
git commit -m "feat(gui): speech-backend select + unavailable hint in Settings

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: Wire the selector in settings.js (populate, capability gating, gather fix)

**Files:**
- Modify: `src/crates/primer-gui/ui/settings.js` — `dom.fields` (~line 82-84), `state` (~line 113-120), `open()` `Promise.all` (line 149-154), `populate()` speech block (line 313-316), `gather()` speech block (line 695-703)

- [ ] **Step 1: Add the new elements to `dom.fields`**

In `src/crates/primer-gui/ui/settings.js`, add to the `dom.fields` object next to the existing speech entries (after `speechOverrides`, ~line 84):

```javascript
    speechBackend: document.getElementById("f-speech-backend"),
    speechBackendUnavailableHint: document.getElementById(
      "f-speech-backend-unavailable-hint",
    ),
```

- [ ] **Step 2: Add capability + timeout snapshot fields to `state`**

In the `state` object (after `lastVoiceModeEnabled`, ~line 113), add:

```javascript
  /// Snapshot of `speech.download_timeout_secs` from the most recent
  /// `get_settings`. The modal doesn't expose a visible input for it, so
  /// gather() round-trips this verbatim — never reset it to the serde
  /// default when the user saves the speech form.
  lastDownloadTimeoutSecs: null,
  /// Whether this binary was compiled with a macOS-native speech stack,
  /// from the `macos_native_speech_available` command. Drives whether the
  /// "macOS Native" backend option is selectable. Cached across opens.
  macosNativeAvailable: false,
```

- [ ] **Step 3: Fetch the capability flag in `open()`'s `Promise.all`**

Replace the `Promise.all` block in `open()` (lines 149-154) with one that also fetches the capability flag:

```javascript
    const [view, sessionInfo, locales, macosNativeAvailable] =
      await Promise.all([
        invoke("get_settings"),
        invoke("current_session_info").catch(() => null),
        localesPromise,
        invoke("macos_native_speech_available").catch(() => false),
      ]);
    state.localeChoices = locales;
    state.macosNativeAvailable = macosNativeAvailable === true;
```

(The `.catch(() => false)` mirrors the defensive `current_session_info` handling — a capability probe failing should degrade to "unavailable", not abort the modal.)

- [ ] **Step 4: Reflect backend + gate the option in `populate()`**

In `populate()`, find the speech block (lines 313-316). After the existing `state.lastVoiceModeEnabled = ...` / `f.speechMicSilenceMs.value = ...` / `f.speechDisableAutoDownload.checked = ...` lines, add:

```javascript
  state.lastDownloadTimeoutSecs = view.speech?.download_timeout_secs ?? null;
  f.speechBackend.value = view.speech?.backend ?? "whisper-piper";
  // Gate the macOS-native option behind the compiled feature. Selecting
  // it on a build without the feature silently falls through to
  // whisper/piper, so show-but-disable with a hint instead of hiding it.
  const macosOption = f.speechBackend.querySelector(
    'option[value="macos-native"]',
  );
  if (macosOption) macosOption.disabled = !state.macosNativeAvailable;
  f.speechBackendUnavailableHint.hidden = state.macosNativeAvailable;
```

- [ ] **Step 5: Round-trip backend + timeout in `gather()`**

In `gather()`, replace the `speech` block (lines 695-703) with one that includes the two previously-dropped fields:

```javascript
    speech: {
      // voice_mode_enabled is owned by a header toggle, not this form —
      // round-trip the persisted value so saving speech settings never
      // silently switches voice mode off.
      voice_mode_enabled: state.lastVoiceModeEnabled,
      disable_auto_download: dom.fields.speechDisableAutoDownload.checked,
      backend: dom.fields.speechBackend.value,
      mic_silence_ms: parseIntOrZero(dom.fields.speechMicSilenceMs.value) || 600,
      // No visible input — round-trip the persisted value so saving never
      // resets it to the serde default. `?? undefined` lets serde apply its
      // default only on the (defensive) never-populated path.
      download_timeout_secs: state.lastDownloadTimeoutSecs ?? undefined,
      overrides: gatherSpeechOverrides(),
    },
```

- [ ] **Step 6: Verify the build embeds the updated assets**

Run: `cd src && ~/.cargo/bin/cargo build -p primer-gui`
Expected: Compiles cleanly (the frontend assets are bundled; this confirms no build break).

- [ ] **Step 7: Commit**

```bash
cd /Users/hherb/src/primer
git add src/crates/primer-gui/ui/settings.js
git commit -m "feat(gui): wire speech-backend selector; fix settings-save reset bug

populate() reflects speech.backend and disables the macOS-native option
when the feature isn't compiled in; gather() now round-trips backend and
download_timeout_secs so saving no longer serde-defaults them.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: Manual end-to-end verification

No code changes — this task confirms the behavior on a real macOS-native build.

**Files:** none.

- [ ] **Step 1: Build the GUI with the macOS-native feature**

Run: `cd src && ~/.cargo/bin/cargo build -p primer-gui --features primer-gui/speech,primer-gui/macos-native`
Expected: Compiles. (Requires `espeak-ng` only for the whisper/piper path, which we are avoiding — not needed here.)

- [ ] **Step 2: Verify the round-trip / reset-bug fix**

1. Launch the GUI build. Open Settings → Speech.
2. Confirm the "Speech backend" dropdown is present and the "macOS Native" option is **enabled** (feature compiled in).
3. Select **macOS Native**, Save.
4. Reopen Settings → confirm it still shows **macOS Native** (no reset).
5. Confirm `~/.primer/gui-config.json` shows `"backend": "macos-native"`.
6. Confirm `download_timeout_secs` retained its prior value (not reset to default) after the save.

Expected: backend persists; no silent revert to whisper-piper.

- [ ] **Step 3: Verify voice mode uses the native stack**

1. Start voice mode.
2. Confirm the logs show **no** `whisper_init_state` / `espeak-ng` lines (those are the whisper/piper path).
3. Speak a short prompt; confirm the Primer responds **aloud** via AVSpeechSynthesizer (the original failing symptom).

Expected: SFSpeechRecognizer + AVSpeechSynthesizer in use; speaking works.

- [ ] **Step 4 (optional): Verify the disabled-hint on a default build**

Run a default build (`~/.cargo/bin/cargo run -p primer-gui --features primer-gui/speech`), open Settings → Speech, confirm "macOS Native" is **disabled** and the hint paragraph is visible.

Expected: option disabled, hint shown.

---

## Notes for the implementer

- The whole change is small; if using subagent-driven development, Tasks 1–4 each produce an independent, compilable commit. Task 2 is a characterization test (passes on first run by design). Task 5 is human-in-the-loop.
- Do NOT add `speech.backend` handling to the Rust `GuiConfigUpdate`/`into_config` — it's already there. The bug is purely that the frontend `gather()` omitted the field.
- The `?? undefined` in gather()'s `download_timeout_secs` is deliberate: emitting `undefined` drops the JSON key so serde's `#[serde(default = "default_download_timeout_secs")]` applies on the defensive never-populated path, while a real captured value is sent verbatim.
