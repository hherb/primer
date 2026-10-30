//! # primer-kb-load
//!
//! JSONL → SQLite knowledge-base loader. Used by the auto-seed path in
//! `SqliteKnowledgeBase::open_for_locale` and by the standalone
//! `primer-kb-load` binary.
//!
//! ## Format
//!
//! Each line is a single JSON object matching [`SeedPassage`]:
//!
//! ```json
//! {
//!   "id": "seed:en:rayleigh-scattering",
//!   "source": "seed:en:rayleigh-scattering",
//!   "license": "CC0-1.0",
//!   "attribution": "The Primer seed corpus",
//!   "source_url": null,
//!   "text": "The sky looks blue because…",
//!   "topics": ["physics", "optics", "weather"]
//! }
//! ```
//!
//! `topics` is informational only and not persisted. `source` is a stable
//! string reused as the foreign key into the `sources` table. An optional
//! `parent_source` object (issue #40) declares an umbrella source the
//! passage belongs to; the loader registers it once and links each child
//! source to it via `sources.parent_source_id`. The flat hand-drafted seed
//! corpus omits `parent_source`.
//!
//! ## Idempotency
//!
//! The loader treats `id` as the deduplication key — a row already in
//! `passages_<pack>_content` with the same `id` is **skipped**, not
//! overwritten, mirroring `primer-storage::SessionStore::save_session`'s
//! append-only behaviour. Re-running the loader on the same JSONL is a no-op.
//! The `sources` table uses upsert semantics (refresh `retrieved_at` and
//! attribution on every load).

use primer_core::error::{PrimerError, Result};
use primer_core::i18n::Locale;
use primer_core::knowledge::SourceMeta;
use primer_knowledge::SqliteKnowledgeBase;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

/// One row in a seed-corpus JSONL file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeedPassage {
    /// Unique passage id (stable across reruns).
    pub id: String,
    /// Source identifier, also used to lookup attribution metadata.
    pub source: String,
    /// Licence tag, e.g. `"CC-BY-SA-4.0"`. Required so we never
    /// accidentally store unlicenced content.
    pub license: String,
    /// Human-readable credit line.
    pub attribution: String,
    /// Canonical URL, if any.
    #[serde(default)]
    pub source_url: Option<String>,
    /// The passage text.
    pub text: String,
    /// Informational topic tags. Not persisted.
    #[serde(default)]
    pub topics: Vec<String>,
    /// Optional umbrella source this passage belongs to (issue #40). When
    /// present, the passage's own source row is linked to `parent_source.id`
    /// and the umbrella row itself is registered in the `sources` table.
    /// Absent for the flat hand-drafted seed corpus.
    #[serde(default)]
    pub parent_source: Option<ParentSource>,
}

/// An umbrella source declaration carried inline on each Wikipedia-shaped
/// passage so a credits UI can render one aggregated "Powered by …" line
/// instead of one row per article. Many passages repeat the same value;
/// the loader de-dupes it into a single `sources` row. See issue #40.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParentSource {
    /// Stable umbrella id, e.g. `"wiki-simple:en"`. Referenced by each
    /// child source's `parent_source_id`.
    pub id: String,
    /// Licence tag for the corpus as a whole.
    pub license: String,
    /// Aggregated human-readable credit line for the whole corpus.
    pub attribution: String,
    /// Canonical site-root URL for the source, if any.
    #[serde(default)]
    pub source_url: Option<String>,
}

/// Summary of a load run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoadStats {
    pub inserted: usize,
    pub skipped_existing: usize,
    pub sources_seen: usize,
}

/// A parsed seed corpus, not yet written: its passages plus every source
/// row they reference (umbrella rows included, de-duped by id).
#[derive(Default)]
struct ParsedSeed {
    passages: Vec<SeedPassage>,
    sources: HashMap<String, SourceMeta>,
}

/// Load a JSONL file into `kb`. Passages are inserted if their `id` is
/// new, skipped otherwise; sources are upserted when any passage is new.
///
/// The whole file is parsed before anything is written, and its sources
/// and passages land in one transaction, so a failed load writes nothing
/// (#308).
///
/// Errors:
/// - I/O errors reading the file → `PrimerError::Knowledge` with context.
/// - Malformed JSON, or an `id` repeated within the file → the file path,
///   line number and cause are returned in `PrimerError::Knowledge`;
///   nothing from the file is written.
pub async fn load_jsonl(kb: &SqliteKnowledgeBase, path: &Path) -> Result<LoadStats> {
    write_seed(kb, parse_jsonl(path)?).await
}

/// Load several seed files as one corpus: every file is parsed, then all
/// of them are written in a single transaction. A bad line or a failed
/// write in any file therefore leaves the KB as it was — a half-loaded KB
/// would be non-empty and stop `auto_seed_if_empty` from ever retrying
/// (#308). An `id` that also appears in an earlier file is skipped, like
/// one already in the KB.
pub async fn load_seed_files(kb: &SqliteKnowledgeBase, paths: &[PathBuf]) -> Result<LoadStats> {
    let mut corpus = ParsedSeed::default();
    for path in paths {
        tracing::info!(
            target = "primer-kb-load",
            path = %path.display(),
            "parsing seed corpus"
        );
        let seed = parse_jsonl(path)?;
        corpus.passages.extend(seed.passages);
        for (id, src) in seed.sources {
            corpus.sources.entry(id).or_insert(src);
        }
    }
    write_seed(kb, corpus).await
}

/// Read and parse a JSONL seed file without touching the KB.
fn parse_jsonl(path: &Path) -> Result<ParsedSeed> {
    let file = std::fs::File::open(path)
        .map_err(|e| PrimerError::Knowledge(format!("open {}: {e}", path.display())))?;
    let reader = BufReader::new(file);
    let mut passages = Vec::new();
    let mut ids = HashSet::new();
    let mut sources: HashMap<String, SourceMeta> = HashMap::new();
    let now = chrono::Utc::now().timestamp();

    for (line_no, line) in reader.lines().enumerate() {
        let line = line.map_err(|e| {
            PrimerError::Knowledge(format!(
                "{}: read line {}: {e}",
                path.display(),
                line_no + 1
            ))
        })?;
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let passage: SeedPassage = serde_json::from_str(line).map_err(|e| {
            PrimerError::Knowledge(format!(
                "{}: parse JSONL line {}: {e}",
                path.display(),
                line_no + 1
            ))
        })?;
        // No UNIQUE constraint backs `id`, so a repeat would land twice (#383).
        if !ids.insert(passage.id.clone()) {
            return Err(PrimerError::Knowledge(format!(
                "{}: line {}: duplicate passage id {}",
                path.display(),
                line_no + 1,
                passage.id
            )));
        }

        // Register the source the first time we see it in this file, linking
        // it to its umbrella parent when the passage declares one (#40).
        sources
            .entry(passage.source.clone())
            .or_insert_with(|| SourceMeta {
                id: passage.source.clone(),
                license: passage.license.clone(),
                attribution: passage.attribution.clone(),
                source_url: passage.source_url.clone(),
                retrieved_at: now,
                parent_source_id: passage.parent_source.as_ref().map(|p| p.id.clone()),
            });

        // Register the umbrella source itself (de-duped across passages that
        // share it). An umbrella has no parent of its own.
        if let Some(parent) = &passage.parent_source {
            sources
                .entry(parent.id.clone())
                .or_insert_with(|| SourceMeta {
                    id: parent.id.clone(),
                    license: parent.license.clone(),
                    attribution: parent.attribution.clone(),
                    source_url: parent.source_url.clone(),
                    retrieved_at: now,
                    parent_source_id: None,
                });
        }
        passages.push(passage);
    }
    Ok(ParsedSeed { passages, sources })
}

/// Write a parsed seed corpus — its sources and every passage whose `id`
/// is not already in the KB (or earlier in the corpus) — in one
/// transaction. With no new passage nothing is written, so topping up an
/// already-current KB on every startup costs no write.
async fn write_seed(kb: &SqliteKnowledgeBase, seed: ParsedSeed) -> Result<LoadStats> {
    let ParsedSeed { passages, sources } = seed;
    let mut seen = existing_passage_ids(kb)?;
    let new: Vec<(&str, &str, &str)> = passages
        .iter()
        .filter(|p| seen.insert(p.id.clone()))
        .map(|p| (p.id.as_str(), p.source.as_str(), p.text.as_str()))
        .collect();
    let sources: Vec<SourceMeta> = sources.into_values().collect();
    // `insert_seed_new` re-checks the ids inside its write transaction: the
    // read above can be stale when another process seeds the same file.
    let inserted = if new.is_empty() {
        0
    } else {
        kb.insert_seed_new(&sources, &new)?
    };
    let stats = LoadStats {
        inserted,
        skipped_existing: passages.len() - inserted,
        sources_seen: sources.len(),
    };

    tracing::info!(
        target = "primer-kb-load",
        inserted = stats.inserted,
        skipped = stats.skipped_existing,
        sources = stats.sources_seen,
        "loaded JSONL into knowledge base"
    );
    Ok(stats)
}

/// Collect the set of passage ids already in the KB, so we can dedup in
/// O(N+M) instead of one SELECT per row.
fn existing_passage_ids(kb: &SqliteKnowledgeBase) -> Result<HashSet<String>> {
    kb.list_passage_ids()
}

/// Search known locations for `seed_passages.<pack_id>.jsonl`, in order:
///
/// 1. `$PRIMER_SEED_DIR/seed_passages.<pack_id>.jsonl` (env override).
/// 2. `$XDG_DATA_HOME/primer/seed/seed_passages.<pack_id>.jsonl`.
/// 3. Cargo dev path: `<workspace_root>/data/seed/seed_passages.<pack_id>.jsonl`.
///
/// Returns the canonical-named file, if any. Use [`discover_seed_files`]
/// to discover *all* matching files (the path that `auto_seed_if_empty`
/// uses).
pub fn discover_seed_jsonl(locale: Locale) -> Option<PathBuf> {
    let canonical = format!("seed_passages.{}.jsonl", locale.pack_id());
    discover_seed_files(locale)
        .into_iter()
        .find(|p| p.file_name().and_then(|n| n.to_str()) == Some(&canonical))
}

/// Discover ALL seed JSONL files for `locale` in the first search-path
/// directory that contains any. The search order matches
/// [`discover_seed_jsonl`] (env override → XDG → cargo dev path); whichever
/// directory yields at least one matching file wins, and all matching
/// files in that directory are returned.
///
/// "Matching" means a regular file whose name ends with `.<pack>.jsonl`,
/// where `<pack>` is `locale.pack_id()`. This lets the in-repo seed dir
/// hold both `seed_passages.en.jsonl` (CC0 hand-drafted) and
/// `wiki_passages.en.jsonl` (CC-BY-SA-3.0 wiki layer) side by side, while
/// `wiki_passages.de.jsonl` is correctly ignored when the locale is
/// English.
///
/// Returns an empty `Vec` if no candidate directory exists.
pub fn discover_seed_files(locale: Locale) -> Vec<PathBuf> {
    let pack = locale.pack_id();
    let suffix = format!(".{pack}.jsonl");

    for dir in candidate_seed_dirs() {
        let mut hits = Vec::new();
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if name.ends_with(&suffix) {
                hits.push(path);
            }
        }
        if !hits.is_empty() {
            hits.sort();
            return hits;
        }
    }
    Vec::new()
}

/// The ordered list of directories to look for seed files in. Mirrors
/// the existing [`discover_seed_jsonl`] precedence.
fn candidate_seed_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(d) = std::env::var("PRIMER_SEED_DIR") {
        dirs.push(PathBuf::from(d));
    }
    if let Some(data_home) = xdg_data_home() {
        dirs.push(data_home.join("primer/seed"));
    }
    if let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
        let mut p = PathBuf::from(manifest_dir);
        for _ in 0..5 {
            dirs.push(p.join("data/seed"));
            if !p.pop() {
                break;
            }
        }
    }
    dirs
}

fn xdg_data_home() -> Option<PathBuf> {
    if let Ok(d) = std::env::var("XDG_DATA_HOME") {
        if !d.is_empty() {
            return Some(PathBuf::from(d));
        }
    }
    std::env::var("HOME")
        .ok()
        .map(|h| PathBuf::from(h).join(".local/share"))
}

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
/// persistent per-model cache passes `true` so a grown corpus reaches an
/// already-seeded file (existing ids are skipped). A seed failure is an
/// error, as in [`auto_seed_if_empty`]. An embedding failure is soft
/// (returned in [`PrepareStats::embed_error`]): the session still works
/// on BM25, and the next open retries.
///
/// When `embedder`'s model id is registered at a different dim (a model
/// re-pulled under the same name), every stored vector is useless to it,
/// so they are cleared and all passages re-embedded. Startup only passes
/// an embedder for a KB whose vectors are derived data (`:memory:` or the
/// per-model cache), never for a user-managed file.
pub async fn prepare_kb(
    kb: &SqliteKnowledgeBase,
    seed_files: &[PathBuf],
    top_up: bool,
    embedder: Option<&dyn primer_core::embedder::Embedder>,
) -> Result<PrepareStats> {
    let mut stats = PrepareStats::default();
    // shortcut: top-up only adds new ids, so a seed passage edited in place
    // keeps its old text (and vector) in the cache; upgrade when seed edits
    // must reach existing caches.
    if !seed_files.is_empty() && (top_up || kb.passage_count()? == 0) {
        stats.seeded = Some(load_seed_files(kb, seed_files).await?);
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
        match reembed_kb(kb, embedder, force, DEFAULT_EMBED_BATCH_SIZE).await {
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

#[cfg(test)]
mod tests;
