# QNN Validation Runbook — Step 1.2.0

**Companion:** [plan §1.2.0](../superpowers/plans/2026-05-28-qnn-backend.md), [design spec](../superpowers/specs/2026-05-28-qnn-backend-design.md)
**Purpose:** Prove the Genie SDK + chatapp_android pipeline works on the RedMagic 11 Pro **before** writing any Primer integration code.
**Estimated total time:** ~6 hours wall-clock, ~30 minutes hands-on (the export step is multi-hour but unattended).
**Decision gate:** If measured Qwen3-4B decode rate < 8 tok/s, **stop and re-scope** Phase 1.2.

## Prerequisites

- A Linux or macOS dev box with at least 32 GB RAM + swap. The `qai-hub-models` export step needs significant memory.
- Python 3.10 installed (newer versions may work but 3.10 is what upstream recommends).
- Android Studio 2024.3.1 or newer.
- A Qualcomm developer portal account (free signup at https://qpm.qualcomm.com/) for QAIRT SDK download. **No paid licence required for evaluation.**
- A free `qai-hub` account at https://aihub.qualcomm.com/ (needed by the `qai-hub-models` exporter to fetch pre-compiled context binaries).
- A RedMagic 11 Pro with **USB debugging enabled** (Developer options → USB debugging) and a USB-C cable to the dev box.
- `adb` on the dev box (`pkg install android-tools` on Termux for desktop; or via Android Studio).

## Step 1 — QAIRT SDK install

1. Sign in at https://qpm.qualcomm.com/ and download **QAIRT SDK 2.29 or newer** for Linux/macOS x86_64.
2. Extract somewhere persistent (not `/tmp`):
   ```bash
   mkdir -p ~/sdk
   tar xzf qairt-2.29.x.tar.gz -C ~/sdk/
   export QAIRT_SDK_ROOT=~/sdk/qairt/2.29.x
   echo "export QAIRT_SDK_ROOT=$QAIRT_SDK_ROOT" >> ~/.bashrc   # (or ~/.zshrc)
   ```
3. Sanity check:
   ```bash
   ls $QAIRT_SDK_ROOT/lib/aarch64-android/libGenie.so       # should exist
   ls $QAIRT_SDK_ROOT/lib/aarch64-android/libQnnHtp.so      # should exist
   ls $QAIRT_SDK_ROOT/lib/aarch64-android/libQnnHtpV79Skel.so   # Hexagon v79 = 8 Elite Gen 5
   ```
   If `libQnnHtpV79Skel.so` is missing, the SDK is too old for Snapdragon 8 Elite Gen 5 — upgrade.
4. Read the QAIRT EULA. Capture redistribution-relevant clauses into `docs/devel/qairt-licence-notes.md` (a separate doc, deferred to step 1.2.1 prep). Until that file exists, treat the QAIRT bundle as **not redistributable** — every Primer user installs it themselves.

## Step 2 — Export the Qwen3-4B genie_bundle

```bash
# Fresh venv to avoid clashing with system Python deps.
python3.10 -m venv ~/venvs/qai-hub
source ~/venvs/qai-hub/bin/activate

# Install the model package. The exact extras name is per the AI Hub
# model card — verify at https://aihub.qualcomm.com/models/qwen3_4b
# (the convention is underscores → hyphens for the extras name).
pip install -U "qai-hub-models[qwen3-4b]"

# Authenticate with AI Hub (one-time; opens browser).
qai-hub configure --api_token <your-aihub-token>

# Qwen3-4B weights are not gated — no `hf auth login` needed. (Llama-3.x
# would require `pip install -U 'huggingface_hub[cli]' && hf auth login`.)

# Export. This is the multi-hour step. Run it in a tmux/screen session.
mkdir -p ~/primer-bundles
python -m qai_hub_models.models.qwen3_4b.export \
    --chipset qualcomm-snapdragon-8-elite \
    --skip-profiling \
    --output-dir ~/primer-bundles/qwen3-4b
```

**Expected duration:** 2–3 hours on a 32 GB host. Heavy RAM + swap pressure for the second hour; do not run on a laptop you need to use.

**Expected output** under `~/primer-bundles/qwen3-4b/`:

```text
genie_config.json
tokenizer.json
weight_sharing_model_1_of_N.serialized.bin
weight_sharing_model_2_of_N.serialized.bin
... (typically N=4 for a 4B model at 4K context)
htp_backend_ext_config.json
```

**If you want lower memory pressure on the device**, add `--context-length 2048` to the export (the Primer prompt budget is tight even at 4K — see spec §8 — so 4K is the default we want to validate, but a 2K-ctx bundle is a useful fallback if 4K won't load).

## Step 3 — Build chatapp_android

```bash
# Clone if you haven't already.
git clone https://github.com/quic/ai-hub-apps.git ~/code/ai-hub-apps

# Stage the bundle into the app's asset path.
cd ~/code/ai-hub-apps/apps/chatapp_android
mkdir -p app/src/main/assets/models/llm/
cp ~/primer-bundles/qwen3-4b/*.bin           app/src/main/assets/models/llm/
cp ~/primer-bundles/qwen3-4b/tokenizer.json   app/src/main/assets/models/llm/
cp ~/primer-bundles/qwen3-4b/genie_config.json app/src/main/assets/models/llm/
# (htp_backend_ext_config.json goes alongside; check the app's README for
# any per-model genie_config.json placeholders that need substitution.)

# Open in Android Studio. Confirm QAIRT_SDK_ROOT is honoured by build.gradle
# (the app reads it via System.getenv() during Gradle sync).
echo $QAIRT_SDK_ROOT

# In Android Studio:
#   1. File → Open → ~/code/ai-hub-apps/apps/chatapp_android
#   2. Wait for Gradle sync. If it fails on QAIRT_SDK_ROOT, edit
#      app/build.gradle directly: ext.qairtSdkRoot = "/absolute/path"
#   3. Build → Build Bundle(s) / APK(s) → Build APK(s)
#   4. Output lands at app/build/outputs/apk/release/ChatApp-release.apk
#      (or debug/ChatApp-debug.apk if you choose debug variant)
```

**Note on bundle size:** the asset directory under `app/src/main/assets/models/llm/` will be ~3.5 GB for Qwen3-4B. Gradle's default APK packaging will inline this — the resulting APK is ~3.5 GB, which is fine for sideloading but never appropriate for a Play Store submission. The Primer integration in subsequent steps avoids this by reading from disk paths, not assets.

## Step 4 — Sideload to the RedMagic 11 Pro

```bash
# Confirm device is visible.
adb devices
# expect:
# List of devices attached
# <serial>    device

# Push and install. The `pm install -t` form on the device works around
# the 1 GB adb-install size cap.
adb push app/build/outputs/apk/release/ChatApp-release.apk /data/local/tmp/
adb shell pm install -t /data/local/tmp/ChatApp-release.apk

# Confirm install.
adb shell pm list packages | grep chatapp
```

**If `pm install` fails with `INSTALL_FAILED_INSUFFICIENT_STORAGE`**, free up phone storage (the install temporarily needs ~7 GB for the unpacked assets).

## Step 5 — Smoke the model on device

1. On the RedMagic, open the **ChatApp** icon from the app drawer.
2. Wait for the first-prompt latency (model load to NPU — expect 10–30 s the first time).
3. Type a Socratic-shaped prompt: e.g. *"My daughter just asked me why the sky is blue. What's a good question I could ask her back to help her think about it?"*
4. Confirm the response is non-trivial English text and the streaming starts within a few seconds.
5. If nothing happens or the app crashes, capture logs:
   ```bash
   adb logcat -d > ~/chatapp-logcat.txt
   grep -E 'Genie|QNN|ChatApp' ~/chatapp-logcat.txt | head -50
   ```

## Step 6 — Capture throughput and thermal

Without an in-app stopwatch, the simplest approach is to time a known prompt-length response against the device clock and sample thermal in parallel.

### Thermal sampling (from dev box, while interacting with the phone)

```bash
# In one terminal: stream thermal samples to a CSV.
(
  echo "timestamp,zone,temp_milli_c"
  while true; do
    for z in $(adb shell ls /sys/class/thermal | tr -d '\r' | grep '^thermal_zone'); do
      t=$(adb shell cat /sys/class/thermal/$z/temp 2>/dev/null | tr -d '\r')
      echo "$(date -u +%FT%TZ),$z,$t"
    done
    sleep 2
  done
) > ~/primer-thermal.csv
```

Let this run for 5 minutes while you carry on a multi-turn conversation in ChatApp. Then Ctrl+C and inspect:

```bash
# Peak temp across all zones, in milli-degrees C.
awk -F, 'NR>1 && $3+0>max{max=$3+0} END{printf "peak: %.1f°C\n", max/1000}' ~/primer-thermal.csv

# Per-zone peak.
awk -F, 'NR>1 {if($3+0 > peak[$2]) peak[$2]=$3+0} END{for(z in peak) printf "%s: %.1f°C\n", z, peak[z]/1000}' ~/primer-thermal.csv | sort
```

### Throughput estimate

In ChatApp, ask a prompt that reliably produces ~500 tokens of response (a "explain X step by step" prompt works well). Time it with a stopwatch from prompt-submit to response-complete. Tokens ÷ seconds gives a rough decode rate. Repeat 3 times and take the median.

For a more accurate measurement, you can later integrate `genie-t2t-run` (the QAIRT-bundled CLI) on the device via Termux with `adb shell` access — but for a Phase 1.2.0 gate, the stopwatch method is sufficient.

## Step 7 — Decision gate

Write up the results to `docs/handoffs/2026-MM-DD-qnn-validation-chatapp.md`. The minimum content:

- **Hardware**: RedMagic 11 Pro, Snapdragon 8 Elite Gen 5, Android version, kernel, free RAM at test start.
- **Software**: QAIRT version, qai-hub-models version, Android Studio version, chatapp_android commit SHA.
- **Bundle**: Qwen3-4B, context length, total disk size.
- **Numbers**: median decode tok/s, time-to-first-token (3 runs), peak thermal across all zones, sustained thermal over 5 minutes.
- **Verdict**: pass / borderline / fail against the gates below.

| Metric | Pass | Borderline | Fail |
|---|---|---|---|
| Decode tok/s (median, 500-token response) | ≥ 15 | 8–14 | < 8 |
| Time-to-first-token | ≤ 3 s | 3–8 s | > 8 s |
| Peak thermal | ≤ 70 °C | 70–80 °C | > 80 °C |

**Pass** → proceed to step 1.2.1 (the `primer-qnn-sys` crate) as planned.
**Borderline** → file a follow-up issue listing the tradeoffs, but proceed; the Primer's conversational latency budget is more forgiving than ChatApp's raw throughput suggests because the dialogue manager already absorbs ~1 s of inter-turn delay.
**Fail** → halt step 1.2.1. Either (a) try a smaller model (Phi-3.5-Mini-Instruct or Llama-3.2-3B), (b) reduce context length to 2K, or (c) defer Phase 1.2 until next QAIRT release.

## Troubleshooting

### `qai-hub configure` fails with "Invalid token"
You signed up for the Hugging Face account, not the AI Hub account. They are separate services. The AI Hub token comes from the account settings page at https://aihub.qualcomm.com/.

### Export crashes with OOM mid-run
Add swap space (`sudo fallocate -l 32G /swap && sudo swapon /swap`) or use a host with more RAM. The export is one of the more memory-hungry pieces of the qai-hub-models toolchain.

### Gradle sync fails: `Could not find QAIRT SDK`
Either `QAIRT_SDK_ROOT` isn't exported in the shell Android Studio inherited from, or the path in `app/build.gradle` is wrong. Edit `app/build.gradle` directly and hard-code the absolute path as a workaround.

### App installs but crashes on launch
Almost always a missing `.so` in the QAIRT lib dir referenced by the build. Check `logcat` for `dlopen failed: library "libQnnHtpV79Skel.so" not found` or similar. The fix is to ensure all Hexagon skel libs are packaged into the APK by the build — `app/build.gradle` typically has a `jniLibs` section that selects which `.so`s to bundle.

### Streaming response is gibberish or empty
Tokenizer mismatch or wrong genie_config.json. Confirm all three files (`*.bin`, `tokenizer.json`, `genie_config.json`) came from the **same export run** — mixing files from different runs is a common foot-gun.

## What this runbook does **not** cover

- Production-quality benchmarks. This is a Phase 1.2.0 gate, not a Phase 1.2.6 benchmark harness. Save the rigorous measurement for the `qnn_bench` example we'll add later.
- Multiple model A/B comparison. Phase 1.2.0 validates exactly one model (Qwen3-4B) to gate the rest of the phase.
- Termux-from-the-device export. The export must happen on a Linux/macOS x86_64 host with sufficient RAM. The RedMagic is the inference target, not the build host.
- Tauri-Android wrapping. Phase 1.2 is CLI-only via Termux on the device; Tauri-Android is Phase 3.
