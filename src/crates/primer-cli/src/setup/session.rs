//! Startup step 2: resolve the locale, open the per-learner session
//! store, and load (or mint) the learner. Moved verbatim out of
//! `async_main`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use primer_core::i18n::Locale;
use primer_core::learner::LearnerModel;
use primer_core::storage::{LearnerStore, SessionStore};
use primer_engine::{
    adopt_or_mint_learner_id, create_learner_with_id, ensure_session_db_dir, parse_languages,
    reconcile_persisted_learner, resolve_session_db_path, should_show_first_run_banner,
    verify_resume_locale_match,
};
use primer_storage::SqliteSessionStore;

use crate::Cli;

/// What [`open_session`] hands back to `async_main`.
pub(crate) struct SessionSetup {
    pub(crate) cli_locale: Locale,
    /// `$HOME` (empty when unset and not needed).
    pub(crate) home: PathBuf,
    pub(crate) session_path: PathBuf,
    pub(crate) session_store: Arc<SqliteSessionStore>,
    pub(crate) learner: LearnerModel,
}

/// Open the session store and learner. Exits the process with a message
/// on a user error (unknown locale, missing `--resume` file, locale
/// mismatch), as before the split; a corrupt store is an `Err`.
pub(crate) async fn open_session(cli: &Cli) -> anyhow::Result<SessionSetup> {
    // Resolve the requested locale. An unknown pack id (e.g. a typo or
    // a build that doesn't include the requested language yet) is a
    // hard error at startup rather than a silent fall-back to English.
    let cli_locale: Locale = match Locale::from_pack_id(&cli.language) {
        Some(l) => l,
        None => {
            let known: Vec<&str> = Locale::ALL.iter().map(|l| l.pack_id()).collect();
            eprintln!(
                "Error: --language {:?} is not supported. Known locales: {:?}",
                cli.language, known
            );
            std::process::exit(1);
        }
    };

    // Session store — defaults to a per-learner file under `~/.primer/`.
    // We look up HOME here (rather than inside `resolve_session_db_path`)
    // so the function stays a pure path computation that's trivial to test.
    let explicit_session_db = cli.session_db.is_some();
    let home = match std::env::var("HOME") {
        Ok(h) => PathBuf::from(h),
        Err(_) => {
            if cli.session_db.is_none() && !cli.no_persist {
                eprintln!(
                    "Error: cannot find HOME env var to default --session-db; pass it explicitly"
                );
                std::process::exit(1);
            }
            // HOME is unused when an explicit path is given or --no-persist is set.
            PathBuf::new()
        }
    };
    let session_path =
        resolve_session_db_path(cli.session_db.clone(), &home, &cli.name, cli.no_persist);

    // Resume requires the file to already exist; do not auto-create on
    // a typo'd path. Catch this BEFORE we open (which would create it).
    // (--resume is mutually exclusive with --no-persist via clap, so the
    // path here is always a real on-disk path.)
    if cli.resume.is_some() && !Path::new(&session_path).exists() {
        eprintln!(
            "Error: --resume requires an existing --session-db; {} does not exist.",
            session_path.display()
        );
        std::process::exit(1);
    }

    // Capture file-existed state BEFORE we open (open creates it). Used
    // for the first-run banner. The :memory: token is never a real path,
    // so its existence check is naturally false — that's fine: the banner
    // is also gated on !no_persist below.
    let file_existed_before = !cli.no_persist && Path::new(&session_path).exists();

    // Ensure the parent directory exists (owner-only) before opening. Not
    // needed for :memory:.
    if !cli.no_persist {
        if let Err(e) = ensure_session_db_dir(&session_path, &home) {
            eprintln!(
                "Error: cannot create session-db directory for {}: {e}",
                session_path.display()
            );
            std::process::exit(1);
        }
    }

    let session_store: Arc<primer_storage::SqliteSessionStore> = Arc::new(
        match primer_storage::SqliteSessionStore::open_for_locale(&session_path, cli_locale) {
            Ok(s) => s,
            Err(e) => {
                eprintln!(
                    "Error: cannot open session-db {}: {e}",
                    session_path.display()
                );
                std::process::exit(1);
            }
        },
    );
    if cli.no_persist {
        eprintln!("Session DB: in-memory (no persistence; session ends when you exit).");
    } else {
        eprintln!("Session DB: {}", session_path.display());
        if should_show_first_run_banner(explicit_session_db, cli.no_persist, file_existed_before) {
            eprintln!(
                "Note: this is your first session for {name}. Conversations are now persisted\n      \
                 locally to {path}. Use `--no-persist` to opt out, or `--session-db` to relocate.",
                name = cli.name,
                path = session_path.display(),
            );
        }
    }

    // Learner model — load from persistent store or mint fresh on first run.
    let learner = match session_store.load_learner().await {
        Ok(Some(existing)) => {
            let reconciled = reconcile_persisted_learner(existing, &cli.name, cli.age);
            if let Err(e) = session_store.save_learner(&reconciled).await {
                tracing::warn!("save_learner on startup failed: {e}");
            }
            reconciled
        }
        Ok(None) => {
            // First run on this file. Two sub-cases:
            //   1. Truly fresh DB → mint a new UUID.
            //   2. v3 DB with sessions but no learners row → adopt the
            //      most-recent session's learner_id so existing sessions
            //      are not orphaned.
            let id = adopt_or_mint_learner_id(session_store.most_recent_session_learner_id().await)
                .map_err(|e| anyhow::anyhow!("reading existing sessions' learner_id: {e}"))?;
            let languages = parse_languages(cli.languages.as_deref(), cli_locale);
            let fresh = create_learner_with_id(id, &cli.name, cli.age, cli_locale, languages);
            if let Err(e) = session_store.save_learner(&fresh).await {
                tracing::warn!("save_learner on startup failed: {e}");
            }
            fresh
        }
        Err(e) => {
            // load_learner failing on startup is catastrophic — the file
            // is unreadable or the schema is corrupt. Propagate.
            return Err(anyhow::anyhow!("load_learner failed on startup: {e}"));
        }
    };

    // Locale guard for --resume: error before any new turn could insert a
    // mistagged concept. Done HERE rather than at session-store open time
    // so the check sees the actual learner row that --resume will hydrate.
    if let Some(resume_id) = cli.resume {
        if let Err(msg) = verify_resume_locale_match(cli_locale, learner.profile.locale, resume_id)
        {
            eprintln!("Error: {msg}");
            std::process::exit(1);
        }
    }

    Ok(SessionSetup {
        cli_locale,
        home,
        session_path,
        session_store,
        learner,
    })
}
