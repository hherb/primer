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
//! The loader treats `id` as the key, and the JSONL as the source of truth
//! for its ids: a row already in `passages_<pack>_content` with the same
//! `id`, source and text is **skipped**; one whose source or text changed
//! is **replaced** and loses its vector, so the next re-embed refills it
//! (#392). Re-running the loader on the same JSONL is a no-op.
//! The `sources` table uses upsert semantics (refresh `retrieved_at` and
//! attribution on every load).

use primer_core::error::{PrimerError, Result};
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
    /// Passages already stored under their id whose source or text changed;
    /// rewritten, with their vector dropped (#392).
    pub replaced: usize,
    /// Stored passages absent from the corpus, deleted with their vector.
    /// Only the cache top-up prunes ([`crate::prepare_kb`], #398).
    pub pruned: usize,
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
/// new, replaced if their source or text changed, skipped otherwise;
/// sources are upserted when any passage is written.
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
    write_seed(kb, parse_jsonl(path)?, false).await
}

/// Load several seed files as one corpus: every file is parsed, then all
/// of them are written in a single transaction. A bad line or a failed
/// write in any file therefore leaves the KB as it was — a half-loaded KB
/// would be non-empty and stop `auto_seed_if_empty` from ever retrying
/// (#308). An `id` that also appears in an earlier file is skipped, like
/// one already in the KB.
pub async fn load_seed_files(kb: &SqliteKnowledgeBase, paths: &[PathBuf]) -> Result<LoadStats> {
    load_seed_corpus(kb, paths, false).await
}

/// [`load_seed_files`]; with `prune`, also delete stored passages whose id
/// the corpus no longer has (#398). Only for a KB that mirrors this one
/// corpus (the per-model cache).
pub(crate) async fn load_seed_corpus(
    kb: &SqliteKnowledgeBase,
    paths: &[PathBuf],
    prune: bool,
) -> Result<LoadStats> {
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
    write_seed(kb, corpus, prune).await
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

/// Write a parsed seed corpus — its sources and every passage that is new
/// to the KB or whose source or text changed (#392) — in one transaction.
/// The first occurrence of an id in the corpus wins. With `prune`, stored
/// passages absent from a non-empty corpus are deleted too (#398). With
/// nothing to write nothing is written, so topping up an already-current KB on every
/// startup costs no write.
async fn write_seed(kb: &SqliteKnowledgeBase, seed: ParsedSeed, prune: bool) -> Result<LoadStats> {
    let ParsedSeed { passages, sources } = seed;
    let stored = kb.stored_passages()?;
    let mut seen = HashSet::new();
    let changed: Vec<(&str, &str, &str)> = passages
        .iter()
        .filter(|p| seen.insert(p.id.as_str()))
        .filter(|p| {
            stored.get(&p.id).map(|(s, t)| (s.as_str(), t.as_str()))
                != Some((p.source.as_str(), p.text.as_str()))
        })
        .map(|p| (p.id.as_str(), p.source.as_str(), p.text.as_str()))
        .collect();
    let corpus_ids: HashSet<&str> = passages.iter().map(|p| p.id.as_str()).collect();
    // An empty corpus (emptied or truncated file) has nothing to mirror;
    // pruning against it would wipe the cache.
    let keep = (prune && !corpus_ids.is_empty()).then_some(&corpus_ids);
    let any_absent = keep.is_some_and(|k| stored.keys().any(|id| !k.contains(id.as_str())));
    let sources: Vec<SourceMeta> = sources.into_values().collect();
    // `insert_seed_new` re-compares inside its write transaction: the read
    // above can be stale when another process seeds the same file.
    let (inserted, replaced, pruned) = if changed.is_empty() && !any_absent {
        (0, 0, 0)
    } else {
        kb.insert_seed_new(&sources, &changed, keep)?
    };
    let stats = LoadStats {
        inserted,
        replaced,
        pruned,
        skipped_existing: passages.len() - inserted - replaced,
        sources_seen: sources.len(),
    };

    tracing::info!(
        target = "primer-kb-load",
        inserted = stats.inserted,
        replaced = stats.replaced,
        pruned = stats.pruned,
        skipped = stats.skipped_existing,
        sources = stats.sources_seen,
        "loaded JSONL into knowledge base"
    );
    Ok(stats)
}

mod discover;
mod prepare;
pub use discover::*;
pub use prepare::*;

#[cfg(test)]
mod tests;
