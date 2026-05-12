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

/// Depth-first search for the first directory under `dir` (inclusive)
/// containing at least one `*.jsonl` file. Bounded at `max_depth` to
/// keep startup latency negligible.
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
    for sub in subdirs {
        if let Some(p) = find_jsonl_dir(&sub, depth + 1, max_depth) {
            return Some(p);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Build a fake .app layout under `temp` with an exe at
    /// `Primer.app/Contents/MacOS/primer-gui`. If `jsonl_depth > 0`,
    /// place one .jsonl file `jsonl_depth` directories deep under
    /// `Resources/`.
    fn create_app_layout(temp: &Path, jsonl_depth: usize) -> PathBuf {
        let app = temp.join("Primer.app");
        let macos = app.join("Contents").join("MacOS");
        let resources = app.join("Contents").join("Resources");
        fs::create_dir_all(&macos).unwrap();
        fs::create_dir_all(&resources).unwrap();
        let exe = macos.join("primer-gui");
        fs::write(&exe, b"").unwrap();

        if jsonl_depth > 0 {
            let mut nested = resources;
            for i in 0..jsonl_depth {
                nested = nested.join(format!("level{i}"));
            }
            fs::create_dir_all(&nested).unwrap();
            fs::write(nested.join("seed_passages.en.jsonl"), b"{}\n").unwrap();
        }
        exe
    }

    #[test]
    fn returns_jsonl_dir_for_app_layout_at_depth_4() {
        let temp = TempDir::new().unwrap();
        let exe = create_app_layout(temp.path(), 4);
        let Some(dir) = resolve_packaged_seed_dir(&exe) else {
            panic!("expected Some(jsonl_dir) for valid .app layout");
        };
        assert!(
            dir.join("seed_passages.en.jsonl").exists(),
            "returned dir {dir:?} should contain the seed file"
        );
    }

    #[test]
    fn returns_jsonl_dir_for_app_layout_at_depth_1() {
        let temp = TempDir::new().unwrap();
        let exe = create_app_layout(temp.path(), 1);
        let Some(dir) = resolve_packaged_seed_dir(&exe) else {
            panic!("expected Some at depth 1");
        };
        assert!(dir.join("seed_passages.en.jsonl").exists());
    }

    #[test]
    fn returns_none_for_dev_layout() {
        let temp = TempDir::new().unwrap();
        let dir = temp.path().join("target").join("debug");
        fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("primer-gui");
        fs::write(&exe, b"").unwrap();
        assert!(resolve_packaged_seed_dir(&exe).is_none());
    }

    #[test]
    fn returns_none_when_app_layout_has_no_jsonl() {
        let temp = TempDir::new().unwrap();
        let exe = create_app_layout(temp.path(), 0);
        assert!(resolve_packaged_seed_dir(&exe).is_none());
    }

    #[test]
    fn returns_none_for_missing_resources_dir() {
        let temp = TempDir::new().unwrap();
        let app = temp.path().join("Primer.app");
        let macos = app.join("Contents").join("MacOS");
        fs::create_dir_all(&macos).unwrap();
        let exe = macos.join("primer-gui");
        fs::write(&exe, b"").unwrap();
        assert!(resolve_packaged_seed_dir(&exe).is_none());
    }
}
