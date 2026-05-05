//! `primer-kb-load` — load a JSONL passage corpus into a Primer knowledge-base
//! SQLite file. Used both for the in-repo seed corpus and for users who
//! download a pre-built JSONL but don't want to run the Python pipeline.
//!
//! ```text
//! primer-kb-load --knowledge-db ./primer-en.db \
//!                --locale en \
//!                --jsonl ./seed_passages.en.jsonl
//! ```

use clap::Parser;
use primer_core::i18n::Locale;
use primer_knowledge::SqliteKnowledgeBase;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "primer-kb-load",
    about = "Load a JSONL passage corpus into a Primer knowledge-base file"
)]
struct Args {
    /// Path to the SQLite knowledge-base file. Created if missing.
    #[arg(long)]
    knowledge_db: PathBuf,

    /// Locale pack id, e.g. `en` or `de`.
    #[arg(long, default_value = "en")]
    locale: String,

    /// Path to the JSONL file to load.
    #[arg(long)]
    jsonl: PathBuf,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let args = Args::parse();
    let locale = Locale::from_pack_id(&args.locale)
        .ok_or_else(|| format!("unknown locale pack id: {:?}", args.locale))?;

    let kb = SqliteKnowledgeBase::open_for_locale(&args.knowledge_db, locale)?;
    let stats = primer_kb_load::load_jsonl(&kb, &args.jsonl).await?;

    println!(
        "loaded {} passages ({} skipped, {} sources) from {} into {} (locale={})",
        stats.inserted,
        stats.skipped_existing,
        stats.sources_seen,
        args.jsonl.display(),
        args.knowledge_db.display(),
        locale.pack_id(),
    );
    Ok(())
}
