# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-06-14 — **🎉 THE PRIMER GENERATES COHERENT REPLIES ON THE HEXAGON NPU — fast, stable across a reboot, full pedagogy stack live.** The CMA memory blocker is **resolved** (the cl2048 re-export). Confirmed on-device: all 4 context binaries load, all 8 graphs execute, and a real templated turn produces a coherent multi-token reply on the DSP. Three small, well-scoped follow-ups remain (none are mysteries): tighten the QNN prompt budget for 2K context, handle the "context limit exceeded" status gracefully, and fix the mobile-portrait GUI layout.

**Context at session start:** PR #218 merged (`7d02130`). **End state:** clean `main`, **no commits this session** (work was the off-repo cl2048 export + on-device validation + two *reverted* throwaway diagnostic edits). Working tree clean. The device (RedMagic 11 Pro, `912607710061`) currently runs a **diagnostic APK** (smoke-check skipped, WARN logging) — that build does NOT match committed source; the committed smoke check still needs the fix below.

## What we proved this session (the big result)

1. **cl2048 fixes CMA.** The 48 h `--context-length 2048` native-V81 re-export (see "Export" below) loads cleanly: verbose genie.log shows all 4 context binaries mapped on the DSP (742+586+586+958 MB) and all 8 graphs (`prompt_ar128` + `token_ar1`, cl2048) **executed** (`execution finished with result 0`). The old cl4096 4th-binary buffer-map failure is **gone**.
2. **Real generation works.** With the construction smoke check skipped (diagnostic build), a real templated turn ("why is the sky blue?") streamed a **coherent, full reply on the NPU, faster than the owner could read.** The debug sidebar populated (pedagogy signals live) — the whole stack works end-to-end on-device.
3. **The "status 4" we hit is fully diagnosed** (was NOT memory, NOT bundle incompatibility — QAIRT versions match exactly, `2.45.0.260326154327` both): it is **"Context limit exceeded (1814 + 641 > 2048)"** — the prompt is **1814 tokens** (Socratic system prompt + 3 KB passages + question), leaving only ~234 of the 2048 context for the reply, which overran it. Full diagnosis + verbose excerpts: `~/qnn-export-2048/genie-status4-diagnosis.txt`.

## ⚠️ First action: pick a follow-up (device is connected, cl2048 bundle staged)

```bash
cd /Users/hherb/src/primer && git status            # clean main
ADB="$HOME/Library/Android/sdk/platform-tools/adb"
"$ADB" devices                                       # 912607710061; if empty, plug in USB + unlock (PIN)
"$ADB" shell run-as org.theprimer.gui cat files/qnn-bundle/genie_config.json | grep -A1 '"size"'  # expect 2048
```
The cl2048 bundle is staged on-device (`files/qnn-bundle`, `size: 2048`, native v81). The committed source is clean (both diagnostic hacks reverted). The diagnostic APK on the device skips the smoke check — to test the *real* fixes, rebuild from clean source after applying them.

## What's next — three well-scoped follow-ups (priority order)

### 1. ⭐ Trim/compress the QNN prompt to fit 2K context (the real blocker to a clean turn)
**Owner decision (2026-06-14): NOT a smaller model.** Qwen3-4B's reasoning is already borderline for the Socratic task (asking a genuinely probing follow-up *is* the reasoning load — sub-4B models degrade into quizzing or just answering). So the fix is **prompt trimming/compression**, not a smaller/larger model and not a bigger context (cl4096 doesn't fit CMA; CMA can't be grown on stock — kernel boot param, needs root). The QNN "small-context" budget (`is_small_context_backend` → 12-turn window, KB top-K 3) was sized for the **4K** bundle; on cl2048 the prompt hits **1814 tokens**, leaving only ~234 for the reply → overflow.

**Target:** prompt ≤ ~1280 tokens, reserving ~768 for the reply (Socratic replies are usually short — they ask more than they answer — so even ~512 reply room is often enough; size for the long case). **Cut ~530+ tokens.** Levers, by leverage:
- **Leaner QNN system prompt (highest leverage — the biggest fixed cost).** Author a compressed Socratic system prompt for small-context backends that keeps every pedagogical *constraint* (more questions than answers, no engagement-maximising, comprehension via transfer, etc. — see CLAUDE.md "Pedagogical principles") but in far fewer tokens. This doesn't cost conversational context at all.
- **Fewer + shorter KB passages.** KB top-K 3 → 1–2, AND truncate each passage to its most relevant ~80–120 tokens (they're whole wiki/seed passages today).
- **Token-budget-driven assembly (most robust).** Instead of fixed K/turn counts, assemble to a hard **token ceiling** that guarantees reply room: system prompt (fixed) → then add KB passages / summary / older turns until ~1280 tokens. The pedagogy layer has no tokenizer; a char/4 ≈ token proxy is good enough to gate (the QNN backend has the real tokenizer if exactness is ever needed).
- Tighter recent-turn window (12 → ~6–8) and a shorter rolling-summary cap for QNN (help later in a conversation; early-session the system prompt + KB dominate).
- Drop/shorten the passive vocab-review hints for small-context.

**Measurement tool:** the genie.log line `Context limit exceeded (PROMPT + GEN > 2048)` reports the prompt token count directly — iterate against it on-device. **Acceptance:** a real turn completes with `GenieDialog_query` returning SUCCESS (no status 4), reply stops at `<|im_end|>`, reproducibly, with pedagogy quality intact (spot-check the compressed system prompt still drives Socratic behaviour).

### 2. Handle the "context limit exceeded" status gracefully (defensive)
`primer-inference/src/qnn/genie/real.rs:365` treats **any** non-`SUCCESS` `GenieDialog_query` return as a hard error (drops the turn). The reply already streamed via the callback before the overflow, so the turn should **complete with what was generated** (or stop cleanly at the limit), not error out. Map the "context limit exceeded" terminal code to a graceful completion. (Get the exact code value from a verbose run — see below — don't blanket-accept all non-zero, which would mask real ABI errors.)

### 3. The construction smoke check (`SMOKE_CHECK_PROMPT = "."`) rambles to context-full
`fire_smoke_check_query` passes raw `"."` (bypassing the chat template) to `query_streaming`, which runs the **full** generation synchronously. Without ChatML structure the model never emits `<|im_end|>`, so it generates ~2045 tokens (~90 s!) until context-full → status 4 → construction fails. **Fix:** bound the smoke check to 1 token (early-stop the callback), OR render `"."` through the chat template, OR drop the smoke check (it was added to surface ABI mismatches at startup; a 1-token bound preserves that). This session skipped it (`run_smoke_check=false`) only to test real generation; that edit is **reverted**.

### 4. Mobile-portrait GUI layout (separate, GUI/CSS — not QNN)
On the phone in **portrait**, the desktop debug/pedagogy sidebar takes the whole width (chat window becomes a thin left stripe), the top status bar breaks, and the "hide sidebar" control is off-screen with no horizontal-scroll affordance. Landscape is fine. The sidebar + status bar need a responsive/collapsed layout on narrow screens. (Confirms the stack works — the sidebar content showed live pedagogy signals.) File as its own issue.

### Re-enabling verbose Genie logging for #2's exact code
Env injection (`PRIMER_GENIE_LOG_LEVEL`) is **SELinux-blocked** on this ROM (`wrap.<pkg>` property set is denied). To capture a verbose trace, flip `DEFAULT_GENIE_LOG_LEVEL` in `primer-inference/src/qnn/genie/log.rs:119` (`GENIE_LOG_LEVEL_WARN` → `GENIE_LOG_LEVEL_VERBOSE`), rebuild the APK, reproduce, read `genie.log` (it hits ~800k lines on a full generation — grep, don't cat). Revert after. (That's exactly how this session's diagnosis was captured.)

### Carried, owner/hardware-gated
- Real `qnn_bench` numbers — now unblocked (NPU generation works); gated on #1 so a turn completes cleanly.
- Latency-aware routing calibration (`--primary-ttft-budget-ms`) — gated on bench numbers.
- llama.cpp bench on real hardware — owner-gated.
- Full `tauri android build` in CI — deferred.
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

- **2K context is genuinely tight for the Primer's prompt.** 1814-token prompts won't fit cl2048 with reasonable reply room. #1 (prompt trimming) is mandatory, not optional. **A smaller model is ruled out (owner, 2026-06-14): Qwen3-4B's reasoning is already borderline for the Socratic task.** CMA can't be grown on stock (kernel boot param, needs root — and the target is unrooted kids' devices), and cl4096 doesn't fit CMA (698 MB buffer vs ~637 MB CmaFree even right after a reboot). So the prompt MUST be compressed to fit 2048. **Only fallback if trimming can't get there without hurting pedagogy:** a cl3072 re-export (≈524 MB buffer — fits right after a reboot but NOT at steady-state ~377 MB CmaFree, so fragile; another ~48 h throttled export). Try hard on trimming first.
- **The device APK is a diagnostic build** (smoke-check skipped, WARN). It does not match committed source. A clean rebuild (after #1–#3) restores the real smoke check — which will FAIL until #3 is done, so do #3 alongside #1.
- **The cl2048 bundle + V81 libs are git-ignored / off-repo.** Bundle: `~/qnn-export-2048/build/qwen3_4b_instruct_2507-genie-w4a16-qualcomm_snapdragon_8_elite_gen5/` (also staged on-device). Re-stage with `~/qnn-export-2048/stage-bundle.sh <SRC>`. V81 libs: re-fetch per the jniLibs README (no-login Software Center) if the tree is fresh.
- **Both runbook-#16 qai-hub upload patches are applied** in `~/venvs/qai-hub` (`EXTERNAL_RESPONSE_TIMEOUT_SECONDS=300`, `use_acceleration=False`); a venv rebuild loses them. Upload is server-side throttled (~30–250 kB/s from Bamaga) regardless of local link — the 48 h was bandwidth, not a bug.
- **Branch protection ACTIVE on `main`** (required `cargo test (default features)`, strict). New code work is PR-first.

## Patterns to reuse, not reinvent

New this session:
- **The CMA fix = a single-value `--context-length` re-export.** The export CLI's `--context-length` is a LIST defaulting to `[512,1024,2048,3072,4096]`; all those graphs bake into the weight-shared binaries and Genie inits **every** one regardless of runtime `size`, so shrinking the on-device config can't help. cl2048 (single value) drops the cl4096 graph → smaller NSP buffers that fit CMA. Buffers scale ~linearly with context (cl4096≈698 MB → cl2048≈350 MB runtime; the `.bin` *weights* are context-independent and barely change size — don't be fooled by `.bin` size).
- **Reading behind a Genie status code = a throwaway verbose APK rebuild.** Env injection is SELinux-blocked on this ROM. Flip `DEFAULT_GENIE_LOG_LEVEL` to VERBOSE (one line in `genie/log.rs`), rebuild, reproduce, grep `genie.log` (800k lines on a full generation), revert. The Genie engine layer logs `Context limit exceeded (PROMPT + GEN > CTX)` and `Step N: AR-x CL-y n_past=…` — the prompt token count and the per-step decode are right there.
- **`new_idx=2047` / clamping is NOT mis-positioning** — n_past correctly starts at the prompt length; the model just generated until the context filled. The diagnostic that matters is the `Context limit exceeded (P + G > C)` line.
- **AI Hub export is set up on this Mac** (`~/venvs/qai-hub`, authenticated, checkpoint cached in `~/.qaihm`); a re-export resumes from cache + only pays ONNX re-export + the throttled upload. `run-export.sh` is a 30-attempt resume loop. Detach long jobs with `nohup … </dev/null & disown` (reparents to launchd, survives the session).
- **QAIRT version coherence:** bundle's `tool-versions.yaml` (`qairt: 2.45.0.260326154327`) must match the staged libs' `AISW_VERSION` / `strings libQnnHtp.so | grep 2.45`. They matched, ruling out version mismatch.

Carried (prior handoffs): Android scoped storage hides `adb`-pushed `/sdcard/Android/data/<pkg>` from the app — stage app-internal; **`adb push /data/local/tmp/<f>` → `run-as <pkg> cp` WORKS** for staging (validated this session, `stage-bundle.sh`); **this RedMagic ROM has dead logcat + black screencap** — read app-internal files via `run-as cat` + owner reads the screen; **`run-as <pkg> sh -c '<redirect>'` runs at cwd `/`** (use `run-as <pkg> <cmd>` directly, e.g. `truncate -s 0 .primer/genie.log`); **a reboot leaves `/data/user/0/<pkg>` encrypted — owner enters PIN before `run-as`/app work**; **reboot maximizes CmaFree** (~637 MB right after boot vs ~377 MB settled — test memory-tight loads right after a reboot); APK rebuild = `--no-default-features --features qnn`; cargo from `src/` with `+1.88`; commits touching `.github/workflows` need `gh auth refresh -s workflow`. Android host facts: JDK 21 = Android Studio JBR for `JAVA_HOME`; NDK symlink `$ANDROID_HOME/ndk/… → /opt/homebrew/share/android-ndk`.

## Exact commands needed to resume

```bash
cd /Users/hherb/src/primer && git status        # clean main

# === Device check (cl2048 bundle staged; diagnostic APK installed) ===
ADB="$HOME/Library/Android/sdk/platform-tools/adb"
"$ADB" devices                                   # 912607710061; plug in + unlock (PIN) if empty
"$ADB" shell run-as org.theprimer.gui cat files/qnn-bundle/genie_config.json | grep -A1 '"size"'  # 2048
"$ADB" shell cat /proc/meminfo | grep -i cmafree # plenty right after a reboot

# === Read the on-device generation log (logcat is DEAD) ===
"$ADB" shell run-as org.theprimer.gui truncate -s 0 .primer/genie.log
"$ADB" shell monkey -p org.theprimer.gui -c android.intent.category.LAUNCHER 1
# owner sends a message, then:
"$ADB" shell run-as org.theprimer.gui cat .primer/genie.log | grep -i 'context limit\|exceeded'   # prompt+gen sizes

# === After applying #1–#3, rebuild + reinstall the QNN APK (clean source) ===
export ANDROID_HOME="$HOME/Library/Android/sdk"; export NDK_HOME=/opt/homebrew/share/android-ndk
export JAVA_HOME="/Applications/Android Studio.app/Contents/jbr/Contents/Home"
export PATH="$NDK_HOME/toolchains/llvm/prebuilt/darwin-x86_64/bin:$JAVA_HOME/bin:$HOME/.cargo/bin:$PATH"
cd src/crates/primer-gui
~/.cargo/bin/cargo-tauri android build --apk --debug --target aarch64 -- --no-default-features --features qnn
"$ADB" install -r gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk
# (the throwaway build script /tmp/build-qnn-apk.sh wraps the env + this command)

# === Re-stage the cl2048 bundle if needed ===
~/qnn-export-2048/stage-bundle.sh ~/qnn-export-2048/build/qwen3_4b_instruct_2507-genie-w4a16-qualcomm_snapdragon_8_elite_gen5

# === Host health check (default features = required gate) ===
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo +1.88 fmt --all -- --check
~/.cargo/bin/cargo +1.88 test --workspace
~/.cargo/bin/cargo +1.88 test -p primer-inference --features qnn genie::

# === New code work: PR-first (branch protection on) ===
git checkout -b <branch> main && git push -u origin <branch> && gh pr create --base main ...
```

## Reporting back

- State plainly what you got working and what you didn't, by acceptance criterion.
- **This session's headline:** the cl2048 re-export **resolved the CMA blocker**, and the Primer's own `QnnBackend` now **generates coherent, fast replies on the Hexagon NPU on-device, stable across a reboot** — the real Phase 1.2 finish (a working turn, not just "1 token"). The remaining `status 4` is fully diagnosed as **"Context limit exceeded"** (1814-token prompt + reply > 2048), needing a tighter QNN prompt budget for 2K context, graceful context-limit handling, and a bounded/templated smoke check — plus a separate mobile-portrait GUI layout fix.
- The GUI is a full app — when a future brief calls it a scaffold, it's wrong; trust the code.
