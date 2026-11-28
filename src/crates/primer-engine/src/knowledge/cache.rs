//! Housekeeping for the knowledge-base cache files in `~/.primer/kb/`
//! (issue #406).
//!
//! Since #401 each corpus folder gets its own cache
//! (`<slug(model)>-<hash>.db`), so a fresh worktree or an upgrade from
//! the pre-#401 `<slug(model)>.db` would re-embed the whole corpus. Two
//! soft steps around [`super::open_prepared_kb`] avoid that:
//!
//! 1. [`seed_from_sibling`]: a cache file that does not exist yet starts
//!    as a snapshot of the newest cache for the same model, so the
//!    corpus top-up re-embeds only the passages that differ.
//! 2. [`remove_legacy_sibling`]: once a hashed cache is ready, the
//!    legacy `<slug(model)>.db` beside it is deleted.
//!
//! Both only log on failure: the caches are rebuildable and hold no
//! child data, so housekeeping must never stop a session. Caches of other
//! corpus folders are kept (one per folder ever used); the folder is safe
//! to delete by hand.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::paths::SEED_DIR_HASH_HEX_LEN;

/// Extension of every cache file.
const CACHE_EXT: &str = ".db";
/// SQLite's rollback-journal suffix, removed together with its database.
const JOURNAL_SUFFIX: &str = "-journal";

/// The model slug a cache file name belongs to: `<slug>.db` or
/// `<slug>-<hex hash>.db`, plus whether the name is hashed. `None` for a
/// file that is not a cache.
fn parse_cache_name(file_name: &str) -> Option<(&str, bool)> {
    let stem = file_name.strip_suffix(CACHE_EXT)?;
    match stem.rsplit_once('-') {
        Some((slug, hash))
            if !slug.is_empty()
                && hash.len() == SEED_DIR_HASH_HEX_LEN
                && hash.bytes().all(|b| b.is_ascii_hexdigit()) =>
        {
            Some((slug, true))
        }
        _ if !stem.is_empty() => Some((stem, false)),
        _ => None,
    }
}

fn file_name(path: &Path) -> Option<&str> {
    path.file_name()?.to_str()
}

/// The legacy `<slug>.db` beside the hashed cache `cache`; `None` when
/// `cache` is itself unhashed (no seed corpus was found), so it never
/// names itself.
pub(super) fn legacy_sibling(cache: &Path) -> Option<PathBuf> {
    match parse_cache_name(file_name(cache)?)? {
        (slug, true) => Some(cache.with_file_name(format!("{slug}{CACHE_EXT}"))),
        (_, false) => None,
    }
}

/// Of `candidates` (path, modified time), the newest cache for the same
/// model as `cache`, other than `cache` itself.
pub(super) fn newest_same_model(
    cache: &Path,
    candidates: &[(PathBuf, SystemTime)],
) -> Option<PathBuf> {
    let (slug, _) = parse_cache_name(file_name(cache)?)?;
    candidates
        .iter()
        .filter(|(p, _)| p != cache)
        .filter(|(p, _)| file_name(p).and_then(parse_cache_name).map(|(s, _)| s) == Some(slug))
        .max_by_key(|(_, mtime)| *mtime)
        .map(|(p, _)| p.clone())
}

/// The files in `dir` with their modified times (unreadable entries are
/// skipped).
fn list_files(dir: &Path) -> std::io::Result<Vec<(PathBuf, SystemTime)>> {
    Ok(std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let meta = e.metadata().ok().filter(|m| m.is_file())?;
            Some((e.path(), meta.modified().ok()?))
        })
        .collect())
}

/// When `cache` does not exist, start it as a snapshot of the newest
/// same-model cache beside it. Returns the source used, `None` when there
/// was nothing to do or the copy failed (logged; the cache then starts
/// empty, as before #406).
///
/// The snapshot goes to a per-process temp file and is hard-linked into
/// place, which fails if `cache` appeared meanwhile, so a concurrent
/// start never has its open cache replaced.
pub(super) fn seed_from_sibling(cache: &Path) -> Option<PathBuf> {
    if cache.exists() {
        return None;
    }
    let dir = cache.parent()?;
    let source = newest_same_model(cache, &list_files(dir).ok()?)?;
    let tmp = cache.with_extension(format!("db.tmp-{}", std::process::id()));
    let _ = std::fs::remove_file(&tmp); // a leftover from a crashed run
    let result = primer_knowledge::snapshot_db(&source, &tmp)
        .map_err(|e| e.to_string())
        .and_then(|()| std::fs::hard_link(&tmp, cache).map_err(|e| e.to_string()));
    let _ = std::fs::remove_file(&tmp);
    match result {
        Ok(()) => {
            tracing::info!(
                target: "primer::startup",
                from = %source.display(),
                to = %cache.display(),
                "new knowledge-base cache starts from an existing one"
            );
            Some(source)
        }
        Err(e) => {
            tracing::warn!(
                target: "primer::startup",
                from = %source.display(),
                to = %cache.display(),
                "could not start the cache from an existing one, building it fresh: {e}"
            );
            None
        }
    }
}

/// Delete the legacy `<slug>.db` (and its journal) beside the hashed
/// cache `cache`, if there is one. Returns the path removed. A failure
/// is logged, never returned.
pub(super) fn remove_legacy_sibling(cache: &Path) -> Option<PathBuf> {
    let legacy = legacy_sibling(cache).filter(|l| l.exists())?;
    if let Err(e) = std::fs::remove_file(&legacy) {
        tracing::warn!(
            target: "primer::startup",
            path = %legacy.display(),
            "could not remove the old knowledge-base cache: {e}"
        );
        return None;
    }
    let mut journal = legacy.clone().into_os_string();
    journal.push(JOURNAL_SUFFIX);
    let _ = std::fs::remove_file(journal);
    tracing::info!(
        target: "primer::startup",
        path = %legacy.display(),
        "removed the old knowledge-base cache"
    );
    Some(legacy)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    const HASH: &str = "0123456789abcdef";

    #[test]
    fn cache_names_parse_into_model_slug_and_hashedness() {
        assert_eq!(
            parse_cache_name(&format!("bge-m3-{HASH}.db")),
            Some(("bge-m3", true))
        );
        assert_eq!(parse_cache_name("bge-m3.db"), Some(("bge-m3", false)));
        // A dash-suffix that is not a full-length hex hash is part of the slug.
        assert_eq!(
            parse_cache_name("bge-m3-abc.db"),
            Some(("bge-m3-abc", false))
        );
        assert_eq!(parse_cache_name("bge-m3.db-journal"), None);
        assert_eq!(parse_cache_name(".db"), None);
        assert_eq!(
            parse_cache_name(&format!("-{HASH}.db")),
            Some((&*format!("-{HASH}"), false))
        );
    }

    #[test]
    fn the_legacy_sibling_of_a_hashed_cache_is_its_unhashed_name() {
        let hashed = PathBuf::from(format!("/kb/bge-m3-{HASH}.db"));
        assert_eq!(
            legacy_sibling(&hashed),
            Some(PathBuf::from("/kb/bge-m3.db"))
        );
        assert_eq!(legacy_sibling(Path::new("/kb/bge-m3.db")), None);
    }

    #[test]
    fn the_newest_same_model_cache_is_picked() {
        let t = |s| SystemTime::UNIX_EPOCH + Duration::from_secs(s);
        let cache = PathBuf::from(format!("/kb/bge-m3-{HASH}.db"));
        let candidates = vec![
            (PathBuf::from("/kb/bge-m3.db"), t(1)),
            (PathBuf::from("/kb/bge-m3-00000000000000aa.db"), t(2)),
            (PathBuf::from("/kb/other-00000000000000bb.db"), t(9)),
            (PathBuf::from("/kb/bge-m3.db-journal"), t(9)),
            (cache.clone(), t(9)),
        ];
        assert_eq!(
            newest_same_model(&cache, &candidates),
            Some(PathBuf::from("/kb/bge-m3-00000000000000aa.db"))
        );
        assert_eq!(newest_same_model(&cache, &candidates[2..]), None);
    }
}
