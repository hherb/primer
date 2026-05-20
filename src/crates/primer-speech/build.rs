//! Build script for `primer-speech`.
//!
//! Only does anything when the `macos-native-26` feature is on. In that
//! case it:
//!   1. Invokes `swift-bridge-build` to generate the C header + Swift
//!      glue from src/macos26/bridge.rs.
//!   2. Invokes `swiftc` to compile the Swift sidecar + generated glue
//!      into a static library.
//!   3. Emits cargo:rustc-link-* directives so the final Rust binary
//!      pulls in the .a and the Swift runtime.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=swift-sources");
    println!("cargo:rerun-if-changed=src/macos26/bridge.rs");

    #[cfg(feature = "macos-native-26")]
    macos_native_26::build();
}

#[cfg(feature = "macos-native-26")]
mod macos_native_26 {
    use std::path::PathBuf;
    use std::process::Command;

    const SWIFT_LIB_NAME: &str = "Macos26Pipeline";

    pub fn build() {
        let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
        let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
        let bridges = vec![manifest_dir.join("src/macos26/bridge.rs")];

        // 1. swift-bridge codegen.
        let generated = out_dir.join("generated");
        std::fs::create_dir_all(&generated).expect("create generated dir");
        swift_bridge_build::parse_bridges(bridges)
            .write_all_concatenated(&generated, SWIFT_LIB_NAME);

        // 2. swiftc — compile the sidecar + generated glue into a static lib.
        let swift_sources = manifest_dir.join("swift-sources");

        // Concatenate the SwiftBridgeCore C header and the bridge-specific C
        // header into a single bridging header so swiftc can resolve C types
        // referenced by the generated Swift glue (RustStr, __private__OptionU8, …).
        let bridging_header = out_dir.join("SwiftBridge_Bridging.h");
        let core_h = generated.join("SwiftBridgeCore.h");
        let bridge_h = generated.join(SWIFT_LIB_NAME).join(format!("{SWIFT_LIB_NAME}.h"));
        let bridging_content = format!(
            "#include \"{}\"\n#include \"{}\"\n",
            core_h.display(),
            bridge_h.display(),
        );
        std::fs::write(&bridging_header, &bridging_content)
            .expect("write bridging header");

        let lib_path = out_dir.join(format!("lib{}.a", SWIFT_LIB_NAME));
        let mut cmd = Command::new("swiftc");
        cmd.arg("-emit-library")
            .arg("-static")
            .arg("-emit-module")
            .arg("-module-name").arg(SWIFT_LIB_NAME)
            .arg("-target").arg(swift_target_triple())
            .arg("-sdk").arg(macos_sdk_path())
            .arg("-O")
            .arg("-parse-as-library")
            .arg("-import-objc-header").arg(&bridging_header)
            .arg(swift_sources.join("Macos26PipelineImpl.swift"))
            .args(walk_swift_files_recursive(&generated))
            .arg("-o").arg(&lib_path);
        let status = cmd.status().expect("invoke swiftc");
        assert!(status.success(), "swiftc failed");

        // 3. Link directives.
        println!("cargo:rustc-link-search=native={}", out_dir.display());
        println!("cargo:rustc-link-lib=static={}", SWIFT_LIB_NAME);
        // Swift runtime libraries — required when linking a Swift staticlib.
        println!("cargo:rustc-link-search=native={}", swift_runtime_dir());
        for fw in ["Foundation", "AVFoundation", "CoreMedia", "Speech"] {
            println!("cargo:rustc-link-lib=framework={fw}");
        }
        println!("cargo:rustc-link-arg=-Wl,-rpath,{}", swift_runtime_dir());
        println!("cargo:rustc-link-arg=-L{}", swift_runtime_dir());
        println!("cargo:rustc-link-arg=-lswiftCore");
        // libswift_Concurrency.dylib lives only in the dyld shared cache
        // on macOS 12+ — it has no on-disk file. Add /usr/lib/swift to
        // the rpath so dyld resolves the @rpath-referenced concurrency
        // back-deployment lib from the cache rather than failing.
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    }

    fn swift_target_triple() -> String {
        let arch = match std::env::var("CARGO_CFG_TARGET_ARCH").unwrap().as_str() {
            "aarch64" => "arm64",
            "x86_64" => "x86_64",
            other => panic!("unsupported arch: {other}"),
        };
        format!("{arch}-apple-macos26.0")
    }

    fn macos_sdk_path() -> String {
        let out = Command::new("xcrun")
            .args(["--show-sdk-path", "--sdk", "macosx"])
            .output()
            .expect("invoke xcrun");
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    fn swift_runtime_dir() -> String {
        let xcode = Command::new("xcode-select")
            .arg("-p")
            .output()
            .expect("invoke xcode-select");
        let xcode_path = String::from_utf8(xcode.stdout).unwrap().trim().to_string();
        format!("{xcode_path}/Toolchains/XcodeDefault.xctoolchain/usr/lib/swift/macosx")
    }

    // Recursively collect all .swift files under `dir` (including subdirs).
    fn walk_swift_files_recursive(dir: &PathBuf) -> Vec<PathBuf> {
        let mut result = Vec::new();
        fn recurse(dir: &std::path::Path, result: &mut Vec<PathBuf>) {
            let rd = match std::fs::read_dir(dir) {
                Ok(r) => r,
                Err(_) => return,
            };
            for entry in rd.filter_map(|e| e.ok()) {
                let path = entry.path();
                if path.is_dir() {
                    recurse(&path, result);
                } else if path.extension().and_then(|s| s.to_str()) == Some("swift") {
                    result.push(path);
                }
            }
        }
        recurse(dir, &mut result);
        result
    }
}
