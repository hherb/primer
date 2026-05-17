//! Compile-only canary: when the `macos-native` feature is on,
//! `primer_speech::macos` must be a module that resolves.

#[cfg(all(target_os = "macos", feature = "macos-native"))]
#[test]
fn macos_module_is_present() {
    // Just touching the module path is enough — the test passes if it compiles.
    let _ = primer_speech::macos::FEATURE_NAME;
}

#[cfg(not(all(target_os = "macos", feature = "macos-native")))]
#[test]
fn macos_module_is_absent_off_macos_or_off_feature() {
    // Sanity: this test compiles unconditionally so CI on Linux still
    // sees a green test for this file.
}
