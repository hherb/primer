//! Pin the `--backend qnn` dispatch path of [`build_backend`].
//!
//! The QNN backend's *positive* construction path is unit-tested
//! at the [`primer_inference::qnn`] module (via the mock
//! `GenieLibrary` trait split). Here we only need to pin
//! [`build_backend`]'s dispatch:
//!
//! - With the feature compiled in, missing `qnn_bundle_dir` is a
//!   clear error from `build_backend` itself (before any FFI).
//! - With the feature compiled in and `qnn_bundle_dir` set,
//!   dispatch reaches `QnnBackend::new`, which on every non-Android
//!   host returns the typed `PlatformUnsupported` error — proving
//!   that the qnn arm fired (any other arm would have produced
//!   either Ok or a different error message).
//! - With the feature *not* compiled in, the user gets a
//!   "rebuild with --features qnn" hint, NOT the generic
//!   "unknown backend" message — that distinction is the
//!   load-bearing UX win of the per-feature dispatch.
use super::*;

/// Build a `BackendParams` skeleton for the qnn dispatch tests.
fn params() -> BackendParams {
    BackendParams {
        api_key: None,
        ollama_url: "http://localhost:11434".into(),
        openai_compat_url: "http://localhost:8000".into(),
        openai_compat_api_key: None,
        classifier_backend: None,
        classifier_model: None,
        extractor_backend: None,
        extractor_model: None,
        comprehension_backend: None,
        comprehension_model: None,
        qnn_bundle_dir: None,
        qnn_qairt_lib_dir: None,
        gguf_path: None,
        llamacpp_gpu_layers: None,
        llamacpp_n_ctx: None,
        reasoning_markers: Vec::new(),
        fallback_backend: None,
        fallback_model: None,
        router_mode: primer_core::router::RouterMode::LocalOnly,
        primary_ttft_budget_ms: None,
    }
}

#[test]
fn default_qairt_lib_dir_lives_one_dir_up_alongside_qairt_lib() {
    // The conventional QAIRT layout from AI Hub apps puts the
    // bundle dir alongside `qairt/` at the same parent level:
    //   ~/primer-bundles/qwen3-4b/{genie_config.json, ...}
    //   ~/primer-bundles/qairt/lib/aarch64-android/libGenie.so
    let bundle = PathBuf::from("/home/user/primer-bundles/qwen3-4b");
    let lib = default_qairt_lib_dir(&bundle);
    assert_eq!(
        lib,
        PathBuf::from("/home/user/primer-bundles/qairt/lib/aarch64-android")
    );
}

#[test]
fn default_qairt_lib_dir_tolerates_root_bundle_path() {
    // A bundle at the filesystem root (no parent) is unusual but
    // should not panic. We fall back to a same-directory relative
    // path — downstream `RealGenieLibrary::open` will report a
    // useful `LibraryLoad` error rather than us deciding here.
    let bundle = PathBuf::from("/");
    let lib = default_qairt_lib_dir(&bundle);
    // On `/`, `.parent()` is `None`, so we fall back to the bare
    // relative path. Exact form is documented in the helper.
    assert_eq!(lib, PathBuf::from("qairt/lib/aarch64-android"));
}

#[test]
fn resolve_qairt_lib_dir_passes_explicit_through_on_every_platform() {
    // An explicit override always wins, regardless of platform — the
    // user knows exactly where their QAIRT libs are.
    let bundle = PathBuf::from("/home/user/bundles/qwen3-4b");
    let explicit = PathBuf::from("/opt/qairt/lib/aarch64-android");
    for is_android in [false, true] {
        assert_eq!(
            resolve_qairt_lib_dir_for(Some(explicit.clone()), &bundle, is_android),
            explicit,
            "explicit override must pass through (is_android={is_android})"
        );
    }
}

#[test]
fn resolve_qairt_lib_dir_android_absent_yields_empty_for_basename_load() {
    // On Android the 9 QAIRT `.so`s ship inside the APK's
    // `lib/arm64-v8a/` (extracted to nativeLibraryDir). An absent
    // override must resolve to an EMPTY path, which signals the QNN
    // backend to dlopen `libGenie.so` by basename so the system
    // linker resolves it (and its DT_NEEDED deps) from that dir.
    let bundle = PathBuf::from("/data/local/bundles/qwen3-4b");
    assert_eq!(
        resolve_qairt_lib_dir_for(None, &bundle, true),
        PathBuf::new(),
        "Android + no override must yield empty (basename load)"
    );
}

#[test]
fn resolve_qairt_lib_dir_desktop_absent_falls_back_to_bundle_relative() {
    // On desktop an absent override keeps today's behaviour: the
    // conventional `<bundle>/../qairt/lib/aarch64-android` layout.
    let bundle = PathBuf::from("/home/user/bundles/qwen3-4b");
    assert_eq!(
        resolve_qairt_lib_dir_for(None, &bundle, false),
        default_qairt_lib_dir(&bundle),
        "desktop + no override must use the bundle-relative default"
    );
}

/// Without the `qnn` feature, the dispatch arm hands back a build
/// hint, NOT the generic "unknown backend: qnn" string. The
/// distinction matters because users who haven't compiled in qnn
/// need a different action than users who typo'd the backend name.
#[cfg(not(feature = "qnn"))]
#[tokio::test]
async fn qnn_without_feature_returns_build_hint() {
    let p = params();
    let result = build_backend("qnn", "qnn-placeholder".into(), &p).await;
    let err = match result {
        Ok(_) => panic!("expected qnn-without-feature to error, got Ok"),
        Err(e) => e,
    };
    let msg = format!("{err}");
    assert!(
        msg.contains("qnn") && msg.contains("feature"),
        "expected build hint mentioning qnn + feature; got: {msg}"
    );
    // And NOT the generic unknown-backend phrasing:
    assert!(
        !msg.contains("unknown backend"),
        "qnn-without-feature should be distinct from unknown-backend; got: {msg}"
    );
}

/// With the `qnn` feature compiled in but no `qnn_bundle_dir`
/// set in params, the dispatch arm reports the missing required
/// input BEFORE any FFI is attempted — exactly the "fast clap-style
/// rejection" UX the plan calls for.
#[cfg(feature = "qnn")]
#[tokio::test]
async fn qnn_with_feature_missing_bundle_dir_errors_pre_ffi() {
    let p = params();
    let result = build_backend("qnn", "qnn-placeholder".into(), &p).await;
    let err = match result {
        Ok(_) => panic!("expected qnn-with-no-bundle to error, got Ok"),
        Err(e) => e,
    };
    let msg = format!("{err}");
    assert!(
        msg.contains("qnn-bundle-dir") || msg.contains("bundle"),
        "expected missing-bundle-dir hint; got: {msg}"
    );
}

/// With the `qnn` feature compiled in and a `qnn_bundle_dir`
/// set, dispatch reaches `QnnBackend::new`. On every host the
/// repo's CI runs on (Linux + macOS), this surfaces the typed
/// `PlatformUnsupported` error from `primer-qnn-sys`. That proves
/// the qnn arm fired — neither the "unknown backend" arm nor the
/// "missing bundle dir" guard could have produced this string.
#[cfg(all(feature = "qnn", not(target_os = "android")))]
#[tokio::test]
async fn qnn_with_feature_and_bundle_dir_dispatches_to_real_lib_on_host() {
    // Build params with a fake (nonexistent) bundle dir — the
    // dispatch arm hands these straight to `QnnBackend::new`, which
    // tries to dlopen `libGenie.so` from the qairt lib dir FIRST.
    // On a non-Android host, that returns `PlatformUnsupported`
    // before the bundle's existence is checked.
    let p = BackendParams {
        qnn_bundle_dir: Some(PathBuf::from("/nonexistent/bundle")),
        ..params()
    };
    let result = build_backend("qnn", "qnn-placeholder".into(), &p).await;
    let err = match result {
        Ok(_) => panic!("expected PlatformUnsupported on non-Android host, got Ok"),
        Err(e) => e,
    };
    let msg = format!("{err}");
    // The dev-facing error string from
    // `GenieCallError::PlatformUnsupported` carries the platform
    // name. On macOS this is `"macos"`, on Linux it's `"linux"`.
    assert!(
        msg.to_lowercase().contains("android")
            || msg.to_lowercase().contains("platform")
            || msg.to_lowercase().contains("only supported"),
        "expected PlatformUnsupported-flavoured error; got: {msg}"
    );
}

/// Without the `llamacpp` feature (the default test build), the
/// dispatch arm hands back a build hint mentioning `llamacpp` and
/// `feature` — not the generic "unknown backend" string.
#[tokio::test]
async fn llamacpp_without_feature_returns_build_hint() {
    let params = BackendParams {
        gguf_path: Some(std::path::PathBuf::from("/tmp/model.gguf")),
        ..params()
    };
    let err = match build_backend("llamacpp", "ignored".into(), &params).await {
        Ok(_) => panic!("expected llamacpp-without-feature to error, got Ok"),
        Err(e) => e,
    };
    let msg = format!("{err}");
    assert!(msg.contains("llamacpp"), "got: {msg}");
    assert!(msg.contains("feature"), "got: {msg}");
}

/// The `"llamacpp"` string reaches its own dispatch arm (the error
/// mentions llamacpp, proving it didn't fall through to the generic
/// unknown-backend arm).
#[tokio::test]
async fn llamacpp_dispatch_reaches_arm() {
    let params = params();
    let err = match build_backend("llamacpp", "ignored".into(), &params).await {
        Ok(_) => panic!("expected llamacpp dispatch to error, got Ok"),
        Err(e) => e,
    };
    assert!(format!("{err}").to_lowercase().contains("llamacpp"));
}
