//! Filesystem path resolution shared between binaries.
//!
//! - `slug` — Unicode-aware NFC-normalised slug for a learner name
//! - `resolve_session_db_path` — explicit / default / in-memory dispatch
//! - `should_show_first_run_banner` — first-launch banner gate

use std::path::{Path, PathBuf};

use unicode_normalization::UnicodeNormalization;

/// SQLite path token for an in-memory database — used as the default
/// when no path is given and `--no-persist` is set. Sessions persist
/// to a per-learner file under `~/.primer/` instead.
pub const IN_MEMORY: &str = ":memory:";

/// Subdirectory under `$HOME` for per-learner session databases.
pub const PRIMER_HOME_DIR: &str = ".primer";

/// Slugify a learner name into a filesystem-safe filename stem.
///
/// The input is first NFC-normalized so two visually identical names
/// (e.g. precomposed `é` vs decomposed `e` + combining acute) map to
/// the same slug. Characters that Unicode classifies as alphanumeric
/// — Latin, Cyrillic, CJK, etc. — are kept (Latin is lowercased; CJK
/// has no case so it round-trips). Every other character becomes `-`;
/// runs of `-` collapse; leading/trailing `-` are stripped. An empty
/// result falls back to `default` so we always produce a valid filename.
pub fn slug(name: &str) -> String {
    let normalized: String = name.nfc().collect();
    let lowered = normalized.to_lowercase();
    let mut out = String::with_capacity(lowered.len());
    let mut last_was_sep = true; // suppress leading sep
    for c in lowered.chars() {
        if c.is_alphanumeric() {
            out.push(c);
            last_was_sep = false;
        } else if !last_was_sep {
            out.push('-');
            last_was_sep = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "default".to_string()
    } else {
        out
    }
}

/// Resolve the path to use for the session database.
/// `:memory:` when `no_persist` is set; otherwise the explicit path if
/// given, falling back to `<home>/.primer/<slug(name)>.db`. The home
/// directory is taken as a parameter so callers can supply it from any
/// source (env var in production, synthetic value in tests) without
/// this function touching the process environment.
pub fn resolve_session_db_path(
    explicit: Option<PathBuf>,
    home: &Path,
    learner_name: &str,
    no_persist: bool,
) -> PathBuf {
    if no_persist {
        return PathBuf::from(IN_MEMORY);
    }
    explicit.unwrap_or_else(|| {
        home.join(PRIMER_HOME_DIR)
            .join(format!("{}.db", slug(learner_name)))
    })
}

/// Should we print the "we just started persisting your sessions"
/// banner? True only when the session DB is at the default path AND
/// the file did not exist before this run AND the user did not opt
/// out via `--no-persist`. The banner answers the legitimate "where
/// did my conversation go?" question that the silent default-path
/// change would otherwise raise.
pub fn should_show_first_run_banner(
    explicit_session_db: bool,
    no_persist: bool,
    file_existed_before: bool,
) -> bool {
    !explicit_session_db && !no_persist && !file_existed_before
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_lowercases_and_keeps_alphanumerics() {
        assert_eq!(slug("Explorer"), "explorer");
        assert_eq!(slug("Binti7"), "binti7");
    }

    #[test]
    fn slug_replaces_special_chars_with_dash() {
        assert_eq!(slug("Anna Maria"), "anna-maria");
        assert_eq!(slug("Lee/Davis"), "lee-davis");
    }

    #[test]
    fn slug_keeps_unicode_letters_lowercased() {
        // The previous ASCII-only rule collapsed `José`, `Łukasz`, `Соня`
        // and `美咲` into ambiguous or empty stems. Children's names are
        // a load-bearing input here — accept anything Unicode considers
        // alphanumeric, lowercased where that exists.
        assert_eq!(slug("José"), "josé");
        assert_eq!(slug("Łukasz"), "łukasz");
        assert_eq!(slug("Соня"), "соня");
        // No case-folding for CJK; the chars round-trip as-is.
        assert_eq!(slug("美咲"), "美咲");
    }

    #[test]
    fn slug_normalizes_nfc_so_decomposed_equals_precomposed() {
        // Same visible name, two Unicode encodings: precomposed `é`
        // (U+00E9) vs decomposed `e` + combining acute (U+0301). Without
        // NFC normalization these slug to different filenames, so two
        // copies of the same child get two session DBs.
        let nfc = "Jos\u{00E9}"; // José (NFC)
        let nfd = "Jose\u{0301}"; // José (NFD)
        assert_eq!(slug(nfc), slug(nfd));
    }

    #[test]
    fn slug_collapses_runs_of_separators() {
        assert_eq!(slug("a   b"), "a-b");
        assert_eq!(slug("a---b"), "a-b");
    }

    #[test]
    fn slug_strips_leading_and_trailing_separators() {
        assert_eq!(slug("  hello  "), "hello");
        assert_eq!(slug("___world___"), "world");
    }

    #[test]
    fn slug_empty_input_falls_back_to_default() {
        assert_eq!(slug(""), "default");
        assert_eq!(slug("!!!"), "default");
    }

    #[test]
    fn resolve_session_db_path_passes_explicit_through() {
        // The home arg is unused when an explicit path is given.
        let home = Path::new("/this/should/be/ignored");
        let p = resolve_session_db_path(
            Some(PathBuf::from("/tmp/explicit.db")),
            home,
            "Anyone",
            false,
        );
        assert_eq!(p, PathBuf::from("/tmp/explicit.db"));
    }

    #[test]
    fn resolve_session_db_path_default_uses_home_and_slug() {
        let home = Path::new("/synthetic/home");
        let p = resolve_session_db_path(None, home, "Binti", false);
        assert_eq!(p, PathBuf::from("/synthetic/home/.primer/binti.db"));
    }

    #[test]
    fn resolve_session_db_path_no_persist_returns_in_memory() {
        // `--no-persist` short-circuits everything: no slug, no home
        // join, no explicit path. The session is throwaway.
        let home = Path::new("/some/home");
        assert_eq!(
            resolve_session_db_path(None, home, "Anyone", true),
            PathBuf::from(IN_MEMORY)
        );
    }

    #[test]
    fn resolve_session_db_path_default_handles_unicode_name() {
        // Confirms the slug + path composition round-trip a non-ASCII
        // name without env mutation. The same name in NFC vs NFD must
        // produce the same path so we don't end up with two DB files.
        let home = Path::new("/h");
        assert_eq!(
            resolve_session_db_path(None, home, "José", false),
            PathBuf::from("/h/.primer/josé.db")
        );
        let nfd = "Jose\u{0301}";
        assert_eq!(
            resolve_session_db_path(None, home, nfd, false),
            PathBuf::from("/h/.primer/josé.db")
        );
    }

    #[test]
    fn first_run_banner_shows_only_for_default_path_first_run() {
        // Default path + brand-new file → show banner (the user just
        // started persisting without explicitly opting in).
        assert!(should_show_first_run_banner(false, false, false));
        // Default path but file already existed → silent (not first run).
        assert!(!should_show_first_run_banner(false, false, true));
        // Explicit path → silent (the user knows where their data is).
        assert!(!should_show_first_run_banner(true, false, false));
        // No-persist → silent (no file is being created at all).
        assert!(!should_show_first_run_banner(false, true, false));
    }
}
