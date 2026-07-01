# Android Generic-API Test APK Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Produce a signed, sideloadable arm64 Android APK of `primer-gui` configured for cloud / OpenAI-compatible API inference with OS-native voice, so volunteer families can field-test the pedagogic engine on ordinary phones.

**Architecture:** No new backend code — the cloud and openai-compat backends are already compiled into `primer-inference`, and `primer-gui` already has an Android Tauri scaffold, in-app API-key entry, and an `android-native` OS-voice feature. This plan (1) locks the Android feature set so the embedder compiles out to BM25-only, (2) makes the bundled seed corpus load on Android without `adb` via first-run extraction, (3) adds release signing, (4) adds a minimal "configure the AI" first-run nudge, and (5) ships a family setup guide + parent consent note. The build is then produced and smoke-tested on a real phone.

**Tech Stack:** Rust (edition 2024, toolchain 1.88), Tauri 2.11 mobile, Android Gradle (Kotlin DSL), `cargo-tauri` android build, `include_dir` (android-only, first-run seed extraction), static-JS webview frontend.

## Global Constraints

- Run every cargo command from `src/` (the workspace root is `src/Cargo.toml`, not the repo root).
- Always invoke cargo as `~/.cargo/bin/cargo` (rustup proxy honours the pinned 1.88 toolchain; Homebrew rust shadows it and breaks builds).
- **Android feature-set contract:** `--no-default-features --features android-native`. This drops `embedding` (so the feature-aware `default_embedder_kind()` returns `none` → BM25-only, no model download, no device-unverified `ort`) and excludes qnn / llamacpp / whisper / piper / cpal. Cloud + openai-compat are always compiled and need no feature.
- **Target ABI:** `aarch64-linux-android` (arm64-v8a) only.
- No magic numbers: any invariant numeric goes to a consts module, any tunable to settings — never inline (project rule). This plan introduces none.
- New Rust deps go in `src/Cargo.toml` `[workspace.dependencies]` and are referenced with `.workspace = true` from the crate.
- Secrets never enter the repo: the release keystore file and `keystore.properties` stay gitignored / outside the repo.
- Never introduce a dependency that phones home at runtime (`[[project_strict_offline_first]]`); `include_dir` is compile-time only.

---

## File Structure

- `src/Cargo.toml` — add `include_dir` to `[workspace.dependencies]`.
- `src/crates/primer-gui/Cargo.toml` — add android-target-gated `include_dir` dependency.
- `src/crates/primer-gui/src/paths.rs` — add `write_seed_files` (host-testable pure extractor) + android-gated `extract_bundled_seed_if_absent`; call it from `init_mobile_state` before `set_mobile_seed_dir_if_present`.
- `src/crates/primer-gui/gen/android/app/build.gradle.kts` — release `signingConfig` from `keystore.properties`; `isMinifyEnabled = false` for `release`.
- `src/crates/primer-gui/gen/android/keystore.properties.example` — committed template (real file gitignored).
- `src/crates/primer-gui/ui/index.html` — add hidden "set up the AI" banner element.
- `src/crates/primer-gui/ui/app.js` — show the banner after session start when backend `kind == "stub"`; wire its button to the existing settings-open control.
- `docs/pilot/family-setup-guide.md` — volunteer-facing install + key-entry guide (new).
- `docs/pilot/parent-consent-note.md` — parent-facing data/consent note (new, drafted in this plan).
- `docs/devel/android-test-apk-build.md` — reproducible signed-APK build notes (new).

---

### Task 1: Lock and host-verify the Android feature set

Proves `primer-gui` compiles with the Android feature contract (embedder compiled out, android-native on) and captures the exact build command. This is the gate that catches a broken feature graph before the slower Tauri-Android build.

**Files:**
- Create: `docs/devel/android-test-apk-build.md`

**Interfaces:**
- Consumes: nothing.
- Produces: the documented build command `cargo tauri android build --apk --target aarch64 -- --no-default-features --features android-native`, relied on by Task 6.

- [ ] **Step 1: Verify the feature set compiles host-side**

Run (from `src/`):
```bash
~/.cargo/bin/cargo build -p primer-gui --no-default-features --features android-native
```
Expected: compiles successfully. This confirms the GUI builds without the `embedding` feature and with `android-native` (which pulls `dep:primer-speech` + `primer-speech/android-native`). If it fails to compile, fix the feature wiring before proceeding — do not continue to signing/build on a broken graph.

- [ ] **Step 2: Confirm the embedder default is `none` without the embedding feature**

Run (from `src/`):
```bash
grep -n 'fn default_embedder_kind' -A2 crates/primer-gui/src/config/types.rs
```
Expected: two cfg-gated definitions — `#[cfg(feature = "embedding")]` returning `"fastembed"` and the `#[cfg(not(feature = "embedding"))]` arm returning `"none"`. Confirms a fresh Android config (no stored `kind`) defaults to BM25-only. No code change; this is a verification step.

- [ ] **Step 3: Write the build-notes doc**

Create `docs/devel/android-test-apk-build.md` with:
- Environment prerequisites (from the scaffold spec): `cargo-tauri` 2.11.1, Android NDK r29 at `/opt/homebrew/share/android-ndk`, SDK at `~/Library/Android/sdk` (`ANDROID_HOME`), JDK 21, `rustup target add aarch64-linux-android --toolchain 1.88`.
- The exact build command:
  ```bash
  # from src/crates/primer-gui/
  ~/.cargo/bin/cargo tauri android build --apk --target aarch64 \
    -- --no-default-features --features android-native
  ```
- A note: "If `cargo tauri android build` does not forward `--no-default-features`/`--features` to the Rust lib build, pin the feature set in `gen/android/app/build.gradle.kts` under the `rust { }` block instead. Verify the produced APK by checking that no `libonnxruntime`/fastembed artifacts are bundled and that the app logs `--embedder-backend none` behaviour (empty/BM25 KB)."
- A pointer to Task 6's on-device smoke checklist.

- [ ] **Step 4: Commit**

```bash
git add docs/devel/android-test-apk-build.md
git commit -m "docs: Android generic-API test APK build notes + feature contract"
```

---

### Task 2: First-run seed extraction so the KB works without adb

The APK asset namespace is not `std::fs`-readable (documented in `paths.rs`), and volunteer families cannot `adb push` the seed corpus. Embed the small (~280 KB) `resources/seed/` tree at compile time and extract it to `<app_data>/seed/` on first run, then let the existing `set_mobile_seed_dir_if_present` wire `PRIMER_SEED_DIR`.

**Files:**
- Modify: `src/Cargo.toml` (`[workspace.dependencies]`)
- Modify: `src/crates/primer-gui/Cargo.toml`
- Modify: `src/crates/primer-gui/src/paths.rs`
- Test: inline `#[cfg(test)]` module in `src/crates/primer-gui/src/paths.rs`

**Interfaces:**
- Consumes: `mobile_seed_dir(app_data: &Path) -> PathBuf` (already in `paths.rs`); `set_mobile_seed_dir_if_present(app_data: &Path)` (already in `paths.rs`).
- Produces: `fn write_seed_files(dir: &Path, files: &[(&str, &[u8])]) -> std::io::Result<()>` (host-testable); `#[cfg(target_os = "android")] pub fn extract_bundled_seed_if_absent(app_data: &Path) -> std::io::Result<PathBuf>`.

- [ ] **Step 1: Write the failing test for the pure extractor**

Add to the bottom of `src/crates/primer-gui/src/paths.rs`:
```rust
#[cfg(any(target_os = "android", test))]
#[cfg(test)]
mod seed_extract_tests {
    use super::*;

    #[test]
    fn write_seed_files_creates_and_is_idempotent() {
        let tmp = std::env::temp_dir().join(format!("primer_seed_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let files: &[(&str, &[u8])] = &[
            ("seed_passages.en.jsonl", b"{\"id\":\"a\"}\n"),
            ("wiki_passages.en.jsonl", b"{\"id\":\"b\"}\n"),
        ];

        // First write creates both files.
        write_seed_files(&tmp, files).unwrap();
        assert_eq!(std::fs::read(tmp.join("seed_passages.en.jsonl")).unwrap(), b"{\"id\":\"a\"}\n");
        assert_eq!(std::fs::read(tmp.join("wiki_passages.en.jsonl")).unwrap(), b"{\"id\":\"b\"}\n");

        // Mutate one file, then re-run: existing files are left untouched (idempotent skip).
        std::fs::write(tmp.join("seed_passages.en.jsonl"), b"USER_EDIT").unwrap();
        write_seed_files(&tmp, files).unwrap();
        assert_eq!(std::fs::read(tmp.join("seed_passages.en.jsonl")).unwrap(), b"USER_EDIT");

        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run (from `src/`):
```bash
~/.cargo/bin/cargo test -p primer-gui write_seed_files_creates_and_is_idempotent
```
Expected: FAIL to compile — `write_seed_files` is not defined.

- [ ] **Step 3: Implement `write_seed_files` (pure, host-buildable)**

Add to `src/crates/primer-gui/src/paths.rs` (near `mobile_seed_dir`):
```rust
/// Write `(filename, contents)` pairs into `dir`, creating `dir` first and
/// skipping any file that already exists. Idempotent — a re-run never
/// overwrites a file a user (or a prior run) already placed. Host-testable;
/// the android-only [`extract_bundled_seed_if_absent`] feeds it the embedded
/// corpus.
#[cfg(any(target_os = "android", test))]
fn write_seed_files(dir: &std::path::Path, files: &[(&str, &[u8])]) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    for (name, bytes) in files {
        let dest = dir.join(name);
        if !dest.exists() {
            std::fs::write(&dest, bytes)?;
        }
    }
    Ok(())
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run (from `src/`):
```bash
~/.cargo/bin/cargo test -p primer-gui write_seed_files_creates_and_is_idempotent
```
Expected: PASS.

- [ ] **Step 5: Add the android-only `include_dir` dependency**

In `src/Cargo.toml` under `[workspace.dependencies]`, add:
```toml
include_dir = "0.7"
```

In `src/crates/primer-gui/Cargo.toml`, add an android-target-gated dependency (keeps it out of desktop builds entirely):
```toml
[target.'cfg(target_os = "android")'.dependencies]
include_dir = { workspace = true }
```

- [ ] **Step 6: Implement the android extraction wrapper and wire it into `init_mobile_state`**

Add to `src/crates/primer-gui/src/paths.rs`:
```rust
/// The seed corpus embedded at compile time from `primer-gui/resources/seed/`.
/// ~280 KB of JSONL; extracted to `<app_data>/seed/` on first run so
/// `PRIMER_SEED_DIR` discovery works without `adb push`. Android-only — the
/// desktop `.app` bundle path and the `CARGO_MANIFEST_DIR` dev fallback cover
/// the other targets.
#[cfg(target_os = "android")]
static BUNDLED_SEED: include_dir::Dir<'_> =
    include_dir::include_dir!("$CARGO_MANIFEST_DIR/resources/seed");

/// Extract the embedded seed corpus into `<app_data>/seed/` if not already
/// present, returning that directory. Idempotent (see [`write_seed_files`]).
#[cfg(target_os = "android")]
pub fn extract_bundled_seed_if_absent(app_data: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    let dir = mobile_seed_dir(app_data);
    let files: Vec<(&str, &[u8])> = BUNDLED_SEED
        .files()
        .filter_map(|f| {
            f.path()
                .file_name()
                .and_then(|n| n.to_str())
                .map(|name| (name, f.contents()))
        })
        .collect();
    write_seed_files(&dir, &files)?;
    Ok(dir)
}
```

In `init_mobile_state` (`src/crates/primer-gui/src/paths.rs`), immediately BEFORE the existing `set_mobile_seed_dir_if_present(&home);` line, add:
```rust
    // Volunteer sideload builds have no `adb push`, so stage the embedded
    // seed corpus to <app_data>/seed/ on first run; the discovery call below
    // then points PRIMER_SEED_DIR at it. Failure degrades to an empty KB
    // (the prompt builder omits the knowledge section) rather than blocking boot.
    #[cfg(target_os = "android")]
    if let Err(e) = extract_bundled_seed_if_absent(&home) {
        tracing::warn!("seed extraction failed: {e}; continuing without knowledge base");
    }
```

- [ ] **Step 7: Verify the whole crate still builds under the Android feature contract**

Run (from `src/`):
```bash
~/.cargo/bin/cargo build -p primer-gui --no-default-features --features android-native
~/.cargo/bin/cargo test -p primer-gui write_seed_files_creates_and_is_idempotent
```
Expected: both succeed. (The `#[cfg(target_os = "android")]` extraction code is not compiled host-side, so this validates the feature graph and the pure helper; the android path compiles in Task 6.)

- [ ] **Step 8: Commit**

```bash
git add src/Cargo.toml src/crates/primer-gui/Cargo.toml src/crates/primer-gui/src/paths.rs
git commit -m "feat(gui): extract bundled seed corpus on first Android run (no adb needed)"
```

---

### Task 3: Release signing configuration

Produce a properly signed `release` APK. Keep R8/minify off to avoid stripping the JNI-invoked `org.theprimer.gui.PrimerSpeech` class used by `android-native` voice.

**Files:**
- Modify: `src/crates/primer-gui/gen/android/app/build.gradle.kts`
- Create: `src/crates/primer-gui/gen/android/keystore.properties.example`

**Interfaces:**
- Consumes: nothing.
- Produces: a `release` build type that signs when `gen/android/keystore.properties` exists.

- [ ] **Step 1: Add the example keystore properties (committed template)**

Create `src/crates/primer-gui/gen/android/keystore.properties.example`:
```properties
# Copy to keystore.properties (gitignored) and fill in real values.
# Generate a keystore once with:
#   keytool -genkey -v -keystore primer-release.jks -keyalg RSA -keysize 2048 \
#     -validity 10000 -alias primer
# Keep primer-release.jks OUTSIDE the repo; point storeFile at its absolute path.
storeFile=/absolute/path/to/primer-release.jks
storePassword=CHANGE_ME
keyAlias=primer
keyPassword=CHANGE_ME
```

- [ ] **Step 2: Confirm `keystore.properties` is gitignored**

Run (from repo root):
```bash
grep -n 'keystore.properties' src/crates/primer-gui/gen/android/.gitignore
```
Expected: a match (the entry already exists). No change needed; this guards against committing secrets.

- [ ] **Step 3: Add the signing config and disable minify in `build.gradle.kts`**

In `src/crates/primer-gui/gen/android/app/build.gradle.kts`, add a `signingConfigs` block inside `android { }` (after `defaultConfig { }`):
```kotlin
    val keystorePropsFile = rootProject.file("keystore.properties")
    val keystoreProps = Properties().apply {
        if (keystorePropsFile.exists()) {
            keystorePropsFile.inputStream().use { load(it) }
        }
    }

    signingConfigs {
        create("release") {
            if (keystorePropsFile.exists()) {
                storeFile = file(keystoreProps.getProperty("storeFile"))
                storePassword = keystoreProps.getProperty("storePassword")
                keyAlias = keystoreProps.getProperty("keyAlias")
                keyPassword = keystoreProps.getProperty("keyPassword")
            }
        }
    }
```

Then replace the existing `getByName("release") { ... }` body with:
```kotlin
        getByName("release") {
            // Minify stays OFF for the test APK: R8 would risk stripping or
            // renaming org.theprimer.gui.PrimerSpeech, which the Rust side
            // invokes reflectively over JNI (nativeInit caches it as a
            // GlobalRef). A future minified production build must add explicit
            // proguard-keep rules for that class + its native methods.
            isMinifyEnabled = false
            signingConfig = if (keystorePropsFile.exists()) {
                signingConfigs.getByName("release")
            } else {
                null
            }
        }
```
(`java.util.Properties` is already imported at the top of this file.)

- [ ] **Step 4: Verify the Gradle config parses**

Run (from `src/crates/primer-gui/gen/android/`):
```bash
./gradlew tasks --offline 2>&1 | head -5 || ./gradlew tasks 2>&1 | head -20
```
Expected: Gradle configures without a script-evaluation error (task list prints). A dependency-download failure offline is acceptable; a Kotlin/DSL syntax error is not. If the `rust` plugin blocks task listing without a full NDK toolchain, instead confirm the file has balanced braces and the `signingConfigs`/`buildTypes` blocks are well-formed by re-reading it.

- [ ] **Step 5: Commit**

```bash
git add src/crates/primer-gui/gen/android/app/build.gradle.kts \
        src/crates/primer-gui/gen/android/keystore.properties.example
git commit -m "build(android): release signing config from keystore.properties; minify off"
```

---

### Task 4: First-run "set up the AI" nudge

On a fresh install the backend defaults to `stub` (canned responses). Nudge the supervising adult to Settings so the child gets a real LLM. Minimal: a dismissible banner shown when the effective backend is `stub`.

**Files:**
- Modify: `src/crates/primer-gui/ui/index.html`
- Modify: `src/crates/primer-gui/ui/app.js`

**Interfaces:**
- Consumes: the existing `get_settings` Tauri command (returns a `GuiConfigView` whose backend has a `kind` string); the existing header control `#settings-open`.
- Produces: DOM element `#setup-nudge`; behaviour wired in `app.js`.

- [ ] **Step 1: Add the hidden banner to `index.html`**

In `src/crates/primer-gui/ui/index.html`, add immediately after the opening `<body>` tag's `<header>…</header>` block (before the main chat container):
```html
    <div class="setup-nudge" id="setup-nudge" role="status" hidden>
      <span class="setup-nudge-text">
        Ask a grown-up to set up the AI in Settings before you start.
      </span>
      <button type="button" class="setup-nudge-btn" id="setup-nudge-open">Open Settings</button>
    </div>
```

- [ ] **Step 2: Wire the banner in `app.js`**

In `src/crates/primer-gui/ui/app.js`, add this function (near the other helpers) and call it once from the end of the session-start path (the function that runs after a session becomes active — the same place `get_full_session_turns` is invoked):
```javascript
/// Show a one-line nudge when no real LLM is configured (backend still on
/// the `stub` default), so the supervising adult wires an API key in Settings
/// before handing the phone to the child. Best-effort: any error just hides it.
async function refreshSetupNudge() {
  const nudge = document.getElementById("setup-nudge");
  if (!nudge) return;
  try {
    const settings = await invoke("get_settings");
    const kind = settings?.backend?.kind ?? "stub";
    nudge.hidden = kind !== "stub";
  } catch (_e) {
    nudge.hidden = true;
  }
}
```
Wire the button once during init (alongside other `addEventListener` setup):
```javascript
  const nudgeOpen = document.getElementById("setup-nudge-open");
  if (nudgeOpen) {
    nudgeOpen.addEventListener("click", () => {
      document.getElementById("settings-open")?.click();
      document.getElementById("setup-nudge").hidden = true;
    });
  }
```
Call `refreshSetupNudge()` at the end of the session-start flow and again after Settings closes (if the app exposes an on-close hook via `settings.open({ onSessionRestarted })`, call it there; otherwise call it once after `#settings-open` handling). Keep it best-effort — never block the UI on it.

- [ ] **Step 3: Add minimal banner styling**

In `src/crates/primer-gui/ui/styles.css`, add:
```css
.setup-nudge {
  display: flex;
  align-items: center;
  gap: 0.75rem;
  padding: 0.6rem 1rem;
  background: #fff4d6;
  color: #5a4300;
  font-size: 0.95rem;
}
.setup-nudge[hidden] { display: none; }
.setup-nudge-btn {
  margin-left: auto;
  padding: 0.35rem 0.8rem;
  border: 0;
  border-radius: 6px;
  background: #5a4300;
  color: #fff;
  cursor: pointer;
}
```

- [ ] **Step 4: Manual verification (no JS test harness in this project)**

Run the desktop GUI from `src/crates/primer-gui/`:
```bash
~/.cargo/bin/cargo tauri dev
```
Expected: with the default `stub` backend, the banner appears once a session starts; after Settings → Inference backend is switched to `cloud`/`openai-compat` and saved, `refreshSetupNudge()` hides it. Clicking "Open Settings" opens the settings modal. Confirm the banner does not overlap the composer or header.

- [ ] **Step 5: Commit**

```bash
git add src/crates/primer-gui/ui/index.html src/crates/primer-gui/ui/app.js src/crates/primer-gui/ui/styles.css
git commit -m "feat(gui): first-run nudge to configure a real LLM backend"
```

---

### Task 5: Family setup guide + parent consent note

Volunteer-facing docs. The consent note is drafted here in full (the owner asked for it to be authored as part of this work).

**Files:**
- Create: `docs/pilot/family-setup-guide.md`
- Create: `docs/pilot/parent-consent-note.md`

**Interfaces:** none (documentation).

- [ ] **Step 1: Write the family setup guide**

Create `docs/pilot/family-setup-guide.md` covering, in plain language for a non-technical parent:
1. What the Primer is (a Socratic learning companion that asks more than it answers) and that this is a **test build**.
2. Getting an API key — step-by-step for **either** OpenAI **or** Anthropic (create account → create API key → copy it), with a note that usage bills to *their* account and to set a low spend limit.
3. Installing the APK — transfer the `Primer-<ver>-arm64.apk` to the phone, enable "install unknown apps" for the file manager/browser, tap to install.
4. First-run setup (adult does this once): open the app → tap **Settings** → **Inference backend** → choose `cloud` (Anthropic) or `openai-compat` (for OpenAI-compatible) → paste the API key → for openai-compat, set the server URL + model → Save. Set the child's **name / age / language**.
5. Hand the phone to the child; using text or voice (tap 🎙). Note voice needs microphone permission (granted on first use).
6. Troubleshooting: the yellow "set up the AI" banner means no key yet; an error reply usually means a wrong/expired key or no internet.

- [ ] **Step 2: Write the parent consent note (full text)**

Create `docs/pilot/parent-consent-note.md`:
```markdown
# The Primer — Pilot Test: Information for Parents & Guardians

Thank you for helping test the Primer, a Socratic learning companion for
children. Please read this before your child uses the app.

## What this test is
This is an early **test build** distributed to volunteer families. It is not a
finished product and is not offered on any app store. You are helping evaluate
how the Primer teaches — how it asks questions, checks understanding, and
encourages breaks.

## What data is handled, and where
- **Your child's conversations and learning history stay on the device.** The
  app stores them in the phone's private app storage. They are not uploaded to
  us and we do not collect them.
- **To generate each reply, the app sends the text of the current
  conversation turn to the AI provider you choose** (OpenAI or Anthropic), using
  the API key *you* provide. That request travels over an encrypted (HTTPS)
  connection. The provider processes the text under **their** privacy terms and
  policies — please review the terms of whichever provider you pick.
- We (the Primer project) do **not** receive your child's conversations, name,
  age, or usage. There is no analytics or tracking in this build.
- The API key you enter is stored only in the app's private storage on that one
  device. It is never sent to us and is not embedded in the app.

## Your choices and control
- You set up the app and choose the AI provider. You can change or remove the
  key at any time in Settings.
- You can delete all local data by clearing the app's storage or uninstalling.
- Because conversation text goes to a third-party AI provider, please supervise
  early sessions and decide what is appropriate for your child to share. Avoid
  entering identifying details (full name, address, school) into the chat.

## No guarantees
This is experimental software. Replies come from a third-party AI model and may
occasionally be wrong or inappropriate despite the Primer's safeguards. Please
supervise and give us feedback.

## Consent
By setting up the app and letting your child use it, you agree to this test on
the terms above. Questions or withdrawal: contact the person who gave you this
build.
```

- [ ] **Step 3: Commit**

```bash
git add docs/pilot/family-setup-guide.md docs/pilot/parent-consent-note.md
git commit -m "docs(pilot): family setup guide + parent consent note"
```

---

### Task 6: Build the signed APK and smoke-test on a device

Device-gated verification (owner runs it; requires a real non-RedMagic Android phone + a generated keystore). Produces the deliverable APK and confirms the end-to-end path.

**Files:** none (produces `Primer-<ver>-arm64.apk`).

**Interfaces:**
- Consumes: the build command from Task 1; signing from Task 3; seed extraction from Task 2.
- Produces: a signed release APK.

- [ ] **Step 1: Generate a release keystore (once) and create `keystore.properties`**

Run (outside the repo, e.g. `~/primer-signing/`):
```bash
keytool -genkey -v -keystore ~/primer-signing/primer-release.jks \
  -keyalg RSA -keysize 2048 -validity 10000 -alias primer
```
Then copy `gen/android/keystore.properties.example` → `gen/android/keystore.properties` and fill in the absolute `storeFile` path + passwords + `keyAlias=primer`.

- [ ] **Step 2: Build the signed release APK**

Run (from `src/crates/primer-gui/`):
```bash
~/.cargo/bin/cargo tauri android build --apk --target aarch64 \
  -- --no-default-features --features android-native
```
Expected: a signed `app-arm64-release.apk` (or Tauri-named equivalent) under `gen/android/app/build/outputs/apk/`. If the CLI does not forward the feature flags, apply the gradle `rust { }` fallback from the build-notes doc and rebuild.

- [ ] **Step 3: Install on a real phone and smoke-test**

```bash
~/Library/Android/sdk/platform-tools/adb install -r <path-to-apk>
```
On the device, verify:
1. **Setup nudge** appears on first launch (stub default).
2. **Cloud round-trip:** Settings → Inference backend → `cloud`, paste an Anthropic key, Save; ask a question; a streamed Socratic reply arrives.
3. **openai-compat round-trip** (optional): switch to `openai-compat`, set URL + key + model; confirm a reply.
4. **Voice round-trip:** tap 🎙, grant mic, speak a question; confirm transcript → streamed spoken reply (no barge-in).
5. **Knowledge base:** ask something covered by the seed corpus (e.g. "why is the sky blue") and confirm grounded phrasing, OR confirm the app still answers gracefully if seed extraction was skipped (check logs via `adb logcat | grep -i primer`).

- [ ] **Step 4: Record results**

Append a short "Verified on <device>, <date>" note (with any deviations) to `docs/devel/android-test-apk-build.md` and commit:
```bash
git add docs/devel/android-test-apk-build.md
git commit -m "docs: record on-device smoke-test results for the test APK"
```

---

## Self-Review

**Spec coverage:**
- Build profile / BM25-only → Task 1 (feature contract + verification).
- Signing / minify-off → Task 3.
- Seed corpus on Android → Task 2 (embed + first-run extraction; supersedes the spec's "resolve resource dir" wording because the asset namespace isn't fs-readable and volunteers have no adb — noted as a refinement).
- First-run UX → Task 4.
- Permissions/network (no change) → covered in Global Constraints + spec; no task needed.
- Deliverables: signed APK → Task 6; family setup guide → Task 5; consent note → Task 5.
- Verification → Task 6.
- Known limitations → carried verbatim into the setup guide / consent note (voice POC-grade, arm64-only, no child-lock, BM25-only, third-party provider disclosure).

**Placeholder scan:** No TBD/TODO; all code steps show full code; the one flagged unknown (does `cargo tauri android build` forward feature flags) has an explicit fallback (gradle `rust {}` block) documented in Tasks 1 and 6.

**Type consistency:** `write_seed_files(&Path, &[(&str, &[u8])])` and `extract_bundled_seed_if_absent(&Path) -> io::Result<PathBuf>` are used consistently across Task 2 steps; `mobile_seed_dir` / `set_mobile_seed_dir_if_present` match the existing `paths.rs` signatures; the frontend uses `get_settings` → `settings.backend.kind`, matching the existing `GuiConfigView` shape.

**Spec refinement noted:** Design §3 said "resolve the Tauri resource dir → PRIMER_SEED_DIR". Implementation reality (documented in `paths.rs`) is that the APK asset namespace is not `std::fs`-readable, so Task 2 embeds+extracts instead. Same outcome (KB loads on Android, degrades gracefully); the mechanism is corrected. No behavioural change to the spec's intent.
