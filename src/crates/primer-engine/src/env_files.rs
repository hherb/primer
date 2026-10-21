//! Env-file loading shared between binaries (issue #337).
//!
//! Both binaries load a project-local `.env` (cwd and ancestors) and then
//! the user-global `~/.primer_env`. Earlier sources win: `dotenvy` never
//! overrides a variable that is already set. The GUI needs this as much as
//! the CLI — a Dock-launched macOS app gets launchd's environment, not the
//! shell profile, so `ANTHROPIC_API_KEY` is otherwise invisible to it.
//!
//! Call [`load_env_files`] before any thread is spawned: `dotenvy` sets
//! variables with `std::env::set_var`, which is unsound under concurrent
//! `getenv` on Unix.

use std::path::Path;

/// File name of the user-global env file, resolved under `$HOME`.
pub const USER_ENV_FILE: &str = ".primer_env";

/// Load `<home>/.primer_env` without overriding already-set variables.
/// Returns whether the file was found and loaded; a missing file is the
/// normal case and is not an error. A malformed file is logged and skipped.
pub fn load_user_env_file(home: &Path) -> bool {
    let path = home.join(USER_ENV_FILE);
    match dotenvy::from_path(&path) {
        Ok(()) => true,
        Err(e) if e.not_found() => false,
        Err(e) => {
            tracing::warn!("ignoring malformed {}: {e}", path.display());
            false
        }
    }
}

/// Load the project-local `.env` (cwd and ancestors), then
/// `$HOME/.primer_env`. Earlier sources win. Missing files are ignored.
pub fn load_env_files() {
    let _ = dotenvy::dotenv();
    if let Some(home) = std::env::var_os("HOME") {
        load_user_env_file(Path::new(&home));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_env_file_sets_unset_variable() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(USER_ENV_FILE),
            "PRIMER_TEST_ENV_FILES_337=from-file\n",
        )
        .unwrap();
        assert!(load_user_env_file(dir.path()));
        assert_eq!(
            std::env::var("PRIMER_TEST_ENV_FILES_337").as_deref(),
            Ok("from-file")
        );
    }

    #[test]
    fn missing_user_env_file_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!load_user_env_file(dir.path()));
    }
}
