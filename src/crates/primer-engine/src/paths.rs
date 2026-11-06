//! Filesystem path resolution shared between binaries.
//!
//! - `slug` — Unicode-aware NFC-normalised slug for a learner name
//! - `resolve_session_db_path` — explicit / default / in-memory dispatch
//! - `should_show_first_run_banner` — first-launch banner gate
//! - `resolve_knowledge_db` — in-memory / explicit / embedded-cache
//!   dispatch for the knowledge base (issue #388)
//! - `seed_corpus_dir` — the canonical seed folder that keys the cache
//!   alongside the model (issue #401)
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

/// FNV-1a 64-bit offset basis and prime (the algorithm's constants).
const FNV1A_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV1A_PRIME: u64 = 0x0000_0100_0000_01b3;

/// FNV-1a 64 over `bytes`. Used instead of `std`'s hasher because a cache
/// file name must stay the same across Rust releases.
fn fnv1a_64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(FNV1A_OFFSET, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(FNV1A_PRIME)
    })
}

/// The folder a discovered seed corpus lives in (every file
/// `primer_kb_load::discover_seed_files` returns shares one), canonicalized
/// so two spellings of one folder share a cache; as given if that fails.
/// `None` when no seed file was found.
pub fn seed_corpus_dir(seed_files: &[PathBuf]) -> Option<PathBuf> {
    let dir = seed_files.first()?.parent()?;
    Some(std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf()))
}

/// Resolve where the knowledge base lives. An explicit path always wins.
/// Otherwise, with an embedder wired (its `model_id` given), a home
/// directory, and persistence on, the KB is the cache
/// `<home>/.primer/kb/<slug(model_id)>-<fnv1a-64 hex of seed_dir>.db`
/// (`<slug(model_id)>.db` when no seed corpus was found). Keying by model
/// keeps one model's vectors from ever meeting another's dim; keying by
/// the corpus folder (from [`seed_corpus_dir`]) keeps two corpora on one
/// machine from pruning each other, since the cache mirrors its corpus
/// (#401). An edit inside one folder still updates its cache in place.
/// Without an embedder there are no vectors worth caching, so it stays
/// `:memory:`. An empty `home` (HOME unset) counts as no home, never as
/// the cwd.
pub fn resolve_knowledge_db(
    explicit: Option<PathBuf>,
    home: Option<&Path>,
    embedder_model_id: Option<&str>,
    seed_dir: Option<&Path>,
    no_persist: bool,
) -> KnowledgeDb {
    if let Some(p) = explicit {
        return KnowledgeDb::Explicit(p);
    }
    match (
        home.filter(|h| !h.as_os_str().is_empty()),
        embedder_model_id,
    ) {
        (Some(home), Some(model)) if !no_persist => {
            let name = match seed_dir {
                Some(dir) => format!(
                    "{}-{:016x}.db",
                    slug(model),
                    fnv1a_64(dir.as_os_str().as_encoded_bytes())
                ),
                None => format!("{}.db", slug(model)),
            };
            KnowledgeDb::Cache(home.join(PRIMER_HOME_DIR).join(KB_CACHE_DIR).join(name))
        }
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
mod tests;
