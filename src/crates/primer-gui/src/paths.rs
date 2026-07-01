//! Packaged-app path resolution.
//!
//! When `primer-gui` runs from inside a macOS `.app` bundle the seed
//! corpus lives under `Contents/Resources/`. The dialogue engine
//! discovers seed files via the `PRIMER_SEED_DIR` env var first, so we
//! resolve the in-bundle path at startup and set the env var before
//! constructing the engine. Outside a `.app` (e.g. `cargo run` from
//! `src/`) this is a no-op and the existing `CARGO_MANIFEST_DIR`
//! fallback in `primer-kb-load` handles dev builds.

use std::path::{Path, PathBuf};

/// Subdirectory under the app's data directory where the seed corpus is
/// staged on Android. The APK asset namespace (`resource_dir()` →
/// `asset://localhost/`) is not `std::fs`-readable, so the bundled
/// `resources/seed/*.jsonl` cannot be discovered the way the desktop
/// `.app` bundle is; instead a real, app-readable directory is staged
/// here (e.g. via `adb push`) and pointed at by `PRIMER_SEED_DIR`.
const MOBILE_SEED_SUBDIR: &str = "seed";

/// The conventional on-device location of the staged seed corpus, given
/// the app's data directory. Pure helper — no filesystem access.
///
/// On Android the seed `*.jsonl` are staged under
/// `<app_data_dir>/seed/` rather than bundled into the APK assets, since
/// the asset namespace is not directly readable with `std::fs`. See
/// [`set_mobile_seed_dir_if_present`] for the discovery + env wiring.
pub fn mobile_seed_dir(app_data: &Path) -> PathBuf {
    app_data.join(MOBILE_SEED_SUBDIR)
}

/// System directories appended (after the app's own native-lib dir) to
/// `ADSP_LIBRARY_PATH` as fallbacks. The bundled QAIRT skel must win, so
/// these come *after* the app dir; non-existent entries are harmless
/// (FastRPC skips them). `/vendor/lib/rfsa/adsp` is where the device
/// firmware keeps its own Hexagon skels.
const ADSP_SYSTEM_FALLBACK_DIRS: &[&str] = &["/vendor/lib/rfsa/adsp", "/vendor/dsp/cdsp", "/dsp"];

/// Filename under `<app_data>/.primer/` to which the Genie logging
/// callback is routed (read on-device via `run-as cat`).
const GENIE_LOG_FILENAME: &str = "genie.log";

/// Environment variable the inference layer reads to enable Genie file
/// logging (see `primer_inference::qnn::genie`'s `GENIE_LOG_PATH_ENV`).
/// Only set on mobile, so the const is mobile-gated to avoid a desktop
/// dead-code warning.
#[cfg(mobile)]
const GENIE_LOG_PATH_ENV: &str = "PRIMER_GENIE_LOG_PATH";

/// Filename under `<app_data>/.primer/` to which the QNN backend appends its
/// per-turn throughput metrics (JSONL: TTFT + decode tok/s). Read on-device
/// via `run-as cat`, like `genie.log`.
const QNN_METRICS_FILENAME: &str = "qnn_metrics.jsonl";

/// Environment variable the inference layer reads to enable QNN per-turn
/// throughput metrics (see `primer_inference::qnn`'s `QNN_METRICS_PATH_ENV`).
/// Mobile-only, so the const is mobile-gated to avoid a desktop dead-code
/// warning.
#[cfg(mobile)]
const QNN_METRICS_PATH_ENV: &str = "PRIMER_QNN_METRICS_PATH";

/// The on-device path of the Genie diagnostics log: `<app_data>/.primer/
/// genie.log`. Pure helper — no filesystem access. Sits next to the GUI
/// config so a developer reads it with the same `run-as cat .primer/...`
/// idiom used for the config.
pub fn mobile_genie_log_path(app_data: &Path) -> PathBuf {
    app_data
        .join(primer_engine::paths::PRIMER_HOME_DIR)
        .join(GENIE_LOG_FILENAME)
}

/// The on-device path of the QNN per-turn metrics log: `<app_data>/.primer/
/// qnn_metrics.jsonl`. Pure helper — no filesystem access. Sits next to
/// `genie.log` so a developer reads it with the same `run-as cat .primer/...`
/// idiom.
pub fn mobile_qnn_metrics_path(app_data: &Path) -> PathBuf {
    app_data
        .join(primer_engine::paths::PRIMER_HOME_DIR)
        .join(QNN_METRICS_FILENAME)
}

/// Parse `/proc/self/maps` content and return the directory of the first
/// mapping whose file path ends in `lib_basename`. Pure helper — the
/// caller reads `/proc/self/maps`.
///
/// Used on Android to discover the app's `nativeLibraryDir` (where the
/// APK's `lib/arm64-v8a/*.so` are extracted, including the bundled QAIRT
/// Hexagon skel) by anchoring on a library known to be loaded from there
/// (the app's own `libprimer_gui.so`). Each maps line is
/// `addr perms offset dev inode  path`; the path is everything after the
/// 5th whitespace-delimited field, so paths containing spaces survive.
pub fn native_lib_dir_from_maps(maps: &str, lib_basename: &str) -> Option<PathBuf> {
    for line in maps.lines() {
        // Split off the 5 leading numeric/columns; the remainder (trimmed)
        // is the mapped path. `splitn(6, …)` keeps any spaces in the path.
        let mut fields = line.splitn(6, char::is_whitespace);
        let path = match (
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
        ) {
            (Some(_), Some(_), Some(_), Some(_), Some(_), Some(rest)) => rest.trim(),
            _ => continue,
        };
        if path.is_empty() {
            continue;
        }
        let p = Path::new(path);
        if p.file_name().and_then(|n| n.to_str()) == Some(lib_basename) {
            return p.parent().map(Path::to_path_buf);
        }
    }
    None
}

/// Build the `ADSP_LIBRARY_PATH` value: the app's native-lib dir (so the
/// bundled QAIRT skel wins) followed by the system DSP fallback dirs,
/// `;`-separated per Qualcomm's FastRPC convention. Pure helper.
pub fn compose_adsp_library_path(native_lib_dir: &Path) -> String {
    let mut parts = vec![native_lib_dir.to_string_lossy().into_owned()];
    parts.extend(ADSP_SYSTEM_FALLBACK_DIRS.iter().map(|s| s.to_string()));
    parts.join(";")
}

/// If the current executable is running inside a macOS `.app` bundle,
/// resolve the directory under `Contents/Resources/` that holds the
/// bundled seed `*.jsonl` files. Returns `None` otherwise.
pub fn resolve_packaged_seed_dir(exe_path: &Path) -> Option<PathBuf> {
    let canonical = exe_path.canonicalize().ok()?;
    let macos_dir = canonical.parent()?;
    if macos_dir.file_name()? != "MacOS" {
        return None;
    }
    let contents_dir = macos_dir.parent()?;
    if contents_dir.file_name()? != "Contents" {
        return None;
    }
    let resources = contents_dir.join("Resources");
    if !resources.is_dir() {
        return None;
    }
    find_jsonl_dir(&resources, 0, 8)
}

/// If we can resolve a packaged seed dir from the current executable,
/// set `PRIMER_SEED_DIR` so the engine's `auto_seed_if_empty` picks
/// it up. Safe to call when not in a `.app` — no env mutation happens
/// in that case.
pub fn set_packaged_seed_dir_if_present() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let Some(dir) = resolve_packaged_seed_dir(&exe) else {
        return;
    };
    // SAFETY: called once at startup before any threads are spawned;
    // the Tauri runtime has not yet been built. Edition 2024 marks
    // set_var as unsafe because it's not thread-safe.
    unsafe {
        std::env::set_var("PRIMER_SEED_DIR", &dir);
    }
    tracing::info!(seed_dir = %dir.display(), "resolved packaged seed dir");
}

/// Initialise the Tauri-managed [`crate::state::AppState`] on mobile,
/// resolving the data directory from Tauri's path API instead of `$HOME`.
///
/// On Android `$HOME` is unset (or points somewhere unwritable), so the
/// desktop [`crate::resolve_home`] path is wrong. `app.path().app_data_dir()`
/// resolves to the app-private `/data/data/<bundle-id>/files`, which the
/// rest of the stack uses as the single base directory: the GUI config
/// (`<data>/.primer/gui-config.json`), the per-learner session DB
/// (`<data>/.primer/<slug>.db`), and the voice-asset cache
/// (`<data>/.cache/primer/models/`) all derive from it via parameters —
/// `primer-engine` never reads `$HOME` directly. Keeping a single knob is
/// what makes the desktop path byte-identical (the value is just `$HOME`
/// there) while Android gets correct app-private storage.
///
/// Called from the Tauri `setup` hook because `app.path()` needs the
/// constructed `App`; the desktop build manages `AppState` before the
/// builder runs (where `$HOME` is already available without an `App`).
#[cfg(mobile)]
pub fn init_mobile_state(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    use tauri::Manager;

    let home = app.path().app_data_dir()?;
    tracing::info!(app_data_dir = %home.display(), "resolved Android app data dir");

    let config = crate::config::load(&home).unwrap_or_else(|e| {
        // A malformed on-disk config must not keep the app from booting —
        // mirror the desktop posture in `crate::run`.
        tracing::error!("loading gui-config.json failed: {e}; using defaults");
        crate::config::GuiConfig::default()
    });
    // Capture the opt-in diagnostics flag before `config` moves into AppState.
    let qnn_metrics_opt_in = config.diagnostics.qnn_metrics_enabled;
    app.manage(crate::state::AppState::new(home.clone(), config));

    // Volunteer sideload builds have no `adb push`, so stage the embedded
    // seed corpus to <app_data>/seed/ on first run; the discovery call below
    // then points PRIMER_SEED_DIR at it. Failure degrades to an empty KB
    // (the prompt builder omits the knowledge section) rather than blocking boot.
    #[cfg(target_os = "android")]
    if let Err(e) = extract_bundled_seed_if_absent(&home) {
        tracing::warn!("seed extraction failed: {e}; continuing without knowledge base");
    }

    set_mobile_seed_dir_if_present(&home);
    set_adsp_library_path_if_present();
    set_genie_log_path(&home);
    // QNN throughput metrics are opt-in (issue #228): only enable the on-device
    // recording when the developer has flipped the Settings → Diagnostics
    // toggle. A child's device records nothing by default.
    if qnn_metrics_opt_in {
        set_qnn_metrics_path(&home);
    }
    Ok(())
}

/// Point the Genie logging callback at `<app_data>/.primer/genie.log` by
/// setting `PRIMER_GENIE_LOG_PATH`, so the cause behind a generic
/// `GenieDialog_create` -1 (a catch-all that `logcat` would normally show,
/// but which is dead on some ROMs) is written to a file the developer reads
/// via `run-as cat`.
///
/// Best-effort: creates the `.primer` parent directory if missing and
/// no-ops when the env var is already set (so an explicit override wins).
/// When `PRIMER_GENIE_LOG_PATH` is unset the inference layer leaves Genie
/// logging disabled, so a failure here only forgoes the diagnostic — it
/// never breaks the backend.
#[cfg(mobile)]
pub fn set_genie_log_path(app_data: &Path) {
    if std::env::var_os(GENIE_LOG_PATH_ENV).is_some() {
        return;
    }
    let log_path = mobile_genie_log_path(app_data);
    if let Some(parent) = log_path.parent() {
        // Best-effort; the inference layer's file open will surface a real
        // error path if this didn't take.
        let _ = std::fs::create_dir_all(parent);
    }
    // SAFETY: called from the Tauri `setup` hook on the main thread before
    // any session/background task is spawned, so no other thread reads the
    // environment concurrently. Mirrors `set_adsp_library_path_if_present`.
    unsafe {
        std::env::set_var(GENIE_LOG_PATH_ENV, &log_path);
    }
    tracing::info!(
        target: "primer-gui::startup",
        genie_log_path = %log_path.display(),
        "routed Genie logging callback to file (logcat is unavailable on some ROMs)"
    );
}

/// Enable QNN per-turn throughput metrics by pointing `PRIMER_QNN_METRICS_PATH`
/// at `<app_data>/.primer/qnn_metrics.jsonl`, so the backend appends a TTFT +
/// decode-tok/s record per turn to a file the developer reads via `run-as cat`.
///
/// This is the only on-device channel for real NPU throughput numbers on the
/// target ROM: the standalone `qnn_bench` example cannot reach the Hexagon DSP
/// from a sideloaded/Termux process, so the measurement must happen inside the
/// running APK.
///
/// Best-effort, mirroring [`set_genie_log_path`]: creates the `.primer` parent
/// if missing and no-ops when the env var is already set (an explicit override
/// wins). When unset the inference layer leaves metrics recording disabled, so
/// a failure here only forgoes the measurement — it never breaks the backend.
#[cfg(mobile)]
pub fn set_qnn_metrics_path(app_data: &Path) {
    if std::env::var_os(QNN_METRICS_PATH_ENV).is_some() {
        return;
    }
    let metrics_path = mobile_qnn_metrics_path(app_data);
    if let Some(parent) = metrics_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // SAFETY: called from the Tauri `setup` hook on the main thread before any
    // session/background task is spawned, so no other thread reads the
    // environment concurrently. Mirrors `set_genie_log_path`.
    unsafe {
        std::env::set_var(QNN_METRICS_PATH_ENV, &metrics_path);
    }
    tracing::info!(
        target: "primer-gui::startup",
        qnn_metrics_path = %metrics_path.display(),
        "enabled QNN per-turn throughput metrics (read on-device via run-as cat)"
    );
}

/// Point the Hexagon DSP's FastRPC at the QAIRT skel libraries bundled in
/// the APK by setting `ADSP_LIBRARY_PATH` to the app's `nativeLibraryDir`
/// (where `lib/arm64-v8a/*.so` — including `libQnnHtpV*Skel.so` — are
/// extracted at install) plus the system DSP fallback dirs.
///
/// Without this, `GenieDialog_create` fails (status -1) because the DSP
/// cannot locate the bundled skel — the device firmware only ships its own
/// native-arch skel. The dir is discovered from `/proc/self/maps` by
/// anchoring on the app's own `libprimer_gui.so` (always mapped from
/// `nativeLibraryDir`); no Android `Context`/JNI is needed.
///
/// No-op when `ADSP_LIBRARY_PATH` is already set, or when the lib dir
/// can't be determined (logged) — in the latter case `GenieDialog_create`
/// will surface the skel-not-found failure as before.
#[cfg(mobile)]
pub fn set_adsp_library_path_if_present() {
    const ADSP_ENV: &str = "ADSP_LIBRARY_PATH";
    if std::env::var_os(ADSP_ENV).is_some() {
        return;
    }
    let maps = match std::fs::read_to_string("/proc/self/maps") {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(target: "primer-gui::startup", "reading /proc/self/maps failed: {e}; ADSP_LIBRARY_PATH unset");
            return;
        }
    };
    let Some(dir) = native_lib_dir_from_maps(&maps, "libprimer_gui.so") else {
        tracing::warn!(
            target: "primer-gui::startup",
            "could not locate libprimer_gui.so in /proc/self/maps; ADSP_LIBRARY_PATH unset \
             (QNN GenieDialog_create may fail to find the bundled Hexagon skel)"
        );
        return;
    };
    let value = compose_adsp_library_path(&dir);
    // SAFETY: called from the Tauri `setup` hook on the main thread, before
    // the webview event loop dispatches any command and before any
    // session/background task is spawned, so no other thread is calling
    // getenv concurrently. Mirrors the `set_mobile_seed_dir_if_present`
    // justification. The value must be set before the first FastRPC session
    // (GenieDialog_create), which only happens later at session start.
    unsafe {
        std::env::set_var(ADSP_ENV, &value);
    }
    tracing::info!(target: "primer-gui::startup", adsp_library_path = %value, "set ADSP_LIBRARY_PATH for Hexagon DSP skel discovery");
}

/// If a staged seed corpus exists under [`mobile_seed_dir`], point
/// `PRIMER_SEED_DIR` at the directory that actually holds the `*.jsonl`
/// files so the engine's `auto_seed_if_empty` discovers them. No-op (with
/// a one-line warning) when nothing is staged — the knowledge base then
/// starts empty and retrieval gracefully returns no passages, exactly as
/// on a desktop run with no seed files.
///
/// Android cannot reuse the desktop `.app`-bundle mechanism: Tauri's
/// `resource_dir()` resolves to `asset://localhost/`, which is not a
/// `std::fs`-readable path, so bundled APK assets can't be enumerated
/// with `read_dir`. The staging convention is therefore a real on-device
/// directory (see [`mobile_seed_dir`]).
#[cfg(mobile)]
pub fn set_mobile_seed_dir_if_present(app_data: &Path) {
    if std::env::var_os("PRIMER_SEED_DIR").is_some() {
        return;
    }
    let seed_root = mobile_seed_dir(app_data);
    match find_jsonl_dir(&seed_root, 0, 8) {
        Some(dir) => {
            // SAFETY: called from the Tauri `setup` hook, which runs on the
            // main thread before the webview event loop dispatches any
            // command and before any session/background task is spawned, so
            // no other thread is calling getenv concurrently. Mirrors the
            // desktop `set_packaged_seed_dir_if_present` justification.
            unsafe {
                std::env::set_var("PRIMER_SEED_DIR", &dir);
            }
            tracing::info!(seed_dir = %dir.display(), "resolved staged seed dir (Android)");
        }
        None => {
            tracing::warn!(
                target: "primer-gui::startup",
                "no staged seed corpus under {}; the knowledge base will start \
                 empty. Stage seed JSONL there (e.g. `adb push`) to populate it.",
                seed_root.display()
            );
        }
    }
}

/// Depth-first search for the first directory under `dir` (inclusive)
/// containing at least one `*.jsonl` file. Bounded at `max_depth` to
/// keep startup latency negligible. Subdirs are visited in sorted
/// order so results are deterministic across filesystems whose
/// `read_dir` enumeration order differs (HFS+ vs APFS vs Linux ext4).
fn find_jsonl_dir(dir: &Path, depth: u32, max_depth: u32) -> Option<PathBuf> {
    if depth > max_depth {
        return None;
    }
    let entries = std::fs::read_dir(dir).ok()?;
    let mut subdirs = Vec::new();
    let mut has_jsonl = false;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() && path.extension().is_some_and(|e| e == "jsonl") {
            has_jsonl = true;
        } else if path.is_dir() {
            subdirs.push(path);
        }
    }
    if has_jsonl {
        return Some(dir.to_path_buf());
    }
    subdirs.sort();
    for sub in subdirs {
        if let Some(p) = find_jsonl_dir(&sub, depth + 1, max_depth) {
            return Some(p);
        }
    }
    None
}

/// Write `(filename, contents)` pairs into `dir`, creating `dir` first and
/// skipping any file that already exists. Idempotent — a re-run never
/// overwrites a file a user (or a prior run) already placed. Host-testable;
/// the android-only [`extract_bundled_seed_if_absent`] feeds it the embedded
/// corpus.
#[cfg(any(target_os = "android", test))]
fn write_seed_files(dir: &std::path::Path, files: &[(&str, &[u8])]) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    for (name, bytes) in files {
        let dest = dir.join(name);
        if !dest.exists() {
            std::fs::write(&dest, bytes)?;
        }
    }
    Ok(())
}

/// The seed corpus embedded at compile time from `primer-gui/resources/seed/`.
/// ~280 KB of JSONL; extracted to `<app_data>/seed/` on first run so
/// `PRIMER_SEED_DIR` discovery works without `adb push`. Android-only — the
/// desktop `.app` bundle path and the `CARGO_MANIFEST_DIR` dev fallback cover
/// the other targets.
#[cfg(target_os = "android")]
static BUNDLED_SEED: include_dir::Dir<'_> =
    include_dir::include_dir!("$CARGO_MANIFEST_DIR/resources/seed");

/// Extract the embedded seed corpus into `<app_data>/seed/` if not already
/// present, returning that directory. Idempotent (see [`write_seed_files`]).
#[cfg(target_os = "android")]
pub fn extract_bundled_seed_if_absent(
    app_data: &std::path::Path,
) -> std::io::Result<std::path::PathBuf> {
    let dir = mobile_seed_dir(app_data);
    let files: Vec<(&str, &[u8])> = BUNDLED_SEED
        .files()
        .filter_map(|f| {
            f.path()
                .file_name()
                .and_then(|n| n.to_str())
                .map(|name| (name, f.contents()))
        })
        .collect();
    write_seed_files(&dir, &files)?;
    Ok(dir)
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod seed_extract_tests {
    use super::*;

    #[test]
    fn write_seed_files_creates_and_is_idempotent() {
        let tmp = std::env::temp_dir().join(format!("primer_seed_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let files: &[(&str, &[u8])] = &[
            ("seed_passages.en.jsonl", b"{\"id\":\"a\"}\n"),
            ("wiki_passages.en.jsonl", b"{\"id\":\"b\"}\n"),
        ];

        // First write creates both files.
        write_seed_files(&tmp, files).unwrap();
        assert_eq!(
            std::fs::read(tmp.join("seed_passages.en.jsonl")).unwrap(),
            b"{\"id\":\"a\"}\n"
        );
        assert_eq!(
            std::fs::read(tmp.join("wiki_passages.en.jsonl")).unwrap(),
            b"{\"id\":\"b\"}\n"
        );

        // Mutate one file, then re-run: existing files are left untouched (idempotent skip).
        std::fs::write(tmp.join("seed_passages.en.jsonl"), b"USER_EDIT").unwrap();
        write_seed_files(&tmp, files).unwrap();
        assert_eq!(
            std::fs::read(tmp.join("seed_passages.en.jsonl")).unwrap(),
            b"USER_EDIT"
        );

        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
