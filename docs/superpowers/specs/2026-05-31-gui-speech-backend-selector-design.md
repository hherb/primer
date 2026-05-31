# GUI speech-backend selector

**Date:** 2026-05-31
**Status:** Approved (design)
**Scope:** `primer-gui` only (frontend + one small Tauri command). No engine/pedagogy changes.

## Problem

Building the GUI with `--features primer-gui/macos-native` does **not**, on its own,
make voice mode use the Apple-native speech stack. The native path in
`primer-gui/src/voice/backends.rs::build_loop_backends` is selected only when
**both**:

1. the binary was compiled with `macos-native` (or `macos-native-26`), **and**
2. the runtime config field `speech.backend == SpeechBackend::MacosNative`.

`SpeechBackend::default()` is `WhisperPiper`, and there is **no UI** to change it —
the only way to flip it is hand-editing `~/.primer/gui-config.json`. A user who built
the native feature but left the default config gets the whisper/piper path: whisper
loads for STT, and piper TTS fails on the bundled-incomplete `espeak-ng-data`
(`espeak-rs-sys/.../espeak-ng-data/phontab: No such file or directory`). Both
symptoms have the same root cause: the native builder was never selected.

### Secondary bug (must fix)

The settings modal's `gather()` (`crates/primer-gui/ui/settings.js`) builds the
`speech` block but **omits** `backend` and `download_timeout_secs`. Because
`SpeechSettings` is `#[serde(default)]`, those fields deserialize back to their
defaults on every save — so opening Settings and clicking Save silently resets
`speech.backend` to `whisper-piper` (and `download_timeout_secs` to its default).
The UI toggle is therefore also the fix for this silent-reset bug.

## Goals

- Add a Settings → Speech control to pick the speech backend
  (Whisper+Piper vs macOS Native).
- The macOS-native option is offered, but **disabled with a hint** on builds that
  weren't compiled with the feature (it would otherwise silently fall through to
  whisper/piper — the exact confusion that triggered this work).
- Saving settings must round-trip `backend` and `download_timeout_secs` so it never
  silently resets them.

## Non-goals (YAGNI)

- No new visible `download_timeout_secs` input — only round-trip the persisted value.
- No voice-mode-interplay changes (`voice_mode_enabled` stays owned by the header
  toggle, round-tripped as today).
- No hot-swap of a running voice loop. Backend change takes effect on the next
  `start_voice_mode`, matching the existing "settings take effect on next start"
  semantics.
- No Rust IPC-DTO changes: `GuiConfigView.speech` / `GuiConfigUpdate.speech` already
  carry the full `SpeechSettings` (incl. `backend`), and `into_config` sets
  `speech: self.speech`.

## Design

### 1. Capability flag — new Tauri command

A macOS-native speech stack exists only when compiled with `macos-native` **or**
`macos-native-26`. Expose this with a plain-bool command that mirrors the existing
`voice_mode_available()` capability command (same file, same shape — keeps
`GuiConfigView` a pure projection of config rather than mixing in compile-time
facts):

```rust
// crates/primer-gui/src/commands/voice.rs — next to voice_mode_available()

#[tauri::command]
pub async fn macos_native_speech_available() -> Result<bool, String> {
    Ok(cfg!(all(
        target_os = "macos",
        any(feature = "macos-native", feature = "macos-native-26"),
    )))
}
```

Register in `commands/mod.rs::register`'s `generate_handler!`. The frontend fetches
it in the same `Promise.all` batch as `get_settings` / `list_locales`.

### 2. HTML — `index.html`, inside `#speech-settings-fields`

Add a backend selector before the existing mic-silence field:

```html
<label class="field">
  <span>Speech backend</span>
  <select id="f-speech-backend">
    <option value="whisper-piper">Whisper + Piper (cross-platform)</option>
    <option value="macos-native">macOS Native (Apple speech)</option>
  </select>
</label>
<p class="hint muted" id="f-speech-backend-unavailable-hint" hidden>
  macOS Native needs a build with
  <code>--features primer-gui/macos-native</code>.
</p>
```

### 3. `settings.js`

- Add `speechBackend` and the hint element to `dom.fields`.
- Add `speech_capabilities` to the `Promise.all` load batch; stash the result
  (e.g. `state.macosNativeAvailable`).
- **populate():**
  - `f.speechBackend.value = view.speech?.backend ?? "whisper-piper"`.
  - When `state.macosNativeAvailable === false`: set the `macos-native`
    `<option>`'s `disabled = true` and unhide
    `#f-speech-backend-unavailable-hint`.
  - Capture `state.lastDownloadTimeoutSecs = view.speech?.download_timeout_secs`
    (alongside the existing `lastVoiceModeEnabled` capture).
- **gather():** add to the `speech` block:
  - `backend: f.speechBackend.value`
  - `download_timeout_secs: state.lastDownloadTimeoutSecs`
  This is the reset-bug fix — both fields are otherwise dropped and serde-defaulted.

### Data flow

```
get_settings ─┐
list_locales  ├─ Promise.all ─→ populate() ─→ select shows speech.backend
speech_capabilities ─┘                         (macos-native disabled if unavailable)

Save → gather() (now incl. backend + download_timeout_secs)
     → update_settings(GuiConfigUpdate)
     → validate → save to disk → swap state.config
Next start_voice_mode reads cfg.speech.backend → build_loop_backends picks native arm.
```

## Testing

- **Rust:** unit test that `macos_native_speech_available()` returns the value
  matching the current build's cfg (i.e. on a default `cargo test` build it is
  `false`; the
  assertion is written against the same `cfg!(...)` expression so it holds on both
  build flavors). Plus a test pinning that a `GuiConfigUpdate` JSON carrying
  `"backend":"macos-native"` survives `into_config` (guards the IPC round-trip; the
  existing config tests already cover the `#[serde(default)]` shape).
- **Frontend:** no JS test harness in-repo. Verify the gather() fix by reasoning +
  a manual save round-trip: set macOS Native, Save, reopen Settings, confirm it
  persists; confirm `~/.primer/gui-config.json` shows `"backend": "macos-native"`.

## Risks / notes

- The two native cargo features are mutually exclusive at compile time
  (`compile_error!` in `lib.rs`) and both map to the single `MacosNative` runtime
  variant, so the dropdown stays two-valued regardless of which native feature is
  built.
- A user on a non-feature build who somehow has `backend: macos-native` in their
  config (e.g. copied a config across machines) sees the disabled option reflect
  the stored value, and that value **persists across Save** (`gather()` reads
  `f.speechBackend.value`, which returns the disabled selection, and
  `update_settings` does not reject it). This is benign: `build_loop_backends`
  falls through to whisper/piper at runtime regardless, and the disabled-with-hint
  state explains why the choice has no effect. We deliberately do not coerce the
  value back to `whisper-piper` on Save — persisting the user's stated intent is
  the more predictable behavior, and the runtime fallthrough makes it safe.
