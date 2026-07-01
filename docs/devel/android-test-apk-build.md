# Android generic-API test APK build notes

Build notes for producing a sideloadable Android test APK of `primer-gui`,
configured for cloud / OpenAI-compatible API inference (no on-device NPU, no
bundled embedder), for volunteer families to test. This is the host-side
verification gate: it proves the Android feature contract
(`--no-default-features --features android-native`) compiles before the
slower Tauri-Android build is attempted.

## Environment prerequisites

- `cargo-tauri` 2.11.1
- Android NDK **r29** at `/opt/homebrew/share/android-ndk`
- Android SDK at `~/Library/Android/sdk` (`ANDROID_HOME`)
- **JDK 21**
- `rustup target add aarch64-linux-android --toolchain 1.88`

See [docs/devel/android-build-quickstart.md](android-build-quickstart.md) for
the fuller environment setup (NDK discovery symlink, `JAVA_HOME` pinning,
etc.) — those details are shared across all Android build flavours and are
not repeated here.

## Step 1: host-verify the feature set compiles

From `src/`:

```bash
~/.cargo/bin/cargo build -p primer-gui --no-default-features --features android-native
```

This confirms the GUI builds without the `embedding` feature and with
`android-native` (which pulls `dep:primer-speech` + `primer-speech/android-native`).

Verified 2026-07-01 on macOS arm64 host: compiles cleanly, no warnings, in
1m 47s (`Finished `dev` profile [unoptimized + debuginfo] target(s)`).

## Step 2: embedder default is feature-aware

`crates/primer-gui/src/config/types.rs` defines `default_embedder_kind()`
twice, cfg-gated:

```rust
#[cfg(feature = "embedding")]
fn default_embedder_kind() -> &'static str {
    "fastembed"
}

#[cfg(not(feature = "embedding"))]
fn default_embedder_kind() -> &'static str {
    "none"
}
```

Without the `embedding` feature (the Android build), a fresh config (no
stored `kind` in `gui-config.json`) resolves to `"none"` — BM25-only
retrieval, no fastembed/ort download. Verified by grep against the built
source, not just static reading.

## Step 3: the on-device build command

```bash
# from src/crates/primer-gui/
~/.cargo/bin/cargo tauri android build --apk --target aarch64 \
  -- --no-default-features --features android-native
```

Flag forwarding via `--` is confirmed working (see the build note below): the
`-- --no-default-features --features android-native` tail reaches the Rust lib
build. If a future Tauri CLI ever stops forwarding it, pin the feature set in
`gen/android/app/build.gradle.kts` under the `rust { }` block instead. Either
way, sanity-check the produced APK by confirming no `libonnxruntime`/fastembed
artifacts are bundled and the app logs `--embedder-backend none` behaviour
(empty/BM25 KB).

With no `gen/android/keystore.properties` present, the `release` build produces
an **unsigned** APK (the signing config no-ops to `null`) — useful for a
compile check; a distributable build needs the keystore (see Task 6).

## Verified build (2026-07-01, macOS arm64 host)

`cargo tauri android build --apk --target aarch64 -- --no-default-features
--features android-native` completed successfully and emitted:

```
gen/android/app/build/outputs/apk/universal/release/app-universal-release-unsigned.apk
```

This exercises the Android-only code paths that no host build covers: the
`include_dir!("$CARGO_MANIFEST_DIR/resources/seed")` embed and the
`#[cfg(target_os = "android")]` first-run seed extraction both compile and link
for `aarch64-linux-android`.

**Seed-refresh caveat (build side):** the seed JSONL is embedded at compile
time via `include_dir!`. After editing `data/seed/*.jsonl` (which `build.rs`
re-copies into `resources/seed/`), do a clean rebuild of `primer-gui` so the
embedded bytes refresh — an incremental build may not re-expand the macro.

**Seed refresh on device (update side):** first-run extraction writes a
`.seed_version` marker (an FNV fingerprint of the embedded corpus) next to the
staged `*.jsonl`. On a later boot the marker is compared against the current
embedded corpus: an unchanged corpus is skipped (nothing rewritten, any staged
edit preserved), and a changed corpus — i.e. a new APK carrying a revised
corpus installed over an already-run one — is re-extracted so the child sees
the current corpus rather than the stale first-install copy.

## Next step

See Task 6's on-device smoke checklist for installing and exercising the
built (signed) APK on a physical device.
