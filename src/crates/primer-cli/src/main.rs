//! Primer CLI — a text-mode REPL for developing and testing the Socratic
//! dialogue engine without any hardware (no microphone, no speaker, no
//! E Ink display). Just a terminal and a conversation.
//!
//! Usage:
//!   primer                         # Stub backend (canned responses, no model needed)
//!   primer --backend cloud          # Anthropic Claude API
//!   primer --name Binti --age 8    # Set learner profile

use chrono::Utc;
use clap::Parser;
use primer_core::config::PedagogyConfig;
use primer_core::knowledge::KnowledgeBase;
use primer_core::learner::*;
use primer_inference::stub::StubBackend;
use primer_knowledge::SqliteKnowledgeBase;
use primer_pedagogy::DialogueManager;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Parser, Debug)]
#[command(name = "primer", about = "The Primer — a Socratic learning companion")]
struct Cli {
    /// Inference backend: "stub" or "cloud".
    #[arg(long, default_value = "stub")]
    backend: String,

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

    /// Anthropic API key (for cloud backend).
    #[arg(long, env = "ANTHROPIC_API_KEY")]
    api_key: Option<String>,
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
    // Initialise tracing (set RUST_LOG=debug for verbose output).
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    // ─── Create backends ─────────────────────────────────────────────

    // For now, only the stub backend is fully wired. Cloud backend
    // requires the API key and is a TODO for the next iteration.
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
            eprintln!("Using cloud inference backend (Anthropic Claude).");
            Box::new(primer_inference::cloud::CloudBackend::new(
                "https://api.anthropic.com".to_string(),
                api_key,
                "claude-sonnet-4-6".to_string(),
            ))
        }
        other => {
            eprintln!("Unknown backend: {other}. Use 'stub' or 'cloud'.");
            std::process::exit(1);
        }
    };

    // Knowledge base — in-memory by default (empty, but functional).
    let knowledge_path = cli
        .knowledge_db
        .unwrap_or_else(|| PathBuf::from(":memory:"));
    let knowledge = SqliteKnowledgeBase::open(&knowledge_path)?;

    // Learner model.
    let learner = create_learner(&cli.name, cli.age);

    // Pedagogy config.
    let pedagogy_config = PedagogyConfig::default();

    // ─── Dialogue manager ────────────────────────────────────────────

    let mut dm = DialogueManager::new(
        learner,
        inference.as_ref(),
        &knowledge as &dyn KnowledgeBase,
        pedagogy_config,
    );

    // ─── REPL ────────────────────────────────────────────────────────

    let greeting = dm.open_session().await?;
    println!("\nPrimer: {greeting}\n");

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
            dm.close_session();
            println!("\nPrimer: That was a good conversation. Until next time.\n");
            break;
        }

        // Generate the Primer's response.
        match dm.respond_to(input).await {
            Ok(response) => {
                println!("\nPrimer: {response}\n");
            }
            Err(e) => {
                eprintln!("Error generating response: {e}");
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
