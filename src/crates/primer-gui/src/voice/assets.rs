//! Voice-asset resolution.
//!
//! `resolve_voice_assets(cfg, locale)` returns either the resolved paths
//! to the model files the active `(stt, tts)` choice consumes (Whisper
//! `.bin`; Piper `.onnx` + `.onnx.json`; or the 7-file Supertonic bundle)
//! or a structured [`AssetMissing`] error the frontend can render.

use std::path::PathBuf;

use crate::commands::voice::{MissingAsset, kind};
use crate::config::SpeechSettings;
use primer_core::consts::speech::{APPROX_PIPER_CONFIG_MB, APPROX_WHISPER_SMALL_MB};
use primer_core::i18n::Locale;
use primer_speech::locale_defaults::{
    DEFAULT_SUPERTONIC_VOICE_STYLE_FILE, LocaleDefault, SupertonicSlot, supertonic_assets,
    voice_default_for,
};

/// Maximum number of `kinds` the IPC will accept in a single
/// `download_voice_assets` call. The legitimate set is at most eight
/// (a Whisper model plus the seven-file Supertonic bundle); this cap is
/// belt-and-suspenders insurance against a buggy or hostile webview
/// submitting a giant payload that would burn memory in the filter loop.
/// Anything above this bound is treated as "nothing to download" — safe
/// in both directions.
pub const MAX_REQUESTED_KINDS: usize = 16;

/// Resolved paths for one voice mode session.
#[derive(Debug, Clone)]
pub struct ResolvedAssets {
    pub piper_onnx: PathBuf,
    pub piper_config: PathBuf,
    pub whisper_model: PathBuf,
    pub voice_id: String,
    /// Effective Supertonic `onnx/` dir. Under `tts == Supertonic` this is
    /// the resolved path (per-locale override, else the default
    /// `<cache>/supertonic/onnx`); for other TTS backends it carries the raw
    /// override (or `None` when unset).
    pub supertonic_onnx_dir: Option<PathBuf>,
    /// Effective Supertonic voice-style JSON. Resolved like
    /// [`Self::supertonic_onnx_dir`] (default `<cache>/supertonic/voice_styles/F1.json`).
    pub supertonic_voice_style: Option<PathBuf>,
}

/// One or more required model files are missing on disk; the user must
/// consent to download (or provide explicit paths in Settings → Speech).
#[derive(Debug, Clone)]
pub struct AssetMissing {
    pub entries: Vec<MissingAsset>,
    pub locale: String,
    /// Sum of `entries`' sizes. Computed for in-crate parity/tests only —
    /// it is NOT carried across the IPC (`StartVoiceModeError::AssetMissing`
    /// serialises just `entries`), and the frontend recomputes its own total
    /// by summing each entry's `approx_size_mb`.
    pub approx_total_mb: u32,
}

/// Per [[project_personal_device_model]], cache lives in the user's home.
pub fn cache_root(home: &std::path::Path) -> PathBuf {
    home.join(".cache").join("primer").join("models")
}

pub fn resolve_voice_assets(
    home: &std::path::Path,
    speech: &SpeechSettings,
    locale: &Locale,
    stt: crate::config::SttBackend,
    tts: crate::config::TtsBackend,
) -> Result<ResolvedAssets, AssetMissing> {
    let default = voice_default_for(locale);
    let override_entry = speech.overrides.get(locale.pack_id());

    let (piper_onnx, piper_config, whisper_model, voice_id) =
        compute_paths(home, locale, default, override_entry);

    // Supertonic effective paths: override wins, else the locale-independent
    // default cache under `supertonic/`. Gated only when TTS is Supertonic.
    let (sup_onnx_dir, sup_voice_style) = supertonic_paths(home, override_entry);
    let mut supertonic_onnx_dir = override_entry.and_then(|o| o.supertonic_onnx_dir.clone());
    let mut supertonic_voice_style =
        override_entry.and_then(|o| o.supertonic_voice_style_path.clone());

    // Decoupled gating: each asset is required only when the resolved
    // (stt, tts) choice actually consumes it. Whisper is gated iff STT is
    // Whisper; Piper files are gated iff TTS is Piper. macOS-native STT/TTS
    // and Supertonic TTS gate nothing here.
    //
    // Every shipping locale today uses a Whisper `small` variant; if a
    // locale upgrades to `medium`/`large` replace the
    // `APPROX_WHISPER_SMALL_MB` references below with a per-id lookup
    // keyed on `d.whisper_model_id`.
    let mut missing = Vec::new();
    if tts == crate::config::TtsBackend::Piper && !piper_onnx.exists() {
        missing.push(MissingAsset {
            kind: kind::PIPER_ONNX.into(),
            path: piper_onnx.clone(),
            suggested_url: default.map(|d| d.piper_onnx_url.to_string()),
            approx_size_mb: default.map(|d| {
                // Piper voice ~ total - whisper. Floor at 1 MB so a freak
                // total ≤ whisper size never produces a misleading 0.
                d.approx_total_mb
                    .saturating_sub(APPROX_WHISPER_SMALL_MB)
                    .max(1)
            }),
        });
    }
    if tts == crate::config::TtsBackend::Piper && !piper_config.exists() {
        missing.push(MissingAsset {
            kind: kind::PIPER_CONFIG.into(),
            path: piper_config.clone(),
            suggested_url: default.map(|d| d.piper_config_url.to_string()),
            approx_size_mb: Some(APPROX_PIPER_CONFIG_MB),
        });
    }
    if stt == crate::config::SttBackend::Whisper && !whisper_model.exists() {
        missing.push(MissingAsset {
            kind: kind::WHISPER_MODEL.into(),
            path: whisper_model.clone(),
            suggested_url: default.map(|d| d.whisper_url.to_string()),
            approx_size_mb: default.map(|_| APPROX_WHISPER_SMALL_MB),
        });
    }

    if tts == crate::config::TtsBackend::Supertonic {
        for asset in supertonic_assets() {
            let path = supertonic_asset_path(&sup_onnx_dir, &sup_voice_style, asset);
            if !path.exists() {
                missing.push(MissingAsset {
                    kind: asset.kind.into(),
                    path,
                    suggested_url: Some(asset.url.to_string()),
                    approx_size_mb: Some(asset.approx_size_mb),
                });
            }
        }
        // Carry the effective paths so the caller builds TtsAssets even when
        // no override was set.
        supertonic_onnx_dir = Some(sup_onnx_dir);
        supertonic_voice_style = Some(sup_voice_style);
    }

    if missing.is_empty() {
        Ok(ResolvedAssets {
            piper_onnx,
            piper_config,
            whisper_model,
            voice_id,
            supertonic_onnx_dir,
            supertonic_voice_style,
        })
    } else {
        // Total reflects exactly what will be downloaded: the sum of the
        // missing entries' sizes. (Previously the Piper/Whisper locale
        // `approx_total_mb` was used; summing the entries is more accurate
        // and works for the Supertonic set too.)
        let approx_total_mb = missing.iter().filter_map(|e| e.approx_size_mb).sum();
        Err(AssetMissing {
            entries: missing,
            locale: locale.pack_id().to_string(),
            approx_total_mb,
        })
    }
}

/// Resolve the effective Supertonic onnx-dir + voice-style path: the
/// per-locale override wins, else the locale-independent default cache
/// location under `supertonic/`.
fn supertonic_paths(
    home: &std::path::Path,
    override_entry: Option<&crate::config::SpeechLocaleOverride>,
) -> (PathBuf, PathBuf) {
    let supertonic_root = cache_root(home).join("supertonic");
    let onnx_dir = override_entry
        .and_then(|o| o.supertonic_onnx_dir.clone())
        .unwrap_or_else(|| supertonic_root.join("onnx"));
    let voice_style = override_entry
        .and_then(|o| o.supertonic_voice_style_path.clone())
        .unwrap_or_else(|| {
            supertonic_root
                .join("voice_styles")
                .join(DEFAULT_SUPERTONIC_VOICE_STYLE_FILE)
        });
    (onnx_dir, voice_style)
}

/// Effective on-disk path for one Supertonic asset, given the resolved
/// onnx-dir + voice-style. Files in the `onnx/` slot live inside the dir;
/// the voice-style slot IS the resolved style path.
fn supertonic_asset_path(
    onnx_dir: &std::path::Path,
    voice_style: &std::path::Path,
    asset: &primer_speech::locale_defaults::SupertonicAsset,
) -> PathBuf {
    match asset.slot {
        SupertonicSlot::OnnxDir => onnx_dir.join(asset.file_name),
        SupertonicSlot::VoiceStyle => voice_style.to_path_buf(),
    }
}

/// Re-resolve a frontend-supplied list of asset kinds against the server's
/// own view of the active locale's voice assets.
///
/// **Why:** keeps the IPC trust boundary tight. The frontend echoes only
/// `kind` strings (`"piper_onnx"`, `"whisper_model"`, the `"supertonic_*"`
/// bundle kinds, …) from the original `AssetMissing` payload; `path` and
/// `suggested_url` are *not* round-tripped through the webview. A
/// compromised webview therefore cannot direct the host to write outside
/// `cache_root(home)` or to fetch from a non-canonical URL — both come from
/// the server's own [`resolve_voice_assets`] call.
///
/// Returns the subset of currently-missing entries whose `kind` matches
/// one of `requested_kinds`. Unknown / already-present kinds are silently
/// dropped (safe — there is nothing to download). An `Ok(ResolvedAssets)`
/// from the inner resolver (every required file present) yields an empty
/// `Vec`, so the caller can unconditionally iterate the result.
pub fn resolve_requested_kinds(
    home: &std::path::Path,
    speech: &SpeechSettings,
    locale: &Locale,
    requested_kinds: &[String],
) -> Vec<MissingAsset> {
    // Cap the request size so a buggy webview submitting a million-entry
    // list cannot blow up the filter. The legitimate set is at most eight
    // entries (Whisper + the seven-file Supertonic bundle);
    // [`MAX_REQUESTED_KINDS`] is comfortably above that.
    if requested_kinds.len() > MAX_REQUESTED_KINDS {
        return Vec::new();
    }
    let (stt, tts) = speech.resolve_backends();
    let missing = match resolve_voice_assets(home, speech, locale, stt, tts) {
        Ok(_) => return Vec::new(),
        Err(am) => am.entries,
    };
    missing
        .into_iter()
        .filter(|entry| requested_kinds.iter().any(|k| k == &entry.kind))
        .collect()
}

fn compute_paths(
    home: &std::path::Path,
    locale: &Locale,
    default: Option<&LocaleDefault>,
    override_entry: Option<&crate::config::SpeechLocaleOverride>,
) -> (PathBuf, PathBuf, PathBuf, String) {
    let voice_id = override_entry
        .and_then(|o| o.voice_id.clone())
        .or_else(|| default.map(|d| d.piper_voice_id.to_string()))
        .unwrap_or_else(|| format!("{}-default", locale.pack_id()));

    let voice_dir = cache_root(home).join("voice").join(locale.pack_id());
    let whisper_dir = cache_root(home).join("whisper");

    let piper_onnx = override_entry
        .and_then(|o| o.piper_onnx_path.clone())
        .unwrap_or_else(|| voice_dir.join(format!("{voice_id}.onnx")));
    let piper_config = override_entry
        .and_then(|o| o.piper_config_path.clone())
        .unwrap_or_else(|| voice_dir.join(format!("{voice_id}.onnx.json")));
    let whisper_model = override_entry
        .and_then(|o| o.whisper_model_path.clone())
        .unwrap_or_else(|| {
            whisper_dir.join(
                default
                    .map(|d| d.whisper_model_id)
                    .unwrap_or("ggml-small.bin"),
            )
        });

    (piper_onnx, piper_config, whisper_model, voice_id)
}

#[cfg(test)]
mod tests;
