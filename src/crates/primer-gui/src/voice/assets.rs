//! Voice-asset resolution.
//!
//! `resolve_voice_assets(cfg, locale)` returns either the resolved paths
//! to the three model files (piper .onnx, piper .onnx.json, whisper .bin)
//! or a structured [`AssetMissing`] error the frontend can render.

use std::path::PathBuf;

use crate::commands::voice::MissingAsset;
use crate::config::SpeechSettings;
use primer_core::consts::speech::{APPROX_PIPER_CONFIG_MB, APPROX_WHISPER_SMALL_MB};
use primer_core::i18n::Locale;
use primer_speech::voice_loop::locale_defaults::{voice_default_for, LocaleDefault};

/// Resolved paths for one voice mode session.
#[derive(Debug, Clone)]
pub struct ResolvedAssets {
    pub piper_onnx: PathBuf,
    pub piper_config: PathBuf,
    pub whisper_model: PathBuf,
    pub voice_id: String,
}

/// One or more required model files are missing on disk; the user must
/// consent to download (or provide explicit paths in Settings → Speech).
#[derive(Debug, Clone)]
pub struct AssetMissing {
    pub entries: Vec<MissingAsset>,
    pub locale: String,
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
) -> Result<ResolvedAssets, AssetMissing> {
    let default = voice_default_for(locale);
    let override_entry = speech.overrides.get(locale.pack_id());

    let (piper_onnx, piper_config, whisper_model, voice_id) =
        compute_paths(home, locale, default, override_entry);

    let mut missing = Vec::new();
    if !piper_onnx.exists() {
        missing.push(MissingAsset {
            kind: "piper_onnx".into(),
            path: piper_onnx.clone(),
            suggested_url: default.map(|d| d.piper_onnx_url.to_string()),
            approx_size_mb: default.map(|d| {
                // Piper voice ~ total - whisper_size_mb. Floor at 1 MB so
                // a freak total ≤ whisper_size_mb never overflows.
                d.approx_total_mb.saturating_sub(whisper_size_mb(d)).max(1)
            }),
        });
    }
    if !piper_config.exists() {
        missing.push(MissingAsset {
            kind: "piper_config".into(),
            path: piper_config.clone(),
            suggested_url: default.map(|d| d.piper_config_url.to_string()),
            approx_size_mb: Some(APPROX_PIPER_CONFIG_MB),
        });
    }
    if !whisper_model.exists() {
        missing.push(MissingAsset {
            kind: "whisper_model".into(),
            path: whisper_model.clone(),
            suggested_url: default.map(|d| d.whisper_url.to_string()),
            approx_size_mb: default.map(whisper_size_mb),
        });
    }

    if missing.is_empty() {
        Ok(ResolvedAssets {
            piper_onnx,
            piper_config,
            whisper_model,
            voice_id,
        })
    } else {
        Err(AssetMissing {
            entries: missing,
            locale: locale.pack_id().to_string(),
            approx_total_mb: default.map(|d| d.approx_total_mb).unwrap_or(0),
        })
    }
}

fn whisper_size_mb(_d: &LocaleDefault) -> u32 {
    // Approx split: the Whisper bin is the bulk (~470 MB for small);
    // Piper medium voices are ~60 MB. Used only for consent-dialog
    // labelling. Every shipping locale today uses a Whisper `small`
    // variant; if a locale upgrades to `medium`/`large` add a per-id
    // branch driven by `_d.whisper_model_id`.
    APPROX_WHISPER_SMALL_MB
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
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn missing_all_three_assets_returns_three_entries() {
        let home = TempDir::new().unwrap();
        let speech = SpeechSettings::default();
        let err = resolve_voice_assets(home.path(), &speech, &Locale::English).unwrap_err();
        assert_eq!(err.entries.len(), 3, "all three files missing on a fresh home");
        assert_eq!(err.locale, "en");
        assert!(err.approx_total_mb >= 400);
        let kinds: Vec<&str> = err.entries.iter().map(|e| e.kind.as_str()).collect();
        assert!(kinds.contains(&"piper_onnx"));
        assert!(kinds.contains(&"piper_config"));
        assert!(kinds.contains(&"whisper_model"));
    }

    #[test]
    fn existing_files_resolve_cleanly() {
        let home = TempDir::new().unwrap();
        let voice_dir = home.path().join(".cache/primer/models/voice/en");
        let whisper_dir = home.path().join(".cache/primer/models/whisper");
        std::fs::create_dir_all(&voice_dir).unwrap();
        std::fs::create_dir_all(&whisper_dir).unwrap();
        std::fs::write(voice_dir.join("en_GB-alba-medium.onnx"), b"").unwrap();
        std::fs::write(voice_dir.join("en_GB-alba-medium.onnx.json"), b"").unwrap();
        std::fs::write(whisper_dir.join("ggml-small.en.bin"), b"").unwrap();

        let speech = SpeechSettings::default();
        let ok = resolve_voice_assets(home.path(), &speech, &Locale::English).unwrap();
        assert!(ok.piper_onnx.ends_with("en_GB-alba-medium.onnx"));
        assert_eq!(ok.voice_id, "en_GB-alba-medium");
    }

    #[test]
    fn per_locale_override_path_takes_precedence_over_cache_default() {
        let home = TempDir::new().unwrap();
        let custom = home.path().join("my_voice.onnx");
        std::fs::write(&custom, b"").unwrap();

        let mut speech = SpeechSettings::default();
        speech.overrides.insert(
            "en".to_string(),
            crate::config::SpeechLocaleOverride {
                piper_onnx_path: Some(custom.clone()),
                piper_config_path: None,
                whisper_model_path: None,
                voice_id: Some("my_voice".to_string()),
            },
        );

        // Piper config & Whisper still missing; the resolver returns
        // AssetMissing but the piper_onnx entry should NOT be in the
        // missing list because the override-pointed path exists.
        let err = resolve_voice_assets(home.path(), &speech, &Locale::English).unwrap_err();
        let kinds: Vec<&str> = err.entries.iter().map(|e| e.kind.as_str()).collect();
        assert!(!kinds.contains(&"piper_onnx"));
        assert!(kinds.contains(&"piper_config"));
        assert!(kinds.contains(&"whisper_model"));
    }

    #[test]
    fn cache_root_is_under_home() {
        let home = std::path::Path::new("/some/home");
        let root = cache_root(home);
        assert_eq!(root, std::path::Path::new("/some/home/.cache/primer/models"));
    }
}
