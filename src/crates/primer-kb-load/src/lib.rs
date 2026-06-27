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
use primer_core::knowledge::{KnowledgeBase, SourceMeta};
use primer_knowledge::SqliteKnowledgeBase;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
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

/// Load a JSONL file into `kb`. Sources are upserted; passages are
/// inserted if their `id` is new, skipped otherwise.
///
/// Errors:
/// - I/O errors reading the file → `PrimerError::Knowledge` with context.
/// - Malformed JSON → the line number and parse error are returned in
///   `PrimerError::Knowledge`; loading aborts (no partial commit beyond
///   the rows already inserted — each pair is its own transaction).
pub async fn load_jsonl(kb: &SqliteKnowledgeBase, path: &Path) -> Result<LoadStats> {
    let file = std::fs::File::open(path)
        .map_err(|e| PrimerError::Knowledge(format!("open {}: {e}", path.display())))?;
    let reader = BufReader::new(file);
    let mut stats = LoadStats::default();
    let mut sources: HashMap<String, SourceMeta> = HashMap::new();
    let now = chrono::Utc::now().timestamp();

    let existing_ids: std::collections::HashSet<String> = existing_passage_ids(kb)?;

    for (line_no, line) in reader.lines().enumerate() {
        let line =
            line.map_err(|e| PrimerError::Knowledge(format!("read line {}: {e}", line_no + 1)))?;
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let passage: SeedPassage = serde_json::from_str(line).map_err(|e| {
            PrimerError::Knowledge(format!("parse JSONL line {}: {e}", line_no + 1))
        })?;

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

        if existing_ids.contains(&passage.id) {
            stats.skipped_existing += 1;
            continue;
        }
        kb.insert_passage(&passage.id, &passage.source, &passage.text)?;
        stats.inserted += 1;
    }

    // Upsert umbrella (parent-less) rows before child rows: the
    // `sources.parent_source_id` FK references `sources(id)`, so a child
    // written before its umbrella exists would fail the constraint. The
    // HashMap iteration order is arbitrary, so we must order explicitly.
    for src in sources.values().filter(|s| s.parent_source_id.is_none()) {
        kb.upsert_source(src).await?;
    }
    for src in sources.values().filter(|s| s.parent_source_id.is_some()) {
        kb.upsert_source(src).await?;
    }
    stats.sources_seen = sources.len();

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
fn existing_passage_ids(kb: &SqliteKnowledgeBase) -> Result<std::collections::HashSet<String>> {
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
/// have one (or for every passage when `force = true`). Calls `embedder`
/// in batches to amortise the per-call cost.
///
/// Returns the number of passages embedded. Errors fail the whole call —
/// embedding is not best-effort here, because a partial fill is exactly
/// the problem this is designed to fix.
pub async fn reembed_kb(
    kb: &SqliteKnowledgeBase,
    embedder: &dyn primer_core::embedder::Embedder,
    force: bool,
    batch_size: usize,
) -> Result<usize> {
    // Today the missing-list is the only public per-passage iteration
    // path on `SqliteKnowledgeBase`. `--force` semantics — re-embed
    // everything regardless — would need a `list_passages_with_text()`
    // accessor; tracked as a follow-up.
    if force {
        tracing::warn!(
            target = "primer-kb-load",
            "reembed --force is not yet a complete operation: it currently \
             re-embeds only passages WITHOUT an embedding row. Re-embedding \
             of already-embedded passages requires a `list_passages_with_text()` \
             accessor on SqliteKnowledgeBase."
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
    let mut total = LoadStats::default();
    for path in &files {
        tracing::info!(
            target = "primer-kb-load",
            locale = locale.pack_id(),
            path = %path.display(),
            "loading seed corpus into empty knowledge base"
        );
        let stats = load_jsonl(kb, path).await?;
        total.inserted += stats.inserted;
        total.skipped_existing += stats.skipped_existing;
        total.sources_seen += stats.sources_seen;
    }
    Ok(Some(total))
}

#[cfg(test)]
mod tests;
