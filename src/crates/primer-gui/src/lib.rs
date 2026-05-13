//! The Primer GUI library — Tauri commands, app state, settings.
//!
//! The binary at `src/main.rs` is a thin shim around [`run`]; everything
//! interesting (commands, state, persistence) lives here so it can be
//! unit-tested without the Tauri WebView in the loop.

pub mod commands;
pub mod config;
pub mod voice;
pub mod paths;
pub mod state;
pub mod types;
pub mod validation;
pub mod wiring;

use std::path::PathBuf;

/// Resolve `$HOME` and return a default path if it isn't set, mirroring
/// the CLI's tolerance for missing-HOME (the binary itself fails later
/// only when an unset HOME would force an empty session-DB path).
pub fn resolve_home() -> PathBuf {
    std::env::var("HOME").map(PathBuf::from).unwrap_or_default()
}

/// Entry point used by `main.rs`. Initialises tracing, builds the
/// `AppState` (loading the persisted `GuiConfig` from `~/.primer/`),
/// registers every Tauri command, and starts the WebView event loop.
///
/// Returns once the user closes the window. A startup-time tracing
/// init failure is non-fatal — we log a stderr line and continue with
/// the unconfigured default subscriber, the same posture the CLI takes.
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    init_tracing();
    paths::set_packaged_seed_dir_if_present();

    let home = resolve_home();
    let config = config::load(&home).unwrap_or_else(|e| {
        // A malformed on-disk config shouldn't keep the GUI from
        // booting — the user needs an interface to fix it. Log and
        // start with defaults; `update_settings` will overwrite on
        // first save.
        tracing::error!("loading gui-config.json failed: {e}; using defaults");
        config::GuiConfig::default()
    });
    let state = state::AppState::new(home, config);

    let builder = tauri::Builder::default().manage(state);
    let builder = commands::register(builder);
    builder
        .run(tauri::generate_context!())
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    if let Err(e) = tracing_subscriber::fmt().with_env_filter(filter).try_init() {
        eprintln!("tracing init failed: {e}");
    }
}
