//! Build script for `primer-cli`.
//!
//! Only does anything when the `macos-native-26` feature is on. In that
//! case it adds `/usr/lib/swift` to the binary's rpath so dyld can
//! resolve `@rpath/libswift_Concurrency.dylib` from the shared cache.
//!
//! This rpath is needed because `primer-speech`'s build.rs emits the
//! same directive, but `cargo:rustc-link-arg=` is package-scoped — it
//! does NOT propagate from a library crate to a downstream binary
//! crate. So we duplicate the rpath addition here.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    #[cfg(feature = "macos-native-26")]
    {
        // libswift_Concurrency.dylib lives only in the dyld shared cache
        // on macOS 12+. Add /usr/lib/swift to the rpath so dyld resolves
        // the @rpath-referenced concurrency back-deployment lib without
        // needing an on-disk file.
        println!("cargo:rustc-link-arg-bins=-Wl,-rpath,/usr/lib/swift");
    }
}
