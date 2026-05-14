fn main() {
    copy_seed_resources();
    tauri_build::build();
}

/// Copy the workspace's `data/seed/*.jsonl` files into a crate-local
/// `resources/seed/` directory so Tauri's bundler can stage them
/// inside the .app's `Contents/Resources/`. Tauri 2's `bundle.resources`
/// glob is rooted at the tauri.conf.json directory and does not accept
/// upward-pointing paths, so the copy is necessary.
fn copy_seed_resources() {
    use std::path::PathBuf;

    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let src = manifest
        .ancestors()
        .nth(3)
        .expect("CARGO_MANIFEST_DIR has at least 3 ancestors")
        .join("data/seed");
    let dst = manifest.join("resources/seed");

    if !src.is_dir() {
        panic!("expected seed source dir not found: {}", src.display());
    }

    // Track the dir itself so adding/removing a *.jsonl re-fires the
    // build script; the per-file rerun-if-changed lines below only
    // catch edits to files that existed at last run.
    println!("cargo:rerun-if-changed={}", src.display());

    if dst.exists() {
        std::fs::remove_dir_all(&dst).unwrap_or_else(|e| panic!("clean {}: {e}", dst.display()));
    }
    std::fs::create_dir_all(&dst).unwrap_or_else(|e| panic!("create {}: {e}", dst.display()));

    let entries = std::fs::read_dir(&src).unwrap_or_else(|e| panic!("read {}: {e}", src.display()));
    for entry in entries {
        let entry = entry.expect("read_dir entry");
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "jsonl") {
            let name = path.file_name().expect("jsonl file has a name");
            let target = dst.join(name);
            std::fs::copy(&path, &target)
                .unwrap_or_else(|e| panic!("copy {} -> {}: {e}", path.display(), target.display()));
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
}
