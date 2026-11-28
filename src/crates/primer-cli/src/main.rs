//! Primer CLI — a text-mode REPL for developing and testing the Socratic
//! dialogue engine without any hardware (no microphone, no speaker, no
//! E Ink display). Just a terminal and a conversation.
//!
//! Usage:
//!
//! ```text
//!   primer                                              # Stub backend (canned responses, no model needed)
//!   primer --backend cloud                              # Anthropic Claude API (default model)
//!   primer --backend cloud --model claude-opus-4-7      # Override the cloud model
//!   primer --backend ollama --model llama3.2            # Local Ollama server
//!   primer --backend qnn --qnn-bundle-dir <path>        # Qualcomm NPU (Android only; --features qnn)
//!   primer --name Binti --age 8                         # Set learner profile
//!   primer --resume <uuid>                              # Resume a past session
//! ```

mod cli;
mod cli_support;
mod repl;
mod setup;

pub(crate) use cli::Cli;
use setup::{Inference, SessionSetup, build_embedder, build_inference, open_session};

#[cfg(feature = "speech")]
mod speech_loop;

#[cfg(all(feature = "macos-native", feature = "macos-native-26"))]
compile_error!(
    "`macos-native` and `macos-native-26` are mutually exclusive — pick one \
     (`macos-native-26` for macOS 26+, `macos-native` for older macOS)"
);

use std::sync::Arc;

use clap::Parser;
use primer_classifier::ClassifierSettings;
use primer_core::config::PedagogyConfig;
use primer_core::knowledge::KnowledgeBase;
use primer_core::storage::{LearnerStore, SessionStore};
use primer_engine::{
    ServedSubsystems, SubsystemSettings, build_served_subsystems, open_prepared_kb,
    resolve_knowledge_db, seed_corpus_dir,
};
use primer_extractor::ExtractorSettings;
use primer_pedagogy::DialogueManager;

#[cfg(all(
    feature = "speech",
    not(all(
        target_os = "macos",
        any(feature = "macos-native", feature = "macos-native-26")
    ))
))]
use cli_support::validate_speech_assets;

/// Probe common system locations for an `espeak-ng-data` directory and
/// set `PIPER_ESPEAKNG_DATA_DIRECTORY` to the parent of the first complete
/// one found. `espeak-rs-sys` ships an incomplete subset (missing `phontab`
/// and other core files); without a system install Piper's phonemizer
/// fails. Skipped if the env var is already set externally.
///
/// MUST run before the tokio runtime is built — `set_var` is `unsafe`
/// because concurrent `getenv` from any other thread is UB on Unix
/// libc. By calling this from the synchronous `main()` before any
/// runtime threads exist, we satisfy that precondition.
#[cfg(feature = "speech")]
fn probe_espeak_ng_data(verbose: bool) {
    if std::env::var_os("PIPER_ESPEAKNG_DATA_DIRECTORY").is_some() {
        return;
    }
    const ESPEAK_PARENT_CANDIDATES: &[&str] = &[
        "/opt/homebrew/share", // macOS Apple Silicon (brew install espeak-ng)
        "/usr/local/share",    // macOS Intel / generic
        "/usr/share",          // Linux (apt/dnf install espeak-ng-data)
    ];
    for parent in ESPEAK_PARENT_CANDIDATES {
        let probe = std::path::Path::new(parent).join("espeak-ng-data/phontab");
        if probe.is_file() {
            if verbose {
                eprintln!("[tts] found espeak-ng-data under {parent}");
            }
            // SAFETY: we are running before the tokio runtime (and any
            // worker threads, audio threads, or third-party library
            // threads) have been started. No other thread can be
            // calling getenv concurrently, so this `set_var` is sound.
            unsafe {
                std::env::set_var("PIPER_ESPEAKNG_DATA_DIRECTORY", parent);
            }
            return;
        }
    }
}

fn main() -> anyhow::Result<()> {
    // Load `.env` then `~/.primer_env` (earlier sources win). Must run
    // before clap parses `--api-key` and before any thread is spawned.
    primer_engine::env_files::load_env_files();

    // Probe for system espeak-ng-data BEFORE the tokio runtime spawns
    // worker threads — `set_var` requires a single-threaded context.
    // Pre-parse `--verbose` so the probe can log its hit on stderr.
    #[cfg(feature = "speech")]
    {
        let verbose = std::env::args().any(|a| a == "--verbose");
        probe_espeak_ng_data(verbose);
    }

    run_tokio_on_main()
}

/// Build the tokio runtime that drives `async_main()`.
///
/// **Non-macos-native (default):** multi-thread runtime on the OS main
/// thread — historical behaviour. tokio's worker pool handles
/// `tokio::spawn` background tasks (classifier, extractor,
/// comprehension, embedding) in parallel with the dialogue's main turn.
///
/// **macos-native:** current-thread runtime on the OS main thread. The
/// voice loop's `MacosTtsSession::push_text` is a synchronous fn that
/// dispatches AVSpeechSynthesizer synthesis on whatever thread it's
/// called from. With current-thread tokio on main, all awaits stay on
/// main, so `push_text` runs on main and takes the main-thread
/// synthesis path (`primer_speech::macos::tts::synthesize_streaming_main_thread`, private) —
/// the path that drives `NSRunLoop::runUntilDate` from inside the call
/// and drains the GCD main queue (AVFoundation primes the
/// queue→runloop integration on its first AVSpeechSynthesizer
/// instantiation; nobody else needs to).
///
/// The background-thread synthesis path (worker → `dispatch_async_f`
/// to the main queue → wait on `dispatch_semaphore`) is **not used**
/// on macos-native CLI: it would only work if AppKit's
/// `NSApplicationMain` (or an explicit `dispatch_main()`) had wired
/// the main queue to the main run loop, and we can't pull AppKit
/// into a text-mode CLI.
///
/// Cost: background tokio tasks (classifier, extractor, etc.) are
/// blocked while `push_text` runs (typically 1–2 s per phrase). They
/// catch up at the start of the next turn via
/// `await_pending_post_response`, so the user-visible behaviour is
/// unchanged.
fn run_tokio_on_main() -> anyhow::Result<()> {
    // Both macOS-native builds (macos-native and macos-native-26) require a
    // current-thread runtime on the OS main thread so that AVSpeechSynthesizer
    // synthesis runs on main (the path that drives NSRunLoop::runUntilDate).
    // See the doc-comment on this function for the full rationale.
    #[cfg(all(
        target_os = "macos",
        any(feature = "macos-native", feature = "macos-native-26")
    ))]
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    #[cfg(not(all(
        target_os = "macos",
        any(feature = "macos-native", feature = "macos-native-26")
    )))]
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async_main())
}

/// Default tracing filter when `RUST_LOG` is unset.
///
/// `info` keeps the Primer's own diagnostics (banners, classifier
/// identifier, dialogue manager warnings) visible. `ort=warn` quiets
/// ONNX Runtime, which emits ~200 INFO lines per silero/piper session
/// init describing graph optimisations — useful exactly once and noise
/// thereafter. `whisper_cpp_plus=warn` and `cpal=warn` silence the
/// other speech-stack libraries we don't normally want to hear from.
/// Set `RUST_LOG=debug` (or `RUST_LOG=ort=info`) for the firehose.
const DEFAULT_LOG_FILTER: &str = "info,ort=warn,whisper_cpp_plus=warn,cpal=warn";

async fn async_main() -> anyhow::Result<()> {
    // Initialise tracing (set RUST_LOG=debug for verbose output, or
    // RUST_LOG=ort=info to see ONNX Runtime session-init logs).
    tracing_subscriber::fmt()
        // stderr, so log lines never mix into the conversation on stdout.
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(DEFAULT_LOG_FILTER)),
        )
        .init();

    let cli = Cli::parse();

    let Inference {
        backend,
        notice,
        main_model,
        backend_params,
    } = build_inference(&cli).await;

    let SessionSetup {
        cli_locale,
        home,
        session_path,
        session_store,
        learner,
    } = open_session(&cli).await?;

    // Pedagogy config.
    let pedagogy_config = PedagogyConfig {
        break_suggest_after_minutes: cli.session_break_after_mins,
        ..PedagogyConfig::default()
    };

    let subsystem_settings = SubsystemSettings {
        classifier: ClassifierSettings {
            blocking_timeout: std::time::Duration::from_millis(cli.classifier_timeout_ms),
            ..ClassifierSettings::default()
        },
        extractor: ExtractorSettings {
            blocking_timeout: std::time::Duration::from_millis(cli.extractor_timeout_ms),
            ..ExtractorSettings::default()
        },
        comprehension: primer_comprehension::ComprehensionSettings {
            blocking_timeout: std::time::Duration::from_millis(cli.comprehension_timeout_ms),
            ..primer_comprehension::ComprehensionSettings::default()
        },
    };

    // Inherited subsystems run on whatever serves turns — the fallback when
    // the primary is unavailable — so the shared helper resolves that first
    // (#351, #364).
    let ServedSubsystems {
        classifier,
        extractor,
        comprehension,
        ..
    } = match build_served_subsystems(
        &backend,
        &cli.backend,
        &main_model,
        notice.as_ref(),
        &backend_params,
        &subsystem_settings,
    )
    .await
    {
        Ok(built) => built,
        Err(e) => {
            eprintln!("Error {e}");
            std::process::exit(1);
        }
    };
    eprintln!("Engagement classifier: {}", classifier.identifier());
    eprintln!("Concept extractor: {}", extractor.identifier());
    eprintln!("Comprehension classifier: {}", comprehension.identifier());

    let embedder = build_embedder(&cli).await;

    // ─── Knowledge base ──────────────────────────────────────────────
    //
    // Locale-scoped: passages are indexed in the locale's own FTS5 table
    // so BM25 statistics stay locale-pure. With an embedder wired the
    // default KB is a per-model cache under `~/.primer/kb/`, so the seed
    // corpus is embedded once rather than on every start (#388); without
    // one it stays `:memory:`. An explicit `--knowledge-db` is seeded only
    // when empty and never embedded here (`primer-kb-load --reembed` owns
    // that). An empty KB is a valid runtime state.
    let seed_files = primer_kb_load::discover_seed_files(cli_locale);
    let knowledge_db = resolve_knowledge_db(
        cli.knowledge_db.clone(),
        Some(home.as_path()),
        embedder.as_ref().map(|e| e.model_id()),
        seed_corpus_dir(&seed_files).as_deref(),
        cli.no_persist,
    );
    // `(0, total)` comes once, before the first batch, and only when there
    // is something to embed: a passage lacks a vector (a fresh or grown
    // cache, or `:memory:`), or a model re-registered at a new dim forces
    // a full re-embed.
    let mut announce_embed = |done: usize, total: usize| {
        if done == 0 && total > 0 {
            eprintln!(
                "Embedding the knowledge base for semantic search (this can take a few minutes)..."
            );
        }
    };
    let (knowledge, prepared) = open_prepared_kb(
        &knowledge_db,
        cli_locale,
        &seed_files,
        embedder.as_deref(),
        &mut announce_embed,
    )
    .await?;
    if let Some(e) = &prepared.embed_error {
        eprintln!(
            "Warning: knowledge search is partly keyword-only (BM25): embedding failed ({e}); \
             it retries next start."
        );
    }

    // ─── Dialogue manager ────────────────────────────────────────────

    let stores = primer_pedagogy::DialogueManagerStores {
        session: Some(Arc::clone(&session_store) as Arc<dyn SessionStore>),
        learner: Some(Arc::clone(&session_store) as Arc<dyn LearnerStore>),
    };
    let vocab_settings = primer_pedagogy::VocabSettings {
        max_per_prompt: cli
            .vocab_max_per_prompt
            .map(|n| n as usize)
            .unwrap_or(primer_core::consts::vocab::DEFAULT_VOCAB_MAX_PER_PROMPT),
    };
    let subsystems = primer_pedagogy::DialogueManagerSubsystems {
        classifier,
        classifier_settings: subsystem_settings.classifier,
        extractor,
        extractor_settings: subsystem_settings.extractor,
        comprehension,
        comprehension_settings: subsystem_settings.comprehension,
        vocab_settings,
        embedder: embedder.clone(),
    };
    let knowledge_arc: Arc<dyn KnowledgeBase> = Arc::new(knowledge);
    let mut dm = DialogueManager::new(
        learner,
        Arc::clone(&backend),
        Arc::clone(&knowledge_arc),
        stores,
        subsystems,
        pedagogy_config,
    );

    // ─── Speech branch ───────────────────────────────────────────────

    #[cfg(feature = "speech")]
    if cli.speech {
        // On either macOS-native build the four whisper/piper flags are not
        // declared at all (#112) — SFSpeechRecognizer / SpeechAnalyzer +
        // AVSpeechSynthesizer carry STT and TTS and the corresponding
        // `SpeechLoopConfig` fields are likewise cfg-gated out.
        #[cfg(not(all(
            target_os = "macos",
            any(feature = "macos-native", feature = "macos-native-26")
        )))]
        let cfg = {
            let whisper_model = cli
                .whisper_model
                .as_ref()
                .expect("clap requires whisper_model with --speech");
            let tts: primer_speech::voice_loop::TtsBackend = cli.tts.into();
            validate_speech_assets(
                whisper_model,
                tts,
                cli.voice_onnx.as_deref(),
                cli.voice_config.as_deref(),
                cli.supertonic_dir.as_deref(),
                cli.supertonic_voice_style.as_deref(),
                &cli.voice,
            )?;
            speech_loop::SpeechLoopConfig {
                whisper_model: whisper_model.clone(),
                voice_onnx: cli.voice_onnx.clone(),
                voice_config: cli.voice_config.clone(),
                voice_id: cli.voice.clone(),
                tts,
                supertonic_dir: cli.supertonic_dir.clone(),
                supertonic_voice_style: cli.supertonic_voice_style.clone(),
                mic_silence_ms: cli.mic_silence_ms,
                verbose: cli.verbose,
                locale: cli_locale,
            }
        };
        #[cfg(all(
            target_os = "macos",
            any(feature = "macos-native", feature = "macos-native-26")
        ))]
        let cfg = speech_loop::SpeechLoopConfig {
            mic_silence_ms: cli.mic_silence_ms,
            verbose: cli.verbose,
            locale: cli_locale,
        };
        // run() builds backends from cfg, wires DialogueManager via a
        // Responder adapter, drives the state machine.
        speech_loop::run(cfg, &mut dm).await?;
        return Ok(());
    }

    repl::run_repl(&cli, &mut dm, &session_store, &session_path, cli_locale).await
}
