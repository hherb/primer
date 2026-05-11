//! `GuiConfig` — the GUI's persisted settings.
//!
//! Mirrors the [`primer-cli`](../../primer-cli) flag set 1-for-1 so a
//! parent who has used the CLI can switch to the GUI without learning
//! a new vocabulary. Persisted to `~/.primer/gui-config.json` (atomic
//! temp-file write + rename, mode 0600 since the file may carry an
//! inline API key).
//!
//! Defaults are derived from the CLI's clap defaults so a brand-new
//! install behaves identically to `primer` with no flags.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Filename inside `~/.primer/` where the GUI config is persisted.
pub const CONFIG_FILENAME: &str = "gui-config.json";

/// Top-level container for every GUI setting.
///
/// Each sub-struct groups one CLI subsystem so the settings modal can
/// render them as collapsible sections without bookkeeping.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct GuiConfig {
    pub learner: LearnerConfig,
    pub backend: BackendConfig,
    pub classifier: SubsystemConfig,
    pub extractor: SubsystemConfig,
    pub comprehension: SubsystemConfig,
    pub embedder: EmbedderConfig,
    pub vocab: VocabConfig,
    pub breaks: BreakConfig,
    pub persistence: PersistenceConfig,
    pub ui: UiConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LearnerConfig {
    pub name: String,
    pub age: u8,
    /// Locale pack id (BCP-47 short — "en", "de", ...).
    pub locale: String,
}

impl Default for LearnerConfig {
    fn default() -> Self {
        Self {
            name: "Explorer".to_string(),
            age: 8,
            locale: "en".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BackendConfig {
    /// "stub" | "cloud" | "ollama"
    pub kind: String,
    /// Model id. None means "use the CLI's per-kind default".
    pub model: Option<String>,
    pub ollama_url: String,
    /// Where to read the API key from when `kind == "cloud"`.
    pub api_key_source: ApiKeySource,
}

impl Default for BackendConfig {
    fn default() -> Self {
        Self {
            kind: "stub".to_string(),
            model: None,
            ollama_url: "http://localhost:11434".to_string(),
            api_key_source: ApiKeySource::default(),
        }
    }
}

/// How the cloud backend obtains its API key.
///
/// Default is `Env` — read `ANTHROPIC_API_KEY` from the process
/// environment at session-start time. `Inline` keeps the key in the
/// config JSON (file mode 0600). The two-variant shape mirrors the
/// CLI's "`--api-key` OR env" behaviour.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ApiKeySource {
    Env,
    Inline { key: String },
}

impl Default for ApiKeySource {
    fn default() -> Self {
        Self::Env
    }
}

/// Settings for the classifier / extractor / comprehension subsystems.
///
/// `match_main = true` collapses all override fields — the subsystem
/// uses the main backend and main model. `match_main = false` requires
/// the kind/model/timeout fields to be respected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SubsystemConfig {
    pub match_main: bool,
    /// "stub" | "cloud" | "ollama"
    pub kind: Option<String>,
    pub model: Option<String>,
    pub timeout_ms: u64,
}

impl SubsystemConfig {
    /// Default for the classifier — 3000 ms timeout, matching CLI.
    pub fn default_classifier() -> Self {
        Self {
            match_main: true,
            kind: None,
            model: None,
            timeout_ms: primer_classifier::consts::DEFAULT_BLOCKING_TIMEOUT_MS,
        }
    }

    /// Default for the extractor — 5000 ms timeout, matching CLI.
    pub fn default_extractor() -> Self {
        Self {
            match_main: true,
            kind: None,
            model: None,
            timeout_ms: primer_extractor::consts::DEFAULT_BLOCKING_TIMEOUT_MS,
        }
    }

    /// Default for the comprehension classifier — 5000 ms timeout, matching CLI.
    pub fn default_comprehension() -> Self {
        Self {
            match_main: true,
            kind: None,
            model: None,
            timeout_ms: primer_comprehension::consts::DEFAULT_BLOCKING_TIMEOUT_MS,
        }
    }
}

impl Default for SubsystemConfig {
    fn default() -> Self {
        Self::default_classifier()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbedderConfig {
    /// "none" | "stub" | "fastembed" | "ollama"
    pub kind: String,
    pub model: Option<String>,
    pub ollama_url: Option<String>,
}

impl Default for EmbedderConfig {
    fn default() -> Self {
        Self {
            kind: "none".to_string(),
            model: None,
            ollama_url: None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct VocabConfig {
    /// Top-K most-overdue concepts to inject into the system prompt as
    /// passive review hints. `None` keeps the CLI default.
    pub max_per_prompt: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BreakConfig {
    /// Minutes between break-suggestion nudges. Must be >= 1.
    pub after_mins: u32,
}

impl Default for BreakConfig {
    fn default() -> Self {
        Self {
            after_mins: primer_core::consts::break_suggest::DEFAULT_INTERVAL_MINUTES,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PersistenceConfig {
    /// Explicit session DB path. `None` → default to
    /// `~/.primer/<slug(name)>.db` at session-start time.
    pub session_db: Option<PathBuf>,
    /// Knowledge DB path. `None` → `:memory:`.
    pub knowledge_db: Option<PathBuf>,
    /// When true, neither DB is written to disk and `session_db` /
    /// `knowledge_db` are ignored.
    pub no_persist: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    /// Right sidebar default-open state. Step 5+ remembers this across launches.
    pub sidebar_open: bool,
    /// Last-active sidebar section: "current_turn" | "learner" | "session".
    /// Free-text on disk so adding a section in a later step doesn't break older
    /// configs.
    pub last_section: String,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            sidebar_open: true,
            last_section: "current_turn".to_string(),
        }
    }
}

/// Errors load/save can produce. Distinguished from a missing file
/// (which is returned as `Ok(Default::default())` so the GUI always
/// has *something* to render).
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("config I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("config JSON decode failed at {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

/// Resolve the absolute path of the GUI config file from a home directory.
pub fn config_path(home: &Path) -> PathBuf {
    home.join(primer_engine::PRIMER_HOME_DIR).join(CONFIG_FILENAME)
}

/// Load the GUI config from disk.
///
/// - Missing file → returns `Ok(GuiConfig::default())` so the GUI can
///   always boot. The caller is responsible for writing the defaults
///   back on first save (no implicit write here — we keep this pure).
/// - Malformed JSON → `Err(ConfigError::Parse)` so the frontend can
///   surface "your config is broken; here's the path" rather than
///   silently clobbering user state.
pub fn load(home: &Path) -> Result<GuiConfig, ConfigError> {
    let path = config_path(home);
    match fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str(&s).map_err(|source| ConfigError::Parse { path, source }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(GuiConfig::default()),
        Err(source) => Err(ConfigError::Io { path, source }),
    }
}

/// Atomically save the GUI config to disk.
///
/// - Creates `~/.primer/` if missing.
/// - Writes to `<file>.tmp` then renames over the destination so a
///   concurrent reader never sees a partial file.
/// - On Unix, sets the destination to mode 0600 because it may carry
///   an inline `ApiKeySource::Inline { key }`. Best-effort on platforms
///   without Unix permissions; the rename still succeeds.
pub fn save(home: &Path, config: &GuiConfig) -> Result<(), ConfigError> {
    let path = config_path(home);
    let parent = path.parent().expect("config_path always has a parent");
    fs::create_dir_all(parent).map_err(|source| ConfigError::Io {
        path: parent.to_path_buf(),
        source,
    })?;

    let json = serde_json::to_string_pretty(config).map_err(|source| ConfigError::Parse {
        path: path.clone(),
        source,
    })?;

    let tmp = path.with_extension("json.tmp");
    {
        let mut f = fs::File::create(&tmp).map_err(|source| ConfigError::Io {
            path: tmp.clone(),
            source,
        })?;
        f.write_all(json.as_bytes()).map_err(|source| ConfigError::Io {
            path: tmp.clone(),
            source,
        })?;
        f.sync_all().map_err(|source| ConfigError::Io {
            path: tmp.clone(),
            source,
        })?;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o600);
        let _ = fs::set_permissions(&tmp, perms);
    }

    fs::rename(&tmp, &path).map_err(|source| ConfigError::Io {
        path: path.clone(),
        source,
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn load_missing_returns_defaults() {
        let dir = TempDir::new().unwrap();
        let cfg = load(dir.path()).unwrap();
        assert_eq!(cfg, GuiConfig::default());
        // Missing file does NOT create one — pure read.
        assert!(!config_path(dir.path()).exists());
    }

    #[test]
    fn load_malformed_surfaces_parse_error() {
        let dir = TempDir::new().unwrap();
        let path = config_path(dir.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"{ this is not json").unwrap();

        let err = load(dir.path()).unwrap_err();
        match err {
            ConfigError::Parse { path: p, .. } => {
                assert_eq!(p, path, "error must name the offending path");
            }
            other => panic!("expected Parse error, got {other:?}"),
        }
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = TempDir::new().unwrap();
        let mut cfg = GuiConfig::default();
        cfg.learner.name = "Binti".to_string();
        cfg.learner.age = 9;
        cfg.learner.locale = "de".to_string();
        cfg.backend.kind = "cloud".to_string();
        cfg.backend.model = Some("claude-sonnet-4-6".to_string());
        cfg.backend.api_key_source = ApiKeySource::Inline {
            key: "test-key-not-real".to_string(),
        };
        cfg.embedder.kind = "fastembed".to_string();
        cfg.vocab.max_per_prompt = Some(6);
        cfg.breaks.after_mins = 45;
        cfg.persistence.no_persist = true;

        save(dir.path(), &cfg).unwrap();
        let round_trip = load(dir.path()).unwrap();
        assert_eq!(round_trip, cfg);
    }

    #[test]
    fn save_creates_primer_subdirectory_if_missing() {
        let dir = TempDir::new().unwrap();
        let primer_dir = dir.path().join(primer_engine::PRIMER_HOME_DIR);
        assert!(!primer_dir.exists());

        save(dir.path(), &GuiConfig::default()).unwrap();
        assert!(primer_dir.is_dir());
        assert!(config_path(dir.path()).exists());
    }

    #[test]
    fn save_is_atomic_no_temp_left_on_success() {
        let dir = TempDir::new().unwrap();
        save(dir.path(), &GuiConfig::default()).unwrap();
        let tmp = config_path(dir.path()).with_extension("json.tmp");
        assert!(!tmp.exists(), "temp file must be renamed away on success");
    }

    #[cfg(unix)]
    #[test]
    fn save_sets_mode_0600_on_unix() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        save(dir.path(), &GuiConfig::default()).unwrap();
        let metadata = fs::metadata(config_path(dir.path())).unwrap();
        let mode = metadata.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "config file must be user-read-write only");
    }

    #[test]
    fn forward_compatibility_unknown_field_is_ignored() {
        // Adding a future field shouldn't poison existing configs; serde
        // skips unknown fields by default. This test pins that contract.
        let dir = TempDir::new().unwrap();
        let path = config_path(dir.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let json = r#"{
            "learner": {"name": "Binti", "age": 9, "locale": "de"},
            "future_field_we_dont_know_about": {"x": 1}
        }"#;
        fs::write(&path, json).unwrap();

        let cfg = load(dir.path()).unwrap();
        assert_eq!(cfg.learner.name, "Binti");
        assert_eq!(cfg.learner.age, 9);
        assert_eq!(cfg.learner.locale, "de");
    }

    #[test]
    fn partial_json_fills_unspecified_fields_with_defaults() {
        // serde(default) on every field/section means an older config
        // missing newer sections still loads cleanly.
        let dir = TempDir::new().unwrap();
        let path = config_path(dir.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let json = r#"{"learner": {"name": "Ada", "age": 7, "locale": "en"}}"#;
        fs::write(&path, json).unwrap();

        let cfg = load(dir.path()).unwrap();
        assert_eq!(cfg.learner.name, "Ada");
        // All other sections come from defaults.
        assert_eq!(cfg.backend, BackendConfig::default());
        assert_eq!(cfg.embedder, EmbedderConfig::default());
        assert_eq!(cfg.ui, UiConfig::default());
    }

    #[test]
    fn subsystem_defaults_match_consts() {
        let cls = SubsystemConfig::default_classifier();
        assert_eq!(
            cls.timeout_ms,
            primer_classifier::consts::DEFAULT_BLOCKING_TIMEOUT_MS
        );
        assert!(cls.match_main);
        let ext = SubsystemConfig::default_extractor();
        assert_eq!(
            ext.timeout_ms,
            primer_extractor::consts::DEFAULT_BLOCKING_TIMEOUT_MS
        );
        let cmp = SubsystemConfig::default_comprehension();
        assert_eq!(
            cmp.timeout_ms,
            primer_comprehension::consts::DEFAULT_BLOCKING_TIMEOUT_MS
        );
    }
}
