//! `primer-kb-load` — load JSONL passage corpora into a Primer
//! knowledge-base SQLite file, OR backfill embeddings for an existing
//! corpus DB via `--reembed`.
//!
//! ```text
//! # JSONL ingestion
//! primer-kb-load --knowledge-db ./primer-en.db \
//!                --locale en \
//!                --jsonl ./seed_passages.en.jsonl
//!
//! # Embedding backfill (requires the `fastembed` cargo feature)
//! primer-kb-load --reembed \
//!                --knowledge-db ./primer-en.db \
//!                --locale en
//! ```

use clap::Parser;
use primer_core::i18n::Locale;
use primer_knowledge::SqliteKnowledgeBase;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser, Debug)]
#[command(
    name = "primer-kb-load",
    about = "Load a JSONL passage corpus into a Primer knowledge-base file, or reembed an existing one"
)]
struct Args {
    /// Path to the SQLite knowledge-base file. Created if missing.
    #[arg(long)]
    knowledge_db: PathBuf,

    /// Locale pack id, e.g. `en` or `de`.
    #[arg(long, default_value = "en")]
    locale: String,

    /// Path to the JSONL file to load. Mutually exclusive with `--reembed`.
    #[arg(long, conflicts_with = "reembed")]
    jsonl: Option<PathBuf>,

    /// Backfill embeddings for every passage missing one (or, with
    /// `--force`, every passage). Requires the `fastembed` feature.
    #[arg(long)]
    reembed: bool,

    /// With `--reembed`, re-embed every passage rather than only those
    /// missing an embedding. Useful after switching embedding models.
    #[arg(long, requires = "reembed")]
    force: bool,

    /// Embedder backend used for `--reembed`. Today only `fastembed`
    /// makes sense — `stub` would replace real embeddings with hashed
    /// noise, which is rarely what the user wants.
    #[arg(long, default_value = "fastembed", value_name = "BACKEND")]
    embedder_backend: String,

    /// Batch size passed to the embedder per call.
    #[arg(long, default_value_t = 16)]
    embed_batch_size: usize,
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

    if args.reembed {
        let embedder = build_embedder(&args.embedder_backend)?;
        let n =
            primer_kb_load::reembed_kb(&kb, embedder.as_ref(), args.force, args.embed_batch_size)
                .await?;
        println!(
            "reembedded {} passages in {} (locale={}) using {}",
            n,
            args.knowledge_db.display(),
            locale.pack_id(),
            embedder.model_id(),
        );
        return Ok(());
    }

    let Some(jsonl) = args.jsonl else {
        eprintln!("Error: pass either --jsonl <path> for ingestion or --reembed for backfill");
        std::process::exit(1);
    };

    let stats = primer_kb_load::load_jsonl(&kb, &jsonl).await?;

    println!(
        "loaded {} passages ({} skipped, {} sources) from {} into {} (locale={})",
        stats.inserted,
        stats.skipped_existing,
        stats.sources_seen,
        jsonl.display(),
        args.knowledge_db.display(),
        locale.pack_id(),
    );
    Ok(())
}

#[cfg(feature = "fastembed")]
fn build_embedder(
    name: &str,
) -> Result<Arc<dyn primer_core::embedder::Embedder>, Box<dyn std::error::Error>> {
    match name {
        "fastembed" => {
            eprintln!(
                "Loading fastembed BGE-M3; first run downloads ~570 MB into ~/.cache/primer/models/."
            );
            let b = primer_embedding::FastEmbedBackend::new()?;
            Ok(Arc::new(b))
        }
        "stub" => {
            eprintln!(
                "Note: --embedder-backend stub will write deterministic hash vectors; not useful for production retrieval."
            );
            Ok(Arc::new(primer_embedding::StubEmbedder::new()))
        }
        other => Err(format!("unknown --embedder-backend {other:?}").into()),
    }
}

#[cfg(not(feature = "fastembed"))]
fn build_embedder(
    name: &str,
) -> Result<Arc<dyn primer_core::embedder::Embedder>, Box<dyn std::error::Error>> {
    match name {
        "stub" => {
            eprintln!(
                "Note: --embedder-backend stub will write deterministic hash vectors; not useful for production retrieval."
            );
            Ok(Arc::new(primer_embedding::StubEmbedder::new()))
        }
        "fastembed" => Err(
            "--embedder-backend fastembed requires the `fastembed` cargo feature; \
             rebuild primer-kb-load with `--features fastembed`"
                .into(),
        ),
        other => Err(format!("unknown --embedder-backend {other:?}").into()),
    }
}
