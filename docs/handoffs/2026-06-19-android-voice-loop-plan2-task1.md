# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-06-19 — work is on feature branch `android-voice-loop-plan2`, pushed and open as **PR #250** (4 commits ahead of `main` at `103a96b`; CI running at handoff). This session **planned Plan 2 of the Android-native voice POC and shipped Task 1** (the `nativeInit`/`ndk_context` bridge-bootstrap fix). The prior brief was stale — PR #249 (Android voice Plan 1, capability gate **GO**) had merged since it was written; this session picked up that thread.

**Context at session start:** the owner chose (via the session-opening question) to pursue **Android voice Plan 2** over the other open items (pedagogy tuning, latency routing). Plan 1 (#249) had landed on `main` and left one carried blocker — the `speech_capabilities` Tauri command panicked on-device in `ndk_context::android_context()`. Plan 2 is the voice-loop implementation; its Task 1 is that bootstrap fix.

## What we shipped this session

All on branch `android-voice-loop-plan2` (PR not yet opened):

- **`ecbc3b9` — Plan 2 implementation plan** ([docs/superpowers/plans/2026-06-18-android-native-voice-loop.md](docs/superpowers/plans/2026-06-18-android-native-voice-loop.md)). A 10-task TDD plan. Tasks 1–6 + 9 are **host-testable** (pure logic, CI-verifiable from a desktop); Tasks 7, 8, 10 are **device-only** (real JNI/Kotlin + on-device acceptance). Key locked design decisions (all precedent-driven, rationale inline in the plan): D1 Android does NOT use the cpal-gated `LocalBackends` — it returns a small `AndroidVoiceBackends { backends, event_rx, stop }` and the GUI passes a no-op `on_committed_audio` + `None` drain + `None` is_speaking to `run_loop` (all already-optional). D2 `ChannelStt` is un-gated from cpal (pure channel code) so the cpal-free Android STT reuses it. D3 `AndroidTts::push_text` calls `bridge.speak()` and **blocks until `onDone`**, emitting no PCM (the OS plays itself). D4 Kotlin→Rust eventing is a **poll model** (`pollSpeechEvent`), Rust→Kotlin only. D5 Android gets its own pure `AndroidDerivedVad` (recognizer events differ from macos26's SpeechAnalyzer).
- **`8cd11fb` — Task 1: `nativeInit` JavaVM cache, retire `ndk_context`.** TDD: a generic host-tested `VmCache<T>` (`primer-speech/src/android/vm.rs`; 3 tests, RED→GREEN — set-once, unset-is-an-error-not-a-panic), an android-only `VmCache<JavaVM>` + `set_java_vm`/`java_vm`, the `Java_org_theprimer_gui_PrimerSpeech_nativeInit` `#[unsafe(no_mangle)]` export (lib.rs), `JniSpeechBridge::new()` switched off `ndk_context` to the cached VM, Kotlin `external fun nativeInit()` + the `MainActivity.onCreate` call. `ndk-context` retired as a **direct** primer-speech dep (still a transitive Tauri-Android dep, correctly kept in `Cargo.lock`). **Verified:** primer-speech host (43 tests) + `aarch64-linux-android` cross-compile green; `primer-gui --features android-native` cross-compile green (with NDK env); fmt + clippy clean.
- **`69efdd8` — ROADMAP entry** (Phase 3 sub-project 6, 🟡) recording the Android-native voice POC (Plan 1 GO #249 + Plan 2 in progress + Task 1 done). **README untouched** — nothing user-facing shipped yet (the POC isn't usable until Task 10).

## What's next (concrete acceptance criteria)

> The next steps are the rest of Plan 2, in order. Tasks 2–6 + 9 are **host-implementable now** (no device); Tasks 7, 8, 10 need the RedMagic.

### 1. ⭐ Land PR #250, then continue Plan 2 host tasks
- **Acceptance:** confirm PR #250 CI is green (`cargo test (default features)`, the android `--features qnn` AND `--features android-native` cross-compile guards) and merge it. Then implement **Plan 2 Tasks 2–6 + 9** by TDD (event types + extended bridge trait; `AndroidDerivedVad`; un-gate `ChannelStt`; `AndroidTts`; `AndroidStt` consumer + `build_android_voice_backends`; Stt/Tts enum variants). Each task in the plan has full failing-test-first code + exact commands. All are host-testable + android-cross-compilable; merge them before the device tasks.
- **Note:** Task 2 Step 4 says to add `not yet implemented (Task 7)` stubs for the five new `JniSpeechBridge` methods so the android cross-compile stays green between Task 2 and Task 7 — don't skip that or the aarch64 guard breaks mid-sequence.

### 2. Device-only Plan 2 tasks (need the RedMagic 11 Pro)
- **Task 7** (real JNI bridge methods + Kotlin recognizer/TTS plugin), **Task 8** (GUI android voice commands + mic runtime permission + frontend), **Task 10** (on-device acceptance — THE GATE).
- **Acceptance (Task 10, the POC gate):** on the RedMagic, in the APK, **in airplane mode**, one full voice turn — speak English → on-device STT transcribes → `QnnBackend` answers → `TextToSpeech` speaks it → loop returns to LISTEN. This is simultaneously the Phase 2 and Phase 3 exit demo. **Task 10 Step 1 also re-proves Task 1**: rebuild the APK and invoke `speech_capabilities` — it must now return real JSON instead of panicking (proves the `nativeInit` JavaVM cache works under Tauri-mobile). If it still fails, apply the `find_class` classloader fallback (Task 7 Step 2 note).

### Carried / owner-or-hardware-gated (unchanged from before #249)
- Pedagogy/answer-quality tuning on the 4B NPU model (owner-in-the-loop — the standing top open question); on-device #224 length-recovery spot-check; latency-routing calibration (`--primary-ttft-budget-ms` around the measured p95 ≈ 2.6 s); #223 GENIE enum; #170 Supertonic Stages E/F; #201 llamacpp BOS; #192/#166 human-at-mic smokes; #157 Termux ONNX validation; #135 glib bump on Tauri 3.

## Open decisions / risks

- **`nativeInit` symbol resolution / load order is the #1 device unknown.** The `Java_org_theprimer_gui_PrimerSpeech_nativeInit` symbol must be in the loaded Tauri app `.so` and not stripped. If Kotlin's `external fun nativeInit()` throws `UnsatisfiedLinkError`, the symbol name or the lib-load order is wrong (it must be called *after* `super.onCreate` loads the Rust lib). Task 10 Step 1 is its gate. The code compiles + cross-compiles; only on-device proves it links + runs.
- **`find_class` on an attached thread** (Plan 1 risk #2, carried) — may need the cached-`Context` classloader fallback; revealed by Task 10.
- **Branch not yet merged.** Everything this session is on `android-voice-loop-plan2` (PR #250, open). Don't assume `main` has it — confirm #250 merged before building on it.
- **`ndk-context` stays in `Cargo.lock`** — that is correct (a transitive Tauri-Android dep); don't try to prune it again. The only intended lock change was removing primer-speech's *direct* dependency line.
- **Plan 2's host/device split is deliberate** — implement + merge Tasks 2–6 + 9 from a desktop; do not block them on the device.

## Patterns to reuse, not reinvent

- **The android module mirrors `primer-inference::qnn`:** pure logic + a `MockBridge`-driven host test path; the real JNI is `#[cfg(target_os = "android")]` and device-verified (no host test), exactly as Plan 1's `jni_bridge` and Task 1's `VmCache<JavaVM>` instantiation. New pure logic gets TDD; new JNI/Kotlin gets exact code + device verification.
- **`run_loop` is untouched by Android.** It already takes `on_committed_audio`, `wait_for_speaker_drain: Option`, `is_speaking: Option` — Android passes a no-op + `None` + `None`. Do not modify the state machine for Android.
- **`ChannelStt` is the STT seam** (the macos-native-26 precedent): a `StreamingSpeechToText` whose `finalize()` yields the next channel-delivered transcript. Android's recognizer consumer feeds it + a derived-VAD `event_rx`, same shape as macos26's `run_consumer_loop`.
- **Android device facts (carried, still true):** `~/Library/Android/sdk/platform-tools/adb -s 912607710061`; **logcat is dead on this ROM** — read app-internal files via `run-as org.theprimer.gui cat files/...`; APK build `NDK_HOME=/opt/homebrew/share/android-ndk ~/.cargo/bin/cargo-tauri android build --apk --debug --target aarch64 -- --no-default-features --features android-native`; commits touching `.github/workflows` need `gh auth refresh -s workflow`.
- **NDK cross-compile env (for the GUI guard locally):** put `/opt/homebrew/share/android-ndk/toolchains/llvm/prebuilt/darwin-x86_64/bin` on `PATH` and set `CC_aarch64_linux_android`/`AR_aarch64_linux_android`/`CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER` to the `aarch64-linux-android24-clang` / `llvm-ar` there (the bare `primer-speech` android build needs none of this; only `primer-gui` pulls a cc-rs dep that does).

## Exact commands needed to resume

```bash
cd /Users/hherb/src/primer && git checkout android-voice-loop-plan2 && git log --oneline -4   # 69efdd8 at HEAD; main at 103a96b

# === Host gate (what CI runs; all green at handoff) ===
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo +1.88 fmt --all -- --check
~/.cargo/bin/cargo +1.88 test -p primer-speech --features android-native          # 43 tests
~/.cargo/bin/cargo +1.88 build -p primer-speech --features android-native --target aarch64-linux-android
~/.cargo/bin/cargo +1.88 test --workspace                                         # full default suite

# === GUI android-native cross-compile (drift guard; needs NDK env) ===
NDK_BIN=/opt/homebrew/share/android-ndk/toolchains/llvm/prebuilt/darwin-x86_64/bin
PATH="$NDK_BIN:$PATH" \
  CC_aarch64_linux_android="$NDK_BIN/aarch64-linux-android24-clang" \
  AR_aarch64_linux_android="$NDK_BIN/llvm-ar" \
  CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$NDK_BIN/aarch64-linux-android24-clang" \
  ~/.cargo/bin/cargo build --target aarch64-linux-android --no-default-features -p primer-gui --features android-native

# === PR #250 is open; confirm green + merge, then continue Tasks 2-6 + 9 (host, TDD) ===
gh pr checks 250
gh pr merge 250 --squash   # once green
# Then implement plan tasks 2-6 + 9 (full TDD code + commands in the plan doc).
```

## Reporting back

- State plainly, by acceptance criterion, what compiles/tests and what is device-unverified.
- The owner chose the Android voice POC this session — continue Plan 2; do not silently switch threads.
- Tasks 2–6 + 9 are host-mergeable now; Tasks 7, 8, 10 need the RedMagic. Open the PR early so CI exercises the branch.
- The GUI is a full app, not a scaffold — trust the code over any stale "scaffold" phrasing.
