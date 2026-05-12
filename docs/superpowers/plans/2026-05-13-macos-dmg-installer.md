# macOS DMG installer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Produce a signed and notarized macOS `.dmg` installer for `primer-gui` so Horst can hand it to evaluators with zero Gatekeeper friction. Spec at [docs/superpowers/specs/2026-05-13-macos-dmg-installer-design.md](../specs/2026-05-13-macos-dmg-installer-design.md).

**Architecture:** Flip `tauri.conf.json` from bundle-off to bundle-on with Developer ID signing and notarization. Bundle the existing seed JSONL corpus as `.app` Resources. Add a small Rust module that sets `PRIMER_SEED_DIR` at startup when running inside a packaged `.app`, so the existing seed-discovery path finds the bundled corpus without any change to `primer-kb-load`. Drive the build through `scripts/build-dmg.sh`.

**Tech Stack:** Tauri 2 (already in workspace at `version = "2"`), `cargo-tauri` CLI 2.x (one-time install), Apple Developer ID Application certificate (already in login keychain), App Store Connect API key (`.p8`) for `notarytool`.

---

## Prerequisites

Before starting the plan, verify these one-time setups exist. None require code changes; if any is missing, do them once and they're permanent.

- [ ] **P1: `cargo-tauri` CLI installed**

  Check:
  ```bash
  ~/.cargo/bin/cargo tauri --version
  ```
  If missing:
  ```bash
  ~/.cargo/bin/cargo install tauri-cli --version "^2.0"
  ```
  Expected: prints `tauri-cli 2.x.y`.

- [ ] **P2: Developer ID Application cert visible in login keychain**

  Check:
  ```bash
  security find-identity -p codesigning -v | grep "Developer ID Application: Horst Herb (X5DWXB4283)"
  ```
  Expected: one matching line. If missing, regenerate at developer.apple.com → Certificates → + → Developer ID Application.

- [ ] **P3: App Store Connect API key for notarization**

  The key is a `.p8` file with two associated IDs (Issuer ID, Key ID). If you already use one for App Store submission, the same key works for `notarytool`. If not, create at appstoreconnect.apple.com → Users and Access → Keys → + → role "Developer". Save the `.p8` somewhere stable (e.g. `~/.appstoreconnect/AuthKey_XXXXXX.p8`) and note the two IDs.

  Export the three env vars in your shell profile (`.zshrc` / `.bashrc`):
  ```bash
  export APPLE_API_ISSUER="<Issuer ID>"
  export APPLE_API_KEY="<Key ID>"
  export APPLE_API_KEY_PATH="$HOME/.appstoreconnect/AuthKey_XXXXXX.p8"
  ```
  Reload the shell. Verify with `echo $APPLE_API_KEY`.

- [ ] **P4: Branch for the implementation work**

  Run from repo root:
  ```bash
  git switch -c feat/macos-dmg-installer
  ```
  All commits in this plan land on this branch; the final PR is opened against `main`.

---

## Task 1: Add macOS app icon set

Generates the full icon set from the existing 1024×1024 source asset. The Tauri bundler needs `.icns` for macOS; `cargo tauri icon` derives it (plus Windows `.ico` and Linux PNGs we don't use) from one source PNG.

**Files:**
- Track: `assets/curious_childs_primer_icon.png` (untracked; already exists at 1024×1024 RGBA)
- Create: `src/crates/primer-gui/icons/source.png` (copy of the asset)
- Create: `src/crates/primer-gui/icons/icon.icns`, `icon.ico`, `icon.png`, `32x32.png`, `128x128.png`, `128x128@2x.png`, and various `Square*Logo.png` (generated)

- [ ] **Step 1: Copy the icon source into the GUI crate's icons dir**

  ```bash
  cp /Users/hherb/src/primer/assets/curious_childs_primer_icon.png \
     /Users/hherb/src/primer/src/crates/primer-gui/icons/source.png
  ```

- [ ] **Step 2: Generate the full icon set from the source**

  ```bash
  cd /Users/hherb/src/primer/src/crates/primer-gui
  ~/.cargo/bin/cargo tauri icon icons/source.png
  ```

  Expected output: roughly 10–15 generated files appear under `src/crates/primer-gui/icons/`, including `icon.icns`, `icon.ico`, several `*.png` files, and an `iOS/`/`android/` subdir (we leave them; they're harmless and may be useful later).

- [ ] **Step 3: Verify the .icns is well-formed**

  ```bash
  file /Users/hherb/src/primer/src/crates/primer-gui/icons/icon.icns
  ```
  Expected: `Mac OS X icon, 1024 x 1024, ...`

- [ ] **Step 4: Stage and commit**

  ```bash
  git -C /Users/hherb/src/primer add \
      assets/curious_childs_primer_icon.png \
      src/crates/primer-gui/icons/
  git -C /Users/hherb/src/primer commit -m "$(cat <<'EOF'
  chore(gui): add macOS app icon set

  Generated from assets/curious_childs_primer_icon.png via
  cargo tauri icon. Used by the upcoming Tauri DMG bundle.
  EOF
  )"
  ```

---

## Task 2: Add `paths` module with `resolve_packaged_seed_dir` (TDD)

A pure Rust function that detects whether the current executable is running inside a macOS `.app` bundle, and if so walks `Contents/Resources/` to find the directory containing the bundled seed `*.jsonl` files. Returns `None` for dev builds so the existing `CARGO_MANIFEST_DIR` fallback in `primer-kb-load` continues to fire.

**Files:**
- Create: `src/crates/primer-gui/src/paths.rs`
- Modify: `src/crates/primer-gui/src/lib.rs` (add `pub mod paths;`)

- [ ] **Step 1: Add the `paths` module declaration to `lib.rs`**

  Edit [src/crates/primer-gui/src/lib.rs](../../../src/crates/primer-gui/src/lib.rs), insert `pub mod paths;` after the existing `pub mod` lines so the new module is visible:

  Before:
  ```rust
  pub mod commands;
  pub mod config;
  pub mod state;
  pub mod types;
  pub mod validation;
  pub mod wiring;
  ```
  After:
  ```rust
  pub mod commands;
  pub mod config;
  pub mod paths;
  pub mod state;
  pub mod types;
  pub mod validation;
  pub mod wiring;
  ```

- [ ] **Step 2: Write the failing test file** at `src/crates/primer-gui/src/paths.rs`

  ```rust
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
  pub fn resolve_packaged_seed_dir(_exe_path: &Path) -> Option<PathBuf> {
      todo!()
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
  ```

- [ ] **Step 3: Run the tests, verify they fail**

  ```bash
  cd /Users/hherb/src/primer/src
  ~/.cargo/bin/cargo test -p primer-gui --lib paths::tests -- --nocapture
  ```
  Expected: 5 tests panic on `not yet implemented` (the `todo!()` in `resolve_packaged_seed_dir`).

- [ ] **Step 4: Replace the `todo!()` with the real implementation**

  Replace the body of `resolve_packaged_seed_dir` in `src/crates/primer-gui/src/paths.rs`:

  ```rust
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
  ```

- [ ] **Step 5: Run the tests, verify they pass**

  ```bash
  cd /Users/hherb/src/primer/src
  ~/.cargo/bin/cargo test -p primer-gui --lib paths::tests
  ```
  Expected: `test result: ok. 5 passed; 0 failed; ...`

- [ ] **Step 6: Run clippy on the new module**

  ```bash
  cd /Users/hherb/src/primer/src
  ~/.cargo/bin/cargo clippy -p primer-gui --all-targets -- -D warnings
  ```
  Expected: no warnings, no errors.

- [ ] **Step 7: Commit**

  ```bash
  git -C /Users/hherb/src/primer add \
      src/crates/primer-gui/src/paths.rs \
      src/crates/primer-gui/src/lib.rs
  git -C /Users/hherb/src/primer commit -m "$(cat <<'EOF'
  feat(gui): resolve packaged-app seed dir

  New paths::resolve_packaged_seed_dir walks the .app bundle's
  Contents/Resources/ tree (DFS, max depth 8) and returns the first
  directory containing a *.jsonl file. Returns None for dev builds
  so the existing CARGO_MANIFEST_DIR fallback continues to fire.

  Used in a follow-up commit to set PRIMER_SEED_DIR before engine init.
  EOF
  )"
  ```

---

## Task 3: Wire `paths` into `run()` so `PRIMER_SEED_DIR` is set in packaged builds

`lib.rs::run()` is the GUI's entry point. Set `PRIMER_SEED_DIR` from the resolved packaged-app path before `state::AppState::new` (which is what eventually triggers `auto_seed_if_empty` deep inside the engine).

**Files:**
- Modify: `src/crates/primer-gui/src/lib.rs:30-49` (the `run()` function body)

- [ ] **Step 1: Add a helper in `paths.rs` that wraps the env-var write**

  Append to `src/crates/primer-gui/src/paths.rs` (after the `find_jsonl_dir` function, before `#[cfg(test)]`):

  ```rust
  /// If we can resolve a packaged seed dir from the current executable,
  /// set `PRIMER_SEED_DIR` so the engine's `auto_seed_if_empty` picks
  /// it up. Safe to call when not in a `.app` — no env mutation happens
  /// in that case.
  pub fn set_packaged_seed_dir_if_present() {
      let Ok(exe) = std::env::current_exe() else { return };
      let Some(dir) = resolve_packaged_seed_dir(&exe) else { return };
      // SAFETY: called once at startup before any threads are spawned;
      // the Tauri runtime has not yet been built. Edition 2024 marks
      // set_var as unsafe because it's not thread-safe.
      unsafe {
          std::env::set_var("PRIMER_SEED_DIR", &dir);
      }
      tracing::info!(seed_dir = %dir.display(), "resolved packaged seed dir");
  }
  ```

- [ ] **Step 2: Call the helper at the top of `run()`**

  Edit `src/crates/primer-gui/src/lib.rs`. Inside `pub fn run() -> Result<(), Box<dyn std::error::Error>> {`, change the first line from `init_tracing();` to:

  ```rust
      init_tracing();
      paths::set_packaged_seed_dir_if_present();
  ```

  Tracing initialises first so the `tracing::info!` inside `set_packaged_seed_dir_if_present` actually lands somewhere.

- [ ] **Step 3: Verify it still builds and the existing tests still pass**

  ```bash
  cd /Users/hherb/src/primer/src
  ~/.cargo/bin/cargo build -p primer-gui
  ~/.cargo/bin/cargo test -p primer-gui
  ```
  Expected: builds cleanly; all existing primer-gui tests pass plus the 5 new `paths::tests`.

- [ ] **Step 4: Smoke-test the dev path (no .app, `PRIMER_SEED_DIR` stays unset)**

  ```bash
  cd /Users/hherb/src/primer/src
  RUST_LOG=info ~/.cargo/bin/cargo run -p primer-gui 2>&1 | head -30
  ```
  Expected: GUI window opens. **No** `resolved packaged seed dir` line in stderr — confirming the function correctly returns `None` for a dev-tree exe. Close the window with `cmd-Q` to end.

- [ ] **Step 5: Commit**

  ```bash
  git -C /Users/hherb/src/primer add \
      src/crates/primer-gui/src/paths.rs \
      src/crates/primer-gui/src/lib.rs
  git -C /Users/hherb/src/primer commit -m "$(cat <<'EOF'
  feat(gui): set PRIMER_SEED_DIR in packaged builds

  When primer-gui runs inside a macOS .app, resolve the bundled
  seed dir and set PRIMER_SEED_DIR before AppState construction so
  auto_seed_if_empty finds the bundled corpus. No-op for cargo run.
  EOF
  )"
  ```

---

## Task 4: Enable Tauri bundle config (unsigned dry-run)

Flip `bundle.active` to true and configure metadata, but leave `signingIdentity` empty for this task so a build failure surfaces as a *bundling* issue, not a *signing* issue. Run `cargo tauri build` and verify an unsigned DMG is produced and contains the bundled seed files.

**Files:**
- Modify: `src/crates/primer-gui/tauri.conf.json`

- [ ] **Step 1: Replace the `bundle` block** in [src/crates/primer-gui/tauri.conf.json](../../../src/crates/primer-gui/tauri.conf.json)

  Replace the existing `"bundle": { ... }` block (lines 28–34 in the current file) with:

  ```jsonc
    "bundle": {
      "active": true,
      "targets": ["dmg", "app"],
      "icon": [
        "icons/32x32.png",
        "icons/128x128.png",
        "icons/128x128@2x.png",
        "icons/icon.icns",
        "icons/icon.ico"
      ],
      "resources": ["../../../../data/seed/*.jsonl"],
      "copyright": "© 2026 Horst Herb",
      "category": "public.app-category.education",
      "shortDescription": "A Socratic AI learning companion for children",
      "longDescription": "The Primer is an offline-friendly Socratic learning companion. It asks more questions than it answers, catches parroting, suggests breaks, and remembers what was discussed across sessions.",
      "macOS": {
        "minimumSystemVersion": "11.0"
      }
    }
  ```

  Note: `signingIdentity` and `entitlements` are deliberately absent — Task 6 adds them.

- [ ] **Step 2: Build the unsigned DMG**

  ```bash
  cd /Users/hherb/src/primer
  ~/.cargo/bin/cargo tauri build \
      --manifest-path src/crates/primer-gui/Cargo.toml \
      --bundles dmg \
      --target aarch64-apple-darwin
  ```

  Expected: build succeeds. Final lines mention `Bundling Primer_0.1.0_aarch64.dmg` and an output path like `src/target/aarch64-apple-darwin/release/bundle/dmg/Primer_0.1.0_aarch64.dmg`.

  **If the upward-glob in `resources` is rejected** (error mentions invalid resource path, or the build succeeds but the DMG contains no `*.jsonl` files), fall back to a `build.rs` copy. See "Fallback A" below before continuing.

- [ ] **Step 3: Mount the DMG and inspect its contents**

  ```bash
  open /Users/hherb/src/primer/src/target/aarch64-apple-darwin/release/bundle/dmg/Primer_0.1.0_aarch64.dmg
  ls -la "/Volumes/Primer/Primer.app/Contents/Resources/" | head -40
  find "/Volumes/Primer/Primer.app/Contents/Resources/" -name "*.jsonl" -type f
  ```

  Expected: `find` lists at least `seed_passages.en.jsonl` and `wiki_passages.en.jsonl` (and Klexikon DE variants). Note their parent directory — that's the path `resolve_packaged_seed_dir` will return at runtime.

- [ ] **Step 4: Drag-install the app and launch it**

  ```bash
  cp -R "/Volumes/Primer/Primer.app" /Applications/
  hdiutil detach "/Volumes/Primer"
  ```

  Right-click `/Applications/Primer.app` → Open (this is the *unsigned* path — Gatekeeper will warn; confirm Open). The session-picker screen should appear. Open Settings, pick the `stub` backend (no API key needed for this smoke test), close Settings, start a new session, type "what is the sun" — the stub canned response should reference one of the seed passages, confirming the bundled corpus was loaded.

- [ ] **Step 5: Tail the unified log for the seed-dir line**

  In a second terminal *before* launching the app:
  ```bash
  log stream --predicate 'process == "primer-gui"' --info | grep -i seed
  ```
  Launch the app. Expected line: `resolved packaged seed dir` with a path under `/Applications/Primer.app/Contents/Resources/`. Quit the app, kill the log stream.

  If the line doesn't appear, the seed-dir discovery is broken — debug before continuing.

- [ ] **Step 6: Remove the test install**

  ```bash
  rm -rf /Applications/Primer.app
  ```

- [ ] **Step 7: Commit**

  ```bash
  git -C /Users/hherb/src/primer add src/crates/primer-gui/tauri.conf.json
  git -C /Users/hherb/src/primer commit -m "$(cat <<'EOF'
  feat(gui): enable Tauri bundle config (unsigned dry-run)

  Flips bundle.active, narrows targets to dmg+app, bundles the
  data/seed/*.jsonl corpus, and sets app metadata. Signing is added
  in a follow-up commit.
  EOF
  )"
  ```

### Fallback A: `build.rs` resource copy (only if Step 2 of Task 4 rejects the upward glob)

If `bundle.resources = ["../../../../data/seed/*.jsonl"]` is rejected by the bundler, switch to copying the seed files into a crate-local `resources/` dir via the existing `build.rs`:

1. Edit `src/crates/primer-gui/build.rs` and append:
   ```rust
   fn main() {
       tauri_build::build();
       copy_seed_resources();
   }

   fn copy_seed_resources() {
       use std::path::PathBuf;
       let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
       let src = manifest
           .ancestors()
           .nth(4)
           .expect("CARGO_MANIFEST_DIR has at least 4 ancestors")
           .join("data/seed");
       let dst = manifest.join("resources/seed");
       if dst.exists() {
           std::fs::remove_dir_all(&dst).expect("clean resources/seed");
       }
       std::fs::create_dir_all(&dst).expect("create resources/seed");
       for entry in std::fs::read_dir(&src).expect("read data/seed") {
           let entry = entry.unwrap();
           let path = entry.path();
           if path.extension().is_some_and(|e| e == "jsonl") {
               let target = dst.join(path.file_name().unwrap());
               std::fs::copy(&path, &target).expect("copy jsonl");
               println!("cargo:rerun-if-changed={}", path.display());
           }
       }
   }
   ```
   (If the existing `build.rs` already calls `tauri_build::build();` standalone, replace its `main()` accordingly.)
2. Add `/resources/` to `src/crates/primer-gui/.gitignore` (create the file if absent).
3. Change `tauri.conf.json`:
   ```jsonc
     "resources": ["resources/seed/*.jsonl"],
   ```
4. Re-run Task 4 Step 2 onward.

---

## Task 5: Add code signing and entitlements

Add the Developer ID Application signing identity and a minimal entitlements file so the bundled `.app` is signed and ready for notarization. Skips notarization itself — that's Task 7.

**Files:**
- Create: `src/crates/primer-gui/entitlements.plist`
- Modify: `src/crates/primer-gui/tauri.conf.json` (macOS sub-block)

- [ ] **Step 1: Create the entitlements file** at `src/crates/primer-gui/entitlements.plist`

  ```xml
  <?xml version="1.0" encoding="UTF-8"?>
  <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
  <plist version="1.0">
  <dict>
      <key>com.apple.security.cs.allow-jit</key>
      <true/>
      <key>com.apple.security.cs.allow-unsigned-executable-memory</key>
      <true/>
  </dict>
  </plist>
  ```

- [ ] **Step 2: Add signing identity and entitlements to `tauri.conf.json`**

  Replace the `bundle.macOS` block in `src/crates/primer-gui/tauri.conf.json` with:

  ```jsonc
        "macOS": {
          "minimumSystemVersion": "11.0",
          "signingIdentity": "Developer ID Application: Horst Herb (X5DWXB4283)",
          "entitlements": "entitlements.plist"
        }
  ```

- [ ] **Step 3: Build the signed (but not notarized) DMG**

  Notarization is automatic when the env vars are set; since we're testing signing in isolation, temporarily unset them:
  ```bash
  cd /Users/hherb/src/primer
  env -u APPLE_API_ISSUER -u APPLE_API_KEY -u APPLE_API_KEY_PATH \
      ~/.cargo/bin/cargo tauri build \
      --manifest-path src/crates/primer-gui/Cargo.toml \
      --bundles dmg \
      --target aarch64-apple-darwin
  ```
  Expected: build succeeds. Toward the end, lines like `Signing /…/Primer.app/...` appear. The notarization step is skipped because the env vars are unset.

- [ ] **Step 4: Verify the signature**

  ```bash
  codesign --verify --deep --strict --verbose=4 \
      /Users/hherb/src/primer/src/target/aarch64-apple-darwin/release/bundle/macos/Primer.app
  ```
  Expected: `…/Primer.app: valid on disk` and `…/Primer.app: satisfies its Designated Requirement`.

  ```bash
  codesign --display --verbose=4 \
      /Users/hherb/src/primer/src/target/aarch64-apple-darwin/release/bundle/macos/Primer.app 2>&1 | grep -E "Authority|TeamIdentifier"
  ```
  Expected: `Authority=Developer ID Application: Horst Herb (X5DWXB4283)` and `TeamIdentifier=X5DWXB4283`.

  Gatekeeper at this point still rejects the app (signed but not notarized):
  ```bash
  spctl -a -t open --context context:primary-signature -vv \
      /Users/hherb/src/primer/src/target/aarch64-apple-darwin/release/bundle/macos/Primer.app
  ```
  Expected: `rejected (the code is signed but not notarized)`. This is correct — Task 7 fixes it.

- [ ] **Step 5: Commit**

  ```bash
  git -C /Users/hherb/src/primer add \
      src/crates/primer-gui/entitlements.plist \
      src/crates/primer-gui/tauri.conf.json
  git -C /Users/hherb/src/primer commit -m "$(cat <<'EOF'
  feat(gui): Developer ID code signing + entitlements

  Signs Primer.app with the Developer ID Application cert (Team
  X5DWXB4283) and adds the JIT/unsigned-executable-memory
  entitlements wry/WebKit needs at runtime under the hardened
  runtime. Notarization is wired in via env vars in a follow-up.
  EOF
  )"
  ```

---

## Task 6: Add the `scripts/build-dmg.sh` driver

Single-purpose build script with prerequisite checks. Lives at repo root since it's project tooling.

**Files:**
- Create: `scripts/build-dmg.sh`

- [ ] **Step 1: Create the directory if missing**

  ```bash
  mkdir -p /Users/hherb/src/primer/scripts
  ```

- [ ] **Step 2: Write the script** at `/Users/hherb/src/primer/scripts/build-dmg.sh`

  ```bash
  #!/usr/bin/env bash
  set -euo pipefail

  # Build a signed and notarized macOS DMG for primer-gui.
  #
  # Prerequisites:
  #   - rustup-installed cargo at ~/.cargo/bin/cargo
  #   - cargo-tauri CLI 2.x (`cargo install tauri-cli --version "^2.0"`)
  #   - Developer ID Application: Horst Herb (X5DWXB4283) in login keychain
  #   - APPLE_API_ISSUER, APPLE_API_KEY, APPLE_API_KEY_PATH env vars
  #
  # Output: src/target/aarch64-apple-darwin/release/bundle/dmg/Primer_*.dmg

  command -v ~/.cargo/bin/cargo >/dev/null 2>&1 || {
      echo "error: ~/.cargo/bin/cargo not found; install via rustup" >&2
      exit 1
  }

  if ! ~/.cargo/bin/cargo tauri --version >/dev/null 2>&1; then
      echo "error: cargo-tauri not installed" >&2
      echo "  install with: ~/.cargo/bin/cargo install tauri-cli --version '^2.0'" >&2
      exit 1
  fi

  if ! security find-identity -p codesigning -v 2>/dev/null \
      | grep -q "Developer ID Application: Horst Herb (X5DWXB4283)"; then
      echo "error: Developer ID Application cert not in login keychain" >&2
      echo "  create at developer.apple.com -> Certificates -> + -> Developer ID Application" >&2
      exit 1
  fi

  for var in APPLE_API_ISSUER APPLE_API_KEY APPLE_API_KEY_PATH; do
      if [ -z "${!var:-}" ]; then
          echo "error: $var not set" >&2
          echo "  see README 'Building the macOS DMG' for setup" >&2
          exit 1
      fi
  done

  if [ ! -r "$APPLE_API_KEY_PATH" ]; then
      echo "error: APPLE_API_KEY_PATH not readable: $APPLE_API_KEY_PATH" >&2
      exit 1
  fi

  repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
  cd "$repo_root"

  ~/.cargo/bin/cargo tauri build \
      --manifest-path src/crates/primer-gui/Cargo.toml \
      --bundles dmg \
      --target aarch64-apple-darwin

  dmg_path="src/target/aarch64-apple-darwin/release/bundle/dmg/Primer_0.1.0_aarch64.dmg"
  if [ ! -f "$dmg_path" ]; then
      echo "error: expected DMG not produced at $dmg_path" >&2
      exit 1
  fi

  echo
  echo "Built and notarized: $repo_root/$dmg_path"
  echo "Test the install on another Mac before distributing."
  ```

- [ ] **Step 3: Make the script executable**

  ```bash
  chmod +x /Users/hherb/src/primer/scripts/build-dmg.sh
  ```

- [ ] **Step 4: Smoke-test the prereq checks**

  Verify the script fails clearly when each env var is missing. Run three times, each time un-setting one var:
  ```bash
  cd /Users/hherb/src/primer
  env -u APPLE_API_ISSUER ./scripts/build-dmg.sh 2>&1 | head -2
  env -u APPLE_API_KEY ./scripts/build-dmg.sh 2>&1 | head -2
  env -u APPLE_API_KEY_PATH ./scripts/build-dmg.sh 2>&1 | head -2
  ```
  Expected (each): `error: APPLE_… not set` followed by the README pointer. Exit code 1.

- [ ] **Step 5: Commit**

  ```bash
  git -C /Users/hherb/src/primer add scripts/build-dmg.sh
  git -C /Users/hherb/src/primer commit -m "$(cat <<'EOF'
  feat(gui): scripts/build-dmg.sh end-to-end builder

  Single-purpose script that verifies prerequisites (cargo-tauri,
  Developer ID cert, notarization env vars) then runs the full
  signed + notarized build via cargo tauri build.
  EOF
  )"
  ```

---

## Task 7: End-to-end signed + notarized build

Run the script with notarization credentials present. Verify the resulting DMG is fully signed, notarized, and stapled.

**Files:** None modified. Verification only.

- [ ] **Step 1: Run the full build**

  ```bash
  cd /Users/hherb/src/primer
  ./scripts/build-dmg.sh
  ```
  Expected: build runs to completion. Toward the end, lines mention `Notarizing` and `Stapling` with a submission ID. Wall-clock: typically 3–10 minutes (notary turnaround varies).

  Output path printed at the end matches `src/target/aarch64-apple-darwin/release/bundle/dmg/Primer_0.1.0_aarch64.dmg`.

- [ ] **Step 2: Verify the notary stamp is stapled**

  ```bash
  xcrun stapler validate \
      /Users/hherb/src/primer/src/target/aarch64-apple-darwin/release/bundle/dmg/Primer_0.1.0_aarch64.dmg
  ```
  Expected: `The validate action worked!`

- [ ] **Step 3: Verify Gatekeeper accepts the bundled .app**

  ```bash
  hdiutil attach /Users/hherb/src/primer/src/target/aarch64-apple-darwin/release/bundle/dmg/Primer_0.1.0_aarch64.dmg
  spctl -a -t open --context context:primary-signature -vv /Volumes/Primer/Primer.app
  ```
  Expected: `source=Notarized Developer ID` and `accepted`. Detach:
  ```bash
  hdiutil detach /Volumes/Primer
  ```

- [ ] **Step 4: No commit**

  This task verifies, doesn't change the tree.

  **If notarization rejected** (the bundler will print a submission ID and "Invalid"): retrieve the log with
  ```bash
  xcrun notarytool log <submission-id> \
      --key "$APPLE_API_KEY_PATH" \
      --key-id "$APPLE_API_KEY" \
      --issuer "$APPLE_API_ISSUER"
  ```
  The JSON `issues` array names specific files and reasons. Typical remediation: add `com.apple.security.cs.disable-library-validation` to `entitlements.plist` if a bundled dylib is rejected. Make the fix, commit a "fix(gui): …" amendment, re-run Task 7 Step 1.

---

## Task 8: Smoke test on a clean profile

Verify a non-developer experience — no Gatekeeper warning, app launches, seed corpus loads, settings work.

**Files:** None modified. Verification only.

Two options for "clean profile": (a) another physical Mac if available, or (b) a second user account on the same Mac. Option (b) is sufficient — Gatekeeper's first-launch check is per-user, not per-machine. Pick whichever is available.

- [ ] **Step 1: Transfer the DMG to the clean profile**

  Either:
  - Copy via AirDrop / a shared folder to another Mac, or
  - Switch to a second user account on the same Mac (System Settings → Users & Groups → +). Copy the DMG to that account's Downloads via Shared.

- [ ] **Step 2: Install the app**

  In the clean profile: double-click the DMG → drag `Primer.app` to Applications → eject the DMG.

- [ ] **Step 3: Launch and confirm zero Gatekeeper friction**

  Double-click `/Applications/Primer.app`. Expected: app opens directly into the session-picker screen. **No** "cannot be opened because it is from an unidentified developer" warning. **No** "Apple cannot check for malicious software" warning.

  If Gatekeeper warns, notarization or stapling failed silently — re-run Task 7 verification.

- [ ] **Step 4: End-to-end conversation smoke test**

  Open Settings. Pick the `cloud` backend, paste a valid `ANTHROPIC_API_KEY`. Close Settings. Start a new session as `TestChild`, age 8. Type "what is the sun?". Expected: streaming response that draws on a bundled passage (the seed corpus loaded from inside `.app/Contents/Resources/`). Quit, re-launch — the past session appears in the picker; resume works.

- [ ] **Step 5: No commit**

  Verification only.

---

## Task 9: README and .gitignore

Document how to build the DMG so the prereqs aren't lost to memory, and prevent Tauri's macOS codegen artifacts from polluting `git status`.

**Files:**
- Modify: `README.md`
- Modify: `.gitignore`

- [ ] **Step 1: Append the build-dmg section to README**

  Open [README.md](../../../README.md). Find the existing "Running" section. After it (before the next top-level section), insert:

  ```markdown
  ## Building the macOS DMG

  Produces a signed and notarized `.dmg` for the desktop GUI, ready to
  hand to evaluators. Apple Silicon only.

  **One-time prerequisites:**

  - Install the Tauri 2 CLI:
    ```bash
    ~/.cargo/bin/cargo install tauri-cli --version "^2.0"
    ```
  - Have a `Developer ID Application` certificate from Apple's Developer
    Program in your login keychain. Verify with
    `security find-identity -p codesigning -v` — you should see a line
    matching `Developer ID Application: <Your Name> (TEAMID)`. If
    missing, create at developer.apple.com → Certificates → + →
    Developer ID Application.
  - Create an App Store Connect API key with the "Developer" role at
    appstoreconnect.apple.com → Users and Access → Keys → +. Download
    the `.p8` file (you only get one chance) and note the Key ID and
    Issuer ID. Export in your shell profile:
    ```bash
    export APPLE_API_ISSUER="<Issuer ID>"
    export APPLE_API_KEY="<Key ID>"
    export APPLE_API_KEY_PATH="$HOME/.appstoreconnect/AuthKey_XXXXXX.p8"
    ```

  **Build:**
  ```bash
  ./scripts/build-dmg.sh
  ```

  Output:
  `src/target/aarch64-apple-darwin/release/bundle/dmg/Primer_0.1.0_aarch64.dmg`.
  Notarization typically takes 3–10 minutes; the script blocks until
  stapling completes.

  **Installing on an evaluator's Mac:** double-click the DMG, drag
  `Primer.app` to Applications, launch — no Gatekeeper warning
  expected. The notary stamp is stapled to the bundle, so Gatekeeper
  accepts it offline.

  **Updating the app icon:** the source is
  [assets/curious_childs_primer_icon.png](assets/curious_childs_primer_icon.png).
  Regenerate the full set with:
  ```bash
  cp assets/curious_childs_primer_icon.png src/crates/primer-gui/icons/source.png
  cd src/crates/primer-gui
  ~/.cargo/bin/cargo tauri icon icons/source.png
  ```
  ```

- [ ] **Step 2: Add Tauri codegen artifacts to `.gitignore`**

  Check whether `src/target/` is already ignored (it should be):
  ```bash
  cd /Users/hherb/src/primer && git check-ignore src/target/foo 2>&1
  ```
  Expected: `src/target/foo` (means it's ignored). If not ignored, append `/src/target/` to the root `.gitignore`.

  Then append to the root [.gitignore](../../../.gitignore):
  ```
  # Tauri 2 macOS codegen artifacts
  /src/crates/primer-gui/gen/apple/
  ```

- [ ] **Step 3: Confirm the working tree is clean except for the changes**

  ```bash
  git -C /Users/hherb/src/primer status
  ```
  Expected: only `README.md` and `.gitignore` show as modified.

- [ ] **Step 4: Commit**

  ```bash
  git -C /Users/hherb/src/primer add README.md .gitignore
  git -C /Users/hherb/src/primer commit -m "$(cat <<'EOF'
  docs(gui): document macOS DMG build pipeline

  Adds "Building the macOS DMG" to the README covering prerequisites,
  the one-line build command, and the icon-regeneration workflow.
  Gitignores Tauri 2's macOS codegen artifacts.
  EOF
  )"
  ```

---

## Task 10: Open the PR

- [ ] **Step 1: Push the branch**

  ```bash
  git -C /Users/hherb/src/primer push -u origin feat/macos-dmg-installer
  ```

- [ ] **Step 2: Create the PR**

  ```bash
  cd /Users/hherb/src/primer
  gh pr create --title "feat(gui): macOS DMG installer (signed + notarized)" --body "$(cat <<'EOF'
  ## Summary
  - Signed and notarized macOS DMG for `primer-gui`, ready for private evaluator distribution
  - Bundles the existing seed corpus into `Primer.app/Contents/Resources/` and sets `PRIMER_SEED_DIR` at startup so the engine finds it
  - `scripts/build-dmg.sh` drives the full build with prereq checks; notarization credentials come from env vars (App Store Connect API key)

  Spec: [docs/superpowers/specs/2026-05-13-macos-dmg-installer-design.md](docs/superpowers/specs/2026-05-13-macos-dmg-installer-design.md)
  Plan: [docs/superpowers/plans/2026-05-13-macos-dmg-installer.md](docs/superpowers/plans/2026-05-13-macos-dmg-installer.md)

  ## Test plan
  - [x] `cargo test -p primer-gui` (5 new `paths::tests` + existing tests pass)
  - [x] `cargo clippy -p primer-gui --all-targets -- -D warnings` clean
  - [x] `cargo tauri build` produces a signed `Primer.app` (verified via `codesign --verify`)
  - [x] `./scripts/build-dmg.sh` produces a notarized DMG (verified via `xcrun stapler validate` and `spctl -a -t open`)
  - [x] DMG installs on a clean profile with no Gatekeeper warning
  - [x] First-launch session-picker → Settings → conversation works end-to-end with the bundled seed corpus
  EOF
  )"
  ```

  Expected: `gh` prints a URL to the new PR.

- [ ] **Step 3: No commit** — PR is opened.

---

## Self-review against the spec

Coverage check against [the spec](../specs/2026-05-13-macos-dmg-installer-design.md)'s seven touch points:

| Spec touch point | Plan task |
|---|---|
| 1. `tauri.conf.json` bundle config | Task 4 (unsigned), Task 5 (signing) |
| 2. `entitlements.plist` | Task 5 |
| 3. `main.rs` packaged-app path | Tasks 2 + 3 (split: paths module + wiring) |
| 4. Icons | Task 1 |
| 5. `scripts/build-dmg.sh` | Task 6 |
| 6. `README.md` updates | Task 9 |
| 7. `.gitignore` additions | Task 9 |

Spec-mentioned fallback (build.rs resource copy if upward glob is rejected) is covered in "Fallback A" of Task 4.

Spec-mentioned testing layers:
- Unit test for `resolve_packaged_seed_dir` → Task 2 (5 tests)
- Smoke test (build) → Tasks 4–6
- Smoke test (install on clean profile) → Task 8

All spec requirements have at least one task.

No placeholders, no "TBD"s. Function names consistent across tasks (`resolve_packaged_seed_dir`, `set_packaged_seed_dir_if_present`, `find_jsonl_dir`). Cert fingerprint and team ID match across all tasks: `Developer ID Application: Horst Herb (X5DWXB4283)`.
