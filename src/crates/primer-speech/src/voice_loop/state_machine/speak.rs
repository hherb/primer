//! SPEAK: synthesise the reply with the mic gated, then wait for the
//! speaker to drain. Split out of `inner.rs` for #325 (stage 1,
//! behaviour-preserving).

use primer_core::consts::speech::DEFAULT_INTER_PHRASE_SILENCE_MS;
use primer_core::error::Result;
use primer_core::speech::{SynthesisEvent, VadEvent};

use crate::voice_loop::observer::{LoopObserver, VoiceState};
use crate::voice_loop::tts_markdown::tts_phrases;

use super::{DrainHook, LoopBackends};

/// The SPEAK state for a non-empty reply: see
/// [`super::inner::run_loop_inner`] for `is_speaking` and the drain hook.
pub(super) async fn speak<O: LoopObserver>(
    accumulated: &str,
    backends: &LoopBackends,
    on_committed_audio: &mut (dyn FnMut(Vec<f32>) + Send),
    wait_for_speaker_drain: &mut Option<DrainHook>,
    is_speaking: &Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    events: &mut tokio::sync::mpsc::Receiver<VadEvent>,
    observer: &mut O,
) -> Result<()> {
    observer.on_state_change(VoiceState::Speak, None);
    // One markdown-stripped phrase per `push_text` (#325 stage 2), so a
    // streamed reply can start speaking at its first sentence. The text
    // shown to the user keeps the markdown; only the audio input is
    // stripped.
    let phrases = tts_phrases(accumulated);
    // Gate the mic: from here until the speaker has drained, the
    // audio thread should discard incoming samples (the Primer's
    // own voice would otherwise be transcribed as the next child
    // utterance via mic→speaker acoustic feedback).
    if let Some(flag) = is_speaking.as_ref() {
        flag.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    // Dispatch on `active_locale`. v1 has a single entry so
    // these lookups can't miss in practice; we treat a miss as
    // a Speech error (rather than a panic) because LoopBackends
    // is a public-ish struct that future code might construct
    // without the convenience helper.
    let active_tts = backends
        .tts_by_locale
        .get(&backends.active_locale)
        .ok_or_else(|| {
            primer_core::error::PrimerError::Speech(format!(
                "no TTS configured for active locale {}",
                backends.active_locale.pack_id()
            ))
        })?;
    let active_voice = backends
        .voice_by_locale
        .get(&backends.active_locale)
        .ok_or_else(|| {
            primer_core::error::PrimerError::Speech(format!(
                "no voice profile configured for active locale {}",
                backends.active_locale.pack_id()
            ))
        })?;
    let mut session = active_tts.open_session(active_voice)?;
    let tts_rate = active_tts.sample_rate();
    // Inter-phrase silence inserted on each `PhraseEnd` event. Value
    // in `primer_core::consts::speech::DEFAULT_INTER_PHRASE_SILENCE_MS`.
    let inter_phrase_silence_samples = (tts_rate * DEFAULT_INTER_PHRASE_SILENCE_MS / 1000) as usize;
    let mut on_event = |event: SynthesisEvent| match event {
        SynthesisEvent::Audio(chunk) => on_committed_audio(chunk.samples),
        SynthesisEvent::PhraseEnd => {
            on_committed_audio(vec![0.0_f32; inter_phrase_silence_samples])
        }
    };
    // Sync calls deliberately not wrapped in `tokio::task::spawn_blocking`,
    // even though the trait's `# Blocking` doc-section asks async callers
    // to do so. Rationale per production backend:
    //
    // * **macOS-native CLI** (`Builder::new_current_thread()` on the OS
    //   main thread): `spawn_blocking` would hop work to a worker
    //   thread; `synthesize_streaming` then sees `is_main_thread() ==
    //   false` and takes the GCD-bounce path; that path drains via
    //   `dispatch2::DispatchQueue::main().exec_async(...)` which only
    //   runs when main is owned by `NSApplicationMain` or
    //   `dispatch_main()`. The CLI's main thread is running tokio's
    //   current-thread runtime instead — the bounced work is queued
    //   onto GCD main but nothing drains it, so synthesis stalls. The
    //   direct sync call keeps `push_text` on main and lets
    //   `synthesize_streaming` take the main-thread `runUntilDate`
    //   path — the architecture the CLI binary is built around (see
    //   the `run_tokio_on_main` doc-comment in
    //   `primer-cli/src/main.rs` for the full deadlock argument).
    //
    // * **macOS-native GUI** (multi-thread tokio + Tauri's
    //   `NSApplicationMain` on main): the direct call blocks one
    //   tokio worker for the synth duration. Other workers keep
    //   running, and the GCD bounce drains because Tauri owns main.
    //   A `spawn_blocking` wrap would free the one worker but
    //   `push_text` still has to bounce to main eventually, so the
    //   net wallclock cost on synthesis is unchanged. Worker-block
    //   accepted as a known cost; background tokio tasks queued
    //   during SPEAK catch up at the next turn via
    //   `DialogueManager::await_pending_post_response`.
    //
    // * **Piper backend (default Linux speech build)**:
    //   `PiperSession::synth_phrase` runs ONNX inference synchronously
    //   on the calling worker. No main-thread requirement; the wrap
    //   would free one worker but the multi-thread runtime has
    //   several so this isn't a regression to leave it.
    //
    // * **Android-native (`android-native`, Tauri GUI)**:
    //   `AndroidTtsSession::push_text` blocks in JNI until the OS
    //   `TextToSpeech` engine reports `onDone` (bounded by the Kotlin
    //   `TTS_SPEAK_TIMEOUT`). No main-thread requirement — only the
    //   recognizer methods post to the main Looper; `tts.speak` runs
    //   on the calling thread. So this is the Piper case: one
    //   multi-thread-tokio worker is blocked for the synth, the
    //   others keep running. Worker-block accepted as a known cost.
    //
    // What would have to change for the wrap to matter:
    // - A non-Apple multi-tenant runtime where the synth worker-block
    //   starves dense background tasks during SPEAK.
    // - A new backend that can synthesize off-main without bouncing
    //   to a specific thread (so `spawn_blocking` would have no
    //   side-effect on which thread the synth runs on).
    // - The macOS-native CLI moving to multi-thread tokio (which
    //   itself requires either calling `NSApplicationMain` or a
    //   GCD-main pump — see the `run_tokio_on_main` doc-comment
    //   in `primer-cli/src/main.rs`).
    for phrase in &phrases {
        session.push_text(phrase, &mut on_event)?;
    }
    session.finalize(&mut on_event)?;
    // Flush sentinel: empty Vec signals on_audio to drain any
    // resampler-leftover tail. Mock callbacks no-op on empty input.
    on_committed_audio(Vec::new());
    // Wait for cpal to actually empty the speaker ringbuf
    // before clearing the mic gate. Going through `spawn_blocking`
    // (in the hook the caller wired) keeps this off the tokio
    // worker so other async work isn't starved during the
    // drain wait. Replaces the old `samples / tts_rate + 0.4s`
    // heuristic sleep — exact instead of "fixed margin that's
    // too short on slow hardware and too long on fast hardware".
    if let Some(hook) = wait_for_speaker_drain.as_mut() {
        hook().await;
    }
    if let Some(flag) = is_speaking.as_ref() {
        flag.store(false, std::sync::atomic::Ordering::SeqCst);
    }
    // Drain any events the audio thread may have queued in the
    // narrow window between un-gating and the next LISTEN read:
    // the speaker→mic acoustic tail of the Primer's own voice
    // can otherwise emit a stale SpeechStart/SpeechEnd that would
    // be processed as a child utterance. Tradeoff: a child whose
    // SpeechStart lands inside this same window (a few ms) loses
    // that event — but their continuing speech immediately fires
    // a fresh SpeechStart on the next VAD chunk, so the start is
    // delayed by ~32 ms at worst. Acceptable for the no-barge-in
    // model.
    while events.try_recv().is_ok() {}
    Ok(())
}
