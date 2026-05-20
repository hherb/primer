//! Default values for invariant numerics shared across primer-core
//! modules. Per the no-magic-numbers convention, every numeric used
//! by primer-core helpers is defined here (or in a sibling settings
//! struct field for tunables).

/// Defaults for the retry helper. Mirrors the per-crate `consts.rs`
/// pattern used by `primer-classifier`, `primer-extractor`, etc.
pub mod retry {
    use std::time::Duration;

    /// Total attempts including the first.
    pub const DEFAULT_MAX_ATTEMPTS: u32 = 3;

    /// Initial backoff delay before the second attempt.
    pub const DEFAULT_BASE_DELAY: Duration = Duration::from_millis(250);

    /// Multiplicative growth factor between attempts (250 → 500 → 1000 ms).
    pub const DEFAULT_BACKOFF_FACTOR: u32 = 2;

    /// Jitter as a fraction of the computed delay (±50 %).
    pub const DEFAULT_JITTER_FRACTION: f32 = 0.5;

    /// Maximum honored Retry-After. Above this we give up immediately
    /// rather than block the conversational hot path.
    pub const DEFAULT_RETRY_AFTER_BUDGET: Duration = Duration::from_secs(5);
}

/// Defaults for the vocabulary spaced-repetition feature.
///
/// See [`crate::vocab`] and the design spec at
/// `docs/superpowers/specs/2026-05-05-vocabulary-spaced-repetition-design.md`.
pub mod vocab {

    /// Box-level interval table (days). Index = `box_level`.
    /// - box 0 (freshly seen / failed) → review after 1 day
    /// - box 1 (one successful review) → 3 days
    /// - box 2 (two)                    → 7 days
    /// - box 3 (three)                  → 14 days
    /// - box 4 (max — never graduates)  → 30 days
    pub const BOX_INTERVALS_DAYS: &[u32] = &[1, 3, 7, 14, 30];

    /// Highest `box_level` a concept can occupy. After reaching this, further
    /// successful reviews keep `box_level` pinned at MAX (interval stays 30d).
    /// There is no terminal "graduated" state — review continues every 30d
    /// until either the child consistently fails (depth=Aware → box reset)
    /// or the concept is genuinely never engaged with again.
    pub const MAX_BOX_LEVEL: u8 = 4;

    /// Minimum confidence for a comprehension assessment to count toward
    /// box advancement. Assessments below this threshold reset the box to 0.
    /// Numerically equal to the comprehension classifier's
    /// `confidence_threshold` (also 0.6) but kept independent so a future
    /// researcher can tune box-advancement strictness without affecting
    /// depth promotion.
    pub const MIN_CONF_FOR_BOX_PROMOTION: f32 = 0.6;

    /// Default cap on overdue concepts injected into the system prompt
    /// per turn. Configurable via `VocabSettings::max_per_prompt` and the
    /// `--vocab-max-per-prompt` CLI flag.
    pub const DEFAULT_VOCAB_MAX_PER_PROMPT: usize = 4;
}

/// Defaults for the session-time-based break-suggestion feature.
///
/// See [`crate::session_timing`] and the design spec at
/// `docs/superpowers/specs/2026-05-05-session-break-suggestion-design.md`.
pub mod break_suggest {

    /// Minutes between break-suggestion nudges. After this many minutes
    /// of session time (or this many minutes since the last suggestion,
    /// whichever is more recent), the next pedagogical intent decision
    /// returns `SuggestBreak`. Configurable via `PedagogyConfig` and the
    /// `--session-break-after-mins` CLI flag. Must be `>= 1` when enabled
    /// (a value of 0 disables the gate entirely).
    pub const DEFAULT_INTERVAL_MINUTES: u32 = 30;
}

/// Defaults for hybrid retrieval (BM25 + dense-vector RRF). Used by the
/// dialogue manager when an `Embedder` is wired; mirror the shape of
/// [`crate::knowledge::HybridParams`] and feed into it directly.
pub mod retrieval {
    /// BM25 leg top-K for knowledge-base retrieval. Wider than the
    /// final K so RRF has a real candidate pool to fuse over. Tuned
    /// against the 90-passage seed corpus + 87-query benchmark via
    /// the 54-cell hybrid sweep at `tests/retrieval_sweep_hybrid.rs`
    /// (run with `--features fastembed`). Every cell with
    /// `bm25_top_k ∈ {20, 30}` and `final_top_k = 5` achieved 100%
    /// loose / 100% strict recall (lifting the BM25-only strict
    /// miss for "how does the sun shine"). 30 was picked as the
    /// final value to leave headroom for corpus growth — the 50%
    /// candidate-pool bump over the BM25-baseline 20 costs almost
    /// nothing on a corpus this size.
    pub const KB_BM25_TOP_K: usize = 30;

    /// Dense-vector leg top-K for knowledge-base retrieval. Same
    /// rationale as `KB_BM25_TOP_K` — tuned via the hybrid sweep.
    /// Every cell with `bm25_top_k ≥ 20` and `final_top_k = 5` hit
    /// 100/100 across `vector_top_k ∈ {10, 20, 30}`; 30 chosen for
    /// symmetry with the BM25 leg and corpus-growth headroom.
    pub const KB_VECTOR_TOP_K: usize = 30;

    /// Number of fused passages handed to the prompt builder per turn.
    /// Matches the BM25-only fallback path's top-K so the system prompt
    /// stays the same shape regardless of which retrieval mode is live.
    /// Tuned against the 90-passage seed corpus via the sweep at
    /// `tests/retrieval_sweep.rs` — see
    /// `docs/superpowers/specs/2026-05-06-retrieval-tuning-design.md`.
    /// At top_k=5 the BM25 path achieves 100% loose recall and 95%
    /// strict recall on the 87-query benchmark; top_k=3 plateaued at
    /// 95% loose. Going beyond 5 added no further gains.
    ///
    /// **Cost note:** Each retrieved passage is injected into the system
    /// prompt every turn. The 3 → 5 bump adds ~67% more retrieval payload
    /// per turn (~200–500 extra tokens at typical passage length).
    /// Comfortable for cloud Anthropic; revisit when the local llama.cpp
    /// path lands and the context window gets tighter.
    pub const KB_FINAL_TOP_K: usize = 5;

    /// Post-fusion score floor for the KB hybrid path. Zero rather than
    /// `f64::NEG_INFINITY` so the fused list stays positive (RRF
    /// contributions are always > 0) without filtering anything that
    /// appeared in either leg.
    pub const KB_MIN_SCORE: f64 = 0.0;

    /// BM25 leg top-K for long-term-memory (session-turn) retrieval.
    /// Smaller than the KB path because the session corpus is usually
    /// orders of magnitude smaller and the fused candidate set
    /// shouldn't drown the prompt builder in noise.
    pub const LTM_BM25_TOP_K: usize = 10;

    /// Dense-vector leg top-K for long-term-memory retrieval.
    pub const LTM_VECTOR_TOP_K: usize = 10;

    /// Number of fused turns handed back to the dialogue manager.
    pub const LTM_FINAL_TOP_K: usize = 3;

    /// Reciprocal Rank Fusion constant `k`. The published default from
    /// Cormack et al. 2009; works well across many IR domains. Smaller
    /// values weight the very top of each list more, larger values
    /// flatten the curve. Confirmed by the 54-cell hybrid sweep:
    /// at `bm25_top_k ≥ 20, final_top_k = 5`, recall is invariant
    /// across `rrf_k ∈ {30, 60, 90}` on this corpus — the canonical
    /// 60 stays.
    pub const RRF_K: f64 = 60.0;

    /// Minimum BM25 score for the BM25-only knowledge-base path
    /// (the fallback when no embedder is wired). Higher = stricter,
    /// fewer noisy hits. The sweep at `tests/retrieval_sweep.rs`
    /// against the 90-passage seed corpus showed every value in
    /// {0.0, 0.25, 0.5, 0.75, 1.0, 1.5} produces identical recall —
    /// every *correct* top-K hit comfortably exceeds 1.5, and the
    /// sub-1.5 scores that exist are 5th-place noise on marginal
    /// queries (no query's best hit drops anywhere near the floor;
    /// the worst top-1 score across the 87-query benchmark is 3.35).
    /// Kept at 0.5 as a defensive floor: a no-op for recall today,
    /// but bites if a future larger corpus dilutes term frequencies
    /// and pushes the marginal scores below 0.5. The tripwire at
    /// `primer-kb-load/tests/bm25_floor_tripwire.rs` (run with
    /// `--ignored`) probes the actual top-K score distribution and
    /// fires loudly when the margin closes. See
    /// `docs/superpowers/specs/2026-05-06-retrieval-tuning-design.md`.
    pub const KB_BM25_ONLY_MIN_SCORE: f64 = 0.5;
}

/// Speech-mode tunables. Mirrors the CLI's `--mic-silence-ms` flag and
/// any future GUI-level speech defaults.
pub mod speech {
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
        /// even if the transcriber never sent `isFinal`. Set to 2× Silero's
        /// 300 ms default because SpeechTranscriber partials don't fire during
        /// true silence even mid-utterance, so a longer gap is safe.
        pub const SPEECH_END_TIMEOUT: Duration = Duration::from_millis(600);

        /// Cadence at which the audio task ticks the state machine to check
        /// for inactivity-driven SpeechEnd. Anything under `SPEECH_END_TIMEOUT`
        /// keeps the worst-case detection latency under 2× this value.
        pub const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(100);
    }
}

/// Defaults shared across the CLI, GUI, and frontend for the learner
/// profile. Kept here so a brand-new install behaves identically across
/// every entry point — and so the JS picker has a documented Rust
/// source of truth to mirror.
pub mod learner {
    /// Placeholder name a fresh learner profile carries until the user
    /// supplies their own. Used as both:
    /// - the CLI's `--name` default
    /// - the GUI's `LearnerConfig::default().name`
    /// - the JS picker's "is the name still the default?" check (it
    ///   suppresses the personalised "Welcome back, {name}" greeting
    ///   for unconfigured installs). The JS side mirrors this literal;
    ///   keep them in sync when changing.
    pub const DEFAULT_NAME: &str = "Explorer";
}
