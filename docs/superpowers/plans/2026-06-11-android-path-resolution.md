# Android path-resolution fix + QNN on-device wiring (Phase 1.2 sub-project 3 prereq)

**Date:** 2026-06-11
**Branch:** `android-path-resolution`
**Goal:** Make `primer-gui` resolve filesystem paths correctly on Android so the
installed APK can launch, persist config/sessions, read a knowledge base, and
construct `QnnBackend` against an app-readable bundle. Desktop behaviour stays
byte-identical.

## Root-cause map (verified this session)

- `home` is the single knob: `config` load/save, the session DB path
  (`primer_engine::resolve_session_db_path`), and the voice cache
  (`voice::assets::cache_root`) ALL derive from `AppState.home` via parameters.
  `primer-engine` already takes `home` as a param and never reads `$HOME`.
  ⇒ Fix = resolve `home` per-platform; everything downstream follows.
- `resolve_home()` reads `$HOME` → wrong on Android. Tauri's
  `app.path().app_data_dir()` is the Android-correct base (`getDataDir` =
  `/data/data/<pkg>/files`). But `app.path()` needs the `App`/`AppHandle`,
  which doesn't exist until the builder runs — current code resolves `home`
  BEFORE the builder. ⇒ Defer construction into `.setup()` on mobile.
- Seed corpus: on Android `resource_dir()` returns `asset://localhost/` (NOT
  `std::fs`-readable). The desktop `.app/Contents/Resources` mechanism cannot
  apply. ⇒ On Android, resolve a real staged seed dir under app data
  (`<app_data>/seed`); if absent, skip gracefully (KB stays empty — existing
  behaviour). Document `adb push` staging.
- QNN libs: `GenieLibrary::open` dlopens `<qairt_lib_dir>/libGenie.so` by full
  path. On Android the 9 QAIRT `.so`s ship in the APK `lib/arm64-v8a/`
  (extracted to `nativeLibraryDir`), reachable by the system linker via
  basename. ⇒ On Android, an empty `qairt_lib_dir` ⇒ dlopen `libGenie.so` by
  basename (linker finds it + its DT_NEEDED deps in nativeLibraryDir), making
  `qnn_qairt_lib_dir` unnecessary on-device.

## Work items (TDD)

1. **primer-engine: `resolve_qairt_lib_dir(explicit, bundle_dir)`** — pure,
   cfg-aware. Android: `explicit.unwrap_or_default()` (empty ⇒ basename load).
   Desktop: `explicit.unwrap_or_else(|| default_qairt_lib_dir(bundle))`.
   Use it in the qnn build arm. Tests per-cfg.
2. **primer-qnn-sys: Android dlopen-by-basename** — pure helper
   `genie_load_target(qairt_lib_dir) -> LoadTarget` (empty ⇒ `Basename`,
   else ⇒ `Path(dir/libGenie.so)`); `open_impl` matches. Tests for both.
3. **primer-gui: per-platform base dir + seed** — `paths::mobile_seed_dir(app_data)`
   pure helper; `run()` cfg-split: desktop unchanged (manage before builder),
   mobile defers `home`/config/manage/seed into `.setup()` via `app.path()`.
4. **Device (sub-project 3):** stage QAIRT libs + seed + bundle, build qnn APK,
   install, confirm `QnnBackend` constructs against the on-device bundle.
5. **Device (sub-project 4, stretch):** first NPU token in logcat.

## Acceptance

- Host: `cargo test -p primer-gui -p primer-engine -p primer-qnn-sys` green;
  `cargo build --target aarch64-linux-android -p primer-gui --features qnn`
  (no-default-features) green; clippy clean.
- Device: installed qnn APK launches, writes config/session under app data,
  constructs `QnnBackend` from an app-readable bundle (sub-project 3).
