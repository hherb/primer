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
//! string reused as the foreign key into the `sources` table.
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

        // Upsert the source the first time we see it in this file.
        sources
            .entry(passage.source.clone())
            .or_insert_with(|| SourceMeta {
                id: passage.source.clone(),
                license: passage.license.clone(),
                attribution: passage.attribution.clone(),
                source_url: passage.source_url.clone(),
                retrieved_at: now,
            });

        if existing_ids.contains(&passage.id) {
            stats.skipped_existing += 1;
            continue;
        }
        kb.insert_passage(&passage.id, &passage.source, &passage.text)?;
        stats.inserted += 1;
    }

    for src in sources.values() {
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
/// 1. `$PRIMER_SEED_DIR/seed_passages.<pack_id>.jsonl` (env override; useful
///    for tests and for users who want to point at a release-downloaded
///    JSONL without copying it into the default location).
/// 2. `$XDG_DATA_HOME/primer/seed/seed_passages.<pack_id>.jsonl`
///    (or `$HOME/.local/share/primer/seed/...` on systems without XDG).
/// 3. Cargo dev path: `<workspace_root>/data/seed/seed_passages.<pack_id>.jsonl`,
///    discovered by walking up from `CARGO_MANIFEST_DIR` if set.
///
/// Returns `None` if no candidate exists.
pub fn discover_seed_jsonl(locale: Locale) -> Option<PathBuf> {
    let pack = locale.pack_id();
    let filename = format!("seed_passages.{pack}.jsonl");

    if let Ok(dir) = std::env::var("PRIMER_SEED_DIR") {
        let p = PathBuf::from(dir).join(&filename);
        if p.is_file() {
            return Some(p);
        }
    }

    if let Some(data_home) = xdg_data_home() {
        let p = data_home.join("primer/seed").join(&filename);
        if p.is_file() {
            return Some(p);
        }
    }

    if let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
        // crates/primer-kb-load/ → walk up to repo root, look in data/seed/.
        let mut p = PathBuf::from(manifest_dir);
        for _ in 0..5 {
            let candidate = p.join("data/seed").join(&filename);
            if candidate.is_file() {
                return Some(candidate);
            }
            if !p.pop() {
                break;
            }
        }
    }

    None
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

/// Auto-seed `kb` from the discovered JSONL if `kb` is empty.
///
/// Returns:
/// - `Ok(Some(stats))` if a seed file was found and loaded.
/// - `Ok(None)` if either the KB already has passages or no seed file
///   could be located. Both are valid runtime states — empty KB is
///   explicitly supported by the rest of the system.
///
/// Errors propagate from the loader; discovery itself never errors.
pub async fn auto_seed_if_empty(
    kb: &SqliteKnowledgeBase,
    locale: Locale,
) -> Result<Option<LoadStats>> {
    if kb.passage_count()? > 0 {
        return Ok(None);
    }
    let Some(path) = discover_seed_jsonl(locale) else {
        tracing::info!(
            target = "primer-kb-load",
            locale = locale.pack_id(),
            "no seed corpus found; knowledge base starts empty"
        );
        return Ok(None);
    };
    tracing::info!(
        target = "primer-kb-load",
        locale = locale.pack_id(),
        path = %path.display(),
        "loading seed corpus into empty knowledge base"
    );
    let stats = load_jsonl(kb, &path).await?;
    Ok(Some(stats))
}

#[cfg(test)]
mod tests {
    use super::*;
    use primer_core::i18n::Locale;
    use primer_core::knowledge::RetrievalParams;
    use std::io::Write;

    fn write_jsonl(lines: &[&str]) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
        f.flush().unwrap();
        f
    }

    #[tokio::test]
    async fn load_two_passages_round_trip() {
        let db = tempfile::NamedTempFile::new().unwrap();
        let kb = SqliteKnowledgeBase::open_for_locale(db.path(), Locale::English).unwrap();
        let jsonl = write_jsonl(&[
            r#"{"id":"p1","source":"seed:en:p1","license":"CC0-1.0","attribution":"x","text":"the sky is blue because of rayleigh scattering"}"#,
            r#"{"id":"p2","source":"seed:en:p2","license":"CC0-1.0","attribution":"x","text":"plants make food via photosynthesis"}"#,
        ]);

        let stats = load_jsonl(&kb, jsonl.path()).await.unwrap();
        assert_eq!(stats.inserted, 2);
        assert_eq!(stats.skipped_existing, 0);
        assert_eq!(stats.sources_seen, 2);

        let got = kb
            .retrieve("photosynthesis", &RetrievalParams::default())
            .await
            .unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, "p2");

        let sources = kb.list_sources().await.unwrap();
        assert_eq!(sources.len(), 2);
    }

    #[tokio::test]
    async fn rerun_is_idempotent() {
        let db = tempfile::NamedTempFile::new().unwrap();
        let kb = SqliteKnowledgeBase::open_for_locale(db.path(), Locale::English).unwrap();
        let jsonl = write_jsonl(&[
            r#"{"id":"p1","source":"seed","license":"CC0-1.0","attribution":"x","text":"hello world"}"#,
        ]);
        let s1 = load_jsonl(&kb, jsonl.path()).await.unwrap();
        let s2 = load_jsonl(&kb, jsonl.path()).await.unwrap();
        assert_eq!(s1.inserted, 1);
        assert_eq!(s2.inserted, 0);
        assert_eq!(s2.skipped_existing, 1);
        assert_eq!(kb.passage_count().unwrap(), 1);
    }

    #[tokio::test]
    async fn blank_lines_and_comments_are_skipped() {
        let db = tempfile::NamedTempFile::new().unwrap();
        let kb = SqliteKnowledgeBase::open_for_locale(db.path(), Locale::English).unwrap();
        let jsonl = write_jsonl(&[
            "# this is a comment, skip me",
            "",
            r#"{"id":"p1","source":"seed","license":"CC0-1.0","attribution":"x","text":"only row"}"#,
            "   ",
        ]);
        let stats = load_jsonl(&kb, jsonl.path()).await.unwrap();
        assert_eq!(stats.inserted, 1);
    }

    #[tokio::test]
    async fn auto_seed_via_explicit_jsonl_path_round_trip() {
        // Direct exercise of the `load_jsonl` half of `auto_seed_if_empty`
        // without touching process env. The discovery path is covered by
        // a dedicated unit test below; this asserts the "load + dedup"
        // contract that production CLI flow depends on.
        let db = tempfile::NamedTempFile::new().unwrap();
        let kb = SqliteKnowledgeBase::open_for_locale(db.path(), Locale::English).unwrap();
        assert_eq!(kb.passage_count().unwrap(), 0);

        let jsonl = write_jsonl(&[
            r#"{"id":"p1","source":"seed","license":"CC0-1.0","attribution":"x","text":"auto-seeded row"}"#,
        ]);
        let stats = load_jsonl(&kb, jsonl.path()).await.unwrap();
        assert_eq!(stats.inserted, 1);
        assert_eq!(kb.passage_count().unwrap(), 1);

        // The auto-seed contract: when the KB is non-empty, return None
        // with no I/O attempt — true regardless of discovery state.
        let result = auto_seed_if_empty(&kb, Locale::English).await.unwrap();
        assert!(
            result.is_none(),
            "auto-seed must skip when KB already has passages"
        );
    }

    #[tokio::test]
    async fn reembed_backfills_passages_missing_embeddings() {
        use primer_embedding::StubEmbedder;
        let db = tempfile::NamedTempFile::new().unwrap();
        let kb = SqliteKnowledgeBase::open_for_locale(db.path(), Locale::English).unwrap();
        // Insert two passages WITHOUT embeddings via the legacy path.
        kb.insert_passage("p1", "src", "first text").unwrap();
        kb.insert_passage("p2", "src", "second text").unwrap();
        assert_eq!(kb.embedding_count().unwrap(), 0);

        let stub = StubEmbedder::with_dim(16);
        let n = reembed_kb(&kb, &stub, false, 8).await.unwrap();
        assert_eq!(n, 2);
        assert_eq!(kb.embedding_count().unwrap(), 2);

        // Idempotent: a second pass touches nothing.
        let n2 = reembed_kb(&kb, &stub, false, 8).await.unwrap();
        assert_eq!(n2, 0);
    }

    #[test]
    fn discover_seed_jsonl_finds_file_under_env_dir() {
        // Process env is tested in isolation here so we don't fight tokio's
        // multi-test scheduling. The real auto-seed flow is exercised in
        // the integration test (data/seed in the repo + a real CLI run).
        let seed_dir = tempfile::tempdir().unwrap();
        let path = seed_dir.path().join("seed_passages.en.jsonl");
        std::fs::write(&path, "{}").unwrap();

        // SAFETY: this test does not run concurrently with other tests
        // that touch PRIMER_SEED_DIR; the `auto_seed` async tests above
        // use explicit paths instead of env vars.
        unsafe {
            std::env::set_var("PRIMER_SEED_DIR", seed_dir.path());
        }
        let found = discover_seed_jsonl(Locale::English);
        unsafe {
            std::env::remove_var("PRIMER_SEED_DIR");
        }
        assert_eq!(found.as_deref(), Some(path.as_path()));
    }

    #[tokio::test]
    async fn malformed_json_reports_line_number() {
        let db = tempfile::NamedTempFile::new().unwrap();
        let kb = SqliteKnowledgeBase::open_for_locale(db.path(), Locale::English).unwrap();
        let jsonl = write_jsonl(&[
            r#"{"id":"p1","source":"seed","license":"CC0-1.0","attribution":"x","text":"ok"}"#,
            r#"{not valid json"#,
        ]);
        let err = load_jsonl(&kb, jsonl.path()).await.unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("line 2"), "expected line number, got: {msg}");
    }
}
