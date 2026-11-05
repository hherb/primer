//! Startup preparation: auto-seed an empty KB, top up the per-model cache,
//! and backfill missing vectors (`reembed_kb*`, `prepare_kb`).

use crate::{LoadStats, discover_seed_files, load_seed_corpus, load_seed_files};
use primer_core::error::{PrimerError, Result};
use primer_core::i18n::Locale;
use primer_knowledge::SqliteKnowledgeBase;
use std::path::PathBuf;

/// Backfill embeddings for every passage in `kb` that doesn't already
/// have one. With `force = true`, first clears every vector in the locale
/// (and unregisters models no locale still uses), so every passage is
/// re-embedded and a model may come back at a new dim — the model-swap
/// path (#377). Calls `embedder` in batches to amortise the per-call cost.
///
/// Returns the number of passages embedded. Errors fail the whole call —
/// embedding is not best-effort here, because a partial fill is exactly
/// the problem this is designed to fix. A `force` run that fails part-way
/// leaves the remaining passages unembedded; rerunning (with or without
/// `force`) completes it.
pub async fn reembed_kb(
    kb: &SqliteKnowledgeBase,
    embedder: &dyn primer_core::embedder::Embedder,
    force: bool,
    batch_size: usize,
) -> Result<usize> {
    reembed_kb_with_progress(kb, embedder, force, batch_size, &mut |_, _| {}).await
}

/// Progress callback for [`reembed_kb_with_progress`] / [`prepare_kb`]:
/// `(passages embedded so far, passages to embed in total)`.
pub type EmbedProgress<'a> = &'a mut (dyn FnMut(usize, usize) + Send);

/// [`reembed_kb`] that also reports progress (#393): `(0, total)` before
/// the first batch, then `(done, total)` after each one, ending at
/// `(total, total)` on success. Nothing is reported when no passage needs
/// a vector.
pub async fn reembed_kb_with_progress(
    kb: &SqliteKnowledgeBase,
    embedder: &dyn primer_core::embedder::Embedder,
    force: bool,
    batch_size: usize,
    on_progress: EmbedProgress<'_>,
) -> Result<usize> {
    if force {
        let cleared = kb.clear_embeddings()?;
        tracing::info!(
            target = "primer-kb-load",
            cleared,
            "reembed --force: cleared existing vectors"
        );
    }
    let candidates: Vec<(String, String)> = kb.passages_missing_embedding()?;

    if candidates.is_empty() {
        tracing::info!(
            target = "primer-kb-load",
            "no passages need embedding; nothing to do"
        );
        return Ok(0);
    }

    tracing::info!(
        target = "primer-kb-load",
        count = candidates.len(),
        model = embedder.model_id(),
        dim = embedder.dim(),
        "reembedding passages"
    );

    let total = candidates.len();
    on_progress(0, total);
    let mut done = 0_usize;
    for chunk in candidates.chunks(batch_size.max(1)) {
        let texts: Vec<&str> = chunk.iter().map(|(_, t)| t.as_str()).collect();
        let vecs = embedder.embed(&texts).await?;
        // `zip` below would silently leave the tail unembedded (#311);
        // checked before any upsert so a bad chunk writes nothing.
        if vecs.len() != chunk.len() {
            return Err(PrimerError::Knowledge(format!(
                "embedder {} returned {} vector(s) for {} input(s); refusing to under-embed",
                embedder.model_id(),
                vecs.len(),
                chunk.len()
            )));
        }
        for ((id, _), v) in chunk.iter().zip(vecs.into_iter()) {
            kb.upsert_embedding(id, embedder.model_id(), embedder.dim(), &v)?;
            done += 1;
        }
        on_progress(done, total);
    }
    Ok(done)
}

/// Auto-seed `kb` from the discovered JSONL file(s) if `kb` is empty.
///
/// All `*.<pack>.jsonl` files in the first matching search-path directory
/// are loaded in lexicographic order (e.g. both `seed_passages.en.jsonl`
/// and `wiki_passages.en.jsonl` will load on a fresh English KB). The
/// returned `LoadStats` aggregates inserts/skips across all loaded files.
///
/// Returns:
/// - `Ok(Some(stats))` if at least one seed file was found and loaded.
/// - `Ok(None)` if either the KB already has passages or no seed files
///   could be located.
///
/// Errors propagate from the loader; discovery itself never errors.
pub async fn auto_seed_if_empty(
    kb: &SqliteKnowledgeBase,
    locale: Locale,
) -> Result<Option<LoadStats>> {
    if kb.passage_count()? > 0 {
        return Ok(None);
    }
    let files = discover_seed_files(locale);
    if files.is_empty() {
        tracing::info!(
            target = "primer-kb-load",
            locale = locale.pack_id(),
            "no seed corpus found; knowledge base starts empty"
        );
        return Ok(None);
    }
    tracing::info!(
        target = "primer-kb-load",
        locale = locale.pack_id(),
        files = files.len(),
        "loading seed corpus into empty knowledge base"
    );
    Ok(Some(load_seed_files(kb, &files).await?))
}

/// Passages per embedder call when startup or `--reembed` fills vectors.
pub const DEFAULT_EMBED_BATCH_SIZE: usize = 16;

/// What [`prepare_kb`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrepareStats {
    /// The seed load, if one ran.
    pub seeded: Option<LoadStats>,
    /// Passages that got a vector (0 when embedding failed, though the
    /// batches embedded before the failure are kept).
    pub embedded: usize,
    /// Why embedding failed, for the caller to show the user: retrieval
    /// runs keyword-only (BM25) on the passages left without a vector.
    pub embed_error: Option<String>,
}

/// Get a knowledge base ready at startup (#388): seed it from
/// `seed_files` (usually [`discover_seed_files`]), then give every passage
/// without a vector one when an `embedder` is wired.
///
/// The seed loads when the KB is empty, or always with `top_up` — the
/// persistent per-model cache passes `true` so a grown or edited corpus
/// reaches an already-seeded file (unchanged ids are skipped; an edited
/// passage is replaced and re-embedded below, #392). `top_up` also makes
/// the KB mirror the corpus: a stored passage the seed files no longer
/// have is deleted (#398), so pass it only for a KB holding this corpus
/// alone, never a user-managed file. The cache then mirrors whichever
/// corpus this run discovered: a seed file that is missing here has its
/// passages pruned (and re-embedded when it is back). An empty corpus
/// prunes nothing. A seed failure is an
/// error, as in [`auto_seed_if_empty`]. An embedding failure is soft
/// (returned in [`PrepareStats::embed_error`]): the session still works
/// on BM25, and the next open retries.
///
/// When `embedder`'s model id is registered at a different dim (a model
/// re-pulled under the same name), every stored vector is useless to it,
/// so they are cleared and all passages re-embedded. Startup only passes
/// an embedder for a KB whose vectors are derived data (`:memory:` or the
/// per-model cache), never for a user-managed file.
///
/// `on_progress` sees the embed step's progress, as in
/// [`reembed_kb_with_progress`] (#393).
pub async fn prepare_kb(
    kb: &SqliteKnowledgeBase,
    seed_files: &[PathBuf],
    top_up: bool,
    embedder: Option<&dyn primer_core::embedder::Embedder>,
    on_progress: EmbedProgress<'_>,
) -> Result<PrepareStats> {
    let mut stats = PrepareStats::default();
    if !seed_files.is_empty() && (top_up || kb.passage_count()? == 0) {
        stats.seeded = Some(load_seed_corpus(kb, seed_files, top_up).await?);
    }
    if let Some(embedder) = embedder {
        let force = match kb.check_embedder(embedder) {
            Ok(()) => false,
            Err(e) => {
                tracing::warn!(
                    target = "primer-kb-load",
                    model = embedder.model_id(),
                    "stored vectors don't fit the embedder; re-embedding every passage: {e}"
                );
                true
            }
        };
        match reembed_kb_with_progress(kb, embedder, force, DEFAULT_EMBED_BATCH_SIZE, on_progress)
            .await
        {
            Ok(n) => stats.embedded = n,
            Err(e) => {
                tracing::warn!(
                    target = "primer-kb-load",
                    model = embedder.model_id(),
                    still_missing = kb.passages_missing_embedding().map(|m| m.len()).ok(),
                    "embedding the knowledge base failed; hybrid retrieval falls back toward BM25: {e}"
                );
                stats.embed_error = Some(e.to_string());
            }
        }
    }
    Ok(stats)
}
