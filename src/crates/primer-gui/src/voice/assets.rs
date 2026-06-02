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
/// `download_voice_assets` call. The legitimate set has exactly three
/// entries; this cap is belt-and-suspenders insurance against a buggy or
/// hostile webview submitting a giant payload that would burn memory in
/// the filter loop. Anything above this bound is treated as
/// "nothing to download" — safe in both directions.
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
    // list cannot blow up the filter. The legitimate set has 3 entries;
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
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn missing_all_three_assets_returns_three_entries() {
        let home = TempDir::new().unwrap();
        let speech = SpeechSettings::default();
        let err = resolve_voice_assets(
            home.path(),
            &speech,
            &Locale::English,
            crate::config::SttBackend::Whisper,
            crate::config::TtsBackend::Piper,
        )
        .unwrap_err();
        assert_eq!(
            err.entries.len(),
            3,
            "all three files missing on a fresh home"
        );
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
        let ok = resolve_voice_assets(
            home.path(),
            &speech,
            &Locale::English,
            crate::config::SttBackend::Whisper,
            crate::config::TtsBackend::Piper,
        )
        .unwrap();
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
                supertonic_onnx_dir: None,
                supertonic_voice_style_path: None,
            },
        );

        // Piper config & Whisper still missing; the resolver returns
        // AssetMissing but the piper_onnx entry should NOT be in the
        // missing list because the override-pointed path exists.
        let err = resolve_voice_assets(
            home.path(),
            &speech,
            &Locale::English,
            crate::config::SttBackend::Whisper,
            crate::config::TtsBackend::Piper,
        )
        .unwrap_err();
        let kinds: Vec<&str> = err.entries.iter().map(|e| e.kind.as_str()).collect();
        assert!(!kinds.contains(&"piper_onnx"));
        assert!(kinds.contains(&"piper_config"));
        assert!(kinds.contains(&"whisper_model"));
    }

    /// Fresh home + Supertonic TTS + Whisper STT → the 7 supertonic files
    /// AND the whisper model are all missing (8 entries). Each supertonic
    /// entry carries a canonical HF url and a size; the onnx files resolve
    /// under the default `supertonic/onnx/` cache dir.
    #[test]
    fn supertonic_missing_emits_seven_entries_plus_whisper() {
        let home = TempDir::new().unwrap();
        let speech = SpeechSettings {
            tts_backend: crate::config::TtsBackend::Supertonic,
            ..Default::default()
        };

        let err = resolve_voice_assets(
            home.path(),
            &speech,
            &Locale::English,
            crate::config::SttBackend::Whisper,
            crate::config::TtsBackend::Supertonic,
        )
        .unwrap_err();

        let kinds: Vec<&str> = err.entries.iter().map(|e| e.kind.as_str()).collect();
        assert!(
            kinds.contains(&"whisper_model"),
            "whisper still gated under Whisper STT"
        );
        assert!(kinds.contains(&"supertonic_vector_estimator"));
        assert!(kinds.contains(&"supertonic_voice_style"));
        assert_eq!(
            kinds
                .iter()
                .filter(|k| k.starts_with("supertonic_"))
                .count(),
            7,
            "all seven supertonic files reported missing",
        );
        assert!(!kinds.contains(&"piper_onnx"));
        assert!(!kinds.contains(&"piper_config"));

        let onnx_dir = cache_root(home.path()).join("supertonic").join("onnx");
        for e in err
            .entries
            .iter()
            .filter(|e| e.kind.starts_with("supertonic_"))
        {
            assert!(e.suggested_url.as_deref().unwrap().contains("supertonic-3"));
            assert!(e.approx_size_mb.unwrap() >= 1);
            if e.kind != "supertonic_voice_style" {
                assert!(
                    e.path.starts_with(&onnx_dir),
                    "{} not under onnx dir",
                    e.kind
                );
            }
        }
        assert!(err.approx_total_mb >= 800);
    }

    /// All 7 supertonic files present (+ whisper) → Ok, with the resolved
    /// onnx dir + voice-style pointing at the default cache locations.
    #[test]
    fn supertonic_all_present_resolves_default_cache_paths() {
        let home = TempDir::new().unwrap();
        let whisper_dir = home.path().join(".cache/primer/models/whisper");
        let onnx_dir = home.path().join(".cache/primer/models/supertonic/onnx");
        let styles_dir = home
            .path()
            .join(".cache/primer/models/supertonic/voice_styles");
        std::fs::create_dir_all(&whisper_dir).unwrap();
        std::fs::create_dir_all(&onnx_dir).unwrap();
        std::fs::create_dir_all(&styles_dir).unwrap();
        std::fs::write(whisper_dir.join("ggml-small.en.bin"), b"").unwrap();
        for f in [
            "vector_estimator.onnx",
            "vocoder.onnx",
            "text_encoder.onnx",
            "duration_predictor.onnx",
            "tts.json",
            "unicode_indexer.json",
        ] {
            std::fs::write(onnx_dir.join(f), b"").unwrap();
        }
        std::fs::write(styles_dir.join("F1.json"), b"").unwrap();

        let speech = SpeechSettings {
            tts_backend: crate::config::TtsBackend::Supertonic,
            ..Default::default()
        };
        let ok = resolve_voice_assets(
            home.path(),
            &speech,
            &Locale::English,
            crate::config::SttBackend::Whisper,
            crate::config::TtsBackend::Supertonic,
        )
        .expect("all assets present");
        assert_eq!(ok.supertonic_onnx_dir, Some(onnx_dir));
        assert_eq!(ok.supertonic_voice_style, Some(styles_dir.join("F1.json")));
    }

    /// Partial presence: only the vocoder is on disk → the other 5 onnx
    /// files + the style are still reported (6 supertonic entries).
    #[test]
    fn supertonic_partial_presence_reports_only_the_gaps() {
        let home = TempDir::new().unwrap();
        let whisper_dir = home.path().join(".cache/primer/models/whisper");
        let onnx_dir = home.path().join(".cache/primer/models/supertonic/onnx");
        std::fs::create_dir_all(&whisper_dir).unwrap();
        std::fs::create_dir_all(&onnx_dir).unwrap();
        std::fs::write(whisper_dir.join("ggml-small.en.bin"), b"").unwrap();
        std::fs::write(onnx_dir.join("vocoder.onnx"), b"").unwrap();

        let speech = SpeechSettings {
            tts_backend: crate::config::TtsBackend::Supertonic,
            ..Default::default()
        };
        let err = resolve_voice_assets(
            home.path(),
            &speech,
            &Locale::English,
            crate::config::SttBackend::Whisper,
            crate::config::TtsBackend::Supertonic,
        )
        .unwrap_err();
        let kinds: Vec<&str> = err.entries.iter().map(|e| e.kind.as_str()).collect();
        assert!(
            !kinds.contains(&"supertonic_vocoder"),
            "present file not reported"
        );
        assert_eq!(
            kinds
                .iter()
                .filter(|k| k.starts_with("supertonic_"))
                .count(),
            6,
            "the other six supertonic files still missing",
        );
    }

    /// resolve_requested_kinds re-resolves supertonic kinds server-side.
    #[test]
    fn resolve_requested_kinds_handles_supertonic() {
        let home = TempDir::new().unwrap();
        let speech = SpeechSettings {
            tts_backend: crate::config::TtsBackend::Supertonic,
            ..Default::default()
        };
        let requested = vec![
            "supertonic_vocoder".to_string(),
            "supertonic_voice_style".to_string(),
        ];
        let result = resolve_requested_kinds(home.path(), &speech, &Locale::English, &requested);
        let kinds: std::collections::BTreeSet<&str> =
            result.iter().map(|e| e.kind.as_str()).collect();
        assert_eq!(
            kinds,
            ["supertonic_vocoder", "supertonic_voice_style"]
                .into_iter()
                .collect()
        );
        for e in &result {
            assert!(e.suggested_url.as_deref().unwrap().contains("supertonic-3"));
        }
    }

    /// Decoupling: a Supertonic-TTS session must NOT demand Piper files.
    /// With whisper present, Piper absent, and the override-pointed
    /// Supertonic assets present, the resolve succeeds and the override
    /// paths flow through into the returned `ResolvedAssets`.
    #[test]
    fn supertonic_tts_does_not_gate_piper_files() {
        let home = TempDir::new().unwrap();
        let whisper_dir = home.path().join(".cache/primer/models/whisper");
        std::fs::create_dir_all(&whisper_dir).unwrap();
        std::fs::write(whisper_dir.join("ggml-small.en.bin"), b"").unwrap();

        let sup_onnx_dir = home.path().join("custom/onnx");
        let sup_style = home.path().join("custom/F1.json");
        std::fs::create_dir_all(&sup_onnx_dir).unwrap();
        for f in [
            "vector_estimator.onnx",
            "vocoder.onnx",
            "text_encoder.onnx",
            "duration_predictor.onnx",
            "tts.json",
            "unicode_indexer.json",
        ] {
            std::fs::write(sup_onnx_dir.join(f), b"").unwrap();
        }
        std::fs::write(&sup_style, b"").unwrap();

        let mut speech = SpeechSettings {
            tts_backend: crate::config::TtsBackend::Supertonic,
            ..Default::default()
        };
        speech.overrides.insert(
            "en".to_string(),
            crate::config::SpeechLocaleOverride {
                piper_onnx_path: None,
                piper_config_path: None,
                whisper_model_path: None,
                voice_id: None,
                supertonic_onnx_dir: Some(sup_onnx_dir.clone()),
                supertonic_voice_style_path: Some(sup_style.clone()),
            },
        );

        let ok = resolve_voice_assets(
            home.path(),
            &speech,
            &Locale::English,
            crate::config::SttBackend::Whisper,
            crate::config::TtsBackend::Supertonic,
        )
        .expect("Supertonic TTS must not require Piper files");
        assert_eq!(ok.supertonic_onnx_dir, Some(sup_onnx_dir));
        assert_eq!(ok.supertonic_voice_style, Some(sup_style));
    }

    #[test]
    fn cache_root_is_under_home() {
        let home = std::path::Path::new("/some/home");
        let root = cache_root(home);
        assert_eq!(
            root,
            std::path::Path::new("/some/home/.cache/primer/models")
        );
    }

    /// Hostile-payload defence: a frontend-supplied unknown kind must not
    /// produce an entry the host would download. Resolver knows only
    /// `piper_onnx` / `piper_config` / `whisper_model`; anything else is
    /// dropped silently.
    #[test]
    fn resolve_requested_kinds_drops_unknown_kinds() {
        let home = TempDir::new().unwrap();
        let speech = SpeechSettings::default();
        let requested = vec![
            "whisper_model".to_string(),
            "executable_payload".to_string(),
            "../../../etc/passwd".to_string(),
        ];
        let result = resolve_requested_kinds(home.path(), &speech, &Locale::English, &requested);
        let kinds: Vec<&str> = result.iter().map(|e| e.kind.as_str()).collect();
        assert_eq!(
            kinds,
            vec!["whisper_model"],
            "only the server-known missing kind is returned"
        );
    }

    /// All three legitimate kinds requested on a fresh home → all three
    /// resolver-emitted entries returned.
    #[test]
    fn resolve_requested_kinds_returns_all_three_when_all_missing() {
        let home = TempDir::new().unwrap();
        let speech = SpeechSettings::default();
        let requested = vec![
            "piper_onnx".to_string(),
            "piper_config".to_string(),
            "whisper_model".to_string(),
        ];
        let result = resolve_requested_kinds(home.path(), &speech, &Locale::English, &requested);
        assert_eq!(result.len(), 3);
    }

    /// When every asset is already on disk, the resolver returns Ok and
    /// the helper short-circuits to an empty Vec — no downloads happen.
    #[test]
    fn resolve_requested_kinds_returns_empty_when_all_present() {
        let home = TempDir::new().unwrap();
        let voice_dir = home.path().join(".cache/primer/models/voice/en");
        let whisper_dir = home.path().join(".cache/primer/models/whisper");
        std::fs::create_dir_all(&voice_dir).unwrap();
        std::fs::create_dir_all(&whisper_dir).unwrap();
        std::fs::write(voice_dir.join("en_GB-alba-medium.onnx"), b"").unwrap();
        std::fs::write(voice_dir.join("en_GB-alba-medium.onnx.json"), b"").unwrap();
        std::fs::write(whisper_dir.join("ggml-small.en.bin"), b"").unwrap();

        let speech = SpeechSettings::default();
        let requested = vec!["whisper_model".to_string()];
        let result = resolve_requested_kinds(home.path(), &speech, &Locale::English, &requested);
        assert!(result.is_empty(), "no assets missing → nothing to download");
    }

    /// Every server-resolved path must live under `cache_root(home)`. A
    /// hostile webview cannot inject a `..`-prefixed path through the
    /// IPC because the path field no longer crosses the trust boundary —
    /// it is computed server-side by [`compute_paths`].
    #[test]
    fn resolve_requested_kinds_paths_are_under_cache_root() {
        let home = TempDir::new().unwrap();
        let speech = SpeechSettings::default();
        let requested = vec![
            "piper_onnx".to_string(),
            "piper_config".to_string(),
            "whisper_model".to_string(),
        ];
        let result = resolve_requested_kinds(home.path(), &speech, &Locale::English, &requested);
        let root = cache_root(home.path());
        for entry in &result {
            assert!(
                entry.path.starts_with(&root),
                "server-resolved path {} must live under cache root {}",
                entry.path.display(),
                root.display(),
            );
        }
    }

    /// Empty input → empty output. Defensive: a frontend that already
    /// re-checked locally has nothing to ask the server to fetch.
    #[test]
    fn resolve_requested_kinds_empty_input_returns_empty() {
        let home = TempDir::new().unwrap();
        let speech = SpeechSettings::default();
        let result = resolve_requested_kinds(home.path(), &speech, &Locale::English, &[]);
        assert!(result.is_empty());
    }

    /// A request exceeding [`MAX_REQUESTED_KINDS`] is dropped wholesale —
    /// a buggy / hostile webview submitting a million-entry list cannot
    /// burn host memory in the filter loop. Verified at the bound + 1
    /// to pin the comparison operator (>, not >=).
    #[test]
    fn resolve_requested_kinds_drops_oversized_request() {
        let home = TempDir::new().unwrap();
        let speech = SpeechSettings::default();
        let too_many: Vec<String> = (0..=MAX_REQUESTED_KINDS)
            .map(|_| kind::WHISPER_MODEL.into())
            .collect();
        assert_eq!(too_many.len(), MAX_REQUESTED_KINDS + 1);
        let result = resolve_requested_kinds(home.path(), &speech, &Locale::English, &too_many);
        assert!(
            result.is_empty(),
            "request above MAX_REQUESTED_KINDS must be dropped"
        );

        // At the cap exactly, the resolver still runs.
        let at_cap: Vec<String> = (0..MAX_REQUESTED_KINDS)
            .map(|_| kind::WHISPER_MODEL.into())
            .collect();
        assert_eq!(at_cap.len(), MAX_REQUESTED_KINDS);
        let result_at_cap =
            resolve_requested_kinds(home.path(), &speech, &Locale::English, &at_cap);
        assert_eq!(
            result_at_cap.len(),
            1,
            "request at the cap is still processed and dedupes to the single missing kind"
        );
    }
}
