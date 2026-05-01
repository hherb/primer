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

use chrono::Utc;
use clap::Parser;
use primer_core::config::PedagogyConfig;
use primer_core::knowledge::KnowledgeBase;
use primer_core::learner::*;
use primer_core::storage::SessionStore;
use primer_inference::stub::StubBackend;
use primer_knowledge::SqliteKnowledgeBase;
use primer_pedagogy::DialogueManager;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
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
    #[arg(long, value_name = "UUID")]
    resume: Option<Uuid>,

    /// Anthropic API key (for cloud backend).
    #[arg(long, env = "ANTHROPIC_API_KEY")]
    api_key: Option<String>,
}

/// Slugify a learner name into a filesystem-safe filename stem. ASCII
/// alphanumerics are kept (lowercased); every other character becomes
/// `-`; runs of `-` collapse; leading/trailing `-` are stripped. An
/// empty result falls back to `default` so we always produce a valid
/// filename.
fn slug(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut last_was_sep = true; // suppress leading sep
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
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
/// `~/.primer/<slug(name)>.db` when no explicit path is given, or the
/// explicit path verbatim when one is. Returns the resolved path along
/// with a flag indicating whether it lives under the default home.
fn resolve_session_db_path(
    explicit: Option<PathBuf>,
    learner_name: &str,
) -> Result<PathBuf, String> {
    if let Some(path) = explicit {
        return Ok(path);
    }
    let home = std::env::var("HOME").map_err(|_| {
        "Cannot find HOME env var to default --session-db; pass it explicitly".to_string()
    })?;
    let dir = PathBuf::from(home).join(PRIMER_HOME_DIR);
    Ok(dir.join(format!("{}.db", slug(learner_name))))
}

fn create_learner(name: &str, age: u8) -> LearnerModel {
    LearnerModel {
        profile: LearnerProfile {
            id: Uuid::new_v4(),
            name: name.to_string(),
            age,
            languages: vec!["en".to_string()],
            created_at: Utc::now(),
            last_active: Utc::now(),
        },
        concepts: vec![],
        preferences: LearningPreferences::default(),
        current_engagement: EngagementState::Engaged,
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

    let inference: Box<dyn primer_core::inference::InferenceBackend> = match cli.backend.as_str() {
        "stub" => {
            eprintln!("Using stub inference backend (canned Socratic responses).");
            Box::new(StubBackend)
        }
        "cloud" => {
            let api_key = cli.api_key.unwrap_or_else(|| {
                eprintln!("Error: --api-key or ANTHROPIC_API_KEY required for cloud backend.");
                std::process::exit(1);
            });
            let model = cli.model.unwrap_or_else(|| "claude-sonnet-4-6".to_string());
            eprintln!("Using cloud inference backend (Anthropic {model}).");
            Box::new(primer_inference::cloud::CloudBackend::new(
                "https://api.anthropic.com".to_string(),
                api_key,
                model,
            ))
        }
        "ollama" => {
            let model = cli.model.unwrap_or_else(|| {
                eprintln!("Error: --model required for ollama backend (e.g., --model llama3.2).");
                std::process::exit(1);
            });
            eprintln!(
                "Using ollama backend at {} with model {model}.",
                cli.ollama_url
            );
            Box::new(primer_inference::ollama::OllamaBackend::new(
                cli.ollama_url,
                model,
            ))
        }
        other => {
            eprintln!("Unknown backend: {other}. Use 'stub', 'cloud', or 'ollama'.");
            std::process::exit(1);
        }
    };

    // Knowledge base — in-memory by default (empty, but functional).
    let knowledge_path = cli.knowledge_db.unwrap_or_else(|| PathBuf::from(IN_MEMORY));
    let knowledge = SqliteKnowledgeBase::open(&knowledge_path)?;

    // Session store — defaults to a per-learner file under `~/.primer/`.
    let session_path = match resolve_session_db_path(cli.session_db, &cli.name) {
        Ok(p) => p,
        Err(msg) => {
            eprintln!("Error: {msg}");
            std::process::exit(1);
        }
    };

    // Resume requires the file to already exist; do not auto-create on
    // a typo'd path. Catch this BEFORE we open (which would create it).
    if cli.resume.is_some() && !Path::new(&session_path).exists() {
        eprintln!(
            "Error: --resume requires an existing --session-db; {} does not exist.",
            session_path.display()
        );
        std::process::exit(1);
    }

    // Ensure parent directory exists before opening.
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

    let session_store = match primer_storage::SqliteSessionStore::open(&session_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "Error: cannot open session-db {}: {e}",
                session_path.display()
            );
            std::process::exit(1);
        }
    };
    eprintln!("Session DB: {}", session_path.display());

    // Learner model.
    let learner = create_learner(&cli.name, cli.age);

    // Pedagogy config.
    let pedagogy_config = PedagogyConfig::default();

    // ─── Dialogue manager ────────────────────────────────────────────

    let mut dm = DialogueManager::new(
        learner,
        inference.as_ref(),
        &knowledge as &dyn KnowledgeBase,
        Some(&session_store as &dyn SessionStore),
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
        assert_eq!(slug("José"), "jos");
        assert_eq!(slug("Lee/Davis"), "lee-davis");
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
        let p = resolve_session_db_path(Some(PathBuf::from("/tmp/explicit.db")), "Anyone").unwrap();
        assert_eq!(p, PathBuf::from("/tmp/explicit.db"));
    }

    #[test]
    fn resolve_session_db_path_default_uses_home_and_slug() {
        // Use a synthetic HOME so the test doesn't depend on the dev's
        // real home directory. Cleanup is done via setting HOME back —
        // but env vars are process-global and tests run in parallel, so
        // we restore on drop.
        struct HomeGuard {
            original: Option<String>,
        }
        impl Drop for HomeGuard {
            fn drop(&mut self) {
                match &self.original {
                    Some(v) => unsafe { std::env::set_var("HOME", v) },
                    None => unsafe { std::env::remove_var("HOME") },
                }
            }
        }
        let _guard = HomeGuard {
            original: std::env::var("HOME").ok(),
        };
        unsafe { std::env::set_var("HOME", "/synthetic/home") };
        let p = resolve_session_db_path(None, "Binti").unwrap();
        assert_eq!(p, PathBuf::from("/synthetic/home/.primer/binti.db"));
    }
}
