# Supertonic 3 Stage D — Asset auto-download + consent (GUI)

**Date:** 2026-06-02
**Issue:** #170 (Supertonic 3 voice-mode TTS) — Stage D
**Status:** design approved, pre-implementation
**Predecessor:** Stage C (`docs/superpowers/specs/2026-05-31-supertonic-stage-c-decoupled-speech-design.md`, merged via PR #191) made STT/TTS decoupled runtime choices and added Supertonic as a selectable TTS — but with **no asset auto-download**. Supertonic paths came only from the per-locale `SpeechLocaleOverride`; an absent path surfaced as a `build_tts` error at session start.

## Goal

A fresh GUI Supertonic-TTS session offers a **consent download** of the ~380 MB Supertonic 3 model bundle instead of erroring on absent paths — mirroring the existing Piper/Whisper download+consent flow. `SpeechSettings.disable_auto_download` stays respected.

## Scope & non-goals

**In scope (GUI only):**
- Add the Supertonic asset bundle to the GUI's missing-asset resolution so missing files produce `MissingAsset` consent entries.
- Reuse the existing single-file download infrastructure (`download_one` / `stream_to_path`) unchanged.

**Out of scope (deferred):**
- **CLI auto-download.** The CLI uses explicit `--supertonic-dir`/`--supertonic-voice-style`; no backend has CLI auto-download (consistent with Piper/Whisper). Unchanged.
- **Persona picker.** F1 is the single hard-wired default voice style. Choosing among F1–F5/M1–M5 in the GUI is a later stage.
- **Archive/multi-file-in-one-kind download.** HF stores the files individually; we fetch them individually.
- **Default-path flip / Stage E A-B numbers / Stage F Hindi→stable.** Stage D only makes the opt-in download work; no default behaviour changes. (An OpenRAIL-M licence read is still required before any future default-path flip — not in this stage.)

## Background — what exists today

- **`MissingAsset`** (`primer-gui/src/commands/voice.rs`): `{ kind: String, path: PathBuf, suggested_url: Option<String>, approx_size_mb: Option<u32> }`. `Serialize`-only (a static-assert forbids `Deserialize`). Current kinds: `piper_onnx`, `piper_config`, `whisper_model`.
- **`resolve_voice_assets(home, speech, locale, stt, tts) -> Result<ResolvedAssets, AssetMissing>`** (`primer-gui/src/voice/assets.rs`): computes effective paths, checks `.exists()`, gates Piper iff `tts == Piper` and Whisper iff `stt == Whisper`. Supertonic is currently **not** gated here — paths come straight from `SpeechLocaleOverride`, and an absent path is surfaced later by `build_tts`.
- **`resolve_requested_kinds(home, speech, locale, requested_kinds) -> Vec<MissingAsset>`**: re-calls `resolve_voice_assets` and filters by the echoed-back `kind` strings, re-resolving `path` + `suggested_url` server-side (trust boundary: the webview never supplies paths/URLs). Caps input at `MAX_REQUESTED_KINDS = 16`.
- **`download_voice_assets(state, app, kinds: Vec<String>)`** Tauri command: resolves kinds → `download_one` per asset.
- **`download_one` / `stream_to_path`** (`primer-gui/src/voice/download.rs`): single-file download with `.partial` resume (`Range:` / 206 / 200 / 416 handling), `download_timeout_secs` (default 30 min), oversize cap (`approx_size_mb × 150%`), atomic rename, `primer://voice/download_progress` events with error kinds (`no_url`/`timeout`/`http_status`/`oversize`/`network`/`io`).
- **Consent modal** (`primer-gui/ui/index.html` + `ui/voice.js`): renders each entry generically (`kind` / `≈N MB` / URL); the Download button echoes back `entries.map(e => e.kind)` to `download_voice_assets`.
- **`SpeechLocaleOverride`** (`primer-gui/src/config.rs`): per-locale `{ piper_onnx_path, piper_config_path, whisper_model_path, voice_id, supertonic_onnx_dir, supertonic_voice_style_path }`.
- **`LocaleDefault`** (`primer-speech/src/locale_defaults.rs`): per-locale Piper/Whisper download URLs + ids + `approx_total_mb`, keyed by `Locale::pack_id()`.
- **`build_tts`** (`primer-speech/src/voice_loop/selectors.rs`): consumes `TtsAssets { supertonic_onnx_dir, supertonic_voice_style, … }` to construct `SupertonicTts::new(onnx_dir, voice_style)`.
- **Supertonic asset layout** (`Supertone/supertonic-3` on HF — one **multilingual** model, voice styles are personas not locales):
  - `onnx/duration_predictor.onnx` (3.5 MB), `onnx/text_encoder.onnx` (35 MB), `onnx/vector_estimator.onnx` (245 MB), `onnx/vocoder.onnx` (97 MB), `onnx/tts.json` (<1 MB), `onnx/unicode_indexer.json` (0.3 MB).
  - `voice_styles/F1.json … F5.json, M1.json … M5.json` (~0.3 MB each).

## Design

### Asset model — 7 kinds, one per file

We model the bundle as **7 individual download entries**, one `kind` per file, each mapping to one HF URL. This reuses `download_one`/`stream_to_path` unchanged — the two big files (245 MB, 97 MB) get per-file resume granularity, which a single-bundle kind would not.

| kind | cache-relative path | URL suffix (under `https://huggingface.co/Supertone/supertonic-3/resolve/main/`) | ≈MB |
|---|---|---|---|
| `supertonic_vector_estimator` | `supertonic/onnx/vector_estimator.onnx` | `onnx/vector_estimator.onnx` | 245 |
| `supertonic_vocoder` | `supertonic/onnx/vocoder.onnx` | `onnx/vocoder.onnx` | 97 |
| `supertonic_text_encoder` | `supertonic/onnx/text_encoder.onnx` | `onnx/text_encoder.onnx` | 35 |
| `supertonic_duration_predictor` | `supertonic/onnx/duration_predictor.onnx` | `onnx/duration_predictor.onnx` | 4 |
| `supertonic_tts_config` | `supertonic/onnx/tts.json` | `onnx/tts.json` | 1 |
| `supertonic_unicode_indexer` | `supertonic/onnx/unicode_indexer.json` | `onnx/unicode_indexer.json` | 1 |
| `supertonic_voice_style` | `supertonic/voice_styles/F1.json` | `voice_styles/F1.json` | 1 |

The cache root is **locale-independent**: `~/.cache/primer/models/supertonic/` (one model serves en/de/hi/ja — a deliberate departure from Piper/Whisper's per-locale `voice/<locale>/`). `build_tts` receives `supertonic_onnx_dir = …/supertonic/onnx` and `supertonic_voice_style = …/supertonic/voice_styles/F1.json`.

The six `onnx/*` files MUST share one parent directory because `SupertonicTts::new` loads the four ONNX sessions + `tts.json` + `unicode_indexer.json` from a single `onnx_dir`. The voice-style JSON is a sibling under `voice_styles/`.

### New data: a locale-independent `SupertonicAsset` table

Add to `primer-speech/src/locale_defaults.rs` (next to `LOCALE_DEFAULTS` — same download-metadata nature, already a `primer-gui` dependency):

```rust
pub struct SupertonicAsset {
    /// MissingAsset `kind` string (e.g. "supertonic_vocoder").
    pub kind: &'static str,
    /// Path relative to the cache root, under `supertonic/`.
    pub cache_rel: &'static str,
    /// Direct Hugging Face download URL.
    pub url: &'static str,
    /// Approximate on-disk size, MB (drives oversize cap + consent display).
    pub approx_size_mb: u32,
}

pub const SUPERTONIC_ASSETS: &[SupertonicAsset] = &[ /* the 7 rows above */ ];

/// The 7 files that make up the default Supertonic bundle (F1 voice).
/// Locale-independent: one multilingual model serves every locale.
pub fn supertonic_assets() -> &'static [SupertonicAsset] { SUPERTONIC_ASSETS }
```

Not keyed by locale (unlike `LOCALE_DEFAULTS`). The default voice-style stem `F1` is encoded in the `supertonic_voice_style` row's `cache_rel`/`url`.

### Path resolution

A `supertonic_paths(home, override)` helper resolves the **effective** onnx-dir + voice-style:
- `onnx_dir` = `override.supertonic_onnx_dir` if set, else `<cache>/supertonic/onnx`.
- `voice_style` = `override.supertonic_voice_style_path` if set, else `<cache>/supertonic/voice_styles/F1.json`.
- Per-file existence-check paths = `onnx_dir.join(<filename>)` for the six onnx files + the voice-style path.

**Override semantics (approved):** match the Piper/Whisper precedent exactly — the effective path wins, and downloads write to the effective path. So a user who sets a custom `supertonic_onnx_dir` override that is missing files gets the canonical HF files downloaded **into that override dir**. (We only ever fetch the canonical HF URLs; we never write outside the user-or-default-resolved path.)

### `resolve_voice_assets` change

Add an arm gated on `tts == TtsBackend::Supertonic`:
1. Resolve the 7 effective paths via `supertonic_paths`.
2. For each of the 7 files that does not `.exists()`, push a `MissingAsset { kind, path, suggested_url: Some(url), approx_size_mb: Some(mb) }` (kind/url/size from `SUPERTONIC_ASSETS`).
3. If the missing list is non-empty → `Err(AssetMissing { entries })`.
4. On success, `ResolvedAssets` carries the resolved `supertonic_onnx_dir` + `supertonic_voice_style` so the voice-backend builder constructs `TtsAssets`.

`resolve_requested_kinds` needs **no shape change** — it already re-calls `resolve_voice_assets` (deriving `(stt, tts)` from `speech.resolve_backends()`) and filters by `kind`, so the new kinds re-resolve server-side automatically. The 7 new kinds stay within `MAX_REQUESTED_KINDS = 16`.

### Download path

`download_voice_assets` → `download_one` → `stream_to_path` are unchanged. Confirm the write path creates the `onnx/` parent directory (`fs::create_dir_all`) before writing — Piper already writes into `voice/<locale>/`, so the precedent exists; reuse it. Each onnx file is gated by its own `approx_size_mb`-derived oversize cap and the shared `download_timeout_secs`.

### Frontend

**No change.** The consent modal renders entries generically and echoes back only `kind` strings; the 7 Supertonic rows render with their raw kind labels (same convention as `piper_onnx`).

### `disable_auto_download`

Supertonic missing-assets route through the **same** `AssetMissing` → consent path as Piper/Whisper. The existing `disable_auto_download` gate (traced during implementation) therefore covers Supertonic with no new logic. Confirm parity; do not introduce a parallel gate.

## Testing (TDD)

- **`supertonic_assets()` drift guard** (`primer-speech`): exactly 7 entries; every `url` starts with `https://huggingface.co/Supertone/supertonic-3/`; every `cache_rel` starts with `supertonic/`; the six onnx kinds' `cache_rel` share the `supertonic/onnx/` prefix; the style kind is under `supertonic/voice_styles/`; `approx_size_mb` sane (sum 350..420; each ≥1).
- **`supertonic_paths` helper** (`primer-gui`): default cache paths when no override; override paths when set; the six onnx file paths are `onnx_dir`-relative.
- **`resolve_voice_assets` Supertonic arm** (`primer-gui`), three states:
  - empty cache + `tts == Supertonic` → `Err` whose 7 entries carry the right kinds/urls/sizes/paths.
  - all 7 files present (touch temp files) → `Ok` with `supertonic_onnx_dir`/`supertonic_voice_style` resolved to the cache paths.
  - override paths set and existing → `Ok` using the override (no download gating).
  - partial presence (e.g. only vocoder present) → `Err` with the remaining 6 entries only.
- **`resolve_requested_kinds`** (`primer-gui`): given `["supertonic_vocoder", "supertonic_voice_style"]` against an empty cache, returns exactly those two `MissingAsset`s with server-resolved path+url; an unknown kind is dropped.
- Existing Piper/Whisper resolution tests must stay green (the new arm is additive, gated on `tts == Supertonic`).

## Files touched

- `primer-speech/src/locale_defaults.rs` — `SupertonicAsset` struct + `SUPERTONIC_ASSETS` const + `supertonic_assets()` + drift-guard tests.
- `primer-gui/src/voice/assets.rs` — `kind` constants, `supertonic_paths` helper, `resolve_voice_assets` Supertonic arm, `ResolvedAssets` supertonic fields (if not already present from Stage C), tests.
- `primer-gui/src/commands/voice.rs` — `kind::SUPERTONIC_*` constants (reconcile with the `compute_paths` that lives here), tests.
- `primer-gui/src/voice/download.rs` — only if parent-dir creation needs to be made explicit for the `onnx/` subdir (likely already covered).
- Docs at session end: `README.md`, `ROADMAP.md`, `CLAUDE.md` (Stage D shipped), `NEXT_SESSION.md` + handoff.

## Risks / open items

- **Download volume.** ~380 MB over 6 files; the 245 MB `vector_estimator` is the long pole. Per-file resume + 30-min timeout cover slow links. Surfaced honestly in the consent modal total.
- **`disable_auto_download` parity.** Must be confirmed by tracing the existing gate, not assumed. If the gate turns out to be weaker than expected for Piper/Whisper, that is a pre-existing gap to flag separately — Stage D aligns Supertonic with whatever exists, it does not redesign the gate.
- **Override-dir downloads.** Writing canonical HF files into a user's custom override dir (Piper precedent) is intentional; documented above so a future reader doesn't read it as a bug.
- **Real-audio still a human gate.** This stage is wiring + unit/integration coverage; actual Supertonic synthesis quality / Hindi intelligibility remains an owner manual check (carried from Stage A.5 / C).
