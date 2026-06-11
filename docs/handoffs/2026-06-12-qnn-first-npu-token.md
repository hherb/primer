# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-06-12 — **🎉 THE PRIMER GENERATED ITS FIRST TOKENS ON THE HEXAGON NPU.** Owner-paired, RedMagic 11 Pro connected. The Primer's own `QnnBackend` ran Qwen3-4B-w4a16 on the DSP and emitted logits/tokens — the **Phase 1.2 finish line**. Three DSP-bring-up blockers were cleared this session (V81 stub, FastRPC `libcdsprpc` manifest declaration, native-lib extraction). PR #218 carries the fixes. A *stable* token across reboots is still gated on one thing: contiguous DSP memory (CMA).

**Context at session start:** PR #217 was already merged to `origin/main` as `03dd6d7`. **End state:** branch `qnn-on-device-first-token` @ `d8857ed`, **PR #218 open** (CI running). Working tree clean on the branch. Branches = `main`, `qnn-on-device-first-token`, `qnn-genie-log-to-file` (merged, deletable), `backup/pre-rebase-stageB`.

## ⚠️ First action: check + merge PR #218

```bash
cd /Users/hherb/src/primer && git status
gh pr checks 218                 # required gate: cargo test (default features)
gh pr view 218
```
If green + owner approves: `gh pr merge 218 --squash --delete-branch`. No proprietary blobs (the V81 `.so` are git-ignored), no `.github/workflows` changes, no new deps.

## The headline: first NPU token (and the one blocker left)

The Primer's `QnnBackend` executed graphs on the Hexagon NPU and produced tokens — `genie.log` showed `Graph token_ar1_cl4096_*_of_4 execution finished with result 0`, `qnn-htp: getLogits Returning 1*151936`, `run-inference complete : 59155 usec` (~17 tok/s). **Acceptance criterion "≥1 token from `QnnBackend` on the Hexagon NPU" is MET.**

Three blockers cleared this session, each read behind a generic status via the PR #217 log-to-file path (`adb shell run-as org.theprimer.gui cat .primer/genie.log`):

1. **`GenieDialog_create` -1 = missing V81 host stub.** QAIRT 2.45 detects the SM8850 as **V81** and `dlopen`s `libQnnHtpV81Stub.so`; only V79 was staged. Fixed by staging a coherent **`2.45.0.260326` V81** lib set (see below).
2. **`libcdsprpc.so not found in namespace clns-9` → `loadRemoteSymbols err 4000`.** FastRPC's vendor client lib is public (`/vendor/etc/public.libraries.txt`) but API-31+ refuses it undeclared. Fixed with `<uses-native-library android:name="libcdsprpc.so" android:required="false"/>` in `AndroidManifest.xml`.
3. **`Failed to load skel, error 1002` = DSP skel had no real file to push.** `extractNativeLibs` defaulted false (libs lived only inside `base.apk!/lib/…`), so FastRPC couldn't push `libQnnHtpV81Skel.so` to the DSP. Fixed with `jniLibs.useLegacyPackaging = true` in `build.gradle.kts` → libs extract to the real `nativeLibraryDir` that `ADSP_LIBRARY_PATH` points at.

Plus the **logging firehose fix**: the diagnostic logger ran at VERBOSE and logged every tensor op — one reply emitted **≈1.4M lines** through a global mutex on the inference path and froze the app. Genie log level is now env-driven (`PRIMER_GENIE_LOG_LEVEL`, default **WARN**) + callback-side threshold filter.

### ⛔ The remaining gate — contiguous DSP memory (CMA)

A token generated **once** (first run after a fresh install, when CMA was momentarily free). On every run after a reboot it fails at **`Failed to map output buffer on NSP` / `map result: 8003` / err 1002** for `prompt_ar128_cl4096_4_of_4` (the **4th** weight-shared context binary). The buffer is **~698 MB** (`Failed to map buffer of size 731906048` with spill-fill set); the device has only **~374 MB CmaFree** of 696 MB CmaTotal (the rest held by display/system at boot). Tried on-device and **did NOT help** (reverted):
- `spill-fill-bufsize: 0 → 320000000` — made it *worse* (consolidated into one 698 MB buffer).
- context `size: 4096 → 2048` — no effect; Genie initializes **every** graph in the binary (incl. cl4096) regardless of `size`.

**This is a contiguous-memory capacity limit, not a config bug.** It needs a real fix next session.

## What we shipped this session (PR #218, branch `qnn-on-device-first-token`)

Two commits:
- **`67f8516`** `feat(qnn): Android on-device DSP bring-up — first NPU token …` — the three fixes + log-level fix. 5 files: `AndroidManifest.xml`, `build.gradle.kts`, jniLibs `README.md` (new sha256 manifest + corrected callout + direct-download docs), `genie/log.rs` (env-driven level + threshold filter + parser + tests), `genie/real.rs` (wire env level).
- **`d8857ed`** `docs: README + ROADMAP — first on-device NPU token …`.
- Host-verified: fmt + clippy (`-D warnings`) clean; `primer-inference --features qnn genie::` 21 green (incl. new `parse_genie_log_level` tests); `cargo test --workspace` (default features) green.

## What's next — by priority

### 1. ⭐ Solve the CMA / NSP-memory blocker → a STABLE token across reboots (Phase 1.2 true finish)
The 4th context binary's NSP buffers don't fit in ~374 MB free CMA. Options, roughly in order of likely payoff:
- **(a) Re-export the model from Qualcomm AI Hub with a smaller max context** (e.g. 2048 or 1024) so the cl3072/cl4096 graphs *don't exist* in the binaries (reducing `size` at runtime didn't help because the graphs are baked in). The Primer's QNN small-context budget already targets short prompts (12-turn window, 3-passage top-K), so 2048 is functionally fine. This directly shrinks the NSP buffers. **Best lead.**
- **(b) Re-export with fewer/larger context-binary splits or a smaller model** (e.g. a 1.5B–3B) whose per-context buffers fit in CMA.
- **(c) Free CMA** before launch (close apps; or relaunch the Primer immediately after boot before system services settle) — fragile, not a product fix, but a quick on-device test of the hypothesis.
- **(d) CMA size** is a kernel/boot-param thing (root) — out of scope for stock.
- **Acceptance:** `QnnBackend` loads all 4 context binaries and generates a coherent multi-token reply that completes (stops at `<|im_end|>`), reproducibly across a reboot. Then capture real `qnn_bench` numbers (gated on this).

### 2. The model also runs to context-full — verify generation termination
In the one successful run, `n_past` climbed toward 4096 and the app showed "Restarting session…". Unclear whether that was a genuinely long reply, the firehose making it *look* infinite, or EOS not being honored. With WARN-level logging now default, re-test once (a) lands and confirm the reply stops at `<|im_end|>` (eos-token 151645 is in the genie config).

### Carried, owner/hardware-gated (unchanged)
- Real `qnn_bench` numbers (now gated on the stable token = the CMA fix).
- Latency-aware routing calibration (`--primary-ttft-budget-ms`) — gated on bench numbers.
- llama.cpp bench on real hardware — owner-gated.
- Full `tauri android build` in CI — deferred (needs JDK+SDK+NDK+Gradle on the runner).
- #170 Supertonic Stages E/F; #192 / #166 human-at-mic smokes; #157 Termux ONNX-runtime validation; #98 split `sweep.rs`; #135 glib bump on Tauri 3; #201 llamacpp BOS.

### Open queue (issues)

| #   | Title | State |
| --- | --- | --- |
| 201 | llamacpp BOS handling across model families | owner-gated (real-model smoke) |
| 192 | Manual smoke: macOS-native STT + injected TTS | needs human at mic |
| 170 | Supertonic 3 voice-mode TTS | Stage D shipped; E/F + manual gates next |
| 166 | Real-audio multi-utterance Whisper smoke | needs human at mic + ggml-small.bin |
| 135 | bump glib → 0.20+ once Tauri 3 ships | waits on Tauri 3 |
| 98  | split tests/common/sweep.rs | defer until 3rd locale |

## Open decisions / risks

- **The CMA limit may force a model change.** If a smaller-context AI Hub export still doesn't fit (or AI Hub won't export <4096 for this model), the standalone-phone product may need a smaller model (1.5B–3B) on this device. Decision deferred to the export experiment.
- **The QAIRT V81 `.so` are git-ignored.** A fresh clone has an empty `jniLibs/arm64-v8a/` (just the README). Re-stage via the **no-login direct download** (documented in the jniLibs README): `2.45.0.260326` from `https://softwarecenter.qualcomm.com/api/download/software/sdks/Qualcomm_AI_Runtime_Community/All/2.45.0.260326/v2.45.0.260326.zip` — pull just the ~110 MB of V81 libs with `uvx --from remotezip remotezip` (no QPM, no Qualcomm login; QPM is Linux/Windows-only and Mac-hostile). The staged set sits in the working tree on this machine (ignored).
- **On-device leftover:** `files/qnn-bundle/genie_config.json.bak0` (my backup) is on the device only; the live `genie_config.json` is restored to original (size 4096, spill-fill 0). Harmless.
- **Branch protection ACTIVE on `main`** (required `cargo test (default features)`, strict). PR #218 has code so CI runs.
- `backup/pre-rebase-stageB` KEPT (intentional); `qnn-genie-log-to-file` merged (PR #217) and deletable. Carried: `--languages` (#21) seeds a fresh learner only; Supertonic OpenRAIL-M licence read before any Stage E/F default flip.

## Patterns to reuse, not reinvent

New from this session:
- **QAIRT SDK without QPM (Mac-friendly).** The Software Center QPM desktop app is Linux/Windows only and disables Mac. But the download API is **open + unauthenticated**: `…/api/download/software/sdks/Qualcomm_AI_Runtime_Community/All/<MAJOR.MINOR.0.YYMMDD>/v<…>.zip`. The lib's internal `AISW_VERSION` (`2.45.41…`) is NOT the package version — the package follows `MAJOR.MINOR.0.YYMMDD` (ours = `2.45.0.260326`; neighbours `2.40.0.251030`, `2.46.0.260424`). Pull just the needed entries with `remotezip` over HTTP range requests instead of the 1.66 GB zip.
- **QNN-on-Android DSP bring-up checklist (FastRPC):** (1) stage the **arch-matching** HTP libs (SM8850 → V81) from the **same SDK version** as `libGenie`/`libQnnHtp`; (2) `<uses-native-library libcdsprpc.so>` in the manifest (API-31+ vendor-lib gate); (3) `jniLibs.useLegacyPackaging = true` so the DSP skel is a real extractable file `ADSP_LIBRARY_PATH` can resolve (default `extractNativeLibs=false` keeps it inside the APK and FastRPC can't push it); (4) bundle in app-*internal* storage; (5) watch contiguous CMA — large LLM context binaries can exceed it.
- **Genie log level is now tunable:** `PRIMER_GENIE_LOG_LEVEL=verbose` (on the GUI process env) restores the full DSP-init trace for the next deep debug; default WARN keeps failure-cause lines without the per-token firehose.

Carried forward (prior handoffs): `home` is the single base-dir knob in `primer-gui`; mobile Tauri setup defers `app.path()` into `.setup()`; Android scoped storage hides `adb`-written `/sdcard/Android/data/<pkg>` from the app — stage assets app-internal via `shell cat <src> | run-as <pkg> sh -c 'cat > files/...'`; **this RedMagic ROM has dead logcat AND black screencap** — observe via `run-as cat` of app-internal files + the owner reading the screen; **`run-as <pkg> sh -c '<cmd with redirect>'` runs with cwd `/`** (use absolute paths or `run-as <pkg> <cmd>` directly — e.g. `run-as <pkg> truncate -s 0 .primer/genie.log` works, the `sh -c '… > .primer/…'` form fails); **a reboot leaves the device at the lock screen with `/data/user/0/<pkg>` encrypted — the owner must enter the PIN before `run-as`/the app work**; APK rebuild is `--no-default-features --features qnn`; run cargo from `src/` with `+1.88`; docs-only PRs are CI-path-ignored (#168). Android host facts: JDK 21 = Android Studio's JBR for `JAVA_HOME`; the `$ANDROID_HOME/ndk/29.0.14206865 → /opt/homebrew/share/android-ndk` symlink Tauri 2.11 needs; commits touching `.github/workflows` need `gh auth refresh -s workflow -h github.com`.

## Exact commands needed to resume

```bash
cd /Users/hherb/src/primer
git status                                    # clean on qnn-on-device-first-token (or main if merged)
gh pr checks 218 ; gh pr view 218             # merge if green + approved: gh pr merge 218 --squash --delete-branch

# === Health check ===
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo +1.88 fmt --all -- --check
~/.cargo/bin/cargo +1.88 clippy -p primer-qnn-sys -p primer-inference --features primer-inference/qnn -p primer-gui -p primer-engine --all-targets -- -D warnings
~/.cargo/bin/cargo +1.88 test --workspace          # default features, the required gate
~/.cargo/bin/cargo +1.88 test -p primer-inference --features qnn genie::   # qnn-gated logging tests

# === Re-stage the V81 libs if the working tree is fresh (no-login, no QPM) ===
URL="https://softwarecenter.qualcomm.com/api/download/software/sdks/Qualcomm_AI_Runtime_Community/All/2.45.0.260326/v2.45.0.260326.zip"
B="qairt/2.45.0.260326/lib"; cd /tmp && uvx --from remotezip remotezip "$URL" \
  "$B/aarch64-android/libGenie.so" "$B/aarch64-android/libQnnHtp.so" \
  "$B/aarch64-android/libQnnHtpNetRunExtensions.so" "$B/aarch64-android/libQnnHtpPrepare.so" \
  "$B/aarch64-android/libQnnSaver.so" "$B/aarch64-android/libQnnSystem.so" \
  "$B/aarch64-android/libQnnHtpV81Stub.so" "$B/aarch64-android/libQnnHtpV81CalculatorStub.so" \
  "$B/hexagon-v81/unsigned/libQnnHtpV81Skel.so"
JNI=/Users/hherb/src/primer/src/crates/primer-gui/gen/android/app/src/main/jniLibs/arm64-v8a
find /tmp/qairt -name '*.so' -exec cp -p {} "$JNI/" \;
( cd "$JNI" && shasum -a 256 -c <(grep -E '^[0-9a-f]{64}  lib' README.md) )   # all 9 must say OK

# === Build + install the QNN APK (device connected, serial 912607710061) ===
export ANDROID_HOME="$HOME/Library/Android/sdk"; export NDK_HOME=/opt/homebrew/share/android-ndk
export JAVA_HOME="/Applications/Android Studio.app/Contents/jbr/Contents/Home"
export PATH="$NDK_HOME/toolchains/llvm/prebuilt/darwin-x86_64/bin:$JAVA_HOME/bin:$HOME/.cargo/bin:$PATH"
cd src/crates/primer-gui
~/.cargo/bin/cargo-tauri android build --apk --debug --target aarch64 -- --no-default-features --features qnn
ADB="$ANDROID_HOME/platform-tools/adb"
"$ADB" install -r gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk

# === Drive a session + read behind any error (logcat is DEAD on this ROM) ===
"$ADB" shell run-as org.theprimer.gui truncate -s 0 .primer/genie.log   # clean baseline (NOT sh -c '> …')
"$ADB" shell monkey -p org.theprimer.gui -c android.intent.category.LAUNCHER 1
# owner sends a message in the app, then:
"$ADB" shell run-as org.theprimer.gui cat .primer/genie.log | tail -60
# bundle internal copy: /data/user/0/org.theprimer.gui/files/qnn-bundle (13 files, ~2.9 GB)
# For the next deep DSP-init trace, the GUI process env can set PRIMER_GENIE_LOG_LEVEL=verbose.

# === The CMA experiment (next session's #1): re-export Qwen3-4B with a smaller max context ===
# Use Qualcomm AI Hub to export qwen3-4b-instruct-2507 w4a16 at context 2048 (or 1024),
# re-stage the new bundle into files/qnn-bundle, retry. Acceptance: stable token across a reboot.

# === New code work: PR-first (branch protection is on) ===
git checkout -b <branch> main && git push -u origin <branch> && gh pr create --base main ...
# NB: commits touching .github/workflows need `gh auth refresh -s workflow -h github.com` first.
```

## Reporting back

When you finish or hit a blocker:
- State plainly what you got working and what you didn't, by acceptance criterion.
- Flag any bugs you exposed in existing behaviour separately from the assigned task.
- **This session's headline:** the Primer's own `QnnBackend` **generated tokens on the Hexagon NPU** (Phase 1.2 finish line) after clearing three DSP-bring-up blockers (V81 stub, FastRPC `libcdsprpc` manifest declaration, native-lib extraction) — PR #218. The single remaining gate for a *stable* token across reboots is contiguous DSP memory (CMA): the 4th weight-shared context binary's ~698 MB NSP buffers exceed ~374 MB free CMA; `spill-fill-bufsize`/`size` tweaks don't help, so the fix is a memory-optimized model export (smaller max context) or CMA tuning.
- The GUI is a full app — when a future brief calls it a scaffold, it's wrong; trust the code.
