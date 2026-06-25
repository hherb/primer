//! Command-line argument parse/validation helpers for the `primer` REPL.
//!
//! Extracted from `main.rs` so the entry point keeps only runtime wiring.
//! The [`crate::Cli`] struct itself stays in the crate root; these free
//! helpers reach its private fields through descendant-module visibility,
//! so no field needed to be made `pub(crate)` for the split.

// `Path` + `PrimerError` are only referenced by `validate_speech_assets`,
// so their imports carry the same cfg gate as that fn to avoid an
// unused-import warning on builds that compile it out.
#[cfg(all(
    feature = "speech",
    not(all(
        target_os = "macos",
        any(feature = "macos-native", feature = "macos-native-26")
    ))
))]
use primer_core::error::PrimerError;
#[cfg(all(
    feature = "speech",
    not(all(
        target_os = "macos",
        any(feature = "macos-native", feature = "macos-native-26")
    ))
))]
use std::path::Path;

#[cfg(feature = "speech")]
pub(crate) fn parse_mic_silence_ms(s: &str) -> std::result::Result<u32, String> {
    let n: u32 = s.parse().map_err(|e| format!("not a u32: {e}"))?;
    if !(50..=5000).contains(&n) {
        return Err(format!(
            "mic-silence-ms must be between 50 and 5000, got {n}"
        ));
    }
    Ok(n)
}

/// CLI value for `--tts`. Mirrors `primer_speech::voice_loop::TtsBackend`
/// minus the macOS-native arm (D2: the CLI native build keeps AVSpeech and
/// is compiled separately).
#[cfg(all(
    feature = "speech",
    not(all(
        target_os = "macos",
        any(feature = "macos-native", feature = "macos-native-26")
    ))
))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
#[clap(rename_all = "kebab-case")]
pub(crate) enum TtsChoice {
    Piper,
    Supertonic,
}

#[cfg(all(
    feature = "speech",
    not(all(
        target_os = "macos",
        any(feature = "macos-native", feature = "macos-native-26")
    ))
))]
impl From<TtsChoice> for primer_speech::voice_loop::TtsBackend {
    fn from(c: TtsChoice) -> Self {
        match c {
            TtsChoice::Piper => Self::Piper,
            TtsChoice::Supertonic => Self::Supertonic,
        }
    }
}

#[cfg(all(
    feature = "speech",
    not(all(
        target_os = "macos",
        any(feature = "macos-native", feature = "macos-native-26")
    ))
))]
pub(crate) fn validate_speech_assets(
    whisper_model: &Path,
    tts: primer_speech::voice_loop::TtsBackend,
    voice_onnx: Option<&Path>,
    voice_config: Option<&Path>,
    supertonic_dir: Option<&Path>,
    supertonic_voice_style: Option<&Path>,
    voice_id: &str,
) -> primer_core::error::Result<()> {
    use primer_speech::voice_loop::TtsBackend;

    if !whisper_model.exists() {
        return Err(PrimerError::Speech(format!(
            "whisper model not found at {}.\n\
             Download a GGML model from https://huggingface.co/ggerganov/whisper.cpp \
             (e.g. ggml-small.en.bin) and pass --whisper-model.",
            whisper_model.display()
        )));
    }

    match tts {
        TtsBackend::Piper => {
            let voice_onnx = voice_onnx.ok_or_else(|| {
                PrimerError::Speech(
                    "piper TTS requires --voice-onnx (clap should enforce this with --speech)"
                        .to_string(),
                )
            })?;
            let voice_config = voice_config.ok_or_else(|| {
                PrimerError::Speech(
                    "piper TTS requires --voice-config (clap should enforce this with --speech)"
                        .to_string(),
                )
            })?;
            if !voice_onnx.exists() {
                return Err(PrimerError::Speech(format!(
                    "voice ONNX not found at {}.\n\
                     Download a Piper voice from https://huggingface.co/rhasspy/piper-voices \
                     and pass --voice-onnx.",
                    voice_onnx.display()
                )));
            }
            if !voice_config.exists() {
                return Err(PrimerError::Speech(format!(
                    "voice config not found at {}.\n\
                     Pass --voice-config alongside --voice-onnx (the .onnx and .onnx.json files \
                     ship together).",
                    voice_config.display()
                )));
            }
            let stem = voice_onnx
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("");
            if stem != voice_id {
                tracing::warn!(
                    voice_id,
                    onnx_stem = stem,
                    "--voice id does not match --voice-onnx file stem; \
                     Piper will reject the session at open time"
                );
            }
        }
        TtsBackend::Supertonic => {
            let dir = supertonic_dir.ok_or_else(|| {
                PrimerError::Speech(
                    "supertonic TTS requires --supertonic-dir (clap should enforce this \
                     with --speech)"
                        .to_string(),
                )
            })?;
            let style = supertonic_voice_style.ok_or_else(|| {
                PrimerError::Speech(
                    "supertonic TTS requires --supertonic-voice-style (clap should enforce \
                     this with --speech)"
                        .to_string(),
                )
            })?;
            if !dir.exists() {
                return Err(PrimerError::Speech(format!(
                    "supertonic model dir not found at {}.\n\
                     Download from https://huggingface.co/Supertone/supertonic-3 and pass \
                     --supertonic-dir.",
                    dir.display()
                )));
            }
            if !style.exists() {
                return Err(PrimerError::Speech(format!(
                    "supertonic voice-style not found at {}.\n\
                     Pass --supertonic-voice-style (e.g. voice_styles/F1.json from the \
                     Supertone/supertonic-3 release).",
                    style.display()
                )));
            }
        }
        TtsBackend::MacosNative | TtsBackend::AndroidNative => {
            // Unreachable on the portable build (the CLI's TtsChoice has no
            // MacosNative/AndroidNative arm), but matched exhaustively so the
            // portable `--features speech` build stays green when new
            // platform-native TtsBackend variants land.
        }
    }
    Ok(())
}

/// Emit best-effort startup-time warnings about subsystem-backend
/// combinations under `--backend qnn`:
///
/// - **All-qnn** (classifier / extractor / comprehension either
///   defaulted to the main backend or explicitly set to qnn): every
///   background LLM call serialises through the dialog mutex along
///   with the main chat turn. Functionally correct, but on a
///   memory-constrained device this means classifier work piles up
///   behind a multi-second decode. The warning is informational —
///   nothing is rejected.
/// - **All-stub** (every subsystem explicitly stubbed): the
///   conversation loses classifier-driven features (engagement
///   detection, concept extraction, comprehension depth promotion).
///   This is sometimes a deliberate choice for offline smoke tests;
///   surfaced as a warning, not an error.
///
/// The "cloud-backed subsystem with missing `ANTHROPIC_API_KEY`" case
/// from the plan is already covered structurally by the
/// `build_classifier` / `build_extractor` / `build_comprehension`
/// builders — they call `build_backend("cloud", ...)` which errors
/// when `api_key` is `None`. No extra check needed here.
///
/// Pure inspection of the `Cli` struct — no I/O. Kept as a free
/// function so we can unit-test the decision logic via the small
/// `npu_serialisation_warning` helper below.
pub(crate) fn warn_on_npu_serialisation(cli: &crate::Cli) {
    let decision = npu_serialisation_warning(
        cli.classifier_backend.as_deref(),
        cli.extractor_backend.as_deref(),
        cli.comprehension_backend.as_deref(),
    );
    if let Some(msg) = decision {
        eprintln!("Warning: {msg}");
    }
}

/// Decide whether to warn about NPU serialisation or feature loss
/// given the explicit subsystem-backend overrides. Returns `None`
/// when the configuration is mixed (some NPU, some not) — the most
/// reasonable case, no warning needed.
///
/// Inputs are `Option<&str>` because each subsystem flag defaults to
/// "unset → reuse the main backend". Under `--backend qnn`, "unset"
/// effectively means "qnn".
pub(crate) fn npu_serialisation_warning(
    classifier: Option<&str>,
    extractor: Option<&str>,
    comprehension: Option<&str>,
) -> Option<String> {
    // Resolve each subsystem to its effective backend name under
    // `--backend qnn`: None → "qnn" (inherit), explicit value wins.
    let resolved: [&str; 3] = [
        classifier.unwrap_or("qnn"),
        extractor.unwrap_or("qnn"),
        comprehension.unwrap_or("qnn"),
    ];

    if resolved.iter().all(|&b| b == "qnn") {
        return Some(
            "every subsystem (classifier, extractor, comprehension) is set to qnn — \
             all background LLM work will serialise behind the chat turn through the \
             dialog mutex. Consider --classifier-backend stub or a separate small model."
                .to_string(),
        );
    }
    if resolved.iter().all(|&b| b == "stub") {
        return Some(
            "every subsystem (classifier, extractor, comprehension) is stub — \
             the conversation runs without engagement detection, concept extraction, \
             or comprehension depth promotion. This is fine for smoke tests."
                .to_string(),
        );
    }
    None
}

/// Pair clap's flat `--reasoning-marker OPEN CLOSE` values (a `Vec` of length
/// `2 × N`) into `(open, close)` tuples. A trailing unpaired value is dropped
/// (clap's `num_args = 2` makes that impossible in practice, but be defensive).
pub(crate) fn pair_reasoning_markers(flat: Vec<String>) -> Vec<(String, String)> {
    let mut it = flat.into_iter();
    let mut out = Vec::new();
    while let (Some(open), Some(close)) = (it.next(), it.next()) {
        out.push((open, close));
    }
    out
}

#[cfg(test)]
mod tests;
