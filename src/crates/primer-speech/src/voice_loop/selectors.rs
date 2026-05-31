//! Runtime STT/TTS backend selectors and the shared construction helpers
//! that turn a `(SttBackend, TtsBackend)` choice plus assets into a built
//! voice-loop backend set. STT picks which builder skeleton runs; TTS picks
//! which `Arc<dyn StreamingTextToSpeech>` is injected. They vary
//! independently — see
//! `docs/superpowers/specs/2026-05-31-supertonic-stage-c-decoupled-speech-design.md`.

use std::path::PathBuf;
use std::sync::Arc;

use primer_core::error::{PrimerError, Result};
use primer_core::speech::{StreamingTextToSpeech, VoiceProfile};
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
    /// Locale used by the macOS-native voice and by Supertonic's synthesis
    /// language.
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
fn build_piper_tts(assets: &TtsAssets) -> Result<(Arc<dyn StreamingTextToSpeech>, VoiceProfile)> {
    let onnx = assets
        .piper_onnx
        .as_ref()
        .ok_or_else(|| PrimerError::Speech("piper TTS requires a voice ONNX path".to_string()))?;
    let config = assets
        .piper_config
        .as_ref()
        .ok_or_else(|| PrimerError::Speech("piper TTS requires a voice config path".to_string()))?;
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
fn build_piper_tts(_assets: &TtsAssets) -> Result<(Arc<dyn StreamingTextToSpeech>, VoiceProfile)> {
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
        PrimerError::Speech("supertonic TTS requires a model directory".to_string())
    })?;
    let style = assets.supertonic_voice_style.as_ref().ok_or_else(|| {
        PrimerError::Speech("supertonic TTS requires a voice-style path".to_string())
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

#[cfg(all(
    target_os = "macos",
    any(feature = "macos-native", feature = "macos-native-26")
))]
fn build_macos_native_tts(
    assets: &TtsAssets,
) -> Result<(Arc<dyn StreamingTextToSpeech>, VoiceProfile)> {
    let tts = crate::macos::MacosTextToSpeech::new(assets.locale.bcp47())?;
    // AVSpeech selects its own voice from the locale; VoiceProfile is ignored.
    Ok((Arc::new(tts), VoiceProfile::default()))
}

#[cfg(not(all(
    target_os = "macos",
    any(feature = "macos-native", feature = "macos-native-26")
)))]
fn build_macos_native_tts(
    _assets: &TtsAssets,
) -> Result<(Arc<dyn StreamingTextToSpeech>, VoiceProfile)> {
    Err(PrimerError::Speech(
        "macOS-native TTS selected but this binary was built without the `macos-native` feature; \
         rebuild with --features macos-native"
            .to_string(),
    ))
}

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
                tts,
                voice,
                whisper_model,
                locale,
                mic_silence_ms,
                verbose,
            )
            .await
        }
        SttBackend::MacosNative => {
            build_macos_native_stt(tts, voice, locale, mic_silence_ms, verbose).await
        }
    }
}

// macOS 26 STT builder arm (SpeechAnalyzer).
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
    crate::voice_loop::build_local_backends_macos_native_26(
        tts,
        voice,
        locale,
        mic_silence_ms,
        verbose,
    )
    .await
}

// macOS 13+ STT builder arm (SFSpeechRecognizer).
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
    crate::voice_loop::build_local_backends_macos_native(
        tts,
        voice,
        locale,
        mic_silence_ms,
        verbose,
    )
    .await
}

// Fallback arm: no macOS-native STT compiled in — selecting it is a
// build-time error with the rebuild hint (mirrors build_tts's pattern).
#[cfg(all(
    not(all(
        target_os = "macos",
        feature = "macos-native-26",
        not(feature = "macos-native")
    )),
    not(all(
        target_os = "macos",
        feature = "macos-native",
        not(feature = "macos-native-26")
    )),
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
        "macOS-native STT selected but this binary was built without the `macos-native` feature; \
         rebuild with --features macos-native"
            .to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stt_backend_serde_round_trips_kebab_case() {
        assert_eq!(
            serde_json::to_string(&SttBackend::Whisper).unwrap(),
            "\"whisper\""
        );
        assert_eq!(
            serde_json::to_string(&SttBackend::MacosNative).unwrap(),
            "\"macos-native\""
        );
        let parsed: SttBackend = serde_json::from_str("\"macos-native\"").unwrap();
        assert_eq!(parsed, SttBackend::MacosNative);
    }

    #[test]
    fn tts_backend_serde_round_trips_kebab_case() {
        assert_eq!(
            serde_json::to_string(&TtsBackend::Piper).unwrap(),
            "\"piper\""
        );
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

    #[test]
    fn build_tts_supertonic_without_feature_errors_with_rebuild_hint() {
        // The `supertonic` feature is off on the default test build, so this
        // exercises the not(feature) arm regardless of the assets passed.
        // `Arc<dyn StreamingTextToSpeech>` isn't `Debug`, so the `Ok` side
        // can't go through `unwrap_err()` — match the error out directly.
        let s = match build_tts(TtsBackend::Supertonic, &TtsAssets::default()) {
            Ok(_) => panic!("expected an error when the supertonic feature is off"),
            Err(e) => format!("{e}"),
        };
        assert!(s.contains("supertonic"), "hint must name the feature: {s}");
        assert!(s.contains("--features"), "hint must say rebuild: {s}");
    }

    /// Real-asset builder smoke for the Supertonic arm: `build_tts` loads the
    /// four ONNX sessions and returns a voice whose `model_id` is the
    /// `supertonic-<style-stem>` form. Skipped unless both
    /// `$SUPERTONIC_TEST_ONNX_DIR` and `$SUPERTONIC_TEST_VOICE_STYLE` are set
    /// (the ~400 MB bundle isn't in CI). Only one set is a misconfiguration
    /// and panics so the running developer notices. Run with
    /// `--features supertonic ... -- --ignored`.
    #[cfg(feature = "supertonic")]
    #[test]
    #[ignore]
    fn build_tts_supertonic_with_assets_returns_supertonic_voice() {
        let (dir, style) = match (
            std::env::var("SUPERTONIC_TEST_ONNX_DIR").ok(),
            std::env::var("SUPERTONIC_TEST_VOICE_STYLE").ok(),
        ) {
            (Some(d), Some(s)) => (d, s),
            (None, None) => return,
            _ => panic!(
                "SUPERTONIC_TEST_ONNX_DIR and SUPERTONIC_TEST_VOICE_STYLE must be set together"
            ),
        };
        let assets = TtsAssets {
            supertonic_onnx_dir: Some(dir.into()),
            supertonic_voice_style: Some(style.into()),
            ..TtsAssets::default()
        };
        let (_tts, voice) = build_tts(TtsBackend::Supertonic, &assets).expect("build supertonic");
        assert!(
            voice.model_id.starts_with("supertonic-"),
            "voice id must carry the supertonic- prefix, got {:?}",
            voice.model_id
        );
    }
}
