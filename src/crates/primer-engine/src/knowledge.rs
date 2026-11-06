//! Open the knowledge base and get it ready at startup (#388, #395).
//!
//! Both binaries call [`open_prepared_kb`], so the policy each
//! [`KnowledgeDb`] variant implies lives in one tested place. Each caller
//! keeps only its own UX: the CLI prints to stderr, the GUI raises a
//! banner from [`PrepareStats::embed_error`].

use std::path::PathBuf;

use primer_core::embedder::Embedder;
use primer_core::error::{PrimerError, Result};
use primer_core::i18n::Locale;
use primer_kb_load::{EmbedProgress, PrepareStats};
use primer_knowledge::SqliteKnowledgeBase;

use crate::paths::{KnowledgeDb, create_private_dir_all};

/// Open `db` under `locale` and run `primer_kb_load::prepare_kb` with the
/// policy the variant implies:
///
/// - [`KnowledgeDb::Cache`]: its parent dir is created owner-only, the
///   `seed_files` corpus is topped up (new and edited passages land,
///   removed ones are pruned), and every passage without a vector is
///   embedded.
/// - [`KnowledgeDb::Explicit`]: seeded only when empty and never
///   embedded. The user manages it with `primer-kb-load --reembed`.
/// - [`KnowledgeDb::InMemory`]: seeded, and embedded when an embedder is
///   given.
///
/// `on_progress` sees the embed progress (see `prepare_kb`). An embed
/// failure is soft, reported in [`PrepareStats::embed_error`]; an open or
/// seed failure is a [`PrimerError::Knowledge`] naming the path.
pub async fn open_prepared_kb(
    db: &KnowledgeDb,
    locale: Locale,
    seed_files: &[PathBuf],
    embedder: Option<&dyn Embedder>,
    on_progress: EmbedProgress<'_>,
) -> Result<(SqliteKnowledgeBase, PrepareStats)> {
    let path = db.path();
    let is_cache = matches!(db, KnowledgeDb::Cache(_));
    // The cache is derived data the user never chose, so say what it is.
    // No "knowledge base" here: `PrimerError::Knowledge` already says it.
    let what = if is_cache {
        format!("{} (a rebuildable cache: safe to delete)", path.display())
    } else {
        path.display().to_string()
    };
    let fail = |step: &str, e: &dyn std::fmt::Display| {
        PrimerError::Knowledge(format!("{step} {what}: {e}"))
    };
    if is_cache && let Some(dir) = path.parent() {
        create_private_dir_all(dir).map_err(|e| fail("creating the directory of", &e))?;
    }
    let kb =
        SqliteKnowledgeBase::open_for_locale(&path, locale).map_err(|e| fail("opening", &e))?;
    let embed_with = match db {
        KnowledgeDb::Explicit(_) => None,
        _ => embedder,
    };
    let stats = primer_kb_load::prepare_kb(&kb, seed_files, is_cache, embed_with, on_progress)
        .await
        .map_err(|e| fail("seeding", &e))?;
    tracing::info!(
        target: "primer::startup",
        path = %path.display(),
        seeded = stats.seeded.as_ref().map_or(0, |s| s.inserted),
        embedded = stats.embedded,
        "knowledge base ready for locale {}",
        locale.pack_id()
    );
    Ok((kb, stats))
}

#[cfg(test)]
mod tests;
