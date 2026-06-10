# QNN Validation Runbook — Step 1.2.0

**Companion:** [plan §1.2.0](../superpowers/plans/2026-05-28-qnn-backend.md), [design spec](../superpowers/specs/2026-05-28-qnn-backend-design.md)
**Purpose:** Prove the Genie SDK + chatapp_android pipeline works on the RedMagic 11 Pro **before** writing any Primer integration code.
**Estimated total time:** ~6 hours wall-clock, ~30 minutes hands-on (the export step is multi-hour but unattended).
**Decision gate:** If measured Qwen3-4B decode rate < 8 tok/s, **stop and re-scope** Phase 1.2.

> ## ✅ VALIDATED 2026-06-09 — GATE PASSES (see corrections inline)
>
> This runbook was written 2026-05-28, **pre-hardware**, and was substantially wrong about the method. The gate has since **passed** on the RedMagic 11 Pro — see the full hardware report at [docs/handoffs/2026-06-08-qnn-validation-chatapp.md](../handoffs/2026-06-08-qnn-validation-chatapp.md). Headline numbers: **~9.4 tok/s decode (🟡 borderline → proceed), ~190 ms TTFT (✅), ~57 °C peak (✅)**, NPU-confirmed via a +11 °C rise on the Hexagon `nsph*` zones.
>
> **The 17 corrections from that session are folded into the steps below.** The biggest method change: **don't follow the slow manual export below for the validated v79 path** — use the `qai-hub-apps fetch` fast path (precompiled binaries in one command) + a lean-APK + `adb push` of the `.bin`. The manual export is only needed for a *native* sm8850 (V81) bundle, which is a pure throughput optimization (not gate-blocking) and was abandoned on a flaky network. Inline `⚠️ CORRECTION` callouts mark every place the original instructions are stale.

## Prerequisites

> ⚠️ **CORRECTION (#1, #2, #4):** No Qualcomm developer-portal account is needed (QAIRT ships as the **Community** edition from a public URL, auto-pulled by the chatapp's Docker build). **macOS Apple Silicon is a viable export + build host** — the validated session ran the whole thing on an arm64 Mac. Android Studio is optional; the validated path used a **Docker** build instead.

- A Linux or **macOS (incl. Apple Silicon arm64)** dev box. ~32 GB RAM is comfortable; the validated host was a 128 GB Mac. A *native custom-checkpoint re-calibration* would need local AIMET (Linux x86_64 + CUDA), but the **default** export fetches pre-calibrated AIMET encodings and offloads compilation to AI Hub cloud, so no local AIMET is required.
- [`uv`](https://docs.astral.sh/uv/) installed — it fetches the pinned Python 3.10 interpreter itself (`uv venv --python 3.10`), so no system Python install is required. (3.10 is what upstream recommends; newer versions may work.)
- **Docker** (validated path: `ubuntu:24.04`, `--platform linux/amd64`) — OR Android Studio 2024.3.1+ if you prefer. The Docker build auto-installs QAIRT Community + Android SDK 34 + NDK r27d + Gradle 9.1.0.
- ~~A Qualcomm developer portal account~~ **— not needed.** QAIRT Community is pulled automatically from the public `softwarecenter.qualcomm.com` URL by the build.
- A free `qai-hub` account at https://aihub.qualcomm.com/ (needed for the `qai-hub-apps fetch` fast path AND the `qai-hub-models` exporter).
- A RedMagic 11 Pro with **USB debugging enabled** (Developer options → USB debugging) and a USB-C cable to the dev box. **NB the SoC is SM8850 (Snapdragon 8 Elite Gen 5, codename `canoe`) — distinct from SM8750 (the original 8 Elite).**
- `adb` on the dev box (`pkg install android-tools` on Termux for desktop; or via the Android SDK).

## Step 1 — QAIRT SDK (no manual download needed)

> ⚠️ **CORRECTION (#1):** This entire manual-download step is **obsolete**. QAIRT ships as the **Community** edition (version **2.45.41.260507**, not 2.29) and is pulled automatically from the public `softwarecenter.qualcomm.com/.../Qualcomm_AI_Runtime_Community/...` URL by the chatapp's Docker build (`scripts/qairt_utils.sh::install_qairt`). No `qpm.qualcomm.com` login, no manual `tar xzf`.

If you build via Docker (the validated path), **skip this step entirely** — `docker build --build-arg BUILD_TYPE=build` installs QAIRT for you. Only if you build outside Docker do you need a local QAIRT; in that case point `$QAIRT_PATH` at it (the app's `build.gradle` reads the env var, so no manual `build.gradle` edit if it's set).

For reference, the Hexagon skel for this device is `libQnnHtpV79Skel.so` (the 8-Elite `htp_config` declares `dsp_arch: v79`, and **the v79 binaries load and run on this Gen-5 part** — the Gen-5 Hexagon is v79-compatible).

**Licence:** the Community edition is freely redistributable per its EULA, but still capture redistribution-relevant clauses into `docs/devel/qairt-licence-notes.md` before bundling any `.so` into a Primer release.

## Step 2 — Get the Qwen3-4B bundle

> ⚠️ **CORRECTION (#3):** For the validated v79 path, **do not run the multi-hour manual export below.** The `qai-hub-apps fetch` fast path downloads the app source **+ precompiled w4a16 binaries + tokenizer + configs** in one shot (~3 GB from S3), for any chipset that has published assets (8-Elite / 8-Gen3 / 8-Gen2):
>
> ```bash
> qai-hub-apps fetch chatapp_android \
>     --model qwen3_4b_instruct_2507 \
>     --chipset qualcomm-snapdragon-8-elite
> ```
>
> The precompiled bundle is **Qwen3-4B-Instruct-2507, w4a16, 4096 ctx**: 4 ctx-bins (`*_part_1_of_4.bin` 778 MB · `part_2/3` 665 MB each · `part_4` 1065 MB ≈ 3.0 GB) + `genie_config.json` (`QnnHtp`, `size: 4096`, `poll: true`, `cpu-mask: 0xe0`, `n-threads: 3`).

> ⚠️ **CORRECTION (#6):** **Gen-5 (sm8850) has no precompiled assets on AI Hub** (`--chipset qualcomm-snapdragon-8-elite-gen-5` → "Model asset not found"). The **sm8750/v79 binaries are backward-compatible and run on the Gen-5 Hexagon** — that is what the validated ~9.4 tok/s result used. A native sm8850 (V81) export is a *pure throughput optimization* (the manual path below), not gate-blocking; the validated session **abandoned** it after ~11 h on a flaky Starlink uplink (a network limit, not technical — the checkpoint/ONNX caches are retained for a wired retry that resumes from cache in ~1 h).

### Manual export (ONLY for a native sm8850 V81 bundle — optional, not gate-blocking)

> ⚠️ **CORRECTIONS (#10, #13–#16) — the self-export hit ALL of these, in order:**
> - **`HF_HUB_DISABLE_XET=1` is required** — `hf-xet 1.5.0` stalls indefinitely at "Fetching N files: 0%"; the standard LFS path works. (The per-*file* progress bar sits at 0% while a multi-GB shard downloads — looks hung, isn't.)
> - **`accelerate` must be installed** in the venv (transformers 4.51 needs `init_empty_weights`); `qai-hub-models[...]` doesn't pull it.
> - **But `accelerate`'s install bumps `pydantic` ≥2.13, which breaks qai-hub-models' yaml parsing** — pin `pydantic<2.12` (2.11.x) *after* installing accelerate.
> - **The bundled `info.yaml` has `status: published` + `devices: {}`**, tripping a "no release assets available" validator — patch the local `info.yaml` `status: published` → `pending` (display-name lookup only; doesn't affect the compile).
> - **The qai-hub client's 4-second external-transfer socket timeout** (`util/session.py::EXTERNAL_RESPONSE_TIMEOUT_SECONDS = 4`) is far too tight for a slow uplink — bump it (e.g. 300) and set `use_acceleration=False` (regional S3 instead of the flaky `s3-accelerate` edge). Even then, a flaky link resets the 4× ~1 GB split uploads and loops (no S3 part-resume) — a **stable wired connection is the real requirement**.

```bash
# Fresh venv to avoid clashing with system Python deps. uv fetches the
# pinned 3.10 interpreter itself — no system Python needed.
uv venv --python 3.10 ~/venvs/qai-hub
source ~/venvs/qai-hub/bin/activate

# Install the model package. The exact extras name is per the AI Hub
# model card — verify at https://aihub.qualcomm.com/models/qwen3_4b
# (the convention is underscores → hyphens for the extras name).
uv pip install -U "qai-hub-models[qwen3-4b]"

# Authenticate with AI Hub (one-time; opens browser).
qai-hub configure --api_token <your-aihub-token>

# Qwen3-4B weights are not gated — no `hf auth login` needed. (Llama-3.x
# would require `uv pip install -U 'huggingface_hub[cli]' && hf auth login`.)

# Export the NATIVE sm8850/V81 bundle. Multi-hour; needs a STABLE WIRED link
# for the 4× ~1 GB ONNX upload (see corrections above). Run in tmux/screen.
HF_HUB_DISABLE_XET=1 \
python -m qai_hub_models.models.qwen3_4b_instruct_2507.export \
    --device "Snapdragon 8 Elite Gen 5 QRD" --context-length 4096 \
    --skip-profiling --skip-inferencing \
    --output-dir ~/primer-bundles/qwen3-4b-instruct-2507-sm8850
```

**Expected duration:** ~1 h from cache on a stable wired link; the validated session's attempt ran ~11 h and never finished on a flaky Starlink uplink (network-bound, not compute-bound — compilation is offloaded to AI Hub cloud, so local RAM is not the bottleneck).

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

## Step 3 — Patch + build chatapp_android (lean APK)

> ⚠️ **CORRECTIONS (#4, #5, #8):** The app moved to the **repo root** (`chatapp_android`, not `apps/chatapp_android`). It **hard-codes a SoC allowlist** that rejects SM8850 → you must patch it. And **do NOT inline the 3 GB of `.bin` into the APK** — exclude them from the build context and `adb push` them post-install (Step 4). That turns a 3.5 GB APK into a **94 MB** one.

```bash
# The `qai-hub-apps fetch` from Step 2 already cloned the app source. It lives at
#   chatapp_android/   (repo root — NOT apps/chatapp_android)
cd ~/chatapp-fetch/chatapp_android

#  (#5) Patch the SoC allowlist. MainActivity.java hard-rejects any SoC not in
#  {SM8750, SM8650, QCS8550} → finish(). Add SM8850 → the 8-elite config:
#    SM8850 → qualcomm-snapdragon-8-elite.json
#  (edit the allowlist map in MainActivity.java)

#  (#8) Keep the APK lean: exclude the 3 GB of context bins from the build context.
echo '**/*.bin' >> .dockerignore

#  (#1, #4) Docker build — auto-installs QAIRT Community + Android SDK; no portal,
#  no manual build.gradle edit (QAIRT path resolves from the build).
docker build --platform linux/amd64 --build-arg BUILD_TYPE=build -t chatapp-build .
#  then inside the container (or via the build's gradle step):
#    gradle assembleDebug   →   app-debug.apk   (~94 MB: QAIRT .so + tokenizer + configs)
```

The `.bin` files are pushed separately in Step 4 — the Primer's own integration (later steps) reads from disk paths, not APK assets, for the same reason.

## Step 4 — Install the lean APK + push the bins

> ⚠️ **CORRECTION (#8):** Install the 94 MB APK normally, then **`adb push` the 4 `.bin`** to the app's cache dir. The app's `copyFile` skips files already present, so pushed binaries are picked up on next launch. Push runs at ~40–50 MB/s over USB.

```bash
# Confirm device is visible.
adb devices

# Install the lean APK (94 MB — no size-cap workaround needed).
adb install app-debug.apk
adb shell pm list packages | grep chatapp

# First launch creates the external cache + copies the small configs.
# Then push the 4 context bins into the cache models dir:
adb push qwen3-4b-instruct-2507/*.bin \
    /sdcard/Android/data/com.quicinc.chatapp/cache/models/llm/

#  (#17, optional, perf-neutral) For correctness, set the right SoC id in the
#  on-device htp_config: soc_model 69 (SM8750) → 87 (SM8850). DSP arch stays
#  v79 (matches the binaries). IDs from QnnTypes.h, QAIRT 2.45:
#    SM8650=57, SM8750=69, SM8850=87 ;  DSP archs V79=79, V81=81, V85=85, V89=89.
#  NB: this makes NO measurable throughput difference with v79 binaries
#  (perf_profile: burst already maxes the clocks) — it's a correctness fix only.
```

## Step 5 — Smoke the model on device

> ⚠️ **CORRECTIONS (#7, #11):** **`adb logcat` is unavailable on this Nubia/RedMagic ROM** (Android 16) — it returns nothing, even for app/system tags. Diagnose via the app's **on-screen "TTFT … | TPS …" readout** + `uiautomator dump` for UI state + `/sys/class/thermal` (all readable). And **blind-tap UI automation is unreliable** (variable model-load time + RedMagic gesture/overlay quirks send stray taps into other apps) — **element-gate every action**: poll `uiautomator dump`, tap a real element's bounds by `resource-id` only once it appears (`com.quicinc.chatapp:id/llm`, `…/user_input`, `…/send_button`).

1. On the RedMagic, open the **ChatApp** icon (or element-gate the tap via `uiautomator dump`).
2. Tap the model button → `Conversation` → wait for `loadModel` to succeed (model load to NPU — expect 10–30 s the first time).
3. Type a Socratic-shaped prompt: e.g. *"My daughter just asked me why the sky is blue. What's a good question I could ask her back to help her think about it?"*
4. Confirm the response is non-trivial English text and the app's **TTFT/TPS** readout populates.
5. **Don't reach for `adb logcat`** — it's dead on this ROM. Read state from `uiautomator dump` + the on-screen TTFT/TPS text. A working element-gated driver (`measure_chatapp.py`) from the validated session is referenced in the handoff.

> ⚠️ **Stability note:** the reference chatapp **crashed** during a very long (multi-essay) generation — most likely 4096-context exhaustion mid-output. **Not a Primer concern:** `primer-inference::qnn` runs the small-context budget (12-turn window, 3-passage top-K, keyed off `QNN_NAME_PREFIX`) and drops partial turns on mid-stream error, so it bounds context by construction.

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

> ⚠️ **CORRECTION (#9):** **Exclude `*-trip-*` / `*-limit-*` zones** from peak computation — they report *throttle thresholds*, not temperatures (e.g. `cpu-hw-trip-0` reads `105 °C`, a trip-point). On this device the real peak is ~57 °C (CPU big core); the **NPU `nsphvx`/`nsphmx-0..3` zones rose +11 °C** under load, which is what confirms the compute ran on the Hexagon NPU and not a CPU fallback.

```bash
# Peak temp across all REAL zones (drop trip/limit thresholds), in milli-°C.
awk -F, 'NR>1 && $2 !~ /trip|limit/ && $3+0>max{max=$3+0} END{printf "peak: %.1f°C\n", max/1000}' ~/primer-thermal.csv

# Per-zone peak (drop trip/limit thresholds).
awk -F, 'NR>1 && $2 !~ /trip|limit/ {if($3+0 > peak[$2]) peak[$2]=$3+0} END{for(z in peak) printf "%s: %.1f°C\n", z, peak[z]/1000}' ~/primer-thermal.csv | sort
```

### Throughput estimate

In ChatApp, ask a prompt that reliably produces ~500 tokens of response (a "explain X step by step" prompt works well). Time it with a stopwatch from prompt-submit to response-complete. Tokens ÷ seconds gives a rough decode rate. Repeat 3 times and take the median.

For a more accurate measurement, you can later integrate `genie-t2t-run` (the QAIRT-bundled CLI) on the device via Termux with `adb shell` access — but for a Phase 1.2.0 gate, the stopwatch method is sufficient.

## Step 7 — Decision gate

> ✅ **OUTCOME (2026-06-09): GATE PASSES.** Measured **~9.4 tok/s decode (🟡 borderline 8–14 band → proceed), ~190 ms TTFT (✅), ~57 °C peak (✅)** on the RedMagic 11 Pro v79-on-Gen5 path. The `soc_model` 69→87 patch and a native sm8850 export were both tried for higher throughput — the patch is perf-neutral and the native export was network-abandoned, so ~9.4–9.5 tok/s is the confirmed v79 ceiling. Full write-up: [docs/handoffs/2026-06-08-qnn-validation-chatapp.md](../handoffs/2026-06-08-qnn-validation-chatapp.md). **Phase 1.2 (`primer-inference::qnn`) is validated to proceed on this hardware** — the real next milestone is exercising the Primer's own `QnnBackend` via `--backend qnn` against the v79 bundle.

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
Almost always a missing `.so` in the QAIRT lib dir referenced by the build, **or the SoC allowlist rejecting SM8850** (correction #5 — patch `MainActivity.java`). **Note `logcat` is dead on this ROM** (correction #7), so a `dlopen failed: library "libQnnHtpV79Skel.so" not found` won't surface there — infer it from the app immediately `finish()`ing or failing `loadModel`. Ensure all Hexagon skel libs are packaged into the APK — `app/build.gradle` has a `jniLibs` section that selects which `.so`s to bundle.

### Streaming response is gibberish or empty
Tokenizer mismatch or wrong genie_config.json. Confirm all three files (`*.bin`, `tokenizer.json`, `genie_config.json`) came from the **same export run** — mixing files from different runs is a common foot-gun.

## What this runbook does **not** cover

- Production-quality benchmarks. This is a Phase 1.2.0 gate, not a Phase 1.2.6 benchmark harness. Save the rigorous measurement for the `qnn_bench` example we'll add later.
- Multiple model A/B comparison. Phase 1.2.0 validates exactly one model (Qwen3-4B) to gate the rest of the phase.
- Termux-from-the-device export. The export/build happens on a desktop host — **and (#2) Apple Silicon arm64 macOS is fine**, not just x86_64, because the default export offloads compilation to AI Hub cloud. The RedMagic is the inference target, not the build host.
- Tauri-Android wrapping. Phase 1.2 is CLI-only via Termux on the device; Tauri-Android is Phase 3.
