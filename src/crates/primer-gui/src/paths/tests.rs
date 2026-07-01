use super::*;
use std::fs;
use tempfile::TempDir;

#[test]
fn native_lib_dir_from_maps_extracts_dir_of_matching_lib() {
    // A representative /proc/self/maps excerpt. The app's own cdylib is
    // always mapped from its nativeLibraryDir; we anchor on its basename
    // and return the containing directory so ADSP_LIBRARY_PATH can point
    // the Hexagon DSP at the bundled QAIRT skel that lives there too.
    let maps = "\
12c00000-12c80000 r--p 00000000 fd:03 1234 /data/app/~~AbC==/org.theprimer.gui-XyZ==/lib/arm64/libprimer_gui.so
12c80000-12d00000 r-xp 00080000 fd:03 1234 /data/app/~~AbC==/org.theprimer.gui-XyZ==/lib/arm64/libprimer_gui.so
70aa000-70ab000 r--p 00000000 fd:03 9999 /apex/com.android.runtime/lib64/bionic/libc.so
";
    let dir = native_lib_dir_from_maps(maps, "libprimer_gui.so");
    assert_eq!(
        dir,
        Some(PathBuf::from(
            "/data/app/~~AbC==/org.theprimer.gui-XyZ==/lib/arm64"
        ))
    );
}

#[test]
fn native_lib_dir_from_maps_returns_none_when_absent() {
    let maps = "70aa000-70ab000 r--p 0 fd:03 1 /apex/com.android.runtime/lib64/bionic/libc.so\n";
    assert_eq!(native_lib_dir_from_maps(maps, "libprimer_gui.so"), None);
}

#[test]
fn native_lib_dir_from_maps_handles_spaces_in_path() {
    // Mapped paths can (rarely) contain spaces; the path is everything
    // after the 5th whitespace-delimited field, so a naive split on the
    // last token would truncate it. Pin the whole-remainder behaviour.
    let maps = "a-b r-xp 0 fd:03 7 /data/app/My App/lib/arm64/libprimer_gui.so\n";
    assert_eq!(
        native_lib_dir_from_maps(maps, "libprimer_gui.so"),
        Some(PathBuf::from("/data/app/My App/lib/arm64"))
    );
}

#[test]
fn compose_adsp_library_path_puts_app_dir_first_then_system_fallbacks() {
    // The bundled v79 skel must win over the device firmware's v81 skel,
    // so the app's nativeLibraryDir comes first; system DSP dirs follow
    // as fallbacks. ADSP_LIBRARY_PATH is ';'-separated (Qualcomm).
    let v = compose_adsp_library_path(Path::new("/data/app/x/lib/arm64"));
    assert!(
        v.starts_with("/data/app/x/lib/arm64;"),
        "app dir must come first: {v}"
    );
    for sys in ADSP_SYSTEM_FALLBACK_DIRS {
        assert!(v.contains(sys), "missing system fallback {sys}: {v}");
    }
    assert!(!v.contains(','), "must use ';' not ',': {v}");
}

#[test]
fn mobile_genie_log_path_is_under_dot_primer() {
    // The Genie diagnostics log sits next to the GUI config under
    // `<app_data>/.primer/`, so the same `run-as cat .primer/...`
    // idiom reads both. Pin the layout the staging/read steps assume.
    let app_data = Path::new("/data/data/org.theprimer.gui/files");
    let p = mobile_genie_log_path(app_data);
    assert_eq!(
        p,
        app_data
            .join(primer_engine::paths::PRIMER_HOME_DIR)
            .join(GENIE_LOG_FILENAME)
    );
    assert!(p.ends_with(".primer/genie.log"), "got {p:?}");
}

#[test]
fn mobile_qnn_metrics_path_is_under_dot_primer() {
    // The QNN throughput metrics file sits next to genie.log so the same
    // `run-as cat .primer/...` idiom reads it. Pin the layout the device
    // read step assumes.
    let app_data = Path::new("/data/data/org.theprimer.gui/files");
    let p = mobile_qnn_metrics_path(app_data);
    assert_eq!(
        p,
        app_data
            .join(primer_engine::paths::PRIMER_HOME_DIR)
            .join(QNN_METRICS_FILENAME)
    );
    assert!(p.ends_with(".primer/qnn_metrics.jsonl"), "got {p:?}");
}

#[test]
fn mobile_seed_dir_is_seed_subdir_of_app_data() {
    // On Android the seed corpus cannot be read from the APK asset
    // namespace (`resource_dir()` is `asset://localhost/`, not a
    // std::fs path). The convention is a real, app-readable staged
    // directory under the app's data dir; document + pin it here so
    // the `adb push` staging step and the resolver agree.
    let app_data = Path::new("/data/data/com.primer.app/files");
    assert_eq!(mobile_seed_dir(app_data), app_data.join(MOBILE_SEED_SUBDIR));
}

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

#[test]
fn finds_jsonl_at_max_depth_boundary() {
    // resolve_packaged_seed_dir starts find_jsonl_dir at depth=0
    // inside Resources/, so a jsonl_depth of 8 lands exactly at
    // the depth=8 limit (still permitted) — the deepest layout
    // the current cap accepts.
    let temp = TempDir::new().unwrap();
    let exe = create_app_layout(temp.path(), 8);
    let Some(dir) = resolve_packaged_seed_dir(&exe) else {
        panic!("expected Some at max-depth boundary");
    };
    assert!(dir.join("seed_passages.en.jsonl").exists());
}

#[test]
fn returns_none_beyond_max_depth() {
    // jsonl_depth=9 is one past the cap — the search must abandon
    // before reaching it. Defends against quietly raising the cap
    // without a paired test.
    let temp = TempDir::new().unwrap();
    let exe = create_app_layout(temp.path(), 9);
    assert!(resolve_packaged_seed_dir(&exe).is_none());
}

#[test]
fn subdir_traversal_is_sorted() {
    // Build a Resources/ tree with two sibling subdirs, both
    // containing a jsonl. The DFS should pick the lexicographically
    // first one regardless of read_dir order. Without the sort,
    // this is filesystem-dependent.
    let temp = TempDir::new().unwrap();
    let app = temp.path().join("Primer.app");
    let macos = app.join("Contents").join("MacOS");
    let resources = app.join("Contents").join("Resources");
    fs::create_dir_all(&macos).unwrap();
    let a = resources.join("aaa");
    let z = resources.join("zzz");
    fs::create_dir_all(&a).unwrap();
    fs::create_dir_all(&z).unwrap();
    fs::write(a.join("seed.jsonl"), b"{}\n").unwrap();
    fs::write(z.join("seed.jsonl"), b"{}\n").unwrap();
    let exe = macos.join("primer-gui");
    fs::write(&exe, b"").unwrap();

    let Some(found) = resolve_packaged_seed_dir(&exe) else {
        panic!("expected Some");
    };
    assert!(
        found.ends_with("aaa"),
        "expected sorted DFS to pick 'aaa' first, got {found:?}"
    );
}

// --- seed extraction (write_seed_files) ---

#[test]
fn write_seed_files_creates_and_skips_when_corpus_unchanged() {
    let tmp = std::env::temp_dir().join(format!("primer_seed_test_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let files: &[(&str, &[u8])] = &[
        ("seed_passages.en.jsonl", b"{\"id\":\"a\"}\n"),
        ("wiki_passages.en.jsonl", b"{\"id\":\"b\"}\n"),
    ];

    // First write creates both files plus the version marker.
    write_seed_files(&tmp, files).unwrap();
    assert_eq!(
        std::fs::read(tmp.join("seed_passages.en.jsonl")).unwrap(),
        b"{\"id\":\"a\"}\n"
    );
    assert_eq!(
        std::fs::read(tmp.join("wiki_passages.en.jsonl")).unwrap(),
        b"{\"id\":\"b\"}\n"
    );
    assert!(tmp.join(SEED_VERSION_MARKER).exists());

    // Mutate one file, then re-run with the SAME corpus: the fingerprint
    // matches the marker, so nothing is rewritten and the edit survives.
    std::fs::write(tmp.join("seed_passages.en.jsonl"), b"USER_EDIT").unwrap();
    write_seed_files(&tmp, files).unwrap();
    assert_eq!(
        std::fs::read(tmp.join("seed_passages.en.jsonl")).unwrap(),
        b"USER_EDIT"
    );

    std::fs::remove_dir_all(&tmp).unwrap();
}

#[test]
fn write_seed_files_reextracts_when_corpus_changes() {
    let tmp = std::env::temp_dir().join(format!("primer_seed_change_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);

    // v1 of the corpus (a first install).
    let v1: &[(&str, &[u8])] = &[("seed_passages.en.jsonl", b"{\"id\":\"a\"}\n")];
    write_seed_files(&tmp, v1).unwrap();

    // v2 carries changed bytes under the same filename (an app update).
    // The fingerprint differs, so the stale file is refreshed.
    let v2: &[(&str, &[u8])] = &[("seed_passages.en.jsonl", b"{\"id\":\"a2\"}\n")];
    write_seed_files(&tmp, v2).unwrap();
    assert_eq!(
        std::fs::read(tmp.join("seed_passages.en.jsonl")).unwrap(),
        b"{\"id\":\"a2\"}\n"
    );

    std::fs::remove_dir_all(&tmp).unwrap();
}
