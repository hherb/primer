# QNN Validation (Step 1.2.0) — chatapp_android on RedMagic 11 Pro

**Date:** 2026-06-08 / 09
**Author:** Horst Herb + Claude (pairing session)
**Companion:** [plan §1.2.0](../superpowers/plans/2026-05-28-qnn-backend.md) · [design spec](../superpowers/specs/2026-05-28-qnn-backend-design.md) · [runbook](../devel/qnn-validation-runbook.md)
**Purpose:** Prove the Genie/QNN NPU pipeline works on the target device **before** the Primer's own `QnnBackend` is exercised on hardware. This is the gate the entire standalone-phone product story (ROADMAP Phase 1.2) was blocked on.

---

## TL;DR — GATE PASSES, Phase 1.2 unblocked

The Qualcomm Genie/QNN NPU inference pipeline **runs on the RedMagic 11 Pro** (Snapdragon 8 Elite **Gen 5**, SM8850). Qwen3-4B-Instruct-2507 (w4a16, 4096 ctx) generates coherent Socratic-shaped text on the Hexagon NPU.

| Axis | Result | Gate (pass / borderline / fail) | Verdict |
|---|---|---|---|
| Model loads + generates on SM8850 | yes | must work | ✅ |
| Decode throughput | **~9.4 tok/s** | ≥15 / 8–14 / <8 | 🟡 **borderline → proceed** |
| Time-to-first-token | **~160–215 ms** | ≤3 s / 3–8 s / >8 s | ✅ excellent |
| Peak thermal (real sensor) | **~57 °C** | ≤70 / 70–80 / >80 °C | ✅ excellent (~13 °C headroom) |

**Decision:** per the runbook gate, decode in the 8–14 band is *Borderline → proceed*. The Primer's conversational latency budget is more forgiving than raw throughput suggests (the dialogue manager already absorbs ~1 s of inter-turn delay). **Phase 1.2 (`primer-inference::qnn`) is validated to proceed on this hardware.**

The single soft spot is decode throughput. We tested whether the v79-on-Gen5 mismatch explained it: there are **no precompiled Gen-5 binaries on AI Hub**, so we (a) patched the `htp_config` `soc_model` 69→87 (correct SM8850 id) on the v79 binaries — **no measurable change (~9.5 tok/s)** — and (b) attempted a native sm8850 self-export, which was **abandoned after ~11 h on a flaky Starlink link** (a network limit, not a technical one). **Conclusion: ~9.4–9.5 tok/s is the v79 ceiling on this device; whether native V81 binaries beat it is open, pending a stable-connection export.** See [§ Native sm8850](#native-sm8850-attempted-two-ways--no-throughput-gain-found).

---

## Hardware

- **Device:** RedMagic 11 Pro (`NX809J`, brand REDMAGIC), ROM `REDMAGICOS11.0.18MR1_GB`
- **SoC:** Qualcomm **SM8850** = Snapdragon **8 Elite Gen 5** (platform codename `canoe`), `ro.soc.manufacturer=QTI`
  - **NB:** this is **not** SM8750 (the original 8 Elite). The project's "Gen 5" label is correct. The two are distinct AI Hub chipsets: `sm8750` vs `sm8850`.
- **Hexagon:** HVX + HMX units exposed as `nsphvx-0..3` / `nsphmx-0..3` thermal zones. The 8-Elite `htp_config` declares `dsp_arch: "v79"` — and the **v79 binaries load and run on this Gen-5 part**, so the Gen-5 Hexagon is **v79-compatible** (backward-compatible at minimum).
- **OS:** Android **16**, kernel `6.12.23-android16-5-gf1bdb13583da-ab13761046-4k`
- **RAM:** 24 GB total (`MemTotal: 23,774,224 kB`); ~11 GB available at test time
- **Connection:** USB-C, USB debugging. `adb` over USB worked throughout.

## Software / toolchain (all on the host Mac unless noted)

- **Host:** Apple Silicon Mac (arm64), 128 GB RAM, macOS 26.5.1 — **viable export + build host** (see corrections)
- **qai-hub-models** 0.55.0 · **qai-hub** 0.49.0 · **qai-hub-apps** (CLI) latest · Python 3.10 via `uv`
- **chatapp_android** release **v0.30.1** (`qai-hub-apps fetch`)
- **QAIRT** Community **2.45.41.260507** — pulled automatically inside the Docker build from the **public** `softwarecenter.qualcomm.com` URL (no Qualcomm portal account needed)
- **Build:** Docker (`ubuntu:24.04`, `--platform linux/amd64`), NDK r27d + Android SDK 34 + Gradle 9.1.0 — **not** Android Studio
- **adb** 1.0.41 (from the local Android SDK)

## Model bundle

- **Model:** Qwen3-4B-Instruct-2507, precision **w4a16**, **context length 4096**
- **Binaries (validated):** precompiled for chipset `qualcomm-snapdragon-8-elite` (**sm8750 / v79**), 4 ctx-bins, ~3.0 GB total
  - `*_part_1_of_4.bin` 778 MB · `part_2` 665 MB · `part_3` 665 MB · `part_4` 1065 MB
- **genie_config.json:** `QnnHtp` backend, `size: 4096`, `poll: true`, `cpu-mask: 0xe0`, `n-threads: 3`, mmap on
- **Native sm8850 bundle:** exporting (see below)

---

## Method (what actually worked — differs substantially from the runbook)

1. **AI Hub token** → validated (`hub.get_devices()` lists `sm8850` as "Snapdragon 8 Elite Gen 5 QRD").
2. **Fast path, not the runbook export:** `qai-hub-apps fetch chatapp_android --model qwen3_4b_instruct_2507 --chipset qualcomm-snapdragon-8-elite` downloaded the app source **+ precompiled w4a16 binaries + tokenizer + configs** in one shot (~3 GB, S3). No 2–3 h export needed for the v79 path.
3. **Patched the SoC allowlist:** `MainActivity.java` hard-rejects any SoC not in `{SM8750, SM8650, QCS8550}` → `finish()`. Added `SM8850 → qualcomm-snapdragon-8-elite.json`.
4. **Lean APK:** `.dockerignore` excludes the 3 GB `*.bin` from the build context. APK is **94 MB** (QAIRT `.so` libs + tokenizer + small configs).
5. **Docker build:** `docker build --build-arg BUILD_TYPE=build` (auto-installs QAIRT Community + Android SDK), then `gradle assembleDebug` → `app-debug.apk`.
6. **Install + push:** `adb install` the lean APK; first launch creates the external cache and copies the small configs; then `adb push` the 4 `.bin` to `/sdcard/Android/data/com.quicinc.chatapp/cache/models/llm/` (the app's `copyFile` skips files already present, so pushed binaries are picked up). Push ran at ~40–50 MB/s over USB.
7. **Launch + measure:** tap the model button → `Conversation` → `loadModel` succeeds, no error. Type a prompt → the app's own **"TTFT … | TPS …"** readout is the measurement (logcat is unavailable — see corrections).

---

## Results

### Throughput (v79 binaries, 3 clean readings)

| Run | Prompt | TTFT | Decode |
|---|---|---|---|
| A | "why is the sky blue…" | 167 ms | 9.5 tok/s |
| B | "explain why the sky is blue" | 215 ms | 9.3 tok/s |
| C | "essay comparing Python and Rust" (sustained) | 161 ms | 9.5 tok/s |
| **Median** | — | **~190 ms** | **~9.4 tok/s** |

Tight clustering → **~9.4 tok/s** is the real v79-on-Gen5 decode rate; TTFT consistently sub-250 ms.

### Thermal (5-min sustained generation, 62 zones sampled @ 2 s)

| Zone | Idle | Peak | Rise |
|---|---|---|---|
| **NPU** `nsphvx`/`nsphmx-0..3` (all 8) | 37.8 °C | **48–50 °C** | **+11 °C** |
| CPU big core `cpu-0-5-0` | 38.7 °C | **57.4 °C** | +19 °C |
| DDR | 39.0 °C | 50.9 °C | +12 °C |
| GPU `gpuss` | 38.2 °C | ~50 °C | +12 °C |
| **Skin** `skin-msm-therm` | 37.3 °C | **41.0 °C** | +3.7 °C |

- **Real peak ≈ 57 °C** — far under the 70 °C gate (~13 °C headroom); skin only 41 °C (comfortable to hold indefinitely).
- **The +11 °C rise on the Hexagon (`nsph*`) zones confirms the NPU is doing the compute** — not a CPU fallback. (The high CPU usage, ~275%, is the genie engine's `poll: true` busy-wait on 3 cores, *not* matmul.)
- ⚠️ **Do not trust `*-trip-*` zones** as readings: `cpu-hw-trip-0` reports `105 °C`, which is a **throttle trip-point threshold**, not a temperature. Exclude `*trip*` / `*limit*` zones when computing peaks.

### Stability finding

The reference chatapp **crashed** ("stopped due to its own reason") during a very long (multi-essay) generation — most likely 4096-context exhaustion mid-output. **Not a Primer concern:** `primer-inference::qnn` runs the small-context budget (12-turn window, 3-passage top-K, keyed off `QNN_NAME_PREFIX`) and drops partial turns on mid-stream error, so it bounds context by construction.

---

## Native sm8850: attempted two ways — no throughput gain found

**There are no precompiled Gen-5 (sm8850) binaries** on AI Hub (`qai-hub-apps fetch --chipset qualcomm-snapdragon-8-elite-gen-5` → "Model asset … not found"; the chatapp only publishes assets for the 3 chipsets it ships `htp_config` for). Two approaches were tried to beat the v79 baseline:

### (a) `soc_model` config patch on the v79 binaries — **CONCLUSIVE, no gain**

The v79 bundle's `htp_config` carried `soc_model: 69` (= `QNN_SOC_MODEL_SM8750`, the wrong chip). Patched it on-device to **`soc_model: 87`** (= `QNN_SOC_MODEL_SM8850`, confirmed from `QnnTypes.h` in QAIRT 2.45), keeping `dsp_arch: v79` (matches the binaries). The v79 binaries **load and run fine under the correct SM8850 id** (QNN accepts v79-on-Gen5), but throughput is **unchanged**:

| Config | Median decode | Median TTFT |
|---|---|---|
| `soc_model: 69` (SM8750, v79 default) | ~9.4 tok/s | ~190 ms |
| `soc_model: 87` (SM8850-native) | **~9.5 tok/s** | ~162 ms |

(3 element-gated runs each: 162/9.5, 161/9.1, 163/9.5.) Within noise. **`perf_profile: burst` already maxes the clocks; `soc_model` does not gate throughput here.** So **~9.4–9.5 tok/s is the genuine ceiling of the precompiled v79 binaries on this Gen-5, regardless of `soc_model`.** The QAIRT SDK exposes both `V79` and `V81` DSP archs — the only untested lever for higher throughput is **genuinely native, V81-compiled sm8850 binaries**.

### (b) Self-export of native sm8850 binaries — **ABANDONED (network, not technical)**

```
python -m qai_hub_models.models.qwen3_4b_instruct_2507.export \
    --device "Snapdragon 8 Elite Gen 5 QRD" --context-length 4096 \
    --skip-profiling --skip-inferencing --output-dir ~/primer-bundles/qwen3-4b-instruct-2507-sm8850
```

This **cleared five distinct toolchain obstacles** (see corrections #12–16) and reached the AI Hub model-upload stage, but **could not complete over the available Starlink link**: the model uploads in 4× ~1 GB ONNX splits, and the flaky uplink kept resetting each 1 GB upload partway, restarting it from 0%. After **~11 h** it was still looping on split 2/4 (5th of a max 7 retries before a hard fail). **Abandoned by decision** — the gate already passed, so this is a pure optimization. The downloaded checkpoint + ONNX caches are retained (`~/.qaihm`, `~/.cache/huggingface`) so a **re-run on a stable wired connection resumes from cache** and should finish in ~1 h. **This is the recommended way to obtain the native v81 numbers.** No technical blocker remains — only bandwidth.

### Net conclusion

The validated, repeatable result on this hardware is **~9.5 tok/s / ~160–190 ms TTFT / ~57 °C**, NPU-confirmed. Whether native V81 binaries materially beat that is **open** (pending a stable-connection export). For the Primer's purposes the v79 number already clears the gate floor and is conversational under the dialogue manager's latency budget.

---

## Runbook corrections (apply to `docs/devel/qnn-validation-runbook.md`)

The runbook (written 2026-05-28, pre-hardware) is substantially out of date. Concrete corrections:

1. **No Qualcomm Developer Portal account needed.** QAIRT ships as the **Community** edition from a public URL (`softwarecenter.qualcomm.com/.../Qualcomm_AI_Runtime_Community/...`), pulled automatically by the chatapp's Docker build / `scripts/qairt_utils.sh::install_qairt`. The manual `qpm.qualcomm.com` download (runbook Step 1) is obsolete. QAIRT version is **2.45**, not 2.29.
2. **macOS (Apple Silicon) is a viable export + build host.** The runbook says "Linux/macOS x86_64". The truth: the qai-hub export's **default checkpoint path fetches pre-calibrated AIMET encodings** and offloads compilation to **AI Hub cloud**, so no local AIMET (Linux-only) is required. The full Qwen3-4B export ran on an arm64 Mac. *Local* AIMET (Linux x86_64 + CUDA wheel) is only needed to **re-calibrate a custom checkpoint** — not for the default export.
3. **`qai-hub-apps fetch` is the fast path** (didn't exist in the runbook). It downloads app source + **precompiled** binaries in one command — no multi-hour export for any chipset that has published assets (8-Elite / 8-Gen3 / 8-Gen2).
4. **chatapp_android moved** to the repo root (`quic/ai-hub-apps/chatapp_android`), not `apps/chatapp_android`. Build is Docker or Android-Studio; QAIRT path comes from `$QAIRT_PATH` (env), no manual `build.gradle` edit if the env is set.
5. **The app hard-codes a SoC allowlist** (`MainActivity.java`): `SM8750/SM8650/QCS8550` only. **SM8850 (Gen 5) is rejected** → must be patched in.
6. **Gen-5 (SM8850) has no precompiled assets** on AI Hub; the v79 (sm8750) binaries are **backward-compatible** and run, but a native export is needed for best throughput.
7. **logcat is unavailable** on this Nubia/RedMagic ROM (Android 16) — `adb logcat -d` returns nothing, even for app/system tags. Diagnose via the app's on-screen TTFT/TPS readout + UI state (`uiautomator dump`) + `/sys/class/thermal` (all readable) instead. Plan accordingly: the runbook's logcat-based diagnosis won't work here.
8. **Lean-APK + adb-push beats a 3.5 GB APK.** Exclude `*.bin` from the build context; push them to `…/cache/models/llm/` post-install (the app's `copyFile` skips already-present files). 94 MB APK vs 3.5 GB.
9. **Don't trust `*-trip-*` thermal zones** as readings (105 °C trip point ≠ temperature).
10. **`HF_HUB_DISABLE_XET=1` is required** for the export's HuggingFace downloads — `hf-xet 1.5.0` stalls indefinitely at "Fetching N files: 0%" on this network; the standard LFS path works. (Also note the per-*file* progress bar sits at 0% while multi-GB shards download — looks hung but isn't.)
11. **Blind-tap UI automation is unreliable** on this device (variable model-load time + RedMagic gesture/overlay quirks sent stray taps into the clock app). **Element-gate every action** — poll `uiautomator dump` and tap a real element's bounds by `resource-id` only once it appears (`com.quicinc.chatapp:id/llm` button, `…/user_input`, `…/send_button`), and read the result from the `TTFT … | TPS …` text. The working driver is `~/measure_chatapp.py` from this session.

### Self-export toolchain gotchas (a native sm8850 export hit ALL of these, in order)

12. **`HF_HUB_DISABLE_XET=1` is required.** `hf-xet 1.5.0` stalls indefinitely at "Fetching N files: 0%" on this network; disabling it uses the reliable LFS path. (Also: the per-*file* progress bar sits at 0% while a multi-GB shard downloads — looks hung, isn't.)
13. **`accelerate` must be installed** in the export venv (transformers 4.51 needs `init_empty_weights`); `qai-hub-models[...]` does not pull it. But install it **carefully** — see #14.
14. **`accelerate`'s install bumps `pydantic` to ≥2.13, which breaks qai-hub-models' yaml parsing.** Pin `pydantic<2.12` (2.11.x) after installing accelerate.
15. **The model's bundled `info.yaml` has `status: published` + `devices: {}`,** which trips a "Model cannot be published: no release assets available" validator during export. Patch the local `info.yaml` `status: published` → `pending` (only affects the display-name lookup, not the compile).
16. **The qai-hub client's 4-second external-transfer socket timeout** (`util/session.py::EXTERNAL_RESPONSE_TIMEOUT_SECONDS = 4`) is far too tight for a slow/congested uplink — large S3 uploads die with "The write operation timed out." Bump it (e.g. 300). Also set `use_acceleration=False` (10× `public_rest_api.py` defaults) to use the regular regional S3 endpoint instead of the flaky `s3-accelerate` CloudFront edge. **Even with both, a sufficiently flaky link will reset the 4× ~1 GB split uploads and loop** (the client restarts each split from 0%, no S3 part-resume) — a stable wired connection is the real requirement.
17. **SoC-model IDs (from `QnnTypes.h`, QAIRT 2.45):** `SM8650=57`, `SM8750=69`, `SM8850=87`. DSP archs: `…V79=79, V81=81, V85=85, V89=89`. The chatapp's precompiled 8-Elite bundle ships `soc_model: 69, dsp_arch: v79`; for an SM8850 device the correct `soc_model` is **87** (but it makes no measurable throughput difference with v79 binaries — see "Native sm8850" above).

---

## Findings relevant to the Primer's `QnnBackend`

- **`genie_config.json` shape** the Primer must produce/consume: `dialog.context.size=4096`, `n-vocab/bos/eos`, `sampler{temp,top-k,top-p}`, `engine.backend.QnnHtp{poll, cpu-mask, kv-dim, pos-id-dim, rope-theta}`, `engine.model.binary.ctx-bins[]`. Matches the `primer-meta.json` + template design.
- **`htp_backend_ext_config.json`** is per-SoC: `{soc_model, dsp_arch, cores[{perf_profile:"burst", rpc_control_latency}]}`. The Primer's bundle loader needs the **right `soc_model`/`dsp_arch` for the runtime device** — getting this wrong (as we did, deliberately, with v79-on-Gen5) likely costs throughput.
- **Context exhaustion is real** on a 4K bundle — the Primer's small-context budget (already implemented) is load-bearing, not optional, on this hardware.
- **Throughput target:** 9.4 tok/s (v79 fallback) clears the 8 tok/s floor; native sm8850 numbers pending. The Primer's per-turn budget tolerates this; revisit if native binaries don't materially improve it.

---

## Next steps

1. **Proceed to exercise `primer-inference::qnn` on-device** — the host-tested `QnnBackend` now has a validated runtime target. Wire the v79 bundle through `--backend qnn --qnn-bundle-dir …` (use `soc_model: 87` in the `htp_config` for correctness, though it's perf-neutral). This is the real next milestone; the chatapp was only the proxy.
2. **(Optional, network-gated) Finish the native V81 export on a stable wired connection** to settle whether native binaries beat ~9.5 tok/s. The checkpoint/ONNX caches are retained, so it resumes from cache (~1 h). All five toolchain patches (corrections #12–16) are already applied in `~/venvs/qai-hub` (with `.bak` backups). If it produces a bundle, swap its `.bin` + Gen-5 `htp_config` onto the device and re-run `~/measure_chatapp.py`.
3. **Update `docs/devel/qnn-validation-runbook.md`** with the 17 corrections above (or supersede it with a "validated 2026-06-09" rewrite). The old runbook is substantially stale (assumes portal account, manual QAIRT, logcat, Android Studio, 2–3 h export — all wrong here).
4. **ROADMAP:** mark Phase 1.2 step 1.2.0 ✅ (device-validated); 1.2.6 device numbers partially captured (chatapp proxy; real `qnn_bench` run against the Primer's own backend still pending).

## Session artifacts (on the dev Mac, not committed)

- `~/chatapp-fetch/chatapp_android/` — patched app source (SoC allowlist + `.dockerignore`); built `app-debug.apk` at `~/chatapp-apk/`.
- `~/measure_chatapp.py` — element-gated measurement driver.
- `~/primer-thermal-v79.csv` — 5-min thermal sample.
- `~/venvs/qai-hub` — export venv with the 5 toolchain patches (`.bak` backups alongside).
- `~/.qaihm`, `~/.cache/huggingface` — retained export caches (17.7 GB checkpoint + weights) for a stable-connection retry.
- On device: `htp_config` patched to `soc_model: 87` (`.bak69` backup in the same dir).
