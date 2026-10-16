//! IPC types shared by the voice-mode commands: the asset-`kind`
//! identifiers, the structured [`StartVoiceModeError`], the
//! [`MissingAsset`] consent-modal entry, and the pure
//! `missing_to_error` gate for `disable_auto_download`.

use serde::Serialize;

/// Asset-kind identifiers. Shared source of truth between the
/// emit site (`voice::assets::resolve_voice_assets`) and the filter site
/// (`voice::assets::resolve_requested_kinds`). Lives here — not under
/// `voice::assets` — because it describes the IPC shape itself and must
/// be addressable in default (non-speech) builds too (the
/// `MissingAsset` type and its serialisation test are always compiled).
pub mod kind {
    pub const PIPER_ONNX: &str = "piper_onnx";
    pub const PIPER_CONFIG: &str = "piper_config";
    pub const WHISPER_MODEL: &str = "whisper_model";

    // Supertonic 3 bundle — one `kind` per file (6 in onnx/ + 1 voice style).
    // The string values MUST equal the `kind` fields in
    // `primer_speech::locale_defaults::SUPERTONIC_ASSETS`.
    pub const SUPERTONIC_VECTOR_ESTIMATOR: &str = "supertonic_vector_estimator";
    pub const SUPERTONIC_VOCODER: &str = "supertonic_vocoder";
    pub const SUPERTONIC_TEXT_ENCODER: &str = "supertonic_text_encoder";
    pub const SUPERTONIC_DURATION_PREDICTOR: &str = "supertonic_duration_predictor";
    pub const SUPERTONIC_TTS_CONFIG: &str = "supertonic_tts_config";
    pub const SUPERTONIC_UNICODE_INDEXER: &str = "supertonic_unicode_indexer";
    pub const SUPERTONIC_VOICE_STYLE: &str = "supertonic_voice_style";
}

/// Structured error returned by `start_voice_mode`.
///
/// Uses `#[serde(tag = "kind", rename_all = "snake_case")]` so the
/// frontend can switch on `err.kind` without deserializing a nested
/// `message` field when it's not needed.
#[derive(Serialize, Debug)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StartVoiceModeError {
    /// Built without the `speech` cargo feature.
    NotBuilt,
    /// One or more required model files are missing on disk.
    AssetMissing { entries: Vec<MissingAsset> },
    /// One or more required model files are missing AND
    /// `disable_auto_download` is set, so no download is offered. The
    /// frontend renders an informational banner (no Download button)
    /// listing the missing `kind`s and pointing the user at
    /// Settings → Speech.
    AutoDownloadDisabled { entries: Vec<MissingAsset> },
    /// The `RECORD_AUDIO` runtime permission is denied (Android voice mode).
    /// The frontend renders a banner explaining the mic is needed, with an
    /// "Open settings" button wired to the `open_app_settings` command.
    PermissionDenied,
    /// Any other error — message is dev-facing; the frontend renders
    /// a generic banner and does not surface the inner string to the user.
    Other { message: String },
}

/// One missing asset entry in [`StartVoiceModeError::AssetMissing`].
///
/// **IPC direction is server→webview only.** `Deserialize` is deliberately
/// NOT derived: the frontend echoes back only the `kind` strings, and the
/// server re-resolves `path` + `suggested_url` server-side via
/// [`crate::voice::assets::resolve_requested_kinds`]. This keeps the IPC
/// trust boundary tight — a compromised webview cannot direct the host to
/// write outside `~/.cache/primer/models/` or to fetch from a non-canonical
/// URL because those fields never cross the trust boundary as input.
#[derive(Serialize, Clone, Debug)]
pub struct MissingAsset {
    /// Asset type identifier. Stable strings: `"piper_onnx"`,
    /// `"piper_config"`, `"whisper_model"`, and the seven `"supertonic_*"`
    /// bundle kinds (see `primer_speech::locale_defaults::SUPERTONIC_ASSETS`).
    pub kind: String,
    /// Absolute path where the asset was expected.
    pub path: std::path::PathBuf,
    /// Suggested download URL, if known. `None` for assets where
    /// no canonical upstream URL is available.
    pub suggested_url: Option<String>,
    /// Approximate on-disk size in MiB after download. `None` when
    /// unknown. Used by the asset-consent modal to show a budget.
    pub approx_size_mb: Option<u32>,
}

impl From<String> for StartVoiceModeError {
    fn from(message: String) -> Self {
        Self::Other { message }
    }
}

/// Map a resolver `AssetMissing` to the right `start_voice_mode` error,
/// honouring the `disable_auto_download` setting. Pure so the gate is
/// unit-testable without the Tauri command. When auto-download is on, the
/// frontend shows the consent modal (`AssetMissing`); when off, it shows an
/// informational banner with no Download button (`AutoDownloadDisabled`).
#[cfg(feature = "speech")]
pub fn missing_to_error(
    disable_auto_download: bool,
    missing: crate::voice::assets::AssetMissing,
) -> StartVoiceModeError {
    if disable_auto_download {
        StartVoiceModeError::AutoDownloadDisabled {
            entries: missing.entries,
        }
    } else {
        StartVoiceModeError::AssetMissing {
            entries: missing.entries,
        }
    }
}
