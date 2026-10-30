//! Filesystem path resolution shared between binaries.
//!
//! - `slug` — Unicode-aware NFC-normalised slug for a learner name
//! - `resolve_session_db_path` — explicit / default / in-memory dispatch
//! - `should_show_first_run_banner` — first-launch banner gate
//! - `resolve_knowledge_db` — in-memory / explicit / embedded-cache
//!   dispatch for the knowledge base (issue #388)
//! - `create_private_dir_all` / `ensure_session_db_dir` — owner-only
//!   (`0700` on Unix) directories for learner data (issue #358)

use std::io;
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

/// Subdirectory of `~/.primer/` holding the embedded knowledge-base
/// caches, one SQLite file per embedder model (issue #388).
pub const KB_CACHE_DIR: &str = "kb";

/// Where the knowledge base lives, which also decides how startup
/// prepares it (see `primer_kb_load::prepare_kb`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KnowledgeDb {
    /// `:memory:`, seeded on every open (and embedded on every open when
    /// an embedder is wired).
    InMemory,
    /// A user-chosen file. Seeded only when empty and never embedded at
    /// startup: the user manages it with `primer-kb-load --reembed`.
    Explicit(PathBuf),
    /// The default cache for one embedder model. The seed corpus is topped
    /// up and missing vectors are embedded on every open, so the embedding
    /// cost is paid once, not per startup.
    Cache(PathBuf),
}

impl KnowledgeDb {
    /// The SQLite path to open.
    pub fn path(&self) -> PathBuf {
        match self {
            Self::InMemory => PathBuf::from(IN_MEMORY),
            Self::Explicit(p) | Self::Cache(p) => p.clone(),
        }
    }
}

/// Resolve where the knowledge base lives. An explicit path always wins.
/// Otherwise, with an embedder wired (its `model_id` given), a home
/// directory, and persistence on, the KB is the per-model cache
/// `<home>/.primer/kb/<slug(model_id)>.db`. Keying the file by model
/// keeps one model's vectors from ever meeting another's dim. Without an
/// embedder there are no vectors worth caching, so it stays `:memory:`.
pub fn resolve_knowledge_db(
    explicit: Option<PathBuf>,
    home: Option<&Path>,
    embedder_model_id: Option<&str>,
    no_persist: bool,
) -> KnowledgeDb {
    if let Some(p) = explicit {
        return KnowledgeDb::Explicit(p);
    }
    match (home, embedder_model_id) {
        (Some(home), Some(model)) if !no_persist => KnowledgeDb::Cache(
            home.join(PRIMER_HOME_DIR)
                .join(KB_CACHE_DIR)
                .join(format!("{}.db", slug(model))),
        ),
        _ => KnowledgeDb::InMemory,
    }
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

/// Unix permission bits for directories holding learner data: owner only.
/// Under the personal-device model other OS logins share the machine, and
/// the session DBs inside carry a child's full transcript history.
#[cfg(unix)]
const PRIVATE_DIR_MODE: u32 = 0o700;

/// Create `dir` and any missing parents. On Unix every directory this call
/// creates is owner-only (`0700`); components that already exist keep
/// their permissions, so a user-chosen `--session-db` directory is never
/// altered.
pub fn create_private_dir_all(dir: &Path) -> io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(PRIVATE_DIR_MODE);
    }
    builder.create(dir)
}

/// Prepare the directory a session DB at `session_path` will live in:
/// create its parent privately, and tighten an existing `<home>/.primer/`
/// to `0700`. Installs from before #358 have a `0755` `~/.primer/`, and an
/// owner-only parent protects the `0644` DBs inside without touching each
/// file. A missing `<home>/.primer/` is not created here (an explicit
/// `--session-db` elsewhere does not need it); a failed tighten is logged,
/// not fatal, because it must not stop a child's session. Callers skip this
/// under `no_persist`.
pub fn ensure_session_db_dir(session_path: &Path, home: &Path) -> io::Result<()> {
    if let Some(parent) = session_path.parent() {
        if !parent.as_os_str().is_empty() {
            create_private_dir_all(parent)?;
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let primer_home = home.join(PRIMER_HOME_DIR);
        if primer_home.is_dir() {
            let private = std::fs::Permissions::from_mode(PRIVATE_DIR_MODE);
            if let Err(e) = std::fs::set_permissions(&primer_home, private) {
                tracing::warn!(
                    dir = %primer_home.display(),
                    "could not make the learner-data directory owner-only: {e}"
                );
            }
        }
    }
    #[cfg(not(unix))]
    let _ = home; // Windows profile-dir ACLs already restrict to the owner.
    Ok(())
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
    fn resolve_knowledge_db_explicit_wins_even_under_no_persist() {
        let p = PathBuf::from("/tmp/kb.db");
        let home = Path::new("/h");
        for no_persist in [false, true] {
            assert_eq!(
                resolve_knowledge_db(Some(p.clone()), Some(home), Some("bge-m3"), no_persist),
                KnowledgeDb::Explicit(p.clone())
            );
        }
    }

    #[test]
    fn resolve_knowledge_db_caches_per_model_under_home() {
        let db = resolve_knowledge_db(None, Some(Path::new("/h")), Some("BAAI/bge-m3"), false);
        assert_eq!(
            db,
            KnowledgeDb::Cache(PathBuf::from("/h/.primer/kb/baai-bge-m3.db"))
        );
        assert_eq!(db.path(), PathBuf::from("/h/.primer/kb/baai-bge-m3.db"));
    }

    #[test]
    fn resolve_knowledge_db_stays_in_memory_without_embedder_home_or_persistence() {
        let home = Path::new("/h");
        for db in [
            resolve_knowledge_db(None, Some(home), None, false),
            resolve_knowledge_db(None, None, Some("bge-m3"), false),
            resolve_knowledge_db(None, Some(home), Some("bge-m3"), true),
        ] {
            assert_eq!(db, KnowledgeDb::InMemory);
            assert_eq!(db.path(), PathBuf::from(IN_MEMORY));
        }
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

    #[cfg(unix)]
    fn mode_of(p: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    #[cfg(unix)]
    #[test]
    fn create_private_dir_all_makes_every_new_component_owner_only() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(PRIMER_HOME_DIR).join("logs");
        create_private_dir_all(&dir).unwrap();
        assert_eq!(mode_of(&tmp.path().join(PRIMER_HOME_DIR)), 0o700);
        assert_eq!(mode_of(&dir), 0o700);
    }

    #[cfg(unix)]
    #[test]
    fn create_private_dir_all_leaves_existing_components_alone() {
        use std::os::unix::fs::PermissionsExt;
        // An explicit --session-db under a user-chosen directory must not
        // have that directory's permissions changed.
        let tmp = tempfile::tempdir().unwrap();
        let existing = tmp.path().join("Documents");
        std::fs::create_dir(&existing).unwrap();
        std::fs::set_permissions(&existing, std::fs::Permissions::from_mode(0o755)).unwrap();
        create_private_dir_all(&existing.join("primer")).unwrap();
        assert_eq!(mode_of(&existing), 0o755);
        assert_eq!(mode_of(&existing.join("primer")), 0o700);
    }

    #[cfg(unix)]
    #[test]
    fn ensure_session_db_dir_tightens_a_pre_358_primer_home() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let primer_home = home.path().join(PRIMER_HOME_DIR);
        std::fs::create_dir(&primer_home).unwrap();
        std::fs::set_permissions(&primer_home, std::fs::Permissions::from_mode(0o755)).unwrap();
        let db = resolve_session_db_path(None, home.path(), "José", false);
        ensure_session_db_dir(&db, home.path()).unwrap();
        assert_eq!(mode_of(&primer_home), 0o700);
    }

    #[cfg(unix)]
    #[test]
    fn ensure_session_db_dir_creates_default_dir_owner_only() {
        let home = tempfile::tempdir().unwrap();
        let db = resolve_session_db_path(None, home.path(), "José", false);
        ensure_session_db_dir(&db, home.path()).unwrap();
        assert_eq!(mode_of(&home.path().join(PRIMER_HOME_DIR)), 0o700);
    }

    #[test]
    fn ensure_session_db_dir_does_not_create_primer_home_for_an_explicit_path() {
        let home = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let db = elsewhere.path().join("sub").join("x.db");
        ensure_session_db_dir(&db, home.path()).unwrap();
        assert!(elsewhere.path().join("sub").is_dir());
        assert!(!home.path().join(PRIMER_HOME_DIR).exists());
    }

    #[test]
    fn ensure_session_db_dir_accepts_a_bare_filename() {
        // `--session-db x.db` has an empty parent: nothing to create.
        let home = tempfile::tempdir().unwrap();
        ensure_session_db_dir(Path::new("x.db"), home.path()).unwrap();
    }
}
