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

If `cargo tauri android build` does not forward `--no-default-features`/`--features`
to the Rust lib build, pin the feature set in `gen/android/app/build.gradle.kts`
under the `rust { }` block instead. Verify the produced APK by checking that
no `libonnxruntime`/fastembed artifacts are bundled and that the app logs
`--embedder-backend none` behaviour (empty/BM25 KB).

## Next step

See Task 6's on-device smoke checklist for installing and exercising the
built APK on a physical device.
