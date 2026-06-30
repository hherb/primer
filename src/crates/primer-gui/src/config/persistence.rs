//! Load/save plumbing for [`GuiConfig`].
//!
//! Pure path resolution ([`config_path`]) plus the atomic-write [`save`]
//! and tolerant [`load`] (missing file → defaults). The on-disk shapes
//! live in [`super::types`].

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use super::GuiConfig;

/// Filename inside `~/.primer/` where the GUI config is persisted.
pub const CONFIG_FILENAME: &str = "gui-config.json";

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
    /// JSON decode failure on `load`.
    #[error("config JSON decode failed at {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    /// JSON encode failure on `save`. Practically never happens for
    /// our `Serialize`-derived types, but keeping it distinct from
    /// `Parse` prevents the misleading "decode failed" message when
    /// the failing direction was an encode.
    #[error("config JSON encode failed for {path}: {source}")]
    Serialize {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

/// Resolve the absolute path of the GUI config file from a home directory.
pub fn config_path(home: &Path) -> PathBuf {
    home.join(primer_engine::PRIMER_HOME_DIR)
        .join(CONFIG_FILENAME)
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

    let json = serde_json::to_string_pretty(config).map_err(|source| ConfigError::Serialize {
        path: path.clone(),
        source,
    })?;

    let tmp = path.with_extension("json.tmp");
    {
        let mut f = fs::File::create(&tmp).map_err(|source| ConfigError::Io {
            path: tmp.clone(),
            source,
        })?;
        f.write_all(json.as_bytes())
            .map_err(|source| ConfigError::Io {
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
