//! Primer CLI — a text-mode REPL for developing and testing the Socratic
//! dialogue engine without any hardware (no microphone, no speaker, no
//! E Ink display). Just a terminal and a conversation.
//!
//! Usage:
//!   primer                                              # Stub backend (canned responses, no model needed)
//!   primer --backend cloud                              # Anthropic Claude API (default model)
//!   primer --backend cloud --model claude-opus-4-7      # Override the cloud model
//!   primer --backend ollama --model llama3.2            # Local Ollama server
//!   primer --name Binti --age 8                         # Set learner profile
//!   primer --resume <uuid>                              # Resume a past session

use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::Utc;
use clap::Parser;
use primer_classifier::{
    ClassifierSettings, EngagementClassifier, LlmEngagementClassifier, StubEngagementClassifier,
};
use primer_core::config::PedagogyConfig;
use primer_core::error::{PrimerError, Result};
use primer_core::inference::InferenceBackend;
use primer_core::knowledge::KnowledgeBase;
use primer_core::learner::*;
use primer_core::storage::{LearnerStore, SessionStore};
use primer_inference::stub::StubBackend;
use primer_knowledge::SqliteKnowledgeBase;
use primer_pedagogy::DialogueManager;
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;

/// SQLite path token for an in-memory database — used as the default
/// for `--knowledge-db` when no path is given. Sessions persist to a
/// per-learner file under `~/.primer/` instead.
const IN_MEMORY: &str = ":memory:";

/// Subdirectory under `$HOME` for per-learner session databases.
const PRIMER_HOME_DIR: &str = ".primer";

#[derive(Parser, Debug)]
#[command(name = "primer", about = "The Primer — a Socratic learning companion")]
struct Cli {
    /// Inference backend: "stub", "cloud", or "ollama".
    #[arg(long, default_value = "stub")]
    backend: String,

    /// Model identifier. For cloud: Anthropic model id (default: claude-sonnet-4-6).
    /// For ollama: local model tag (e.g., "llama3.2", "qwen2.5:7b") — required.
    #[arg(long)]
    model: Option<String>,

    /// Ollama server URL (used when --backend ollama).
    #[arg(long, default_value = "http://localhost:11434")]
    ollama_url: String,

    /// Child's name (for the learner profile).
    #[arg(long, default_value = "Explorer")]
    name: String,

    /// Child's age in years.
    #[arg(long, default_value_t = 8)]
    age: u8,

    /// Path to knowledge base SQLite file.
    /// If omitted, uses an in-memory database.
    #[arg(long)]
    knowledge_db: Option<PathBuf>,

    /// Path to session database SQLite file.
    /// If omitted, defaults to `~/.primer/<slug-of-name>.db` (per-learner
    /// file, created if missing). Pass an explicit path only when you
    /// want a non-default location.
    #[arg(long)]
    session_db: Option<PathBuf>,

    /// Resume an existing session by UUID. Read from `--session-db`
    /// (default `~/.primer/<name>.db`). Errors if the file doesn't
    /// exist or no session with that id is stored. Works with any
    /// backend including stub: historical turns provide context, new
    /// turns get backend-appropriate responses.
    #[arg(long, value_name = "UUID", conflicts_with = "no_persist")]
    resume: Option<Uuid>,

    /// Run the session in-memory only — nothing is written to disk
    /// and the conversation evaporates on exit. Useful for quick
    /// experiments and tests; the per-learner default-path persistence
    /// stays out of the way. Mutually exclusive with `--resume` and
    /// `--session-db`.
    #[arg(long, conflicts_with_all = ["session_db", "resume"])]
    no_persist: bool,

    /// Anthropic API key (for cloud backend).
    #[arg(long, env = "ANTHROPIC_API_KEY")]
    api_key: Option<String>,

    /// Backend used for the engagement classifier. Defaults to the same
    /// backend used for the chat (--backend); `stub` forces deterministic
    /// classification regardless of main backend.
    #[arg(long)]
    classifier_backend: Option<String>,

    /// Model used for the engagement classifier. Defaults to the same
    /// model used for the chat (--model). Useful for haiku-as-classifier
    /// + sonnet-as-chat configurations on bigger machines.
    #[arg(long)]
    classifier_model: Option<String>,

    /// Maximum time to block awaiting the previous turn's classification
    /// before the next intent decision. Default 500ms.
    #[arg(long, default_value_t = 500)]
    classifier_timeout_ms: u64,

    /// Print pedagogical decisions (intent chosen, classifier output)
    /// alongside the conversation, on stderr. Stdout stays clean.
    #[arg(long)]
    verbose: bool,
}

/// Slugify a learner name into a filesystem-safe filename stem.
///
/// The input is first NFC-normalized so two visually identical names
/// (e.g. precomposed `é` vs decomposed `e` + combining acute) map to
/// the same slug. Characters that Unicode classifies as alphanumeric
/// — Latin, Cyrillic, CJK, etc. — are kept (Latin is lowercased; CJK
/// has no case so it round-trips). Every other character becomes `-`;
/// runs of `-` collapse; leading/trailing `-` are stripped. An empty
/// result falls back to `default` so we always produce a valid filename.
fn slug(name: &str) -> String {
    let normalized: String = name.nfc().collect();
    let lowered = normalized.to_lowercase();
    let mut out = String::with_capacity(lowered.len());
    let mut last_was_sep = true; // suppress leading sep
    for c in lowered.chars() {
        if c.is_alphanumeric() {
            out.push(c);
            last_was_sep = false;
        } else if !last_was_sep {
            out.push('-');
            last_was_sep = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "default".to_string()
    } else {
        out
    }
}

/// Resolve the path to use for the session database.
/// `:memory:` when `no_persist` is set; otherwise the explicit path if
/// given, falling back to `<home>/.primer/<slug(name)>.db`. The home
/// directory is taken as a parameter so callers can supply it from any
/// source (env var in production, synthetic value in tests) without
/// this function touching the process environment.
fn resolve_session_db_path(
    explicit: Option<PathBuf>,
    home: &Path,
    learner_name: &str,
    no_persist: bool,
) -> PathBuf {
    if no_persist {
        return PathBuf::from(IN_MEMORY);
    }
    explicit.unwrap_or_else(|| {
        home.join(PRIMER_HOME_DIR)
            .join(format!("{}.db", slug(learner_name)))
    })
}

/// Should we print the "we just started persisting your sessions"
/// banner? True only when the session DB is at the default path AND
/// the file did not exist before this run AND the user did not opt
/// out via `--no-persist`. The banner answers the legitimate "where
/// did my conversation go?" question that the silent default-path
/// change would otherwise raise.
fn should_show_first_run_banner(
    explicit_session_db: bool,
    no_persist: bool,
    file_existed_before: bool,
) -> bool {
    !explicit_session_db && !no_persist && !file_existed_before
}

/// Parameters needed by `build_backend` and `build_classifier` that would
/// otherwise require borrowing from `Cli`. Extracted early (before any partial
/// moves of `Cli` fields) so the helpers can be called after those moves.
struct BackendParams {
    api_key: Option<String>,
    ollama_url: String,
    classifier_backend: Option<String>,
    classifier_model: Option<String>,
}

/// Construct an `InferenceBackend` of the named type with the given model.
///
/// All three backend variants are synchronous at construction time; the
/// function signature is `async` only for uniformity with `build_classifier`.
async fn build_backend(
    backend_name: &str,
    model: String,
    params: &BackendParams,
) -> Result<Arc<dyn InferenceBackend>> {
    match backend_name {
        "stub" => Ok(Arc::new(StubBackend)),
        "cloud" => {
            let api_key = params.api_key.clone().ok_or_else(|| {
                PrimerError::Inference(
                    "--api-key or ANTHROPIC_API_KEY required for cloud backend".into(),
                )
            })?;
            Ok(Arc::new(primer_inference::cloud::CloudBackend::new(
                "https://api.anthropic.com".to_string(),
                api_key,
                model,
            )))
        }
        "ollama" => Ok(Arc::new(primer_inference::ollama::OllamaBackend::new(
            params.ollama_url.clone(),
            model,
        ))),
        other => Err(PrimerError::Inference(format!("unknown backend: {other}"))),
    }
}

/// Construct the engagement classifier according to the dispatch matrix:
///
/// | main backend | --classifier-backend | --classifier-model | outcome                               |
/// |--------------|----------------------|--------------------|---------------------------------------|
/// | stub         | (unset)              | (any)              | StubEngagementClassifier              |
/// | *            | "stub"               | (any)              | StubEngagementClassifier              |
/// | *            | some(X)              | None               | error (model required for non-stub)   |
/// | *            | some(X)              | some(M)            | LlmEngagementClassifier(new backend X, model M) |
/// | non-stub     | (unset)              | None               | LlmEngagementClassifier(Arc::clone main) |
/// | non-stub     | (unset)              | some(M)            | LlmEngagementClassifier(new backend same type, model M) |
async fn build_classifier(
    main_backend: Arc<dyn InferenceBackend>,
    main_backend_name: &str,
    main_model: &str,
    params: &BackendParams,
    settings: ClassifierSettings,
) -> Result<Arc<dyn EngagementClassifier>> {
    match (main_backend_name, params.classifier_backend.as_deref()) {
        // Explicit stub override always wins regardless of main backend.
        (_, Some("stub")) => Ok(Arc::new(StubEngagementClassifier::new())),

        // Main backend is stub and no classifier backend specified → default to stub.
        // (LlmEngagementClassifier wrapping a stub backend would silently return
        // Unknown on every classify call — that's worse than a deterministic stub.)
        ("stub", None) => Ok(Arc::new(StubEngagementClassifier::new())),

        // Explicit non-stub classifier backend → need a model too.
        (_, Some(cls_backend_name)) => {
            let model = params.classifier_model.clone().ok_or_else(|| {
                PrimerError::Inference(
                    "--classifier-model is required when --classifier-backend is set to a non-stub backend".into(),
                )
            })?;
            let cls_backend = build_backend(cls_backend_name, model.clone(), params).await?;
            Ok(Arc::new(LlmEngagementClassifier::new(
                cls_backend,
                model,
                settings,
            )))
        }

        // No --classifier-backend specified, main is not stub.
        (_, None) => {
            match params.classifier_model.as_deref() {
                // No model override → reuse the main backend via Arc::clone.
                // The classifier identifier becomes "llm:<main-model>".
                None => Ok(Arc::new(LlmEngagementClassifier::new(
                    Arc::clone(&main_backend),
                    main_model.to_string(),
                    settings,
                ))),
                // Model override only → construct a fresh backend of the same TYPE
                // with the override model, because InferenceBackend::generate() does
                // not accept a model argument — model is baked in at construction.
                Some(override_model) => {
                    let cls_backend =
                        build_backend(main_backend_name, override_model.to_string(), params)
                            .await?;
                    Ok(Arc::new(LlmEngagementClassifier::new(
                        cls_backend,
                        override_model.to_string(),
                        settings,
                    )))
                }
            }
        }
    }
}

fn create_learner_with_id(id: Uuid, name: &str, age: u8) -> LearnerModel {
    LearnerModel {
        profile: LearnerProfile {
            id,
            name: name.to_string(),
            age,
            languages: vec!["en".to_string()],
            created_at: Utc::now(),
            last_active: Utc::now(),
        },
        concepts: vec![],
        preferences: LearningPreferences::default(),
        current_engagement: EngagementState::Engaged,
        recent_assessments: vec![],
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load env files. Project-local `.env` first (searches cwd and ancestors),
    // then a user-global `~/.primer_env` for secrets that should live outside
    // any single repo. Earlier sources win — `from_path` does not override
    // existing env vars by default. Must run before clap parses `--api-key`.
    let _ = dotenvy::dotenv();
    if let Ok(home) = std::env::var("HOME") {
        let path = std::path::PathBuf::from(home).join(".primer_env");
        let _ = dotenvy::from_path(&path);
    }

    // Initialise tracing (set RUST_LOG=debug for verbose output).
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    // ─── Create backends ─────────────────────────────────────────────

    // Resolve the model for the main backend early so we can report it
    // in the banner AND pass the resolved value to build_classifier later.
    let main_model: String = match cli.backend.as_str() {
        "cloud" => cli
            .model
            .clone()
            .unwrap_or_else(|| "claude-sonnet-4-6".to_string()),
        "ollama" => cli.model.clone().unwrap_or_else(|| {
            eprintln!("Error: --model required for ollama backend (e.g., --model llama3.2).");
            std::process::exit(1);
        }),
        // stub (and anything else — will error in build_backend below)
        _ => cli.model.clone().unwrap_or_else(|| "stub".to_string()),
    };

    // Extract the fields that build_backend / build_classifier need BEFORE
    // any partial moves of other Cli fields (knowledge_db, session_db etc.).
    let backend_params = BackendParams {
        api_key: cli.api_key.clone(),
        ollama_url: cli.ollama_url.clone(),
        classifier_backend: cli.classifier_backend.clone(),
        classifier_model: cli.classifier_model.clone(),
    };

    let backend: Arc<dyn InferenceBackend> =
        match build_backend(&cli.backend, main_model.clone(), &backend_params).await {
            Ok(b) => b,
            Err(e) => {
                eprintln!("Error constructing backend: {e}");
                std::process::exit(1);
            }
        };

    match cli.backend.as_str() {
        "stub" => eprintln!("Using stub inference backend (canned Socratic responses)."),
        "cloud" => eprintln!("Using cloud inference backend (Anthropic {main_model})."),
        "ollama" => eprintln!(
            "Using ollama backend at {} with model {main_model}.",
            cli.ollama_url
        ),
        other => {
            eprintln!("Unknown backend: {other}. Use 'stub', 'cloud', or 'ollama'.");
            std::process::exit(1);
        }
    }

    // Knowledge base — in-memory by default (empty, but functional).
    let knowledge_path = cli.knowledge_db.unwrap_or_else(|| PathBuf::from(IN_MEMORY));
    let knowledge = SqliteKnowledgeBase::open(&knowledge_path)?;

    // Session store — defaults to a per-learner file under `~/.primer/`.
    // We look up HOME here (rather than inside `resolve_session_db_path`)
    // so the function stays a pure path computation that's trivial to test.
    let explicit_session_db = cli.session_db.is_some();
    let home = match std::env::var("HOME") {
        Ok(h) => PathBuf::from(h),
        Err(_) => {
            if cli.session_db.is_none() && !cli.no_persist {
                eprintln!(
                    "Error: cannot find HOME env var to default --session-db; pass it explicitly"
                );
                std::process::exit(1);
            }
            // HOME is unused when an explicit path is given or --no-persist is set.
            PathBuf::new()
        }
    };
    let session_path = resolve_session_db_path(cli.session_db, &home, &cli.name, cli.no_persist);

    // Resume requires the file to already exist; do not auto-create on
    // a typo'd path. Catch this BEFORE we open (which would create it).
    // (--resume is mutually exclusive with --no-persist via clap, so the
    // path here is always a real on-disk path.)
    if cli.resume.is_some() && !Path::new(&session_path).exists() {
        eprintln!(
            "Error: --resume requires an existing --session-db; {} does not exist.",
            session_path.display()
        );
        std::process::exit(1);
    }

    // Capture file-existed state BEFORE we open (open creates it). Used
    // for the first-run banner. The :memory: token is never a real path,
    // so its existence check is naturally false — that's fine: the banner
    // is also gated on !no_persist below.
    let file_existed_before = !cli.no_persist && Path::new(&session_path).exists();

    // Ensure parent directory exists before opening. Not needed for :memory:.
    if !cli.no_persist {
        if let Some(parent) = session_path.parent() {
            if !parent.as_os_str().is_empty() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    eprintln!(
                        "Error: cannot create session-db directory {}: {e}",
                        parent.display()
                    );
                    std::process::exit(1);
                }
            }
        }
    }

    let session_store: Arc<primer_storage::SqliteSessionStore> =
        Arc::new(match primer_storage::SqliteSessionStore::open(&session_path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!(
                    "Error: cannot open session-db {}: {e}",
                    session_path.display()
                );
                std::process::exit(1);
            }
        });
    if cli.no_persist {
        eprintln!("Session DB: in-memory (no persistence; session ends when you exit).");
    } else {
        eprintln!("Session DB: {}", session_path.display());
        if should_show_first_run_banner(explicit_session_db, cli.no_persist, file_existed_before) {
            eprintln!(
                "Note: this is your first session for {name}. Conversations are now persisted\n      \
                 locally to {path}. Use `--no-persist` to opt out, or `--session-db` to relocate.",
                name = cli.name,
                path = session_path.display(),
            );
        }
    }

    // Learner model — load from persistent store or mint fresh on first run.
    let learner = match session_store.load_learner().await {
        Ok(Some(mut existing)) => {
            if existing.profile.name != cli.name {
                tracing::warn!(
                    "CLI --name {:?} differs from persisted learner name {:?}; using persisted",
                    cli.name,
                    existing.profile.name
                );
            }
            existing.profile.age = cli.age;
            existing.profile.last_active = Utc::now();
            if let Err(e) = session_store.save_learner(&existing).await {
                tracing::warn!("save_learner on startup failed: {e}");
            }
            existing
        }
        Ok(None) => {
            // First run on this file. Two sub-cases:
            //   1. Truly fresh DB → mint a new UUID.
            //   2. v3 DB with sessions but no learners row → adopt the
            //      most-recent session's learner_id so existing sessions
            //      are not orphaned.
            let id = match session_store.most_recent_session_learner_id().await {
                Ok(Some(uuid)) => {
                    tracing::info!("adopted learner_id {uuid} from existing sessions");
                    uuid
                }
                Ok(None) => Uuid::new_v4(),
                Err(e) => {
                    tracing::warn!(
                        "most_recent_session_learner_id failed: {e}; minting fresh UUID"
                    );
                    Uuid::new_v4()
                }
            };
            let fresh = create_learner_with_id(id, &cli.name, cli.age);
            if let Err(e) = session_store.save_learner(&fresh).await {
                tracing::warn!("save_learner on startup failed: {e}");
            }
            fresh
        }
        Err(e) => {
            // load_learner failing on startup is catastrophic — the file
            // is unreadable or the schema is corrupt. Propagate.
            return Err(anyhow::anyhow!("load_learner failed on startup: {e}"));
        }
    };

    // Pedagogy config.
    let pedagogy_config = PedagogyConfig::default();

    let classifier_settings = ClassifierSettings {
        blocking_timeout: std::time::Duration::from_millis(cli.classifier_timeout_ms),
        ..ClassifierSettings::default()
    };

    let classifier: Arc<dyn EngagementClassifier> = match build_classifier(
        Arc::clone(&backend),
        &cli.backend,
        &main_model,
        &backend_params,
        classifier_settings.clone(),
    )
    .await
    {
        Ok(c) => {
            eprintln!("Engagement classifier: {}", c.identifier());
            c
        }
        Err(e) => {
            eprintln!("Error constructing engagement classifier: {e}");
            std::process::exit(1);
        }
    };

    // ─── Dialogue manager ────────────────────────────────────────────

    let mut dm = DialogueManager::new(
        learner,
        backend.as_ref(),
        &knowledge as &dyn KnowledgeBase,
        Some(Arc::clone(&session_store) as Arc<dyn SessionStore>),
        Some(Arc::clone(&session_store) as Arc<dyn LearnerStore>),
        classifier,
        classifier_settings,
        pedagogy_config,
    );

    // ─── REPL ────────────────────────────────────────────────────────

    if let Some(resume_id) = cli.resume {
        match session_store.load_session(resume_id).await? {
            None => {
                eprintln!(
                    "Error: no session with id {resume_id} found in {}",
                    session_path.display()
                );
                std::process::exit(1);
            }
            Some(loaded) => {
                let n_turns = loaded.turns.len();
                dm.resume_session(loaded).await?;
                eprintln!("\nResumed session {resume_id} with {n_turns} prior turn(s).\n");
            }
        }
    } else {
        let greeting = dm.open_session().await?;
        println!("\nPrimer: {greeting}\n");
    }

    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let reader = stdin.lock();

    for line in reader.lines() {
        let line = line?;
        let input = line.trim();

        // Exit commands.
        if input.is_empty() {
            continue;
        }
        if input.eq_ignore_ascii_case("quit")
            || input.eq_ignore_ascii_case("exit")
            || input.eq_ignore_ascii_case("bye")
        {
            dm.close_session().await;
            println!("\nPrimer: That was a good conversation. Until next time.\n");
            break;
        }

        // Generate the Primer's response, printing tokens as they arrive.
        // The "Primer: " prefix is held back until the first non-empty chunk
        // so that an immediate failure (no API key, ollama down) doesn't leave
        // a dangling "Primer: " above the error message.
        let mut prefix_printed = false;
        let result = dm
            .respond_to_streaming(input, |chunk| {
                if !prefix_printed {
                    print!("\nPrimer: ");
                    prefix_printed = true;
                }
                print!("{chunk}");
                let _ = io::stdout().flush();
            })
            .await;
        match result {
            Ok(_) => {
                if prefix_printed {
                    println!("\n");
                } else {
                    eprintln!("\n(no response generated)\n");
                }
            }
            Err(e) => {
                if prefix_printed {
                    println!();
                }
                eprintln!("Error generating response: {e}\n");
            }
        }

        // Print pedagogical debug info when --verbose is set.
        if cli.verbose {
            if let Some(intent) = dm.last_intent() {
                eprintln!(
                    "[intent] {:?} -> {:?}",
                    dm.learner.current_engagement, intent
                );
            }
            if let Some(a) = dm.last_assessment() {
                let r = a.reasoning.as_deref().unwrap_or("");
                eprintln!(
                    "[classifier] {:?} conf={:.2} ({})",
                    a.state,
                    a.confidence,
                    dm.classifier_identifier()
                );
                if !r.is_empty() {
                    eprintln!("             — {r}");
                }
            }
        }

        // Check if the session has run long.
        if dm.should_suggest_break() {
            println!("Primer: We've been talking for a while. Want to take a break?\n");
        }

        print!("{}: ", dm.learner.profile.name);
        stdout.flush()?;
    }

    Ok(())
}

#[cfg(test)]
mod classifier_construction_tests {
    use super::*;

    /// Build a minimal `BackendParams` for testing.
    fn params_with(
        classifier_backend: Option<&str>,
        classifier_model: Option<&str>,
    ) -> BackendParams {
        BackendParams {
            api_key: None,
            ollama_url: "http://localhost:11434".into(),
            classifier_backend: classifier_backend.map(String::from),
            classifier_model: classifier_model.map(String::from),
        }
    }

    /// With main=stub and no classifier flags, we get a stub classifier.
    #[tokio::test]
    async fn stub_main_no_flags_gives_stub_classifier() {
        let params = params_with(None, None);
        let main = build_backend("stub", "main".into(), &params).await.unwrap();
        let c = build_classifier(main, "stub", "main", &params, ClassifierSettings::default())
            .await
            .unwrap();
        assert_eq!(c.identifier(), "stub");
    }

    /// `--classifier-backend stub` wins regardless of the main backend.
    #[tokio::test]
    async fn explicit_stub_override_wins_over_any_main() {
        // Main is stub (would default to stub anyway), but the explicit flag
        // should also work when the main is declared as a different type.
        // We use stub here to avoid needing a real cloud backend in tests.
        let params_main = params_with(None, None);
        let main = build_backend("stub", "main-model".into(), &params_main)
            .await
            .unwrap();
        let params = params_with(Some("stub"), None);
        let c = build_classifier(
            main,
            "cloud",
            "main-model",
            &params,
            ClassifierSettings::default(),
        )
        .await
        .unwrap();
        assert_eq!(c.identifier(), "stub");
    }

    /// No classifier flags on a non-stub main → LlmEngagementClassifier wrapping
    /// Arc::clone of main (identifier is "llm:<main-model>").
    #[tokio::test]
    async fn non_stub_main_no_flags_gives_llm_with_main_model() {
        // We simulate a non-stub main by using "stub" type but treating it as
        // cloud in the dispatch (the type doesn't matter for the Arc::clone path;
        // what matters is that main_backend_name is NOT "stub").
        // Since we can't construct a real cloud backend without an API key,
        // we build a stub backend but pass "cloud" as the name to exercise
        // the dispatch arm that reuses main via Arc::clone.
        let params = params_with(None, None);
        let main = build_backend("stub", "claude-sonnet-4-6".into(), &params)
            .await
            .unwrap();
        let c = build_classifier(
            main,
            "cloud", // use "cloud" as name to exercise the non-stub arm
            "claude-sonnet-4-6",
            &params,
            ClassifierSettings::default(),
        )
        .await
        .unwrap();
        assert_eq!(c.identifier(), "llm:claude-sonnet-4-6");
    }

    /// `--classifier-backend ollama` without `--classifier-model` is an error.
    #[tokio::test]
    async fn classifier_backend_without_model_is_error() {
        let params_main = params_with(None, None);
        let main = build_backend("stub", "main".into(), &params_main)
            .await
            .unwrap();
        let params = params_with(Some("ollama"), None /* no model */);
        let result =
            build_classifier(main, "stub", "main", &params, ClassifierSettings::default()).await;
        assert!(
            result.is_err(),
            "should error when --classifier-backend is non-stub and --classifier-model is missing"
        );
    }

    /// `--classifier-model` override on a stub main still yields stub classifier
    /// (the stub arm fires before we reach the model-override arm).
    #[tokio::test]
    async fn classifier_model_override_on_stub_main_still_yields_stub() {
        let params = params_with(None, Some("override-model"));
        let main = build_backend("stub", "main".into(), &params).await.unwrap();
        let c = build_classifier(main, "stub", "main", &params, ClassifierSettings::default())
            .await
            .unwrap();
        // Main is stub → classifier defaults to stub regardless of model override.
        assert_eq!(c.identifier(), "stub");
    }

    /// `--classifier-model` override on a NON-stub main constructs a fresh
    /// backend of the same type with the override model (case 5 in the dispatch
    /// matrix, lines 284-292 in build_classifier).
    ///
    /// The `main_backend` Arc is a stub (we cannot unit-test real cloud/ollama
    /// without live infrastructure), but `main_backend_name` is "ollama" so the
    /// dispatch falls through to the model-override branch and calls
    /// `build_backend("ollama", "override-model", params)`.  `OllamaBackend::new`
    /// is purely constructive (no I/O), so this succeeds in unit tests.
    #[tokio::test]
    async fn classifier_model_override_on_non_stub_main_constructs_fresh_backend() {
        let params = params_with(None, Some("override-model"));
        // The Arc itself is a stub, but the *name* string "ollama" drives dispatch.
        let main = build_backend("stub", "main".into(), &params).await.unwrap();
        let c = build_classifier(
            main,
            "ollama", // non-stub name → falls through to model-override arm
            "main-model",
            &params,
            ClassifierSettings::default(),
        )
        .await
        .unwrap();
        // A fresh OllamaBackend is constructed with "override-model"; the
        // classifier identifier must reflect the override, not the main model.
        assert_eq!(c.identifier(), "llm:override-model");
    }

    /// An unknown `--classifier-backend` value must return an error (the
    /// explicit-backend arm calls `build_backend`, which errors on unknown names).
    #[tokio::test]
    async fn unknown_classifier_backend_returns_error() {
        let params = params_with(Some("nonexistent"), Some("any-model"));
        let main = build_backend("stub", "main".into(), &params).await.unwrap();
        let result =
            build_classifier(main, "stub", "main", &params, ClassifierSettings::default()).await;
        assert!(
            result.is_err(),
            "should error when --classifier-backend names an unknown backend"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_lowercases_and_keeps_alphanumerics() {
        assert_eq!(slug("Explorer"), "explorer");
        assert_eq!(slug("Binti7"), "binti7");
    }

    #[test]
    fn slug_replaces_special_chars_with_dash() {
        assert_eq!(slug("Anna Maria"), "anna-maria");
        assert_eq!(slug("Lee/Davis"), "lee-davis");
    }

    #[test]
    fn slug_keeps_unicode_letters_lowercased() {
        // The previous ASCII-only rule collapsed `José`, `Łukasz`, `Соня`
        // and `美咲` into ambiguous or empty stems. Children's names are
        // a load-bearing input here — accept anything Unicode considers
        // alphanumeric, lowercased where that exists.
        assert_eq!(slug("José"), "josé");
        assert_eq!(slug("Łukasz"), "łukasz");
        assert_eq!(slug("Соня"), "соня");
        // No case-folding for CJK; the chars round-trip as-is.
        assert_eq!(slug("美咲"), "美咲");
    }

    #[test]
    fn slug_normalizes_nfc_so_decomposed_equals_precomposed() {
        // Same visible name, two Unicode encodings: precomposed `é`
        // (U+00E9) vs decomposed `e` + combining acute (U+0301). Without
        // NFC normalization these slug to different filenames, so two
        // copies of the same child get two session DBs.
        let nfc = "Jos\u{00E9}"; // José (NFC)
        let nfd = "Jose\u{0301}"; // José (NFD)
        assert_eq!(slug(nfc), slug(nfd));
    }

    #[test]
    fn slug_collapses_runs_of_separators() {
        assert_eq!(slug("a   b"), "a-b");
        assert_eq!(slug("a---b"), "a-b");
    }

    #[test]
    fn slug_strips_leading_and_trailing_separators() {
        assert_eq!(slug("  hello  "), "hello");
        assert_eq!(slug("___world___"), "world");
    }

    #[test]
    fn slug_empty_input_falls_back_to_default() {
        assert_eq!(slug(""), "default");
        assert_eq!(slug("!!!"), "default");
    }

    #[test]
    fn resolve_session_db_path_passes_explicit_through() {
        // The home arg is unused when an explicit path is given.
        let home = Path::new("/this/should/be/ignored");
        let p = resolve_session_db_path(
            Some(PathBuf::from("/tmp/explicit.db")),
            home,
            "Anyone",
            false,
        );
        assert_eq!(p, PathBuf::from("/tmp/explicit.db"));
    }

    #[test]
    fn resolve_session_db_path_default_uses_home_and_slug() {
        let home = Path::new("/synthetic/home");
        let p = resolve_session_db_path(None, home, "Binti", false);
        assert_eq!(p, PathBuf::from("/synthetic/home/.primer/binti.db"));
    }

    #[test]
    fn resolve_session_db_path_no_persist_returns_in_memory() {
        // `--no-persist` short-circuits everything: no slug, no home
        // join, no explicit path. The session is throwaway.
        let home = Path::new("/some/home");
        assert_eq!(
            resolve_session_db_path(None, home, "Anyone", true),
            PathBuf::from(IN_MEMORY)
        );
    }

    #[test]
    fn no_persist_conflicts_with_resume_at_parse_time() {
        // clap should reject a `--no-persist --resume <uuid>` invocation
        // before we ever try to open anything. In-memory + resume is
        // a contradiction (nothing to resume from).
        let result = Cli::try_parse_from([
            "primer",
            "--no-persist",
            "--resume",
            "00000000-0000-0000-0000-000000000000",
        ]);
        assert!(result.is_err(), "expected clap to reject the combination");
    }

    #[test]
    fn no_persist_conflicts_with_session_db_at_parse_time() {
        // Naming a session DB while asking for in-memory is also a
        // contradiction; clap should reject it up front.
        let result = Cli::try_parse_from(["primer", "--no-persist", "--session-db", "/tmp/x.db"]);
        assert!(result.is_err(), "expected clap to reject the combination");
    }

    #[test]
    fn first_run_banner_shows_only_for_default_path_first_run() {
        // Default path + brand-new file → show banner (the user just
        // started persisting without explicitly opting in).
        assert!(should_show_first_run_banner(false, false, false));
        // Default path but file already existed → silent (not first run).
        assert!(!should_show_first_run_banner(false, false, true));
        // Explicit path → silent (the user knows where their data is).
        assert!(!should_show_first_run_banner(true, false, false));
        // No-persist → silent (no file is being created at all).
        assert!(!should_show_first_run_banner(false, true, false));
    }

    #[test]
    fn resolve_session_db_path_default_handles_unicode_name() {
        // Confirms the slug + path composition round-trip a non-ASCII
        // name without env mutation. The same name in NFC vs NFD must
        // produce the same path so we don't end up with two DB files.
        let home = Path::new("/h");
        assert_eq!(
            resolve_session_db_path(None, home, "José", false),
            PathBuf::from("/h/.primer/josé.db")
        );
        let nfd = "Jose\u{0301}";
        assert_eq!(
            resolve_session_db_path(None, home, nfd, false),
            PathBuf::from("/h/.primer/josé.db")
        );
    }

    #[tokio::test]
    async fn cli_birthday_case_updates_age_and_keeps_uuid() {
        // Save a learner with age=8, simulate startup with --age=9, verify
        // the persisted row has age=9 with the same UUID and created_at.
        use chrono::Utc;
        use primer_core::storage::LearnerStore;
        use primer_storage::SqliteSessionStore;
        use std::sync::Arc;

        let store = Arc::new(SqliteSessionStore::open(std::path::Path::new(":memory:")).unwrap());
        let original_id = Uuid::new_v4();
        let original_created = Utc::now() - chrono::Duration::days(365);
        let mut original = create_learner_with_id(original_id, "Binti", 8);
        original.profile.created_at = original_created;
        store.save_learner(&original).await.unwrap();

        // Simulate startup with --age=9.
        let cli_age: u8 = 9;

        let learner = match store.load_learner().await.unwrap() {
            Some(mut existing) => {
                existing.profile.age = cli_age;
                existing.profile.last_active = Utc::now();
                store.save_learner(&existing).await.unwrap();
                existing
            }
            None => unreachable!(),
        };

        assert_eq!(learner.profile.id, original_id, "UUID stable across launches");
        assert_eq!(learner.profile.age, 9, "age updated to CLI value");
        assert_eq!(
            learner.profile.created_at.timestamp(),
            original_created.timestamp(),
            "created_at preserved",
        );
    }

    #[tokio::test]
    async fn cli_name_mismatch_keeps_persisted_name() {
        // Save with name="Binti", reload with --name="Other", verify the
        // persisted name stays "Binti". This test does not assert the
        // tracing::warn! emission (subscriber-capture would over-couple
        // the test); it asserts the data invariant only.
        use primer_core::storage::LearnerStore;
        use primer_storage::SqliteSessionStore;
        use std::sync::Arc;

        let store = Arc::new(SqliteSessionStore::open(std::path::Path::new(":memory:")).unwrap());
        let original = create_learner_with_id(Uuid::new_v4(), "Binti", 8);
        store.save_learner(&original).await.unwrap();

        // Simulate startup with --name=Other.
        let cli_name = "Other";
        let cli_age: u8 = 8;

        let learner = match store.load_learner().await.unwrap() {
            Some(mut existing) => {
                if existing.profile.name != cli_name {
                    // Production code emits tracing::warn! here. Not asserted in this test.
                }
                existing.profile.age = cli_age;
                existing.profile.last_active = chrono::Utc::now();
                store.save_learner(&existing).await.unwrap();
                existing
            }
            None => unreachable!(),
        };

        assert_eq!(learner.profile.name, "Binti", "persisted name wins over CLI");
    }
}
