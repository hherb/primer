//! Speech-mode tunables. Mirrors the CLI's `--mic-silence-ms` flag and
//! any future GUI-level speech defaults.

/// Milliseconds of post-end-of-speech silence VAD waits before
/// firing SpeechEnd. The CLI's `--mic-silence-ms` defaults to
/// this value; the GUI's `SpeechSettings::mic_silence_ms` default
/// reads it via this constant.
///
/// Lifted from a 600 ms default at the original `--speech` POC
/// (PR for spec 2026-05-02). Tuning rationale: silero's 300 ms
/// default is too aggressive given cancel-on-resume; 600 ms
/// reduces false trips without hurting perceived response time.
pub const DEFAULT_MIC_SILENCE_MS: u32 = 600;

/// Milliseconds of silence the state machine inserts between
/// consecutive phrases during TTS playback. The voice loop's SPEAK
/// phase fires this much zero-sample audio into the speaker after
/// each [`primer_core::speech::SynthesisEvent::PhraseEnd`], giving
/// the listener a perceptible pause at sentence boundaries.
///
/// User-tunable: lower if the voice feels too halting, higher if
/// phrases run into each other. Referenced by the
/// `SynthesisEvent::PhraseEnd` doc comment.
pub const DEFAULT_INTER_PHRASE_SILENCE_MS: u32 = 200;

/// `recv_timeout` slice in milliseconds for the macOS-native TTS
/// background-path streaming drain loop. Short enough that the
/// [`STREAM_DRAIN_TIMEOUT_SECS`] overall streaming-drain deadline
/// fires promptly on a hung synth; long enough to amortise wakeup
/// cost. Not used by the main-thread path (which drives the
/// NSRunLoop in [`STREAM_RUN_LOOP_SLICE_MS`]-wide slices and uses
/// `try_recv`).
///
/// The streaming channel itself is **unbounded** by design. The PCM
/// callback fires synchronously on the GCD main queue; a bounded
/// channel that backed up while the producer was inside the runloop
/// would deadlock the main-thread path (consumer would be stuck
/// inside `runUntilDate` waiting for the callback to return, while
/// the callback was stuck waiting for the consumer to drain). An
/// unbounded channel makes the GCD main queue's hard "never block"
/// invariant a structural property rather than a tunable budget.
pub const STREAM_DRAIN_POLL_MS: u64 = 10;

/// Overall sanity-cap deadline for the macOS-native TTS streaming
/// drain loops (both main-thread and background paths). If no
/// `SynthesisEvent::PhraseEnd` arrives within this window the synth
/// is considered hung and the call returns an error. AVSpeechSynthesizer
/// terminates well within this budget for any plausible utterance length
/// in practice; the cap is defensive insurance against driver-level
/// hangs, not a tuning parameter.
pub const STREAM_DRAIN_TIMEOUT_SECS: u64 = 30;

/// NSRunLoop slice (milliseconds) for the macOS-native TTS main-thread
/// drain path. Each `runUntilDate` call blocks for this long, draining
/// any pending GCD main-queue callbacks (including AVSpeechSynthesizer
/// PCM callbacks) before returning to the channel `try_recv` loop.
/// Short enough that interleaved channel drains stay responsive; long
/// enough that the per-slice wakeup cost is amortised against actual
/// callback delivery.
pub const STREAM_RUN_LOOP_SLICE_MS: u64 = 10;

/// Approximate Whisper `small`/`small.en` model size in MiB. Used
/// by the asset-consent modal as the "whisper portion" of a locale
/// bundle's download budget so the piper-voice portion can be
/// derived as `total - whisper`. Both the multilingual `ggml-small.bin`
/// and English-only `ggml-small.en.bin` are ~470 MiB; if a future
/// locale upgrades to `ggml-medium.bin` (~1.5 GB), add a per-model
/// table here rather than tweaking this constant.
pub const APPROX_WHISPER_SMALL_MB: u32 = 470;

/// Approximate size in MiB of a Piper voice's `.onnx.json` config
/// sidecar. The file is a small JSON document (phoneme tables +
/// metadata); a single MiB is a comfortable upper-bound estimate
/// for the consent modal's download budget.
pub const APPROX_PIPER_CONFIG_MB: u32 = 1;

/// Overall request timeout for voice-asset downloads, in seconds.
/// Whisper `small` at ~3 Mbps takes ~22 minutes; 30 min is a humane
/// cap that catches a stalled TCP connection (NAT idle-timeout,
/// captive portal limbo) without aborting a slow but progressing
/// transfer. Configurable per install via
/// `SpeechSettings.download_timeout_secs` in `gui-config.json`.
pub const DEFAULT_DOWNLOAD_TIMEOUT_SECS: u64 = 30 * 60;

/// Safety multiplier (expressed as a percentage of the declared
/// `approx_size_mb`) used to compute the maximum number of bytes
/// the downloader will accept before aborting. A redirected URL
/// (e.g. canonical Hugging Face URL replaced with an attacker page
/// serving a 50 GB ISO) would otherwise fill the disk. The 50 %
/// headroom covers the fact that `approx_size_mb` is rounded down
/// to the nearest MiB and that on-disk size can legitimately
/// exceed the rounded estimate by a few percent.
pub const DOWNLOAD_SIZE_SAFETY_MULTIPLIER_PCT: u64 = 150;

/// Bytes per MiB. Named so the `× 1_048_576` factors throughout the
/// download-cap math read as unit conversions rather than magic
/// numbers.
pub const BYTES_PER_MIB: u64 = 1_048_576;

/// Divisor used when converting a percentage to a fraction (i.e. 100).
/// Pairs with [`DOWNLOAD_SIZE_SAFETY_MULTIPLIER_PCT`] so the
/// `× pct / 100` formula reads as percentage-of arithmetic rather
/// than a bare literal.
pub const PERCENT_DIVISOR: u64 = 100;

/// Tunable thresholds for the macos-native-26 derived-VAD state machine.
/// See `crates/primer-speech/src/macos26/vad.rs` and the design doc at
/// `docs/superpowers/specs/2026-05-20-macos-native-26-design.md`.
pub mod macos26 {
    use std::time::Duration;

    /// Empty or whitespace-only transcriber partials don't fire SpeechStart;
    /// at least this many non-whitespace characters must be present.
    pub const SPEECH_START_MIN_TEXT_CHARS: usize = 1;

    /// Inactivity threshold after which the state machine emits SpeechEnd
    /// even if the transcriber never sent `isFinal`. SpeechTranscriber
    /// with `.progressiveTranscription` only emits volatile partials
    /// during free-running audio (real isFinal arrives only on full
    /// pipeline teardown), so the synthetic-final path at this timeout
    /// is the load-bearing way transcripts reach the dialogue manager.
    ///
    /// Empirical tuning (manual smoke, PR #134): 600 ms cuts off mid-
    /// sentence on natural child-paced speech with brief inter-word
    /// pauses; 1200 ms is too conservative and adds noticeable post-
    /// utterance latency on short sentences. 1000 ms is the
    /// compromise: covers natural inter-word pauses while keeping
    /// the perceived "Primer is silent" gap below the threshold a
    /// child notices as "slow to respond". Long, naturally-ended
    /// sentences trip SpeechTranscriber's real `isFinal=true` and
    /// bypass this timeout entirely.
    pub const SPEECH_END_TIMEOUT: Duration = Duration::from_millis(1000);

    /// Cadence at which the audio task ticks the state machine to check
    /// for inactivity-driven SpeechEnd. Anything under `SPEECH_END_TIMEOUT`
    /// keeps the worst-case detection latency under 2× this value.
    pub const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(100);
}

/// Android on-device `SpeechRecognizer` derived-VAD tunables. The
/// recognizer endpoints itself (`onEndOfSpeech`), so unlike macos26 we do
/// not need an inactivity timer in the common case — but we keep a guard
/// path for engines that emit `onResults` without a preceding
/// `onEndOfSpeech`.
pub mod android {
    use std::time::Duration;

    /// Minimum trimmed characters in a partial/final transcript to treat
    /// as speech onset (mirrors macos26's `SPEECH_START_MIN_TEXT_CHARS`
    /// so empty/whitespace partials don't fire SpeechStart).
    pub const SPEECH_START_MIN_TEXT_CHARS: usize = 1;

    /// How long the recognizer consumer waits per `poll_event` call
    /// before looping. Short enough that a `stop`/`cancel` signal is
    /// observed promptly; long enough that the per-poll JNI round-trip
    /// overhead is amortised. Mirrors macos26's `EVENT_POLL_INTERVAL`.
    pub const POLL_TIMEOUT: Duration = Duration::from_millis(100);

    /// Backoff before re-arming the recognizer after a recoverable error
    /// (`ERROR_NO_MATCH` / `ERROR_SPEECH_TIMEOUT` / `ERROR_RECOGNIZER_BUSY`).
    /// Small so the loop keeps listening responsively, but non-zero so a
    /// pathological immediate-error engine can't tight-spin the CPU.
    pub const REARM_BACKOFF: Duration = Duration::from_millis(150);

    /// Liveness watchdog for a silently-dead recognizer. Even with the
    /// recreate-per-arm fix, a freshly created on-device recognizer can die
    /// with a terminal error (e.g. `ERROR_SERVER_DISCONNECTED`) and then
    /// emit NO further events, leaving the loop stuck in `armed=true` with a
    /// dead recognizer (device-found 2026-06-24, RedMagic 11 Pro; issue
    /// #259). If no recognizer event arrives within this window while armed
    /// and not speaking, the loop drops the armed state so the recognizer is
    /// recreated. Must be comfortably longer than the ~5 s NO_MATCH /
    /// SPEECH_TIMEOUT cadence (a healthy idle recognizer fires one of those
    /// every window, so it never trips the watchdog) yet short enough that a
    /// dead mic recovers in seconds, not the ~3 min wedge observed on-device.
    pub const RECOGNIZER_WATCHDOG_TIMEOUT: Duration = Duration::from_secs(12);

    /// `android.speech.SpeechRecognizer` `onError` codes the consumer
    /// treats as RECOVERABLE — the recognizer is one-shot and these are
    /// the expected "heard nothing this window" outcomes, so the loop
    /// re-arms and keeps listening rather than dying. Any other code
    /// (permissions, language unavailable, client, server) is terminal
    /// — re-arming would either spin or never succeed.
    ///
    /// Values from the Android SDK (`SpeechRecognizer.ERROR_*`); pinned
    /// here so the pure re-arm classifier needs no Android dep.
    pub const ERROR_SPEECH_TIMEOUT: i32 = 6;
    pub const ERROR_NO_MATCH: i32 = 7;
    pub const ERROR_RECOGNIZER_BUSY: i32 = 8;

    /// `SpeechRecognizer.ERROR_INSUFFICIENT_PERMISSIONS` — the
    /// `RECORD_AUDIO` runtime permission was denied. TERMINAL (never in
    /// the recoverable set), so the recognizer loop does not re-arm into
    /// a permission it cannot satisfy. The GUI checks the permission
    /// up front before arming and surfaces a user-visible message; this
    /// const names the async code for the classifier and any future
    /// mid-session-revocation handling.
    pub const ERROR_INSUFFICIENT_PERMISSIONS: i32 = 9;
}
