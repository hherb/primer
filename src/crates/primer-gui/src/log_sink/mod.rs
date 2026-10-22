//! Desktop log-file sink (issue #355).
//!
//! A Dock/Finder launch points the process's stdout and stderr at
//! `/dev/null`, so a stdout-only `tracing` subscriber loses every warning the
//! GUI emits (embedder fallback, subsystem construction, voice loop). The
//! desktop subscriber therefore tees events into `~/.primer/logs/gui.log`
//! as well as stdout, so a terminal launch keeps its output and a Dock launch
//! still leaves something a parent or developer can inspect. The file sits
//! next to the session DBs: local-only, like all learner data.
//!
//! Android routes `tracing` to logcat instead (see `init_tracing` in the
//! crate root), so this module is desktop-only.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tracing::Subscriber;
use tracing_subscriber::EnvFilter;

/// Sub-directory of `~/.primer/` holding the GUI's log files.
const LOGS_DIRNAME: &str = "logs";

/// Filename of the live desktop GUI log.
const GUI_LOG_FILENAME: &str = "gui.log";

/// Extension appended to the live log's name for the single rotated
/// generation (`gui.log` → `gui.log.1`).
const ROTATED_EXTENSION: &str = "1";

/// Size at which the log is rotated at startup. One rotated generation is
/// kept, so disk use stays near twice this.
pub const GUI_LOG_MAX_BYTES: u64 = 5 * 1024 * 1024;

/// `<home>/.primer/logs/gui.log`. Pure — no filesystem access.
pub fn desktop_log_path(home: &Path) -> PathBuf {
    home.join(primer_engine::paths::PRIMER_HOME_DIR)
        .join(LOGS_DIRNAME)
        .join(GUI_LOG_FILENAME)
}

/// The rotated generation's path: `gui.log` → `gui.log.1`.
fn rotated_path(log: &Path) -> PathBuf {
    let mut name = log.as_os_str().to_owned();
    name.push(".");
    name.push(ROTATED_EXTENSION);
    PathBuf::from(name)
}

/// Move `log` aside to `<log>.1` (replacing any previous rotation)
/// when it is at least `max_bytes`. A missing log is not an error.
pub fn rotate_if_oversized(log: &Path, max_bytes: u64) -> io::Result<()> {
    match fs::metadata(log) {
        Ok(meta) if meta.len() >= max_bytes => fs::rename(log, rotated_path(log)),
        Ok(_) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Create the log's parent directory, rotate an oversized log, and open the
/// log for appending.
///
/// shortcut: rotation runs only at startup, so one very long-running session
/// can grow the live log past the cap; switch to a size-checking writer if
/// that ever shows up in practice.
pub fn open_log_file(log: &Path, max_bytes: u64) -> io::Result<File> {
    if let Some(dir) = log.parent() {
        fs::create_dir_all(dir)?;
    }
    rotate_if_oversized(log, max_bytes)?;
    OpenOptions::new().create(true).append(true).open(log)
}

/// The desktop subscriber: `filter` over a stdout layer plus, when `file` is
/// `Some`, an ANSI-free layer writing into it.
pub fn desktop_subscriber(filter: EnvFilter, file: Option<File>) -> impl Subscriber + Send + Sync {
    use tracing_subscriber::prelude::*;
    let file_layer = file.map(|f| {
        tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_writer(Mutex::new(f))
    });
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer())
        .with(file_layer)
}

#[cfg(test)]
mod tests;
