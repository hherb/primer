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
            resolve_knowledge_db(
                Some(p.clone()),
                Some(home),
                Some("bge-m3"),
                None,
                no_persist
            ),
            KnowledgeDb::Explicit(p.clone())
        );
    }
}

#[test]
fn resolve_knowledge_db_caches_per_model_under_home() {
    let db = resolve_knowledge_db(
        None,
        Some(Path::new("/h")),
        Some("BAAI/bge-m3"),
        None,
        false,
    );
    assert_eq!(
        db,
        KnowledgeDb::Cache(PathBuf::from("/h/.primer/kb/baai-bge-m3.db"))
    );
    assert_eq!(db.path(), PathBuf::from("/h/.primer/kb/baai-bge-m3.db"));
}

/// Issue #401: one cache per (model, seed dir), so two corpora on one
/// machine (an installed app's bundle and a dev checkout) stop pruning
/// and re-embedding each other's passages. The literal pins the hash:
/// changing it orphans every existing cache (a full re-embed).
#[test]
fn resolve_knowledge_db_keys_cache_by_seed_dir() {
    let home = Path::new("/h");
    let at = |dir: &str| {
        resolve_knowledge_db(
            None,
            Some(home),
            Some("bge-m3"),
            Some(Path::new(dir)),
            false,
        )
    };
    assert_eq!(
        at("/s"),
        KnowledgeDb::Cache(PathBuf::from("/h/.primer/kb/bge-m3-07d65507b49cba97.db"))
    );
    assert_eq!(at("/s"), at("/s"));
    assert_ne!(at("/s"), at("/t"));
}

#[test]
fn seed_corpus_dir_is_the_canonical_parent_of_the_seed_files() {
    assert_eq!(seed_corpus_dir(&[]), None);
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join(".").join("a.en.jsonl");
    assert_eq!(
        seed_corpus_dir(&[file]),
        Some(std::fs::canonicalize(tmp.path()).unwrap())
    );
    // A dir that can't be canonicalized (gone) keys by its path as given.
    let gone = PathBuf::from("/no/such/dir/a.en.jsonl");
    assert_eq!(
        seed_corpus_dir(&[gone]),
        Some(PathBuf::from("/no/such/dir"))
    );
}

#[test]
fn resolve_knowledge_db_stays_in_memory_without_embedder_home_or_persistence() {
    let home = Path::new("/h");
    let seed = Path::new("/s");
    for db in [
        resolve_knowledge_db(None, Some(home), None, Some(seed), false),
        resolve_knowledge_db(None, None, Some("bge-m3"), Some(seed), false),
        resolve_knowledge_db(None, Some(Path::new("")), Some("bge-m3"), Some(seed), false),
        resolve_knowledge_db(None, Some(home), Some("bge-m3"), Some(seed), true),
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
