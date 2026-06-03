# Supertonic 3 Stage D — Asset auto-download + consent Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make a fresh GUI Supertonic-TTS session offer a consent download of the ~380 MB Supertonic 3 bundle (mirroring the Piper/Whisper flow) instead of erroring on absent paths, and wire the `disable_auto_download` gate for all backends.

**Architecture:** Model the multilingual Supertonic bundle as 7 individual single-file `kind`s (6 in `onnx/` + 1 voice-style JSON), reusing the existing `download_one`/`stream_to_path` infra unchanged. A locale-independent `SUPERTONIC_ASSETS` table in `primer-speech` carries the HF URLs + sizes; a new `tts == Supertonic` arm in `resolve_voice_assets` emits `MissingAsset` consent entries; a pure `missing_to_error` helper routes the `disable_auto_download` gate.

**Tech Stack:** Rust (edition 2024, toolchain 1.88), Tauri 2.x, the `primer-gui` + `primer-speech` crates. Tests via `cargo +1.88 test` run from `src/`.

**TOOLCHAIN:** Always run cargo from inside `/Users/hherb/src/primer/src` as `~/.cargo/bin/cargo +1.88 …`. The `rust-toolchain.toml` 1.88 pin is only honored there.

**Reference spec:** `docs/superpowers/specs/2026-06-02-supertonic-stage-d-asset-download-design.md`

---

## File structure

- **`src/crates/primer-speech/src/locale_defaults.rs`** (modify) — add `SupertonicSlot` enum, `SupertonicAsset` struct, `SUPERTONIC_ASSETS` const table, `DEFAULT_SUPERTONIC_VOICE_STYLE_FILE` const, `supertonic_assets()` accessor, drift-guard tests. This is the single source of truth for Supertonic download metadata (same nature as `LOCALE_DEFAULTS`), and `primer-gui` already depends on this module.
- **`src/crates/primer-gui/src/commands/voice.rs`** (modify) — add 7 `kind::SUPERTONIC_*` constants; add `StartVoiceModeError::AutoDownloadDisabled` variant; add the pure `missing_to_error` helper; route `start_voice_mode` step 4 through it; tests.
- **`src/crates/primer-gui/src/voice/assets.rs`** (modify) — add `supertonic_paths` + `supertonic_asset_path` helpers; add the `tts == Supertonic` arm to `resolve_voice_assets`; compute `approx_total_mb` from the missing entries; update the existing `supertonic_tts_does_not_gate_piper_files` test; add new Supertonic resolution tests.
- **`src/crates/primer-gui/ui/voice.js`** (modify) — add an `auto_download_disabled` branch (informational banner, no Download button).
- **Docs** (modify, final task) — `README.md`, `ROADMAP.md`, `CLAUDE.md`, `NEXT_SESSION.md` + handoff.

---

## Task 1: `SUPERTONIC_ASSETS` table in `primer-speech`

**Files:**
- Modify: `src/crates/primer-speech/src/locale_defaults.rs`
- Test: same file (`#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing drift-guard test**

Add to the `tests` module at the bottom of `locale_defaults.rs`:

```rust
    #[test]
    fn supertonic_assets_has_seven_entries_six_onnx_one_style() {
        let assets = supertonic_assets();
        assert_eq!(assets.len(), 7, "6 onnx files + 1 voice style");
        let onnx = assets
            .iter()
            .filter(|a| matches!(a.slot, SupertonicSlot::OnnxDir))
            .count();
        let style = assets
            .iter()
            .filter(|a| matches!(a.slot, SupertonicSlot::VoiceStyle))
            .count();
        assert_eq!(onnx, 6, "six files live in the onnx/ dir");
        assert_eq!(style, 1, "one voice-style JSON");
    }

    #[test]
    fn supertonic_asset_urls_are_under_supertonic_3_repo() {
        for a in supertonic_assets() {
            assert!(
                a.url
                    .starts_with("https://huggingface.co/Supertone/supertonic-3/resolve/main/"),
                "{} url not under the supertonic-3 repo: {}",
                a.kind,
                a.url,
            );
            // The URL's tail must match the asset's file name so a typo in
            // either is caught here rather than at download time.
            assert!(a.url.ends_with(a.file_name), "{} url/file_name mismatch", a.kind);
        }
    }

    #[test]
    fn supertonic_asset_kinds_are_unique_and_prefixed() {
        let assets = supertonic_assets();
        for a in assets {
            assert!(a.kind.starts_with("supertonic_"), "{} kind unprefixed", a.kind);
        }
        let mut kinds: Vec<&str> = assets.iter().map(|a| a.kind).collect();
        kinds.sort_unstable();
        kinds.dedup();
        assert_eq!(kinds.len(), assets.len(), "kinds must be unique");
    }

    #[test]
    fn supertonic_total_size_is_sane() {
        let total: u32 = supertonic_assets().iter().map(|a| a.approx_size_mb).sum();
        assert!(
            (350..=420).contains(&total),
            "supertonic bundle total {total} MB outside expected ~380 MB band",
        );
        for a in supertonic_assets() {
            assert!(a.approx_size_mb >= 1, "{} size floored at 1 MB", a.kind);
        }
    }

    #[test]
    fn default_voice_style_file_is_an_entry() {
        // The default cache path uses this filename; it must correspond to
        // the single VoiceStyle asset so the default-path resolution and the
        // download table never drift apart.
        let style = supertonic_assets()
            .iter()
            .find(|a| matches!(a.slot, SupertonicSlot::VoiceStyle))
            .expect("one voice-style asset");
        assert_eq!(style.file_name, DEFAULT_SUPERTONIC_VOICE_STYLE_FILE);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd /Users/hherb/src/primer/src && ~/.cargo/bin/cargo +1.88 test -p primer-speech locale_defaults::tests::supertonic 2>&1 | tail -20`
Expected: FAIL to compile — `supertonic_assets`, `SupertonicSlot`, `SupertonicAsset`, `DEFAULT_SUPERTONIC_VOICE_STYLE_FILE` not defined.

- [ ] **Step 3: Add the types, const table, and accessor**

Insert above the `#[cfg(test)]` module in `locale_defaults.rs` (after `voice_default_for`):

```rust
/// Which Supertonic asset slot a file belongs to. The six model files share
/// one `onnx/` directory (the loader reads them all from a single dir); the
/// voice-style JSON is a sibling under `voice_styles/`. The slot decides how
/// the effective on-disk path is derived when a per-locale override is set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupertonicSlot {
    /// A file inside the shared `onnx/` model directory.
    OnnxDir,
    /// The per-voice style JSON.
    VoiceStyle,
}

/// One downloadable Supertonic asset.
///
/// Unlike [`LocaleDefault`], this table is **locale-independent**: Supertonic
/// is one multilingual model (31 languages), so the same bundle serves every
/// locale. Voice styles (F1..F5, M1..M5) are personas, not languages; Stage D
/// ships only the default `F1` voice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupertonicAsset {
    /// `MissingAsset` `kind` string (e.g. `"supertonic_vocoder"`).
    pub kind: &'static str,
    /// File name on disk and the tail of the download URL.
    pub file_name: &'static str,
    /// Which slot (onnx dir vs voice-style file) this asset occupies.
    pub slot: SupertonicSlot,
    /// Direct Hugging Face download URL.
    pub url: &'static str,
    /// Approximate on-disk size in MB. Drives the oversize cap and the
    /// consent-modal budget. Floored at 1 for the tiny JSON files.
    pub approx_size_mb: u32,
}

/// Default voice-style file shipped by Stage D. The default cache path and
/// the [`SUPERTONIC_ASSETS`] voice-style row both reference this so they can
/// never drift.
pub const DEFAULT_SUPERTONIC_VOICE_STYLE_FILE: &str = "F1.json";

/// The 7 files that make up the default Supertonic bundle (F1 voice).
/// Source: `huggingface.co/Supertone/supertonic-3` (sizes rounded up).
pub const SUPERTONIC_ASSETS: &[SupertonicAsset] = &[
    SupertonicAsset {
        kind: "supertonic_vector_estimator",
        file_name: "vector_estimator.onnx",
        slot: SupertonicSlot::OnnxDir,
        url: "https://huggingface.co/Supertone/supertonic-3/resolve/main/onnx/vector_estimator.onnx",
        approx_size_mb: 245,
    },
    SupertonicAsset {
        kind: "supertonic_vocoder",
        file_name: "vocoder.onnx",
        slot: SupertonicSlot::OnnxDir,
        url: "https://huggingface.co/Supertone/supertonic-3/resolve/main/onnx/vocoder.onnx",
        approx_size_mb: 97,
    },
    SupertonicAsset {
        kind: "supertonic_text_encoder",
        file_name: "text_encoder.onnx",
        slot: SupertonicSlot::OnnxDir,
        url: "https://huggingface.co/Supertone/supertonic-3/resolve/main/onnx/text_encoder.onnx",
        approx_size_mb: 35,
    },
    SupertonicAsset {
        kind: "supertonic_duration_predictor",
        file_name: "duration_predictor.onnx",
        slot: SupertonicSlot::OnnxDir,
        url: "https://huggingface.co/Supertone/supertonic-3/resolve/main/onnx/duration_predictor.onnx",
        approx_size_mb: 4,
    },
    SupertonicAsset {
        kind: "supertonic_tts_config",
        file_name: "tts.json",
        slot: SupertonicSlot::OnnxDir,
        url: "https://huggingface.co/Supertone/supertonic-3/resolve/main/onnx/tts.json",
        approx_size_mb: 1,
    },
    SupertonicAsset {
        kind: "supertonic_unicode_indexer",
        file_name: "unicode_indexer.json",
        slot: SupertonicSlot::OnnxDir,
        url: "https://huggingface.co/Supertone/supertonic-3/resolve/main/onnx/unicode_indexer.json",
        approx_size_mb: 1,
    },
    SupertonicAsset {
        kind: "supertonic_voice_style",
        file_name: "F1.json",
        slot: SupertonicSlot::VoiceStyle,
        url: "https://huggingface.co/Supertone/supertonic-3/resolve/main/voice_styles/F1.json",
        approx_size_mb: 1,
    },
];

/// The default Supertonic bundle (locale-independent). See
/// [`SUPERTONIC_ASSETS`].
pub fn supertonic_assets() -> &'static [SupertonicAsset] {
    SUPERTONIC_ASSETS
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd /Users/hherb/src/primer/src && ~/.cargo/bin/cargo +1.88 test -p primer-speech locale_defaults::tests::supertonic 2>&1 | tail -20`
Expected: PASS (5 supertonic-prefixed tests + `default_voice_style_file_is_an_entry`).

- [ ] **Step 5: Commit**

```bash
cd /Users/hherb/src/primer/src
git add crates/primer-speech/src/locale_defaults.rs
git commit -m "feat(speech): locale-independent SUPERTONIC_ASSETS download table

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: `kind::SUPERTONIC_*` constants in `primer-gui`

**Files:**
- Modify: `src/crates/primer-gui/src/commands/voice.rs:25-29` (the `kind` module)

- [ ] **Step 1: Add the 7 constants**

Replace the `kind` module body (currently three constants) with:

```rust
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
```

- [ ] **Step 2: Write a test pinning the kind strings to the table**

Add to the `#[cfg(test)] mod tests` in `commands/voice.rs` (this guards against the two `kind` lists drifting):

```rust
    #[test]
    fn supertonic_kind_constants_match_the_asset_table() {
        use primer_speech::locale_defaults::supertonic_assets;
        let table_kinds: std::collections::BTreeSet<&str> =
            supertonic_assets().iter().map(|a| a.kind).collect();
        let const_kinds: std::collections::BTreeSet<&str> = [
            kind::SUPERTONIC_VECTOR_ESTIMATOR,
            kind::SUPERTONIC_VOCODER,
            kind::SUPERTONIC_TEXT_ENCODER,
            kind::SUPERTONIC_DURATION_PREDICTOR,
            kind::SUPERTONIC_TTS_CONFIG,
            kind::SUPERTONIC_UNICODE_INDEXER,
            kind::SUPERTONIC_VOICE_STYLE,
        ]
        .into_iter()
        .collect();
        assert_eq!(const_kinds, table_kinds);
    }
```

Note: if `primer_speech` is not already a dependency of `primer-gui` for the non-speech build, this test must be feature-gated. Check `crates/primer-gui/Cargo.toml` — `primer-speech` is an **optional** dep (only under `speech`). So gate the test: prefix it with `#[cfg(feature = "speech")]`.

- [ ] **Step 3: Run the test to verify it passes**

Run: `cd /Users/hherb/src/primer/src && ~/.cargo/bin/cargo +1.88 test -p primer-gui --features speech supertonic_kind_constants 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
cd /Users/hherb/src/primer/src
git add crates/primer-gui/src/commands/voice.rs
git commit -m "feat(gui): supertonic_* asset kind constants

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Supertonic arm in `resolve_voice_assets`

**Files:**
- Modify: `src/crates/primer-gui/src/voice/assets.rs`
- Test: same file

This task adds the path helpers and the gating arm, computes the consent total from the missing entries, and **updates one existing test** whose precondition changes under gating.

- [ ] **Step 1: Write the failing tests**

Add these to the `tests` module in `assets.rs`:

```rust
    /// Fresh home + Supertonic TTS + Whisper STT → the 7 supertonic files
    /// AND the whisper model are all missing (8 entries). Each supertonic
    /// entry carries a canonical HF url and a size; the onnx files resolve
    /// under the default `supertonic/onnx/` cache dir.
    #[test]
    fn supertonic_missing_emits_seven_entries_plus_whisper() {
        let home = TempDir::new().unwrap();
        let mut speech = SpeechSettings::default();
        speech.tts_backend = crate::config::TtsBackend::Supertonic;

        let err = resolve_voice_assets(
            home.path(),
            &speech,
            &Locale::English,
            crate::config::SttBackend::Whisper,
            crate::config::TtsBackend::Supertonic,
        )
        .unwrap_err();

        let kinds: Vec<&str> = err.entries.iter().map(|e| e.kind.as_str()).collect();
        assert!(kinds.contains(&"whisper_model"), "whisper still gated under Whisper STT");
        assert!(kinds.contains(&"supertonic_vector_estimator"));
        assert!(kinds.contains(&"supertonic_voice_style"));
        assert_eq!(
            kinds.iter().filter(|k| k.starts_with("supertonic_")).count(),
            7,
            "all seven supertonic files reported missing",
        );
        // Piper must NOT be gated for a Supertonic session.
        assert!(!kinds.contains(&"piper_onnx"));
        assert!(!kinds.contains(&"piper_config"));

        // Every supertonic entry has a canonical url + size, and the onnx
        // files resolve under cache_root/supertonic/onnx.
        let onnx_dir = cache_root(home.path()).join("supertonic").join("onnx");
        for e in err.entries.iter().filter(|e| e.kind.starts_with("supertonic_")) {
            assert!(e.suggested_url.as_deref().unwrap().contains("supertonic-3"));
            assert!(e.approx_size_mb.unwrap() >= 1);
            if e.kind != "supertonic_voice_style" {
                assert!(e.path.starts_with(&onnx_dir), "{} not under onnx dir", e.kind);
            }
        }
        // Total reflects the whole download (supertonic ~384 + whisper 470).
        assert!(err.approx_total_mb >= 800);
    }

    /// All 7 supertonic files present (+ whisper) → Ok, with the resolved
    /// onnx dir + voice-style pointing at the default cache locations.
    #[test]
    fn supertonic_all_present_resolves_default_cache_paths() {
        let home = TempDir::new().unwrap();
        let whisper_dir = home.path().join(".cache/primer/models/whisper");
        let onnx_dir = home.path().join(".cache/primer/models/supertonic/onnx");
        let styles_dir = home.path().join(".cache/primer/models/supertonic/voice_styles");
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

        let mut speech = SpeechSettings::default();
        speech.tts_backend = crate::config::TtsBackend::Supertonic;
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

        let mut speech = SpeechSettings::default();
        speech.tts_backend = crate::config::TtsBackend::Supertonic;
        let err = resolve_voice_assets(
            home.path(),
            &speech,
            &Locale::English,
            crate::config::SttBackend::Whisper,
            crate::config::TtsBackend::Supertonic,
        )
        .unwrap_err();
        let kinds: Vec<&str> = err.entries.iter().map(|e| e.kind.as_str()).collect();
        assert!(!kinds.contains(&"supertonic_vocoder"), "present file not reported");
        assert_eq!(
            kinds.iter().filter(|k| k.starts_with("supertonic_")).count(),
            6,
            "the other six supertonic files still missing",
        );
    }

    /// resolve_requested_kinds re-resolves supertonic kinds server-side.
    #[test]
    fn resolve_requested_kinds_handles_supertonic() {
        let home = TempDir::new().unwrap();
        let mut speech = SpeechSettings::default();
        speech.tts_backend = crate::config::TtsBackend::Supertonic;
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
                .collect(),
        );
        for e in &result {
            assert!(e.suggested_url.as_deref().unwrap().contains("supertonic-3"));
        }
    }
```

- [ ] **Step 2: Update the existing `supertonic_tts_does_not_gate_piper_files` test**

Under gating, the override Supertonic paths must now point at **existing** files or the resolve returns `AssetMissing`. Replace the existing test body (currently asserts Ok with non-existent override paths) so it creates the override files first:

```rust
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

        // Override Supertonic paths to a custom dir; create the 6 onnx files
        // + the style so the (now-gating) resolver is satisfied.
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

        let mut speech = SpeechSettings::default();
        speech.tts_backend = crate::config::TtsBackend::Supertonic;
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
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cd /Users/hherb/src/primer/src && ~/.cargo/bin/cargo +1.88 test -p primer-gui --features speech voice::assets 2>&1 | tail -30`
Expected: FAIL — `supertonic_paths` not defined / the new arm absent / the four new tests fail.

- [ ] **Step 4: Add the path helpers and the gating arm**

Add the import at the top of `assets.rs` (extend the existing `use primer_speech::locale_defaults::...` line):

```rust
use primer_speech::locale_defaults::{
    DEFAULT_SUPERTONIC_VOICE_STYLE_FILE, LocaleDefault, SupertonicSlot, supertonic_assets,
    voice_default_for,
};
```

Add these helpers below `compute_paths`:

```rust
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
```

Now modify `resolve_voice_assets`. Replace the existing Supertonic block + the `if missing.is_empty()` tail. Specifically, replace lines from the `// Supertonic paths come straight from…` comment through the end of the function body with:

```rust
    // Supertonic effective paths: override wins, else the locale-independent
    // default cache under `supertonic/`. Gated only when TTS is Supertonic.
    let (sup_onnx_dir, sup_voice_style) = supertonic_paths(home, override_entry);
    let mut supertonic_onnx_dir = override_entry.and_then(|o| o.supertonic_onnx_dir.clone());
    let mut supertonic_voice_style =
        override_entry.and_then(|o| o.supertonic_voice_style_path.clone());

    // Decoupled gating: each asset is required only when the resolved
    // (stt, tts) choice actually consumes it. Whisper is gated iff STT is
    // Whisper; Piper files iff TTS is Piper; the 7 Supertonic files iff TTS
    // is Supertonic. macOS-native STT/TTS gate nothing here.
    let mut missing = Vec::new();
    if tts == crate::config::TtsBackend::Piper && !piper_onnx.exists() {
        missing.push(MissingAsset {
            kind: kind::PIPER_ONNX.into(),
            path: piper_onnx.clone(),
            suggested_url: default.map(|d| d.piper_onnx_url.to_string()),
            approx_size_mb: default.map(|d| {
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
```

Note: this removes the now-unused `default`-based `approx_total_mb` expression. Keep the `default` binding (still used by the Piper/Whisper `suggested_url`/size). Confirm no `unused variable` warning on `sup_onnx_dir`/`sup_voice_style` when tts != Supertonic — they are always consumed by `supertonic_paths`'s return and the later `Some(...)` only in the Supertonic arm, but the bindings are read in that arm; for non-Supertonic builds they're constructed-but-unused. Add `let _ = (&sup_onnx_dir, &sup_voice_style);` is NOT needed because they're moved into the `supertonic_onnx_dir`/`supertonic_voice_style` only in the arm — for the non-Supertonic path they're simply dropped, which is fine (no warning for a plain unused owned binding that was used to build nothing). If clippy warns, prefix with `let (sup_onnx_dir, sup_voice_style)` → it's used in the `if tts == Supertonic` arm so the compiler sees a use; no warning expected.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cd /Users/hherb/src/primer/src && ~/.cargo/bin/cargo +1.88 test -p primer-gui --features speech voice::assets 2>&1 | tail -30`
Expected: PASS — all existing assets tests + the 4 new ones + the updated `supertonic_tts_does_not_gate_piper_files`.

Also re-check the existing `missing_all_three_assets_returns_three_entries` still passes (it asserts `approx_total_mb >= 400`; the new sum is 60+1+470 = 531, holds).

- [ ] **Step 6: Commit**

```bash
cd /Users/hherb/src/primer/src
git add crates/primer-gui/src/voice/assets.rs
git commit -m "feat(gui): gate Supertonic assets in resolve_voice_assets (Stage D)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: `disable_auto_download` gate (`missing_to_error` + new error variant)

**Files:**
- Modify: `src/crates/primer-gui/src/commands/voice.rs`
- Test: same file

- [ ] **Step 1: Write the failing test for the pure helper**

Add to `#[cfg(test)] mod tests` in `commands/voice.rs`:

```rust
    #[test]
    fn missing_to_error_offers_download_when_auto_download_enabled() {
        let missing = crate::voice::assets::AssetMissing {
            entries: vec![MissingAsset {
                kind: kind::SUPERTONIC_VOCODER.into(),
                path: std::path::PathBuf::from("/x/vocoder.onnx"),
                suggested_url: Some("https://example/vocoder.onnx".into()),
                approx_size_mb: Some(97),
            }],
            locale: "en".into(),
            approx_total_mb: 97,
        };
        let err = missing_to_error(false, missing);
        match err {
            StartVoiceModeError::AssetMissing { entries } => {
                assert_eq!(entries.len(), 1);
            }
            other => panic!("expected AssetMissing, got {other:?}"),
        }
    }

    #[test]
    fn missing_to_error_blocks_download_when_disabled() {
        let missing = crate::voice::assets::AssetMissing {
            entries: vec![MissingAsset {
                kind: kind::SUPERTONIC_VOCODER.into(),
                path: std::path::PathBuf::from("/x/vocoder.onnx"),
                suggested_url: Some("https://example/vocoder.onnx".into()),
                approx_size_mb: Some(97),
            }],
            locale: "en".into(),
            approx_total_mb: 97,
        };
        let err = missing_to_error(true, missing);
        match err {
            StartVoiceModeError::AutoDownloadDisabled { entries } => {
                assert_eq!(entries.len(), 1, "entries carried for the informational banner");
            }
            other => panic!("expected AutoDownloadDisabled, got {other:?}"),
        }
    }
```

This test references `crate::voice::assets::AssetMissing`; that module is `speech`-gated in some builds. Gate the two tests with `#[cfg(feature = "speech")]` if `voice::assets` is only compiled under `speech`. (Check `crate::voice` mod declaration in `lib.rs` — if `mod voice;` is `#[cfg(feature = "speech")]`, gate these tests.)

- [ ] **Step 2: Run to verify it fails**

Run: `cd /Users/hherb/src/primer/src && ~/.cargo/bin/cargo +1.88 test -p primer-gui --features speech missing_to_error 2>&1 | tail -20`
Expected: FAIL — `missing_to_error` and `AutoDownloadDisabled` not defined.

- [ ] **Step 3: Add the variant and helper**

Add the variant to `StartVoiceModeError`:

```rust
    /// One or more required model files are missing AND
    /// `disable_auto_download` is set, so no download is offered. The
    /// frontend renders an informational banner (no Download button)
    /// listing the missing `kind`s and pointing the user at
    /// Settings → Speech.
    AutoDownloadDisabled { entries: Vec<MissingAsset> },
```

Add the pure helper (place it after the `From<String>` impl, gate it on `speech` since it names `voice::assets::AssetMissing`):

```rust
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
```

- [ ] **Step 4: Wire `start_voice_mode` step 4 through the helper**

Replace the resolve-and-map block (currently lines ~126-130) with:

```rust
    let assets =
        crate::voice::assets::resolve_voice_assets(&state.home, &cfg.speech, &locale, stt, tts)
            .map_err(|missing| missing_to_error(cfg.speech.disable_auto_download, missing))?;
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cd /Users/hherb/src/primer/src && ~/.cargo/bin/cargo +1.88 test -p primer-gui --features speech missing_to_error 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
cd /Users/hherb/src/primer/src
git add crates/primer-gui/src/commands/voice.rs
git commit -m "feat(gui): honour disable_auto_download via missing_to_error gate

Adds StartVoiceModeError::AutoDownloadDisabled and a pure helper that
routes a missing-assets resolution to either the consent modal
(AssetMissing) or an informational banner (AutoDownloadDisabled),
backend-agnostically. Closes the long-standing no-op gap where the flag
was stored but never enforced.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: Frontend `auto_download_disabled` branch

**Files:**
- Modify: `src/crates/primer-gui/ui/voice.js` (the `asset_missing` handler around line 82)

- [ ] **Step 1: Read the current handler + consent helpers**

Run: `cd /Users/hherb/src/primer/src && sed -n '70,120p' crates/primer-gui/ui/voice.js`
Confirm the exact shape of the `if (err && err.kind === "asset_missing")` branch and how `showConsentModal` / banners are surfaced.

- [ ] **Step 2: Add the new branch**

Immediately after the existing `asset_missing` branch, add a sibling branch (adapt identifiers to the actual ones found in Step 1 — the existing error-banner mechanism is reused, NOT a new modal):

```javascript
      if (err && err.kind === "auto_download_disabled") {
        const names = (err.entries || []).map((e) => e.kind).join(", ");
        // No consent modal: auto-download is disabled. Inform the user and
        // point them at Settings → Speech to provide explicit paths.
        showVoiceError(
          "Voice models aren't downloaded and automatic download is off. " +
            "Add the model paths in Settings → Speech, or re-enable " +
            "automatic download. Missing: " + (names || "(none)"),
        );
        return;
      }
```

Use whatever the file's existing user-error surface is (e.g. `showVoiceError`, a banner element, or `console`+toast) — match the mechanism the `Other` error path already uses. Do not invent a new modal.

- [ ] **Step 3: Manual verification note**

There is no JS unit harness in this repo; this branch is verified in the GUI click-through (Task 7 / owner manual check): set `disable_auto_download = true` with missing Supertonic assets → starting voice mode shows the banner, not the consent modal.

- [ ] **Step 4: Commit**

```bash
cd /Users/hherb/src/primer/src
git add crates/primer-gui/ui/voice.js
git commit -m "feat(gui): voice.js auto_download_disabled banner (no consent modal)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: Workspace verification

**Files:** none (verification only)

- [ ] **Step 1: fmt**

Run: `cd /Users/hherb/src/primer/src && ~/.cargo/bin/cargo +1.88 fmt --all -- --check`
Expected: clean (no diff).

- [ ] **Step 2: clippy (default + speech)**

Run:
```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo +1.88 clippy --workspace --all-targets -- -D warnings
~/.cargo/bin/cargo +1.88 clippy -p primer-gui --features speech --all-targets -- -D warnings
```
Expected: clean both.

- [ ] **Step 3: tests (default + speech)**

Run:
```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo +1.88 test --workspace --no-fail-fast 2>&1 | tail -15
~/.cargo/bin/cargo +1.88 test -p primer-gui --features speech --no-fail-fast 2>&1 | tail -15
```
Expected: all pass (970 baseline + the new tests; 0 failures).

- [ ] **Step 4: feature-build confirmation**

Run: `cd /Users/hherb/src/primer/src && ~/.cargo/bin/cargo +1.88 build -p primer-gui --features supertonic 2>&1 | tail -5`
Expected: `Finished`.

- [ ] **Step 5: Commit (only if any fmt/clippy fix was needed)**

```bash
cd /Users/hherb/src/primer/src
git add -A
git commit -m "style: fmt/clippy for Stage D

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: Docs

**Files:**
- Modify: `README.md`, `ROADMAP.md`, `CLAUDE.md`

- [ ] **Step 1: README.md** — under the speech/voice status, note Supertonic TTS assets now auto-download (GUI consent flow), one multilingual ~380 MB bundle covering all locales, default voice F1.

- [ ] **Step 2: ROADMAP.md** — mark Supertonic Stage D (asset auto-download + consent) done; Stage E (A/B numbers) / Stage F (Hindi→stable) remain.

- [ ] **Step 3: CLAUDE.md** — extend the Supertonic Stage C bullet: Stage D adds GUI auto-download via 7 single-file `kind`s + the locale-independent `SUPERTONIC_ASSETS` table in `primer-speech/locale_defaults.rs`, a `tts == Supertonic` arm in `resolve_voice_assets` (cache root `~/.cache/primer/models/supertonic/`, default voice F1), and the now-enforced `disable_auto_download` gate (`missing_to_error` + `StartVoiceModeError::AutoDownloadDisabled`) which was previously a no-op for all backends.

- [ ] **Step 4: Commit**

```bash
cd /Users/hherb/src/primer
git add README.md ROADMAP.md CLAUDE.md
git commit -m "docs: Supertonic Stage D shipped (asset auto-download + consent)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Self-review notes (resolved during planning)

- **Spec coverage:** asset table (T1), kinds (T2), resolve arm + paths + total + resolve_requested_kinds (T3), disable_auto_download gate (T4), frontend branch (T5), verification (T6), docs (T7). All spec sections mapped.
- **Existing-test breakage:** `supertonic_tts_does_not_gate_piper_files` precondition changes under gating; T3 Step 2 rewrites it to create the override files. Called out explicitly.
- **Type consistency:** `SupertonicAsset{kind,file_name,slot,url,approx_size_mb}`, `SupertonicSlot{OnnxDir,VoiceStyle}`, `supertonic_assets()`, `DEFAULT_SUPERTONIC_VOICE_STYLE_FILE`, `supertonic_paths`, `supertonic_asset_path`, `missing_to_error`, `AutoDownloadDisabled` — names used consistently across T1–T4.
- **Feature gating:** `primer-speech` is an optional dep of `primer-gui` (only under `speech`); tests that name `primer_speech::...` or `voice::assets::...` are gated `#[cfg(feature = "speech")]`. The implementer must confirm the exact gating of `mod voice` in `primer-gui/src/lib.rs` and match it.
- **Download infra:** unchanged — `stream_to_path` already `create_dir_all`s the parent, so the `onnx/` subdir is handled; big files inherit per-file resume + the 30-min timeout + oversize cap.
