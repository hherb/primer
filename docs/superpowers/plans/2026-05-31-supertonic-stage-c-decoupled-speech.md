# Supertonic 3 Stage C — Decoupled STT/TTS Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the voice-loop TTS a runtime choice independent of STT, delivering Whisper-STT + Supertonic-TTS (the Hindi unlock) through a CLI `--tts` flag and a decoupled GUI two-dropdown selector.

**Architecture:** Two orthogonal selector enums (`SttBackend` / `TtsBackend`) in `primer-speech`. All three voice-loop builders are refactored to accept an injected `Arc<dyn StreamingTextToSpeech>` + `VoiceProfile` instead of constructing the TTS internally. A shared `build_tts` helper (feature-gated per arm) constructs the synthesiser + matching voice; a `build_voice_backends` dispatcher selects the STT builder. CLI and GUI both call these two helpers.

**Tech Stack:** Rust (edition 2024, toolchain 1.88), cargo features, async-trait, serde, clap, Tauri 2.x.

**ALWAYS run cargo from `src/` with `~/.cargo/bin/cargo +1.88`** — the `rust-toolchain.toml` pin is only honored there.

---

## File Structure

**`primer-speech` (core refactor):**
- Create: `crates/primer-speech/src/voice_loop/selectors.rs` — `SttBackend`, `TtsBackend`, `TtsAssets`, `build_tts`, `build_voice_backends`.
- Modify: `crates/primer-speech/src/voice_loop/mod.rs` — module decl + re-exports + drop `piper` gate on `backends`.
- Modify: `crates/primer-speech/src/voice_loop/backends.rs` — inject `tts`+`voice` into `build_local_backends`.
- Modify: `crates/primer-speech/src/voice_loop/backends_macos_native.rs` + `backends_macos_native_26.rs` — inject `tts`+`voice`.

**`primer-cli`:**
- Modify: `crates/primer-cli/Cargo.toml` — `supertonic` feature.
- Modify: `crates/primer-cli/src/main.rs` — `--tts` / `--supertonic-dir` / `--supertonic-voice-style` flags + dispatch.
- Modify: `crates/primer-cli/src/speech_loop/mod.rs` — `SpeechLoopConfig` fields + build via `build_tts`/`build_voice_backends`.

**`primer-gui`:**
- Modify: `crates/primer-gui/Cargo.toml` — `supertonic` feature.
- Modify: `crates/primer-gui/src/config.rs` — `stt_backend`/`tts_backend` + legacy migration + Supertonic override fields.
- Modify: `crates/primer-gui/src/commands/` — `supertonic_tts_available` capability command.
- Modify: `crates/primer-gui/src/voice/assets.rs` + `voice/backends.rs` — resolve + dispatch.
- Modify: `crates/primer-gui/ui/index.html` + `ui/settings.js` — two dropdowns + per-locale Supertonic fields.

**Docs:** `README.md`, `ROADMAP.md`, `CLAUDE.md`.

---

## Phase 1 — `primer-speech` core

### Task 1: `SttBackend` / `TtsBackend` selector enums

**Files:**
- Create: `crates/primer-speech/src/voice_loop/selectors.rs`
- Modify: `crates/primer-speech/src/voice_loop/mod.rs`

- [ ] **Step 1: Write the failing test** — create `selectors.rs` with the enums + a tests module:

```rust
//! Runtime STT/TTS backend selectors and the shared construction helpers
//! that turn a `(SttBackend, TtsBackend)` choice plus assets into a built
//! voice-loop backend set. STT picks which builder skeleton runs; TTS picks
//! which `Arc<dyn StreamingTextToSpeech>` is injected. They vary
//! independently — see
//! `docs/superpowers/specs/2026-05-31-supertonic-stage-c-decoupled-speech-design.md`.

use serde::{Deserialize, Serialize};

/// Which speech-to-text builder skeleton the voice loop runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum SttBackend {
    /// whisper.cpp streaming STT (cross-platform; the only STT on a
    /// default build).
    #[default]
    Whisper,
    /// Apple Speech.framework STT (macOS-only; SFSpeechRecognizer on
    /// `macos-native`, SpeechAnalyzer on `macos-native-26`).
    MacosNative,
}

/// Which synthesiser the voice loop injects as its TTS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum TtsBackend {
    /// Piper ONNX voices (the cross-platform default).
    #[default]
    Piper,
    /// Supertonic 3 multilingual TTS (Hindi/Japanese unlock).
    Supertonic,
    /// Apple AVSpeechSynthesizer (macOS-only).
    MacosNative,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stt_backend_serde_round_trips_kebab_case() {
        assert_eq!(serde_json::to_string(&SttBackend::Whisper).unwrap(), "\"whisper\"");
        assert_eq!(
            serde_json::to_string(&SttBackend::MacosNative).unwrap(),
            "\"macos-native\""
        );
        let parsed: SttBackend = serde_json::from_str("\"macos-native\"").unwrap();
        assert_eq!(parsed, SttBackend::MacosNative);
    }

    #[test]
    fn tts_backend_serde_round_trips_kebab_case() {
        assert_eq!(serde_json::to_string(&TtsBackend::Piper).unwrap(), "\"piper\"");
        assert_eq!(
            serde_json::to_string(&TtsBackend::Supertonic).unwrap(),
            "\"supertonic\""
        );
        assert_eq!(
            serde_json::to_string(&TtsBackend::MacosNative).unwrap(),
            "\"macos-native\""
        );
    }

    #[test]
    fn defaults_are_whisper_and_piper() {
        assert_eq!(SttBackend::default(), SttBackend::Whisper);
        assert_eq!(TtsBackend::default(), TtsBackend::Piper);
    }
}
```

This needs `serde_json` as a dev-dependency in `primer-speech/Cargo.toml`. Check `[dev-dependencies]`; if absent add `serde_json = { workspace = true }`. `serde` is already a dep (piper feature uses serde_json; serde itself is workspace). Add `serde = { workspace = true, features = ["derive"] }` to `[dependencies]` if not present (it is used by other crates; verify).

Then declare the module in `mod.rs` — add near the top module decls (NOT feature-gated; pure data + the helpers are internally gated):

```rust
pub mod selectors;
```

and re-export at the bottom:

```rust
pub use selectors::{SttBackend, TtsBackend};
```

- [ ] **Step 2: Run test to verify it fails (then passes once compiled)**

Run: `~/.cargo/bin/cargo +1.88 test -p primer-speech selectors`
Expected: compile, then 3 tests PASS. If `serde`/`serde_json` missing, fix Cargo.toml first.

- [ ] **Step 3: Commit**

```bash
git add crates/primer-speech/src/voice_loop/selectors.rs crates/primer-speech/src/voice_loop/mod.rs crates/primer-speech/Cargo.toml
git commit -m "feat(speech): SttBackend/TtsBackend runtime selector enums"
```

---

### Task 2: `TtsAssets` + `build_tts` feature-gated constructor

**Files:**
- Modify: `crates/primer-speech/src/voice_loop/selectors.rs`

`build_tts` returns `(Arc<dyn StreamingTextToSpeech>, VoiceProfile)`. Each arm is feature-gated; an uncompiled choice returns a `PrimerError::Speech` naming the cargo feature to rebuild with (mirrors the qnn "rebuild with --features qnn" pattern).

- [ ] **Step 1: Write the failing test** — append to `selectors.rs`:

```rust
use std::path::PathBuf;
use std::sync::Arc;

use primer_core::error::{PrimerError, Result};
use primer_core::speech::{StreamingTextToSpeech, VoiceProfile};

/// Neutral asset bundle for [`build_tts`]. Each backend reads only the
/// fields it needs; the others are ignored. Keeps `build_tts` free of any
/// CLI- or GUI-specific asset type so both callers share one path.
#[derive(Debug, Clone, Default)]
pub struct TtsAssets {
    /// Piper voice ONNX file.
    pub piper_onnx: Option<PathBuf>,
    /// Piper voice JSON sidecar.
    pub piper_config: Option<PathBuf>,
    /// Supertonic `onnx/` asset directory.
    pub supertonic_onnx_dir: Option<PathBuf>,
    /// Supertonic voice-style JSON (e.g. `voice_styles/F1.json`).
    pub supertonic_voice_style: Option<PathBuf>,
    /// BCP-47 / pack-id locale, used by the macOS-native voice and by
    /// Supertonic's synthesis language.
    pub locale: primer_core::i18n::Locale,
}

/// Construct the chosen TTS synthesiser and the matching `VoiceProfile`.
///
/// Feature-gated per arm. A choice whose backend was not compiled into
/// this binary returns a `PrimerError::Speech` naming the cargo feature to
/// rebuild with — deliberately distinct from a generic load failure so the
/// user knows the fix is build-time, not a bad path.
pub fn build_tts(
    choice: TtsBackend,
    assets: &TtsAssets,
) -> Result<(Arc<dyn StreamingTextToSpeech>, VoiceProfile)> {
    match choice {
        TtsBackend::Piper => build_piper_tts(assets),
        TtsBackend::Supertonic => build_supertonic_tts(assets),
        TtsBackend::MacosNative => build_macos_native_tts(assets),
    }
}

#[cfg(feature = "piper")]
fn build_piper_tts(
    assets: &TtsAssets,
) -> Result<(Arc<dyn StreamingTextToSpeech>, VoiceProfile)> {
    let onnx = assets
        .piper_onnx
        .as_ref()
        .ok_or_else(|| PrimerError::Speech("piper TTS requires --voice-onnx".to_string()))?;
    let config = assets
        .piper_config
        .as_ref()
        .ok_or_else(|| PrimerError::Speech("piper TTS requires --voice-config".to_string()))?;
    let tts = crate::PiperTts::new(onnx, config)?;
    let model_id = onnx
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("piper-voice")
        .to_string();
    let voice = VoiceProfile {
        model_id,
        rate: 0.9,
        ..VoiceProfile::default()
    };
    Ok((Arc::new(tts), voice))
}

#[cfg(not(feature = "piper"))]
fn build_piper_tts(
    _assets: &TtsAssets,
) -> Result<(Arc<dyn StreamingTextToSpeech>, VoiceProfile)> {
    Err(PrimerError::Speech(
        "piper TTS selected but this binary was built without the `piper` feature; \
         rebuild with --features piper"
            .to_string(),
    ))
}

#[cfg(feature = "supertonic")]
fn build_supertonic_tts(
    assets: &TtsAssets,
) -> Result<(Arc<dyn StreamingTextToSpeech>, VoiceProfile)> {
    let dir = assets.supertonic_onnx_dir.as_ref().ok_or_else(|| {
        PrimerError::Speech("supertonic TTS requires --supertonic-dir".to_string())
    })?;
    let style = assets.supertonic_voice_style.as_ref().ok_or_else(|| {
        PrimerError::Speech("supertonic TTS requires --supertonic-voice-style".to_string())
    })?;
    let tts = crate::SupertonicTts::new(dir, style)?.with_language(assets.locale.pack_id());
    let model_id = style
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| format!("supertonic-{s}"))
        .unwrap_or_else(|| "supertonic-voice".to_string());
    let voice = VoiceProfile {
        model_id,
        rate: 0.9,
        ..VoiceProfile::default()
    };
    Ok((Arc::new(tts), voice))
}

#[cfg(not(feature = "supertonic"))]
fn build_supertonic_tts(
    _assets: &TtsAssets,
) -> Result<(Arc<dyn StreamingTextToSpeech>, VoiceProfile)> {
    Err(PrimerError::Speech(
        "supertonic TTS selected but this binary was built without the `supertonic` feature; \
         rebuild with --features supertonic"
            .to_string(),
    ))
}

#[cfg(all(target_os = "macos", any(feature = "macos-native", feature = "macos-native-26")))]
fn build_macos_native_tts(
    assets: &TtsAssets,
) -> Result<(Arc<dyn StreamingTextToSpeech>, VoiceProfile)> {
    let bcp47 = assets.locale.bcp47();
    let tts = crate::macos::MacosTextToSpeech::new(&bcp47)?;
    // AVSpeech selects its own voice from the locale; VoiceProfile is ignored.
    Ok((Arc::new(tts), VoiceProfile::default()))
}

#[cfg(not(all(target_os = "macos", any(feature = "macos-native", feature = "macos-native-26"))))]
fn build_macos_native_tts(
    _assets: &TtsAssets,
) -> Result<(Arc<dyn StreamingTextToSpeech>, VoiceProfile)> {
    Err(PrimerError::Speech(
        "macОS-native TTS selected but this binary was built without the `macos-native` feature; \
         rebuild with --features macos-native"
            .to_string(),
    ))
}
```

Add re-exports in `mod.rs`: `pub use selectors::{SttBackend, TtsBackend, TtsAssets, build_tts};`

**Verify `Locale::bcp47()` exists** — grep `crates/primer-core/src/i18n.rs` for `bcp47`. The macos builders call it (`MacosTextToSpeech::new(bcp47)`); confirm the exact method name and adjust. If it's `bcp47()` returning `String`, the code above is correct.

Tests (append; these run on a default build where piper IS compiled but supertonic/macos-native are NOT):

```rust
    #[test]
    fn build_tts_supertonic_without_feature_errors_with_rebuild_hint() {
        // On a default `cargo test` build the `supertonic` feature is off.
        let err = build_tts(TtsBackend::Supertonic, &TtsAssets::default()).unwrap_err();
        let s = format!("{err}");
        assert!(s.contains("supertonic"), "hint must name the feature: {s}");
        assert!(s.contains("--features"), "hint must say rebuild: {s}");
    }
```

(Do NOT assert the piper arm here — on the `voice-loop`/`speech` test build piper IS on, so it would try to load a real model. The supertonic-off path is the always-true assertion on a default test run.)

- [ ] **Step 2: Run test to verify it passes**

Run: `~/.cargo/bin/cargo +1.88 test -p primer-speech --features piper selectors`
Expected: PASS (piper on, supertonic off → the rebuild-hint test holds).

- [ ] **Step 3: Commit**

```bash
git add crates/primer-speech/src/voice_loop/selectors.rs crates/primer-speech/src/voice_loop/mod.rs
git commit -m "feat(speech): build_tts feature-gated TTS constructor + TtsAssets"
```

---

### Task 3: Inject `tts`+`voice` into the Whisper builder

**Files:**
- Modify: `crates/primer-speech/src/voice_loop/backends.rs`
- Modify: `crates/primer-speech/src/voice_loop/mod.rs`

- [ ] **Step 1: Change the module gate** — in `mod.rs`, the `backends` module and its re-export are gated on `silero + whisper + piper + cpal`. Drop `piper` from BOTH (the builder no longer constructs Piper):

```rust
#[cfg(all(feature = "silero", feature = "whisper", feature = "cpal"))]
pub mod backends;
```

and

```rust
#[cfg(all(feature = "silero", feature = "whisper", feature = "cpal"))]
pub use backends::build_local_backends;
```

- [ ] **Step 2: Refactor the signature + TTS construction** in `backends.rs`. Change the module gate at the top of the file the same way (drop `piper`):

```rust
#![cfg(all(feature = "silero", feature = "whisper", feature = "cpal"))]
```

Change the `use crate::{...}` line — remove `PiperTts`:

```rust
use crate::{Resampler, SileroVad, SileroVadParams, WhisperStt};
```

Add `StreamingTextToSpeech` is already imported via the `primer_core::speech` use block (it is). Replace the signature (currently `piper_onnx`, `piper_config`, `whisper_model`, `voice_id`, `locale`, `mic_silence_ms`, `verbose`) with:

```rust
pub async fn build_local_backends(
    tts: std::sync::Arc<dyn StreamingTextToSpeech>,
    voice: VoiceProfile,
    whisper_model: &Path,
    locale: primer_core::i18n::Locale,
    mic_silence_ms: u32,
    verbose: bool,
) -> Result<LocalBackends> {
```

Delete the `// ── Build TTS (piper) ──` block:

```rust
    let tts: Arc<dyn StreamingTextToSpeech> = Arc::new(PiperTts::new(piper_onnx, piper_config)?);
    let tts_sample_rate = tts.sample_rate();
```

replace with:

```rust
    let tts_sample_rate = tts.sample_rate();
```

Delete the later `let voice = VoiceProfile { model_id: voice_id..., rate: 0.9, .. };` block (the caller now passes `voice` in). The `LoopBackends::single_locale(... Arc::clone(&tts), voice, locale)` call already uses the `voice` binding — now it's the parameter.

- [ ] **Step 3: Run existing tests to verify the refactor compiles + passes**

Run: `~/.cargo/bin/cargo +1.88 test -p primer-speech --features voice-loop`
Expected: compile error at call sites is fine for now ONLY inside this crate's own tests — fix any in-crate test callers to construct a `PiperTts` and pass it. Then PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/primer-speech/src/voice_loop/backends.rs crates/primer-speech/src/voice_loop/mod.rs
git commit -m "refactor(speech): inject tts+voice into Whisper build_local_backends"
```

---

### Task 4: Inject `tts`+`voice` into the macОS-native builders

**Files:**
- Modify: `crates/primer-speech/src/voice_loop/backends_macos_native.rs`
- Modify: `crates/primer-speech/src/voice_loop/backends_macos_native_26.rs`

- [ ] **Step 1: `backends_macos_native.rs`** — change the signature (currently `locale`, `mic_silence_ms`, `verbose`) to take the injected TTS first:

```rust
pub async fn build_local_backends_macos_native(
    tts: std::sync::Arc<dyn StreamingTextToSpeech>,
    voice: VoiceProfile,
    locale: primer_core::i18n::Locale,
    mic_silence_ms: u32,
    verbose: bool,
) -> Result<LocalBackends> {
```

Remove the `use crate::macos::{MacosSpeechToText, MacosTextToSpeech};` → keep only `MacosSpeechToText` (STT stays native; TTS is injected). Delete:

```rust
    let tts: Arc<dyn StreamingTextToSpeech> = Arc::new(MacosTextToSpeech::new(bcp47)?);
    let tts_sample_rate = tts.sample_rate();
```

replace with `let tts_sample_rate = tts.sample_rate();`. If the builder constructs its own `VoiceProfile` later, delete that and use the `voice` parameter.

- [ ] **Step 2: `backends_macos_native_26.rs`** — same change. Remove `use crate::macos::MacosTextToSpeech;`, delete the `let tts = Arc::new(MacosTextToSpeech::new(&bcp47)?); let tts_sample_rate = tts.sample_rate();` pair (keep `tts_sample_rate` from the param), thread `voice` in.

- [ ] **Step 3: Verify it compiles** (only meaningful on macOS with the feature):

Run on macOS: `~/.cargo/bin/cargo +1.88 build -p primer-speech --features macos-native,voice-loop`
Expected: compiles. If macOS-26/swiftc unavailable, compile-check `macos-native` only and note `_26` as inspection-reviewed.

- [ ] **Step 4: Commit**

```bash
git add crates/primer-speech/src/voice_loop/backends_macos_native.rs crates/primer-speech/src/voice_loop/backends_macos_native_26.rs
git commit -m "refactor(speech): inject tts+voice into macOS-native builders"
```

---

### Task 5: `build_voice_backends` STT dispatcher

**Files:**
- Modify: `crates/primer-speech/src/voice_loop/selectors.rs`
- Modify: `crates/primer-speech/src/voice_loop/mod.rs`

This centralises the `match stt { … call the right builder }` the CLI/GUI shims do today. It is cfg-gated so the macОS arms only exist where their builders compile.

- [ ] **Step 1: Add the dispatcher** to `selectors.rs` (gated on `voice-loop` building blocks so it only exists where the Whisper builder exists):

```rust
/// Dispatch to the STT builder selected by `stt`, injecting the already-
/// constructed `tts` + `voice`. Centralises the per-STT cfg-gated dispatch
/// the CLI and GUI would otherwise each duplicate.
#[cfg(all(feature = "silero", feature = "whisper", feature = "cpal"))]
#[allow(clippy::too_many_arguments)]
pub async fn build_voice_backends(
    stt: SttBackend,
    tts: Arc<dyn StreamingTextToSpeech>,
    voice: VoiceProfile,
    whisper_model: &std::path::Path,
    locale: primer_core::i18n::Locale,
    mic_silence_ms: u32,
    verbose: bool,
) -> Result<crate::voice_loop::LocalBackends> {
    match stt {
        SttBackend::Whisper => {
            crate::voice_loop::build_local_backends(
                tts, voice, whisper_model, locale, mic_silence_ms, verbose,
            )
            .await
        }
        SttBackend::MacosNative => build_macos_native_stt(tts, voice, locale, mic_silence_ms, verbose).await,
    }
}

#[cfg(all(
    target_os = "macos",
    feature = "macos-native-26",
    not(feature = "macos-native"),
    feature = "silero",
    feature = "whisper",
    feature = "cpal"
))]
async fn build_macos_native_stt(
    tts: Arc<dyn StreamingTextToSpeech>,
    voice: VoiceProfile,
    locale: primer_core::i18n::Locale,
    mic_silence_ms: u32,
    verbose: bool,
) -> Result<crate::voice_loop::LocalBackends> {
    crate::voice_loop::build_local_backends_macos_native_26(tts, voice, locale, mic_silence_ms, verbose).await
}

#[cfg(all(
    target_os = "macos",
    feature = "macos-native",
    not(feature = "macos-native-26"),
    feature = "silero",
    feature = "whisper",
    feature = "cpal"
))]
async fn build_macos_native_stt(
    tts: Arc<dyn StreamingTextToSpeech>,
    voice: VoiceProfile,
    locale: primer_core::i18n::Locale,
    mic_silence_ms: u32,
    verbose: bool,
) -> Result<crate::voice_loop::LocalBackends> {
    crate::voice_loop::build_local_backends_macos_native(tts, voice, locale, mic_silence_ms, verbose).await
}

#[cfg(all(
    not(all(target_os = "macos", feature = "macos-native-26", not(feature = "macos-native"))),
    not(all(target_os = "macos", feature = "macos-native", not(feature = "macos-native-26"))),
    feature = "silero",
    feature = "whisper",
    feature = "cpal"
))]
async fn build_macos_native_stt(
    _tts: Arc<dyn StreamingTextToSpeech>,
    _voice: VoiceProfile,
    _locale: primer_core::i18n::Locale,
    _mic_silence_ms: u32,
    _verbose: bool,
) -> Result<crate::voice_loop::LocalBackends> {
    Err(PrimerError::Speech(
        "macОS-native STT selected but this binary was built without the `macos-native` feature; \
         rebuild with --features macos-native"
            .to_string(),
    ))
}
```

Re-export in `mod.rs` under the same gate:

```rust
#[cfg(all(feature = "silero", feature = "whisper", feature = "cpal"))]
pub use selectors::build_voice_backends;
```

- [ ] **Step 2: Verify compile + full speech test suite**

Run: `~/.cargo/bin/cargo +1.88 test -p primer-speech --features voice-loop`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/primer-speech/src/voice_loop/selectors.rs crates/primer-speech/src/voice_loop/mod.rs
git commit -m "feat(speech): build_voice_backends STT dispatcher"
```

---

## Phase 2 — CLI

### Task 6: CLI `supertonic` feature

**Files:**
- Modify: `crates/primer-cli/Cargo.toml`

- [ ] **Step 1: Add the feature** right after the `speech` feature block:

```toml
# Supertonic 3 TTS as a voice-mode --tts choice. Additive to `speech`;
# forwards to primer-speech/supertonic. Heavy ort+ONNX asset path, so
# opt-in. See issue #170.
supertonic = ["speech", "primer-speech/supertonic"]
```

- [ ] **Step 2: Verify it resolves**

Run: `~/.cargo/bin/cargo +1.88 build -p primer-cli --features supertonic --no-run 2>/dev/null || ~/.cargo/bin/cargo +1.88 check -p primer-cli --features supertonic`
Expected: resolves (will compile vendored supertonic + ort; may download ort runtime — run unsandboxed).

- [ ] **Step 3: Commit**

```bash
git add crates/primer-cli/Cargo.toml
git commit -m "feat(cli): supertonic cargo feature forwarding to primer-speech"
```

---

### Task 7: CLI `--tts` + Supertonic asset flags

**Files:**
- Modify: `crates/primer-cli/src/main.rs`

These flags are declared ONLY on the portable build (the same `not(all(macos, native))` cfg the existing `--voice-onnx` carries — see decision D2). The `--voice-onnx`/`--voice-config` requirement must become conditional on `--tts piper` (the default), not unconditional.

- [ ] **Step 1: Relax the `--speech` requires + add `--tts`** — the `speech: bool` field's portable `requires_all = ["whisper_model", "voice_onnx", "voice_config"]` over-constrains (it forces piper assets even for supertonic). Change the portable arm to require only `whisper_model` (always needed) and add a `--tts` arg with conditional requirements. Replace the portable `arg(...)` on `speech` with:

```rust
        arg(long, requires = "whisper_model")
```

Add a new field after `voice_config` (portable cfg only):

```rust
    /// TTS backend for voice mode: `piper` (default) or `supertonic`.
    /// `supertonic` requires building with `--features supertonic` and
    /// the `--supertonic-dir` + `--supertonic-voice-style` asset flags.
    #[cfg(all(
        feature = "speech",
        not(all(target_os = "macos", any(feature = "macos-native", feature = "macos-native-26")))
    ))]
    #[arg(long, value_enum, default_value_t = TtsChoice::Piper, requires = "speech")]
    tts: TtsChoice,

    /// Supertonic `onnx/` asset directory. Required when `--tts supertonic`.
    #[cfg(all(
        feature = "speech",
        not(all(target_os = "macos", any(feature = "macos-native", feature = "macos-native-26")))
    ))]
    #[arg(long, value_name = "DIR", required_if_eq("tts", "supertonic"))]
    supertonic_dir: Option<PathBuf>,

    /// Supertonic voice-style JSON (e.g. voice_styles/F1.json). Required
    /// when `--tts supertonic`.
    #[cfg(all(
        feature = "speech",
        not(all(target_os = "macos", any(feature = "macos-native", feature = "macos-native-26")))
    ))]
    #[arg(long, value_name = "FILE", required_if_eq("tts", "supertonic"))]
    supertonic_voice_style: Option<PathBuf>,
```

Make `--voice-onnx` / `--voice-config` required only for `--tts piper`. The cleanest clap idiom: add `required_if_eq("tts", "piper")` to each:

```rust
    #[arg(long, value_name = "PATH", required_if_eq("tts", "piper"))]
    voice_onnx: Option<PathBuf>,
```
(same for `voice_config`). Remove them from the `--speech` `requires_all` (done in the relax step above).

Define the `TtsChoice` clap enum near the top of `main.rs` (portable cfg):

```rust
/// CLI value for `--tts`. Mirrors `primer_speech::voice_loop::TtsBackend`
/// minus the macОS-native arm (D2: the CLI native build keeps AVSpeech).
#[cfg(all(
    feature = "speech",
    not(all(target_os = "macos", any(feature = "macos-native", feature = "macos-native-26")))
))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
#[clap(rename_all = "kebab-case")]
enum TtsChoice {
    Piper,
    Supertonic,
}
```

- [ ] **Step 2: Write a clap parse test** — add to the existing CLI test module (find `mod tests` in `main.rs`; the codebase has `Cli::try_parse_from` tests). Add:

```rust
    #[cfg(all(
        feature = "speech",
        not(all(target_os = "macos", any(feature = "macos-native", feature = "macos-native-26")))
    ))]
    #[test]
    fn tts_supertonic_requires_supertonic_assets() {
        // --tts supertonic without the asset flags must fail to parse.
        let res = Cli::try_parse_from([
            "primer", "--speech", "--whisper-model", "/m.bin", "--tts", "supertonic",
        ]);
        assert!(res.is_err(), "supertonic without assets should error");
    }

    #[cfg(all(
        feature = "speech",
        not(all(target_os = "macos", any(feature = "macos-native", feature = "macos-native-26")))
    ))]
    #[test]
    fn tts_supertonic_parses_with_assets() {
        let res = Cli::try_parse_from([
            "primer", "--speech", "--whisper-model", "/m.bin",
            "--tts", "supertonic",
            "--supertonic-dir", "/sup/onnx",
            "--supertonic-voice-style", "/sup/voice_styles/F1.json",
        ]);
        assert!(res.is_ok(), "supertonic with assets should parse: {res:?}");
    }

    #[cfg(all(
        feature = "speech",
        not(all(target_os = "macos", any(feature = "macos-native", feature = "macos-native-26")))
    ))]
    #[test]
    fn tts_piper_default_still_requires_piper_assets() {
        let res = Cli::try_parse_from(["primer", "--speech", "--whisper-model", "/m.bin"]);
        assert!(res.is_err(), "default --tts piper still needs --voice-onnx/--voice-config");
    }
```

- [ ] **Step 3: Run the tests**

Run: `~/.cargo/bin/cargo +1.88 test -p primer-cli --features speech tts_`
Expected: 3 PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/primer-cli/src/main.rs
git commit -m "feat(cli): --tts piper|supertonic + supertonic asset flags"
```

---

### Task 8: CLI dispatch via `build_tts` / `build_voice_backends`

**Files:**
- Modify: `crates/primer-cli/src/speech_loop/mod.rs`
- Modify: `crates/primer-cli/src/main.rs` (the `SpeechLoopConfig` construction site, ~line 1133)

- [ ] **Step 1: Extend `SpeechLoopConfig`** (portable cfg fields) in `speech_loop/mod.rs` — add alongside `voice_onnx`/`voice_config`/`voice_id`:

```rust
    #[cfg(not(all(target_os = "macos", any(feature = "macos-native", feature = "macos-native-26"))))]
    pub tts: primer_speech::voice_loop::TtsBackend,
    #[cfg(not(all(target_os = "macos", any(feature = "macos-native", feature = "macos-native-26"))))]
    pub supertonic_dir: Option<PathBuf>,
    #[cfg(not(all(target_os = "macos", any(feature = "macos-native", feature = "macos-native-26"))))]
    pub supertonic_voice_style: Option<PathBuf>,
```

- [ ] **Step 2: Replace the portable `build_local_backends` call** in `run()` (the `#[cfg(not(any(... macos native ...)))]` arm, ~line 102) with `build_tts` + `build_voice_backends`:

```rust
    #[cfg(not(any(
        all(target_os = "macos", feature = "macos-native-26"),
        all(target_os = "macos", feature = "macos-native"),
    )))]
    let mut local = {
        use primer_speech::voice_loop::{TtsAssets, TtsBackend, build_tts, build_voice_backends, SttBackend};
        let assets = TtsAssets {
            piper_onnx: Some(cfg.voice_onnx.clone()),
            piper_config: Some(cfg.voice_config.clone()),
            supertonic_onnx_dir: cfg.supertonic_dir.clone(),
            supertonic_voice_style: cfg.supertonic_voice_style.clone(),
            locale: cfg.locale,
        };
        let (tts, mut voice) = build_tts(cfg.tts, &assets)?;
        // Piper's CLI voice_id flag overrides the derived model_id so an
        // explicit --voice-id keeps working; Supertonic derives its own.
        if matches!(cfg.tts, TtsBackend::Piper) {
            voice.model_id = cfg.voice_id.clone();
        }
        build_voice_backends(
            SttBackend::Whisper,
            tts,
            voice,
            cfg.whisper_model.as_path(),
            cfg.locale,
            cfg.mic_silence_ms,
            cfg.verbose,
        )
        .await?
    };
```

Note: `voice_onnx`/`voice_config` are `PathBuf` (required for piper via clap). When `--tts supertonic`, clap leaves them `None` — but the portable `SpeechLoopConfig` currently types them as `PathBuf` (non-optional). Change those three (`voice_onnx`, `voice_config`) to `Option<PathBuf>` in `SpeechLoopConfig` and in the construction at main.rs, and map `Some` only when present. Simplest: type `piper_onnx: cfg.voice_onnx.clone()` as `Option<PathBuf>` directly. Update `SpeechLoopConfig.voice_onnx`/`voice_config` to `Option<PathBuf>`.

- [ ] **Step 3: Update the `SpeechLoopConfig` construction** at main.rs (~line 1133). Currently:

```rust
            let voice_onnx = cli.voice_onnx.as_ref().expect("clap requires_all");
            let voice_config = cli.voice_config.as_ref().expect("clap requires_all");
            validate_speech_assets(whisper_model, voice_onnx, voice_config, &cli.voice)?;
            ... SpeechLoopConfig { voice_onnx: voice_onnx.clone(), voice_config: voice_config.clone(), ... }
```

Make piper-asset validation conditional on `--tts piper`; for supertonic, validate the supertonic paths exist instead. Build:

```rust
            let tts = cli.tts.into(); // TtsChoice -> TtsBackend (add a From impl)
            SpeechLoopConfig {
                // ...
                voice_onnx: cli.voice_onnx.clone(),
                voice_config: cli.voice_config.clone(),
                voice_id: cli.voice.clone(),
                tts,
                supertonic_dir: cli.supertonic_dir.clone(),
                supertonic_voice_style: cli.supertonic_voice_style.clone(),
                // ...
            }
```

Add the `From<TtsChoice> for TtsBackend` conversion near `TtsChoice`:

```rust
#[cfg(all(feature = "speech", not(all(target_os = "macos", any(feature = "macos-native", feature = "macos-native-26")))))]
impl From<TtsChoice> for primer_speech::voice_loop::TtsBackend {
    fn from(c: TtsChoice) -> Self {
        match c {
            TtsChoice::Piper => Self::Piper,
            TtsChoice::Supertonic => Self::Supertonic,
        }
    }
}
```

Update `validate_speech_assets` to take the choice + supertonic paths and validate the right set (piper paths for piper; supertonic dir+style for supertonic). Keep the existing piper validation behaviour intact for `--tts piper`.

- [ ] **Step 4: Build + test**

Run: `~/.cargo/bin/cargo +1.88 build -p primer-cli --features speech` then `--features supertonic`.
Expected: both compile. Run `~/.cargo/bin/cargo +1.88 test -p primer-cli --features speech`.

- [ ] **Step 5: Commit**

```bash
git add crates/primer-cli/src/speech_loop/mod.rs crates/primer-cli/src/main.rs
git commit -m "feat(cli): wire --tts through build_tts/build_voice_backends"
```

---

## Phase 3 — GUI

### Task 9: GUI `supertonic` feature

**Files:**
- Modify: `crates/primer-gui/Cargo.toml`

- [ ] **Step 1: Add** after the `speech` feature:

```toml
# Supertonic 3 TTS as a Settings → Speech TTS choice. Additive to `speech`.
supertonic = ["speech", "primer-speech/supertonic"]
```

- [ ] **Step 2: Verify resolve** — `~/.cargo/bin/cargo +1.88 check -p primer-gui --features supertonic`. Expected: resolves.

- [ ] **Step 3: Commit**

```bash
git add crates/primer-gui/Cargo.toml
git commit -m "feat(gui): supertonic cargo feature"
```

---

### Task 10: GUI config — decoupled selectors + legacy migration + Supertonic overrides

**Files:**
- Modify: `crates/primer-gui/src/config.rs`

- [ ] **Step 1: Write the failing tests** in the config test module:

```rust
    #[test]
    fn legacy_backend_macos_native_migrates_to_both_native_halves() {
        // A pre-Stage-C gui-config.json had `speech.backend: "macos-native"`.
        let json = r#"{ "backend": "macos-native" }"#;
        let speech: SpeechSettings = serde_json::from_str(json).unwrap();
        let (stt, tts) = speech.resolve_backends();
        assert_eq!(stt, primer_speech::voice_loop::SttBackend::MacosNative);
        assert_eq!(tts, primer_speech::voice_loop::TtsBackend::MacosNative);
    }

    #[test]
    fn legacy_backend_whisper_piper_migrates_to_whisper_piper() {
        let json = r#"{ "backend": "whisper-piper" }"#;
        let speech: SpeechSettings = serde_json::from_str(json).unwrap();
        let (stt, tts) = speech.resolve_backends();
        assert_eq!(stt, primer_speech::voice_loop::SttBackend::Whisper);
        assert_eq!(tts, primer_speech::voice_loop::TtsBackend::Piper);
    }

    #[test]
    fn new_fields_take_precedence_over_legacy_backend() {
        let json = r#"{ "stt_backend": "whisper", "tts_backend": "supertonic" }"#;
        let speech: SpeechSettings = serde_json::from_str(json).unwrap();
        let (stt, tts) = speech.resolve_backends();
        assert_eq!(stt, primer_speech::voice_loop::SttBackend::Whisper);
        assert_eq!(tts, primer_speech::voice_loop::TtsBackend::Supertonic);
    }

    #[test]
    fn supertonic_override_paths_round_trip() {
        let ov = SpeechLocaleOverride {
            supertonic_onnx_dir: Some("/sup/onnx".into()),
            supertonic_voice_style_path: Some("/sup/F1.json".into()),
            ..SpeechLocaleOverride::default()
        };
        let json = serde_json::to_string(&ov).unwrap();
        let back: SpeechLocaleOverride = serde_json::from_str(&json).unwrap();
        assert_eq!(back.supertonic_onnx_dir, ov.supertonic_onnx_dir);
        assert_eq!(back.supertonic_voice_style_path, ov.supertonic_voice_style_path);
    }
```

- [ ] **Step 2: Implement.** Re-use the `primer-speech` enums (one source of truth) — `primer-gui` already depends on `primer-speech` (optional, under `speech`). BUT config.rs must compile WITHOUT the `speech` feature (it's always built). So define a thin local mirror only if `primer-speech` isn't always available. Check: is `primer-speech` a hard dep or `optional`? It's `optional` (under `speech`). Therefore config.rs cannot name `primer_speech::...::SttBackend` unconditionally.

  **Resolution:** keep `SttBackend`/`TtsBackend` defined in `primer-gui::config` (local enums, kebab-case serde) — the GUI config layer owns its serialized form — and convert to the `primer_speech` enums at the `speech`-gated wiring boundary (`voice/backends.rs`). Update the test paths above to use `crate::config::SttBackend` etc. accordingly. This mirrors how `BackendConfig.kind` is a GUI-owned `String`/enum converted at wiring time.

  Replace the `backend: SpeechBackend` field with:

```rust
    /// STT half of the voice stack. Defaults to `whisper`.
    #[serde(default)]
    pub stt_backend: SttBackend,
    /// TTS half of the voice stack. Defaults to `piper`.
    #[serde(default)]
    pub tts_backend: TtsBackend,
    /// Pre-Stage-C coupled selector (#189). Deserialized for one-time
    /// migration in `resolve_backends`; never re-serialized (skipped when
    /// `None`). Remove once no stored config carries it.
    #[serde(default, skip_serializing)]
    pub backend: Option<SpeechBackend>,
```

Define the two local enums (kebab-case, `Default`) and keep the existing `SpeechBackend` enum (now legacy-only). Add:

```rust
impl SpeechSettings {
    /// Resolve the effective `(SttBackend, TtsBackend)`, applying the
    /// one-time legacy `backend` migration when the new fields are at
    /// their defaults and a legacy value is present.
    pub fn resolve_backends(&self) -> (SttBackend, TtsBackend) {
        if let Some(legacy) = self.backend {
            // New fields default → honor the legacy coupled choice.
            if self.stt_backend == SttBackend::default()
                && self.tts_backend == TtsBackend::default()
            {
                return match legacy {
                    SpeechBackend::WhisperPiper => (SttBackend::Whisper, TtsBackend::Piper),
                    SpeechBackend::MacosNative => (SttBackend::MacosNative, TtsBackend::MacosNative),
                };
            }
        }
        (self.stt_backend, self.tts_backend)
    }
}
```

Add the two fields to `SpeechLocaleOverride`:

```rust
    pub supertonic_onnx_dir: Option<PathBuf>,
    pub supertonic_voice_style_path: Option<PathBuf>,
```

Thread `stt_backend`/`tts_backend` through `SpeechSettings::default()` and the View/Update DTOs IF the speech settings are exposed via IPC (grep for where `SpeechSettings` enters `GuiConfigView`/`into_config`; add the two enum fields + the two per-locale paths to those DTOs, sending kebab-case strings or the enums directly).

- [ ] **Step 3: Run the tests**

Run: `~/.cargo/bin/cargo +1.88 test -p primer-gui --lib config`
Expected: the 4 new tests PASS + existing config tests still green (update any existing test JSON that named `"backend"` to use the new fields, or rely on migration).

- [ ] **Step 4: Commit**

```bash
git add crates/primer-gui/src/config.rs
git commit -m "feat(gui): decoupled stt_backend/tts_backend config + legacy migration"
```

---

### Task 11: GUI `supertonic_tts_available` capability command

**Files:**
- Modify: `crates/primer-gui/src/commands/` (the file holding `macos_native_speech_available` — grep for it)

- [ ] **Step 1: Mirror the existing capability command.** Find `macos_native_speech_available` and add beside it:

```rust
/// True when this binary was compiled with the `supertonic` feature, so
/// the Settings → Speech TTS dropdown can enable the Supertonic option.
#[tauri::command]
pub fn supertonic_tts_available() -> bool {
    cfg!(feature = "supertonic")
}
```

Register it in the Tauri `invoke_handler!` list next to `macos_native_speech_available`.

- [ ] **Step 2: Test** — mirror the sibling capability test (asserts the bool matches `cfg!`):

```rust
    #[test]
    fn supertonic_tts_available_matches_cfg() {
        assert_eq!(supertonic_tts_available(), cfg!(feature = "supertonic"));
    }
```

Run: `~/.cargo/bin/cargo +1.88 test -p primer-gui supertonic_tts_available`. Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/primer-gui/src/commands/
git commit -m "feat(gui): supertonic_tts_available capability command"
```

---

### Task 12: GUI voice asset resolution + backend dispatch

**Files:**
- Modify: `crates/primer-gui/src/voice/assets.rs`
- Modify: `crates/primer-gui/src/voice/backends.rs`

- [ ] **Step 1: Extend `ResolvedAssets`** (in `assets.rs`) with the resolved Supertonic paths (from the active locale's `SpeechLocaleOverride`):

```rust
    pub supertonic_onnx_dir: Option<std::path::PathBuf>,
    pub supertonic_voice_style: Option<std::path::PathBuf>,
```

Populate them in the resolver from the per-locale override (no auto-download — if absent, they stay `None` and selecting Supertonic later errors clearly).

- [ ] **Step 2: Rewrite `build_loop_backends`** (`voice/backends.rs`) to take `(stt, tts)` and dispatch via the shared helpers. Replace the `backend: SpeechBackend` param with `stt: SttBackend, tts: TtsBackend` (the GUI config enums), convert to the `primer_speech` enums, build the TTS, dispatch:

```rust
pub async fn build_loop_backends(
    assets: &ResolvedAssets,
    locale: primer_core::i18n::Locale,
    mic_silence_ms: u32,
    stt: crate::config::SttBackend,
    tts: crate::config::TtsBackend,
) -> Result<primer_speech::voice_loop::LocalBackends, String> {
    use primer_speech::voice_loop::{TtsAssets, build_tts, build_voice_backends};
    let ps_stt = stt.into(); // add From<config::SttBackend> for voice_loop::SttBackend
    let ps_tts = tts.into();
    let tts_assets = TtsAssets {
        piper_onnx: Some(assets.piper_onnx.clone()),
        piper_config: Some(assets.piper_config.clone()),
        supertonic_onnx_dir: assets.supertonic_onnx_dir.clone(),
        supertonic_voice_style: assets.supertonic_voice_style.clone(),
        locale,
    };
    let (tts_arc, mut voice) = build_tts(ps_tts, &tts_assets).map_err(|e| e.to_string())?;
    if matches!(ps_tts, primer_speech::voice_loop::TtsBackend::Piper) {
        voice.model_id = assets.voice_id.clone();
    }
    build_voice_backends(
        ps_stt, tts_arc, voice, &assets.whisper_model, locale, mic_silence_ms, false,
    )
    .await
    .map_err(|e| e.to_string())
}
```

Add `From<config::SttBackend> for primer_speech::voice_loop::SttBackend` (and TTS) in `voice/backends.rs` (this file is `speech`-gated so naming `primer_speech` is fine here). Update the caller in `commands/voice.rs::start_voice_mode` to pass `settings.resolve_backends()`.

- [ ] **Step 3: Build the GUI with speech**

Run: `~/.cargo/bin/cargo +1.88 build -p primer-gui --features speech` then `--features speech,supertonic`.
Expected: both compile. (GUI needs webkit2gtk on Linux; on macOS it builds natively.)

- [ ] **Step 4: Commit**

```bash
git add crates/primer-gui/src/voice/
git commit -m "feat(gui): dispatch voice backends via build_tts/build_voice_backends"
```

---

### Task 13: GUI Settings two dropdowns + Supertonic path fields

**Files:**
- Modify: `crates/primer-gui/ui/index.html`
- Modify: `crates/primer-gui/ui/settings.js`

- [ ] **Step 1: HTML** — in the Speech settings grid, replace the single backend `<select>` (from #189) with two: `id="f-speech-stt-backend"` (options: Whisper, macОS Native) and `id="f-speech-tts-backend"` (options: Piper, Supertonic, macОS Native). Add two text inputs in the per-locale override block: `id="f-speech-supertonic-dir"` and `id="f-speech-supertonic-voice-style"`, with format hints.

- [ ] **Step 2: settings.js** — DOM refs for the four new controls. `populate()` reads `view.speech.stt_backend` / `tts_backend` and the per-locale Supertonic paths; `gather()` sends them (mandatory if the Update DTO field is non-`serde(default)` — send even when hidden). Disable the Supertonic `<option>` and macОS-native `<option>`s based on the capability commands (`supertonic_tts_available`, `macos_native_speech_available`), showing the #189-style "requires building with --features …" hint. Show the Supertonic path inputs only when `tts_backend === "supertonic"` (an `applyTtsReveal()` mirroring `applyBackendKindReveal()`).

- [ ] **Step 3: Manual smoke (owner, post-merge)** — documented in NEXT_SESSION; the JS has no unit harness here. Verify the dropdowns render, options disable correctly, and the Supertonic path fields reveal on selection.

- [ ] **Step 4: Commit**

```bash
git add crates/primer-gui/ui/index.html crates/primer-gui/ui/settings.js
git commit -m "feat(gui): decoupled STT/TTS dropdowns + Supertonic path fields in Settings"
```

---

## Phase 4 — docs + verification

### Task 14: Docs + final verification

**Files:**
- Modify: `README.md`, `ROADMAP.md`, `CLAUDE.md`

- [ ] **Step 1: README** — under voice/speech status, note Supertonic is now a selectable TTS (CLI `--tts supertonic`, GUI Settings) with the Hindi/Japanese coverage; assets supplied manually (auto-download pending Stage D).

- [ ] **Step 2: ROADMAP** — mark issue #170 Stage C done (voice-loop wiring + CLI/GUI selector); Stages D/E/F remain.

- [ ] **Step 3: CLAUDE.md** — add a bullet documenting: TTS is injected into all three voice-loop builders (D1); `build_tts`/`build_voice_backends` are the shared construction path; `SttBackend`/`TtsBackend` live in `primer-speech` (CLI) / mirrored in `primer-gui::config` (GUI converts at wiring); CLI native build stays AVSpeech (D2); legacy `SpeechSettings.backend` migration (D3).

- [ ] **Step 4: Full verification gauntlet** (from `src/`):

```bash
~/.cargo/bin/cargo +1.88 fmt --all -- --check
~/.cargo/bin/cargo +1.88 clippy --workspace --all-targets -- -D warnings
~/.cargo/bin/cargo +1.88 test --workspace --no-fail-fast
~/.cargo/bin/cargo +1.88 build -p primer-cli --features supertonic   # unsandboxed (ort + ~400MB assets)
# macOS host only:
~/.cargo/bin/cargo +1.88 build -p primer-speech --features macos-native,voice-loop
```

Expected: fmt/clippy clean; workspace tests 0 failed; supertonic + macos-native compile.

- [ ] **Step 5: Optional ignored builder smoke** — add an `#[ignore]` test in `selectors.rs` under `#[cfg(feature = "supertonic")]` that runs `build_tts(Supertonic, …)` against `SUPERTONIC_TEST_ONNX_DIR`/`SUPERTONIC_TEST_VOICE_STYLE` env vars (mirror the existing supertonic smoke), asserting a non-error `Arc` + `model_id` starting `"supertonic-"`.

- [ ] **Step 6: Commit + push + PR**

```bash
git add README.md ROADMAP.md CLAUDE.md crates/primer-speech/src/voice_loop/selectors.rs
git commit -m "docs: Supertonic Stage C shipped (README/ROADMAP/CLAUDE) + ignored builder smoke"
git push -u origin feat/supertonic-stage-c-decoupled-speech
gh pr create --title "feat(speech): Supertonic 3 Stage C — decoupled STT/TTS + CLI/GUI selectors (#170)" --body "..."
```

---

## Self-Review notes

- **Spec coverage:** §1 selectors → T1; §2 injection → T3/T4; build_tts/dispatch → T2/T5; CLI → T6–T8; GUI → T9–T13; feature gating → T6/T9/T2/T5; testing → embedded per task; docs → T14. All spec sections mapped.
- **Open risk to confirm during execution:** `primer-gui` depends on `primer-speech` as `optional` (under `speech`), so `config.rs` (always built) cannot name `primer_speech::voice_loop::SttBackend`. T10 resolves this by keeping GUI-owned `config::SttBackend`/`TtsBackend` enums and converting at the `speech`-gated `voice/backends.rs` boundary. Verify the dep is optional during T10 and adjust the test paths to `crate::config::*`.
- **`Locale::bcp47()` / `pack_id()`:** T2 assumes `bcp47()` (macos) and `pack_id()` (supertonic/whisper) exist — both are used by existing builders; confirm exact names when implementing T2.
- **clap `required_if_eq`:** moving `--voice-onnx`/`--voice-config` from `requires_all` to `required_if_eq("tts","piper")` changes parse behaviour; the T7 tests pin both the piper-default and supertonic paths.
