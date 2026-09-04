//! Speech settings — STT/TTS backend selection and per-locale assets.
//!
//! [`SttBackend`] and [`TtsBackend`] are deliberately GUI-local mirrors
//! of the `primer-speech` enums: this module compiles on every build,
//! but `primer-speech` is an optional dependency, so the conversion to
//! the real enums happens at the `speech`-gated wiring boundary rather
//! than here. [`SpeechBackend`] is the pre-split coupled enum, kept only
//! so older `gui-config.json` files still deserialize and migrate.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Which speech backend stack to use. `WhisperPiper` is the default and
/// works on every supported OS. `MacosNative` is macOS-only and requires
/// building with `--features primer-gui/macos-native`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum SpeechBackend {
    #[default]
    WhisperPiper,
    MacosNative,
}

/// STT half of the voice stack (GUI-owned mirror of
/// `primer_speech::voice_loop::SttBackend`; converted at the speech-gated
/// wiring boundary in `voice/backends.rs`). Defined locally because
/// `config.rs` is always compiled but `primer-speech` is an optional dep.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum SttBackend {
    #[default]
    Whisper,
    MacosNative,
}

/// TTS half of the voice stack (GUI-owned mirror of
/// `primer_speech::voice_loop::TtsBackend`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum TtsBackend {
    #[default]
    Piper,
    Supertonic,
    MacosNative,
}

/// Voice-mode settings.
///
/// `voice_mode_enabled` is the sticky toggle (per device, not per
/// learner — see spec rationale). `overrides` is keyed by
/// `Locale::pack_id()` so switching locales doesn't clobber the path
/// the user typed in for the other one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpeechSettings {
    pub voice_mode_enabled: bool,
    pub disable_auto_download: bool,
    /// STT half of the voice stack. Defaults to `whisper`.
    #[serde(default)]
    pub stt_backend: SttBackend,
    /// TTS half of the voice stack. Defaults to `piper`.
    #[serde(default)]
    pub tts_backend: TtsBackend,
    /// Pre-Stage-C coupled selector (#189). Deserialized only so an older
    /// `gui-config.json` that stored `backend` migrates via
    /// [`SpeechSettings::resolve_backends`]; never written back out.
    #[serde(default, skip_serializing)]
    pub backend: Option<SpeechBackend>,
    /// Milliseconds of post-end-of-speech silence the VAD waits before
    /// firing SpeechEnd. Default reads from
    /// `primer_core::consts::speech::DEFAULT_MIC_SILENCE_MS`.
    pub mic_silence_ms: u32,
    /// Overall request timeout, in seconds, for each voice-asset
    /// download. `0` means "no timeout" (NOT recommended — a stalled
    /// connection then locks the consent modal indefinitely). Default
    /// reads from `primer_core::consts::speech::DEFAULT_DOWNLOAD_TIMEOUT_SECS`.
    #[serde(default = "default_download_timeout_secs")]
    pub download_timeout_secs: u64,
    /// Per-locale path / voice-id overrides. Keyed by `Locale::pack_id()`.
    pub overrides: std::collections::BTreeMap<String, SpeechLocaleOverride>,
}

fn default_download_timeout_secs() -> u64 {
    primer_core::consts::speech::DEFAULT_DOWNLOAD_TIMEOUT_SECS
}

impl Default for SpeechSettings {
    fn default() -> Self {
        Self {
            voice_mode_enabled: false,
            disable_auto_download: false,
            stt_backend: SttBackend::default(),
            tts_backend: TtsBackend::default(),
            backend: None,
            mic_silence_ms: primer_core::consts::speech::DEFAULT_MIC_SILENCE_MS,
            download_timeout_secs: default_download_timeout_secs(),
            overrides: std::collections::BTreeMap::new(),
        }
    }
}

impl SpeechSettings {
    /// The effective `(stt, tts)` choice. Applies the one-time legacy
    /// `backend` migration: when the new fields are still at their defaults
    /// AND a legacy `backend` value is present, map the old coupled stack to
    /// the two halves. Otherwise the new fields win.
    ///
    /// "At default" can't distinguish "explicitly chose `whisper`/`piper`"
    /// from "never set," so a config carrying BOTH a legacy `backend` and
    /// new fields pinned to their defaults would migrate to the legacy
    /// stack. That state can't arise from the real save path — old configs
    /// never have the new keys (so migration is correct), and saved configs
    /// never have the legacy key (gather drops it; `backend` is
    /// `skip_serializing`). It is reachable only by hand-editing
    /// `gui-config.json`, where the legacy-wins behaviour is acceptable.
    pub fn resolve_backends(&self) -> (SttBackend, TtsBackend) {
        if let Some(legacy) = self.backend {
            if self.stt_backend == SttBackend::default()
                && self.tts_backend == TtsBackend::default()
            {
                return match legacy {
                    SpeechBackend::WhisperPiper => (SttBackend::Whisper, TtsBackend::Piper),
                    SpeechBackend::MacosNative => {
                        (SttBackend::MacosNative, TtsBackend::MacosNative)
                    }
                };
            }
        }
        (self.stt_backend, self.tts_backend)
    }
}

/// Per-locale path/voice override for `SpeechSettings`. `None` on any
/// field means "fall through to the locale default" (see
/// `primer_speech::locale_defaults::voice_default_for`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SpeechLocaleOverride {
    pub piper_onnx_path: Option<PathBuf>,
    pub piper_config_path: Option<PathBuf>,
    pub whisper_model_path: Option<PathBuf>,
    pub voice_id: Option<String>,
    pub supertonic_onnx_dir: Option<PathBuf>,
    pub supertonic_voice_style_path: Option<PathBuf>,
}
