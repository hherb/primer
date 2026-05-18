//! The Primer GUI library — Tauri commands, app state, settings.
//!
//! The binary at `src/main.rs` is a thin shim around [`run`]; everything
//! interesting (commands, state, persistence) lives here so it can be
//! unit-tested without the Tauri WebView in the loop.

pub mod commands;
pub mod config;
pub mod csp;
pub mod paths;
pub mod state;
pub mod types;
pub mod validation;
pub mod voice;
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
    // Must run before Tauri spawns any worker threads — `set_var` is
    // unsafe under concurrent libc-getenv on Unix. The Piper TTS in
    // voice mode needs the system `espeak-ng-data` directory; without
    // it, phoneme lookup fails at synthesis time (the `espeak-rs-sys`
    // bundled subset ships without `phontab` and other core files).
    // Mirrors the CLI's `probe_espeak_ng_data` at primer-cli/src/main.rs.
    //
    // Skipped when `macos-native` is active: the macOS-native path uses
    // AVSpeechSynthesizer (not Piper), so espeak-ng is structurally unused
    // and the warning would be unactionable noise for evaluators.
    #[cfg(all(
        feature = "speech",
        not(all(target_os = "macos", feature = "macos-native"))
    ))]
    probe_espeak_ng_data();

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

/// Probe common system locations for an `espeak-ng-data` directory and
/// set `PIPER_ESPEAKNG_DATA_DIRECTORY` to the parent of the first complete
/// one found. `espeak-rs-sys` ships an incomplete subset (missing `phontab`
/// and other core files); without a system install Piper's phonemizer
/// fails. Skipped if the env var is already set externally.
///
/// MUST run before Tauri (and any tokio runtime threads) start.
/// `set_var` is `unsafe` because concurrent `getenv` from any other
/// thread is UB on Unix libc. Called from the synchronous prefix of
/// [`run`] before the Tauri builder kicks off any worker thread.
///
/// Mirrors `primer-cli/src/main.rs::probe_espeak_ng_data` byte-for-byte
/// except for the verbose flag — the GUI logs hits via `tracing::info!`
/// instead of stderr.
///
/// Not compiled when `macos-native` is active — espeak-ng is unused on that
/// path (AVSpeechSynthesizer does its own phonemisation) and the warning
/// would be unactionable noise for evaluators.
#[cfg(all(
    feature = "speech",
    not(all(target_os = "macos", feature = "macos-native"))
))]
fn probe_espeak_ng_data() {
    if std::env::var_os("PIPER_ESPEAKNG_DATA_DIRECTORY").is_some() {
        return;
    }
    const ESPEAK_PARENT_CANDIDATES: &[&str] = &[
        "/opt/homebrew/share", // macOS Apple Silicon (brew install espeak-ng)
        "/usr/local/share",    // macOS Intel / generic
        "/usr/share",          // Linux (apt/dnf install espeak-ng-data)
    ];
    for parent in ESPEAK_PARENT_CANDIDATES {
        let probe = std::path::Path::new(parent).join("espeak-ng-data/phontab");
        if probe.is_file() {
            tracing::info!(
                target: "primer-gui::startup",
                "found espeak-ng-data under {parent}; setting PIPER_ESPEAKNG_DATA_DIRECTORY"
            );
            // SAFETY: we are running before the Tauri builder (and any
            // tokio runtime, worker threads, or third-party library
            // threads) have been started. No other thread can be
            // calling getenv concurrently, so this `set_var` is sound.
            unsafe {
                std::env::set_var("PIPER_ESPEAKNG_DATA_DIRECTORY", parent);
            }
            return;
        }
    }
    tracing::warn!(
        target: "primer-gui::startup",
        "no system espeak-ng-data found under {ESPEAK_PARENT_CANDIDATES:?}; \
         Piper TTS will fail at synthesis time. Install via `brew install espeak-ng` \
         (macOS) or `apt install espeak-ng-data` (Linux)."
    );
}
