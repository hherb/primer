//! Seed-corpus discovery: which `*.<pack>.jsonl` files a locale loads,
//! searched in `$PRIMER_SEED_DIR` → `$XDG_DATA_HOME/primer/seed/` → the
//! cargo dev path.

use primer_core::i18n::Locale;
use std::path::PathBuf;

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
