# Android Generic-API Test APK — Design

**Date:** 2026-07-01
**Status:** approved (owner-paired brainstorming)
**Scope:** A signed, sideloadable Android APK of `primer-gui` configured for cloud / OpenAI-compatible
API inference, so volunteer families can field-test the pedagogic engine on ordinary phones — no
local LLM, no RedMagic-class hardware, no NPU.
**Relation to prior work:** A distribution branch parallel to the QNN path. It builds on the
Tauri-Android scaffold from [`2026-06-11-android-packaging-scaffold-design.md`](2026-06-11-android-packaging-scaffold-design.md)
(sub-project 1) but does **not** touch QNN (sub-projects 2–4). Where that effort targets a
fully-offline on-device NPU token, this one targets a cloud-backed test build for external testers.

## Context

Volunteers want to evaluate the Primer's Socratic pedagogy but have neither local-LLM capability nor
high-end phones. The engine already speaks to cloud (Anthropic) and any OpenAI-compatible API, and
`primer-gui` is a fully working Tauri app with in-app API-key entry. The Tauri-Android scaffold
exists and has produced APKs during the QNN work. So this is a **packaging + Android-defaults +
signing + light setup UX** effort, not new backend work.

### Decisions locked during brainstorming

- **Target:** an installable **test APK** (sideload, or a Play Internal Testing link later). No Play
  production review, so children's-app compliance is handled via **informed consent between the
  project and participating families**, not Google's Designed-for-Families review.
- **Testers:** real children, each supervised by an adult, one child per device
  (`[[project_personal_device_model]]`).
- **API keys:** **each family brings its own** OpenAI/Anthropic key. The supervising adult pastes it
  into Settings once, then hands over the phone. The key lives only in that device's app-private
  config — never in the APK. Zero inference cost to the project.
- **Voice:** **include OS-native voice** via the `android-native` feature (Android `SpeechRecognizer`
  + `TextToSpeech`). Works on any phone; the LLM stays in the cloud. POC-grade (sustained-run
  acceptance is issue #260) — acceptable, since testers are the acceptance testing.

## Chosen approach

**A — Android build profile + release signing, reusing the existing Settings UX**, with a minimal
first-run "no key yet" nudge. Rejected alternatives: **B** (full adult-setup wizard + child-locked
Settings — more UX than a supervised pilot needs now; child-lock deferred) and **C** (sign today's
default build unchanged — pulls the 570 MB fastembed model and device-unverified `ort` on Android,
and ignores voice/seed defaults).

## Design

### 1. Android build profile

Build the test APK with the embedder **compiled out** so the feature-aware default lands on
`--embedder-backend none` (BM25-only — the standing Android guidance, no model download, no
device-unverified `ort`):

```
cargo tauri android build -- --no-default-features --features android-native
```

Resulting build: cloud + openai-compat LLM (always compiled in `primer-inference`), OS-native voice,
BM25-only retrieval, **no** qnn / llamacpp / fastembed / whisper / piper / cpal.

- **Open item for the plan:** confirm `cargo tauri android build` forwards `--no-default-features
  --features …` to the Rust lib build. If it does not, pin the feature set in the gradle `rust { }`
  block (`gen/android/app/build.gradle.kts`) instead. Either way the effective feature set above is
  the contract.

### 2. Release signing

- Add `gen/android/app/build.gradle.kts` a release `signingConfig` that reads
  `gen/android/keystore.properties` (`storeFile`, `storePassword`, `keyAlias`, `keyPassword`).
- `keystore.properties` and the `.jks`/`.keystore` file are **gitignored**; the keystore lives
  outside the repo. Document how to generate one (`keytool`) in the build notes.
- Keep `isMinifyEnabled = false` for the `release` build type in this test APK. R8 would otherwise
  risk stripping/renaming `org.theprimer.gui.PrimerSpeech`, which Rust invokes reflectively over JNI
  (`nativeInit` caches it as a `GlobalRef`). Minified production builds are a later concern and would
  need explicit proguard-keep rules for that class + its native methods.

### 3. Seed corpus on Android

At GUI startup, resolve the Tauri **resource directory** and export `PRIMER_SEED_DIR` pointing at the
bundled `*.jsonl`, so `auto_seed_if_empty` finds them (mirrors the existing startup-hook pattern used
for the espeak data dir). If resolution fails, **degrade gracefully** — the Socratic engine works
without retrieval; the prompt builder already omits the knowledge section on an empty KB. The seed
files are already declared in `tauri.conf.json` under `bundle.resources`.

### 4. First-run UX (minimal)

On launch, if no API key / usable backend is configured, surface a **one-line prompt** steering the
adult to Settings → Inference backend before the child starts. Reuses the existing settings modal —
no new wizard, no child-lock in this iteration.

### 5. Permissions & network

No change needed. The manifest already declares `INTERNET` + `RECORD_AUDIO` and requests
`RECORD_AUDIO` at runtime (`MainActivity.onCreate`); `usesCleartextTraffic=false` for release (APIs
are HTTPS). Inference HTTP runs in Rust/`reqwest`, so the webview CSP does not gate it — the existing
`connect-src` CSP stays as-is.

### 6. Deliverables

1. **Signed APK** — `Primer-<version>-arm64.apk` (arm64-v8a only; `armeabi-v7a` is an optional later
   addition, not needed for modern phones).
2. **Family setup guide** — how to create an OpenAI/Anthropic key, install the APK (enable "install
   unknown apps"), enter the key + pick backend/model in Settings, and set the child's name/age/locale.
3. **Parent-facing data/consent note** — plainly states: all learner data stays on-device; only the
   per-turn conversation text is sent to the family's chosen LLM provider per request; the provider's
   own terms apply to that text.

### 7. Verification

- Build the signed APK host-side (macOS + NDK r29, already verified environment).
- Install on a **real non-RedMagic Android phone**.
- Smoke-test: (a) a cloud text round-trip; (b) one OS-native voice round-trip (speak → transcript →
  streamed spoken reply); (c) confirm the KB seeds, or degrades cleanly if the resource dir can't be
  resolved.

## Known limitations (stated up front)

- Voice is POC-grade (issue #260).
- arm64-v8a only.
- No child-lock on Settings — the adult is trusted to configure and hand over the device.
- BM25-only retrieval on Android (no dense-vector leg); acceptable per standing guidance.
- Children's conversation text reaches a third-party LLM provider — disclosed in the consent note;
  this build is for consented pilot families only, not public distribution.

## Out of scope

- Play **production** submission (Designed-for-Families review, COPPA/GDPR-K, data-safety form,
  content rating) — a separate, mostly-compliance effort if the pilot graduates to public release.
- QNN / local-LLM Android inference (the parallel sub-project 2–4 path).
- A shared/embedded key or project-hosted proxy (rejected in brainstorming; families bring own keys).
- Minified/R8 production Android builds and their proguard-keep rules.
