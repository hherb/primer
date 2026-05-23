//! CLI adapter for the shared `primer_speech::voice_loop`.
//!
//! Builds [`LoopBackends`] + the audio thread + cpal mic/speaker via the
//! shared `primer_speech::voice_loop::build_local_backends` helper,
//! instantiates a [`StdoutObserver`] that preserves the existing CLI
//! print formatting, and calls
//! `primer_speech::voice_loop::run_loop_borrowed` (the
//! borrowed-`&mut DialogueManager` variant; the CLI owns the DM directly
//! so no Arc<Mutex<>> is needed here).
//!
//! CLI-specific glue kept here (not lifted to primer-speech):
//!   - `DialogueResponder` — depends on `primer-pedagogy::DialogueManager`
//!   - `StdoutObserver` — CLI stdout/stderr formatting

mod dialogue_responder;
pub mod stdout_observer;

#[cfg(not(all(
    target_os = "macos",
    any(feature = "macos-native", feature = "macos-native-26")
)))]
use std::path::PathBuf;
use std::sync::Arc;

use primer_core::error::Result;
use primer_pedagogy::DialogueManager;
use primer_speech::voice_loop::run_loop_borrowed;

pub use stdout_observer::StdoutObserver;

/// Configuration passed into [`run`] from `main`. The whisper/piper
/// asset fields are cfg-gated out on the macOS-native build (#112) —
/// SFSpeechRecognizer + AVSpeechSynthesizer carry STT and TTS and the
/// corresponding CLI flags are not declared either. Owned `PathBuf` /
/// `String` (rather than the pre-refactor `&'a Path` / `&'a str`)
/// avoids a `PhantomData<&'a ()>` workaround when the cfg-gated branch
/// would otherwise leave `'a` unused.
pub struct SpeechLoopConfig {
    #[cfg(not(all(
        target_os = "macos",
        any(feature = "macos-native", feature = "macos-native-26")
    )))]
    pub whisper_model: PathBuf,
    #[cfg(not(all(
        target_os = "macos",
        any(feature = "macos-native", feature = "macos-native-26")
    )))]
    pub voice_onnx: PathBuf,
    #[cfg(not(all(
        target_os = "macos",
        any(feature = "macos-native", feature = "macos-native-26")
    )))]
    pub voice_config: PathBuf,
    #[cfg(not(all(
        target_os = "macos",
        any(feature = "macos-native", feature = "macos-native-26")
    )))]
    pub voice_id: String,
    pub mic_silence_ms: u32,
    pub verbose: bool,
    /// Active locale for TTS dispatch. Today's CLI binds this to the
    /// resolved `--language` once and uses the same locale for the
    /// whole session.
    pub locale: primer_core::i18n::Locale,
}

/// CLI entry point for `--speech` mode.
///
/// Builds the local speech backends via the shared
/// `primer_speech::voice_loop::build_local_backends` helper (or the
/// macOS-native variant when `--features primer-cli/macos-native` is
/// set at compile time on macOS), instantiates a [`StdoutObserver`],
/// and calls [`primer_speech::voice_loop::run_loop_borrowed`] (the
/// borrowed-`&mut DialogueManager` variant; the CLI owns the DM
/// directly so no Arc<Mutex<>> is needed here).
#[cfg(feature = "speech")]
pub async fn run(cfg: SpeechLoopConfig, dialogue: &mut DialogueManager) -> Result<()> {
    #[cfg(all(target_os = "macos", feature = "macos-native-26"))]
    let mut local = primer_speech::voice_loop::build_local_backends_macos_native_26(
        cfg.locale,
        cfg.mic_silence_ms,
        cfg.verbose,
    )
    .await?;

    #[cfg(all(
        target_os = "macos",
        feature = "macos-native",
        not(feature = "macos-native-26"),
    ))]
    let mut local = primer_speech::voice_loop::build_local_backends_macos_native(
        cfg.locale,
        cfg.mic_silence_ms,
        cfg.verbose,
    )
    .await?;

    #[cfg(not(any(
        all(target_os = "macos", feature = "macos-native-26"),
        all(target_os = "macos", feature = "macos-native"),
    )))]
    let mut local = primer_speech::voice_loop::build_local_backends(
        cfg.voice_onnx.as_path(),
        cfg.voice_config.as_path(),
        cfg.whisper_model.as_path(),
        cfg.voice_id.as_str(),
        cfg.locale,
        cfg.mic_silence_ms,
        cfg.verbose,
    )
    .await?;

    // Wire DialogueManager via the borrowed-Responder adapter.
    let responder: Box<dyn primer_speech::voice_loop::Responder + '_> =
        Box::new(dialogue_responder::DialogueResponder { dialogue });

    let observer = StdoutObserver::new(cfg.verbose);

    let backends = local
        .backends
        .take()
        .expect("build_local_backends always returns backends");
    let event_rx = local
        .event_rx
        .take()
        .expect("build_local_backends always returns event_rx");
    let on_audio = local
        .on_audio
        .take()
        .expect("build_local_backends always returns on_audio");
    let drain_hook = local
        .drain_hook
        .take()
        .expect("build_local_backends always returns drain_hook");
    let is_speaking = Arc::clone(&local.is_speaking);

    let result = run_loop_borrowed(
        backends,
        event_rx,
        responder,
        on_audio,
        Some(drain_hook),
        cfg.verbose,
        Some(is_speaking),
        observer,
    )
    .await;

    // Stop the audio thread + drop cpal streams via shutdown(); the
    // Drop impl on LocalBackends also defends against a missed call,
    // so we don't need a manual drop here.
    local.shutdown();

    let transcripts = result?;
    if cfg.verbose {
        eprintln!("[speech] session ended after {} turn(s)", transcripts.len());
    }
    Ok(())
}

/// Stub implementation when the `speech` feature is disabled. Returns
/// an error so the binary fails fast if `--speech` is somehow set
/// without the feature.
#[cfg(not(feature = "speech"))]
pub async fn run(_cfg: SpeechLoopConfig<'_>, _dialogue: &mut DialogueManager) -> Result<()> {
    Err(primer_core::error::PrimerError::Speech(
        "primer-cli was built without the `speech` feature".into(),
    ))
}

// ── Tests ─────────────────────────────────────────────────────────────
//
// The state-machine behaviour tests (happy path, cancel on resumed speech,
// quit phrase, is_speaking gate, LLM error fallback) live in
// `primer-speech::voice_loop::state_machine`. They exercise
// `run_loop_borrowed` directly and do not need to be duplicated here.
