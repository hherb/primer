# Primer Roadmap

**Principle:** get a working conversation loop running fast, then improve every layer independently. The trait architecture lets us swap inference backends, add speech, integrate hardware, and refine pedagogy without rewriting the core.

Status key: ✅ done · 🟡 in progress · [ ] not started.

---

## Phase 0 — Cloud-Backed Proof of Concept

**Goal:** a text-mode Primer that holds a genuine Socratic conversation with a child on any machine with Rust + internet. **Phases 0.1–0.3 complete; 0.4 nearly done.** RedMagic 11 Pro validated 2026-05-26 (cloud REPL usable; on-device CPU Ollama too slow — needs NPU, Phase 1.2). **NPU path now unblocked:** the Genie/QNN pipeline was device-validated on the RedMagic 11 Pro 2026-06-09 (Phase 1.2 step 1.2.0 ✅, ~9.4 tok/s on the Hexagon NPU).

### 0.1 — Cloud backend conversationally useful

- ✅ SSE streaming (`CloudBackend`) + NDJSON streaming (`OllamaBackend`); tokens drip to the terminal live.
- ✅ `--model` flag to pick Claude models (default `claude-sonnet-4-6`; required for ollama).
- ✅ Graceful API errors: typed `InferenceError`, pre-stream classification + jittered retry, single i18n render boundary.
- ✅ Session persistence to SQLite (`primer-storage`), default `~/.primer/<slug>.db` or `--session-db`.
- ✅ Resume past session via `--resume <uuid>` (no greeting; clear error if missing).
- ✅ Long-term memory: rolling LLM summary + FTS5 retrieval of older turns injected into the system prompt, keeping the chat timeline bounded.

### 0.2 — Knowledge base bootstrapping

- ✅ Hybrid retrieval (BM25 + dense-vector RRF, `k=60`): `Embedder` trait, `primer-embedding` crate (`StubEmbedder` always built; `FastEmbedBackend`/BGE-M3, `OllamaEmbedder`, `OpenAiCompatEmbedder` behind features). Falls back to BM25-only when no embedder is wired. Default-on via `--embedder-backend` (feature-aware: `fastembed` on a default build, `none` on `--no-default-features`); the cdn.pyke.io ort-runtime download is proven in CI on Linux + macOS. Android stays BM25-only by guidance (#157).
- ✅ JSONL ingestion + auto-seed-on-empty + `--reembed` backfill (`primer-kb-load`); discovers all `*.<pack>.jsonl` in the seed dir.
- ✅ Seed corpus: 56 hand-drafted CC0 EN passages across five clusters; 35-article Simple English Wikipedia layer (CC-BY-SA-3.0) auto-loads alongside.
- ✅ German Klexikon corpus (CC-BY-SA-4.0), expanded to 66 articles; auto-loads on `--language de`.
- ✅ Retrieval tuned + benchmarked: 91-query EN benchmark (24 strict mappings, 100% loose/strict hybrid recall) and 31-query DE benchmark, each with BM25 + hybrid sweep diagnostics, quality regression tests, and a BM25 floor tripwire.
- ✅ Ingest pipeline generalised around a frozen `WikiSource` dataclass with HTTP 429/5xx backoff; two presets (Simple English, Klexikon).
- 🟡 Hindi (`hi`) preview scaffolding: `Locale::Hindi` reachable via `--language hi` but excluded from pickers (`status = "preview"` packs); no corpus/tests yet. Flip to stable = one commit.

### 0.3 — Pedagogical engine refinement

- ✅ Concept extraction (`primer-extractor`): per-exchange LLM call returns separated child/primer concepts, applied to the learner model at the next-turn boundary.
- ✅ Comprehension assessment (`primer-comprehension`): per-concept `{depth, confidence, evidence}`, monotonic-max depth promotion, persisted to `turn_comprehensions`.
- ✅ Learner-model persistence (`LearnerStore`, schema v4) across sessions.
- ✅ Vocabulary spaced repetition: `primer-core::vocab` Leitner-box scheduler; most-overdue concepts injected as a passive prompt section. `--vocab-max-per-prompt`.
- ✅ Session-time break suggestions: `PedagogicalIntent::SuggestBreak`, engagement-state overrides win, locale-aware `{minutes}`. `--session-break-after-mins`.
- ✅ `decide_intent()` characterization tests (18); `Encouragement`, `DirectAnswer`/`AnswerThenPivot`, and session-length-aware `Disengaging` routing all reachable via the engagement classifier.
- ✅ Reasoning-token stripping (`primer-core::reasoning`): stateful streaming filter removes per-model chain-of-thought markers (`<think>…</think>`, Gemma4 `<|channel>…<channel|>`) from `OllamaBackend` + `OpenAiCompatBackend` (and therefore the classifier/extractor/comprehension subsystems) before they reach a child; reasoning-without-answer falls back to a localized "thinking problem, try again" via the i18n boundary. Built-in marker table always-on; CLI `--reasoning-marker <OPEN> <CLOSE>` appends custom pairs. ✅ GUI custom-marker editor shipped: Settings → Inference backend has a "Reasoning markers" textarea (one `open close` pair per line, shown for ollama / openai-compat); a pure `primer-gui::reasoning_markers::parse_reasoning_markers` converts the stored text into pairs at session-wiring time.

### 0.4 — Developer experience

- ✅ OpenAI-compatible inference + embedding backend (`OpenAiCompatBackend`, `OpenAiCompatEmbedder`): `/v1/chat/completions` SSE + `/v1/embeddings` batch. Unblocks oMLX, LM Studio, vLLM, llama.cpp `--server`, Together/Groq/OpenRouter.
- ✅ Tauri desktop GUI (`primer-gui`): full working app — session picker, streaming chat with mid-stream cancel, settings modal, resume, pedagogy/learner sidebar, voice mode (`--features speech`). Backend + embedder parity with the CLI including `openai-compat` and `qnn` (bundle-dir / QAIRT-lib-dir pickers in Settings; qnn construction needs `--features qnn`).
- ✅ `CLAUDE.md`, `--verbose` pedagogical-decision tracing, `.env`/`~/.primer_env` auto-loading.
- 🟡 CI (GitHub Actions): Linux complete (test + fmt + clippy + non-default-feature drift-guards + Android cross-compile + ort-sys cfg guard); macOS partial (clippy drift-guard for Apple-native combos). Missing: full `cargo test` on macOS, `macos-native-26` coverage (no hosted image yet). Branch protection on `main` is the recommended structural fix.
- [ ] `primer-knowledge` is the only crate still without retrieval test coverage.

**Phase 0 exit criteria:** a child can run `cargo run --bin primer -- --backend cloud --name Binti --age 8` and have a 15-minute Socratic conversation that asks more than it answers, catches parroting, suggests breaks, and remembers last time. Met. Next milestone: Phase 1 (local llama.cpp).

---

## Phase 1 — Local Inference

**Goal:** run the conversation loop offline on available hardware.

### 1.1 — llama.cpp integration 🟡

- [x] `LlamaCppBackend` via `llama-cpp-2` bindings; GGUF loading from a configurable path (behind the non-default `llamacpp` feature; CPU + Metal/CUDA/Vulkan passthrough). Benchmarking (bullet 2) + local→cloud fallback (bullet 3) still open.
- [ ] Benchmark Qwen3 7B Q4_K_M on MacBook (Metal), DGX (CUDA), RedMagic (Vulkan).
  - Harness shipped: `examples/llamacpp_bench.rs` + the shared, host-tested `primer-inference::bench` module (extracted from the QNN bench; `BenchTargets` all-`Option`, `evaluate` treats `None` as vacuous pass). Measurement-first — a flagless run is a pure probe; `--min-decode-tps`/`--max-ttft-ms`/`--max-peak-temp-c` opt into a pass/fail gate. **What remains is running it on the three accelerators to collect the actual tok/s + TTFT numbers** (owner-gated device runs — no GGUF is auto-downloaded).
- [x] Automatic local→cloud fallback (opt-in `--fallback-backend`/`--fallback-model`; `FallbackBackend` decorator falls back at startup + pre-stream, never mid-stream; CLI + GUI both shipped — GUI mirror via Settings → Inference backend, issue #205). 3B constrained-device chain still ahead.

### 1.2 — Qualcomm NPU (Snapdragon 8 Elite) 🟡

Step 1.2.0 (device validation) ✅ **PASSED on hardware** (RedMagic 11 Pro, 2026-06-09); steps 1.2.1–1.2.5 landed; the Primer's own `QnnBackend` is now **device-validated to the HTP device-creation boundary** (PR #213, 2026-06-10); 1.2.6's harness is built + host-tested (chatapp-proxy numbers captured, the real `qnn_bench` against the Primer's own backend still pending an app-uid runtime).

- [x] **Step 1.2.0 — device validation (RedMagic 11 Pro / SM8850 / Snapdragon 8 Elite Gen 5).** The Genie/QNN NPU pipeline runs on hardware: Qwen3-4B-Instruct-2507 (w4a16, 4096 ctx) generates coherent Socratic text on the Hexagon NPU at **~9.4 tok/s decode (🟡 borderline 8–14 band → proceed), ~190 ms TTFT (✅), ~57 °C peak (✅, ~13 °C headroom)**, NPU-confirmed via a +11 °C rise on the `nsph*` zones. Validated through the `chatapp_android` proxy (the `qai-hub-apps fetch` fast path + a lean-APK + `adb push` of the v79/sm8750 binaries, which are backward-compatible on the Gen-5 part). The `soc_model` 69→87 patch is perf-neutral; a native sm8850/V81 export was network-abandoned (a pure throughput optimization, not gate-blocking). Full report: [docs/handoffs/2026-06-08-qnn-validation-chatapp.md](docs/handoffs/2026-06-08-qnn-validation-chatapp.md); corrected runbook: [docs/devel/qnn-validation-runbook.md](docs/devel/qnn-validation-runbook.md). **Phase 1.2 is unblocked to exercise `primer-inference::qnn` on-device** via `--backend qnn`.
- [x] **`primer-inference::qnn` corrected to the QAIRT 2.45 Genie API + on-device-validated (PR #213, 2026-06-10).** Running the Primer's own `QnnBackend` against the v79 bundle on the RedMagic surfaced three pre-2.45-API bugs no host/mock test could catch — fixed and confirmed against the authoritative QAIRT Community 2.45 headers: (1) `GenieDialog_query` now takes the streaming token callback as a parameter (the separate `setTokenCallback` symbol was removed in 2.45); (2) `GenieDialogConfig_createFromJson` consumes the JSON *content*, not a path; (3) relative bundle paths (`ctx-bins[]`, tokenizer, htp-extensions) are absolutized against the bundle dir via the new pure `absolutize_genie_config` helper (7 host unit tests). With the fixes the backend drives the real `libGenie.so` through dlopen → symbol resolution → config load → backend-extensions → HTP **device-creation entry**. **Remaining gap is deployment, not code:** the Hexagon DSP (`/dev/fastrpc-cdsp`) is SELinux/DAC-gated to properly-packaged apps — a sideloaded `shell`-uid binary AND a Termux app-uid process are both denied (`Failed to create device: 14001`), while the reference chatapp reaches it as a normally-launched app. So the first real on-device Primer NPU token needs the **Tauri-Android APK** (Phase 3 packaging), not a sideload. See [[project_qnn_dsp_needs_app_packaging]].
- [x] `primer-qnn-sys` FFI scaffold (hand-rolled Genie C API + runtime dlopen). **Corrected to QAIRT 2.45 in PR #213** (five resolved symbols, not six — `setTokenCallback` gone).
- [x] `QnnBackend` safe wrapper: trait-abstracted Genie handle, `primer-meta.json` parser, minijinja template, mutex-serialised dialog, ABI smoke check.
- [x] Per-token streaming bridge: C-ABI callback → `mpsc::UnboundedSender`, query in `spawn_blocking`.
- [x] CLI wiring: `--backend qnn`, `--qnn-bundle-dir`, `--qnn-qairt-lib-dir` (+ env fallbacks).
- [x] GUI wiring: QNN backend + bundle-dir / QAIRT-lib-dir pickers in Settings (always shown; selecting qnn on a non-`qnn`-feature build surfaces the "rebuild with --features qnn" hint inline). Host-tested; runtime still device-unverified.
- [x] Per-backend 4K context budget for small-context backends (12-turn window, 3-passage top-K), keyed off `QNN_NAME_PREFIX`.
- [x] Benchmark + thermal harness built: `examples/qnn_bench.rs` + 30-prompt `data/bench/socratic_prompts.jsonl` + pure host-tested metrics/thermal/loading modules + Android CI compile guard. Targets (15+ tok/s decode on Qwen3-4B W4A16, TTFT < 3s, peak < 70°C) are encoded as the verdict. **Proxy numbers captured during step 1.2.0** (chatapp_android: ~9.4 tok/s / ~190 ms TTFT / ~57 °C on v79-on-Gen5); a real `qnn_bench` run against the Primer's own `QnnBackend` is the remaining piece.

### 1.3 — Hybrid inference 🟡

- [x] Inference router (`RouterBackend` decorator + `--router-mode local-only|cloud-preferred|hybrid`; pure composite-complexity policy in `primer_core::router`; CLI + GUI). Routes routine turns to the local primary and complex/knowledge-intensive turns to the cloud secondary, self-failing-over at the pre-stream boundary; reuses the `--fallback-*` secondary leg.
- [x] Latency-aware routing — **shipped, config-gated and OFF by default** (`--primary-ttft-budget-ms` / GUI "Primary TTFT budget (ms)"). The `RouterBackend` owns a rolling primary-leg TTFT EMA (router-owned for correct leg attribution) and folds a `latency_term` into the `hybrid` complexity score: a slow local leg *nudges* borderline turns to the cloud while trivial turns stay local (self-healing). No magic threshold ships — the budget is owner-calibrated from bench numbers; with no budget set, behaviour is byte-identical to the no-latency router. **Threshold calibration** (picking the real budget) stays gated on the owner-gated llama.cpp/QNN bench numbers.

**Phase 1 exit criteria:** the Phase 0 conversation works offline, <3s to first token on at least one local platform.

---

## Phase 2 — Speech

**Goal:** talk to the Primer instead of typing.

### 2.1 — Speech-to-text

- [x] `WhisperStt` streaming STT + Silero VAD (vendored).
- [ ] Graceful ambient-noise handling.

### 2.2 — Text-to-speech

- [x] `PiperTts` streaming TTS (sentence-boundary chunking).
- [ ] Warm, patient voice profile selection.

### 2.3 — Conversation flow

- [x] Voice round-trip POC (`--speech`): LISTEN → LATENT_THINK → SPEAK → LISTEN; interrupt handling (cancel-on-SpeechStart); no barge-in either direction.
- [ ] Echo cancellation; long-pause/silence handling.

### 2.4 — Native Apple speech

- [x] macOS-native backend (`--features macos-native`): `SFSpeechRecognizer` on-device STT + streaming `AVSpeechSynthesizer` TTS; Silero VAD; en-US + de-DE.
- [x] macOS 26 backend (`--features macos-native-26`): `SpeechAnalyzer`/`SpeechTranscriber`/`SpeechDetector` via a Swift sidecar; ~100× faster to first partial than Whisper. Mutually exclusive with `macos-native`.
- [x] Supertonic TTS (`--features supertonic`): `TextToSpeech` + `StreamingTextToSpeech` impl; #170 v2/v3 spike passed (Piper-class CPU RTF, 32 languages incl. Hindi/Japanese). **Stage C done** — STT and TTS decoupled (the three voice-loop builders take an injected TTS via `build_tts`/`build_voice_backends`); CLI `--tts piper|supertonic`; GUI separate STT/TTS dropdowns with feature-gated disabling. **Stage D done** — GUI asset auto-download: the ~380 MB Supertonic bundle (one locale-independent multilingual model, 7 files, default voice F1) is modelled as 7 single-file `kind`s reusing the Piper/Whisper consent+download flow, and `disable_auto_download` is now actually enforced for every backend (was a stored-but-ignored no-op). In-loop A/B numbers (Stage E) + Hindi preview→stable (Stage F) still ahead.

**Phase 2 exit criteria:** a child can have the Phase 0 conversation entirely by voice.

---

## Phase 3 — Hardware Enclosure

**Goal:** a physical device a child can hold.

- [x] **Android app packaging — scaffold (sub-project 1, 2026-06-11).** `cargo tauri android init` + a `mobile`-gated entry point in `primer-gui`; a debug APK of the GUI builds host-side for `aarch64-linux-android` (BM25-only, mirroring #157; ~196 MB debug, no device needed to build). This is the deployment path to the **first on-device QNN NPU token** — the Hexagon DSP grant only applies to a normally-launched app, not a sideload (PR #213 / `[[project_qnn_dsp_needs_app_packaging]]`). Build runbook: [docs/devel/android-build-quickstart.md](docs/devel/android-build-quickstart.md); design: [docs/superpowers/specs/2026-06-11-android-packaging-scaffold-design.md](docs/superpowers/specs/2026-06-11-android-packaging-scaffold-design.md).
- [x] **Android app packaging — QNN backend + QAIRT jniLibs (sub-project 2, 2026-06-11).** `primer-gui` builds for `aarch64-linux-android` with `--features qnn` (chaining to `primer-engine/qnn` → `QnnBackend` + `primer-qnn-sys`), and the 9 proprietary QAIRT / Genie runtime `.so`s bundle into the APK's `lib/arm64-v8a/`. Verified: a ~406 MB debug APK carrying `libGenie.so` + 8 `libQnn*.so` (v79/SM8850 build, sha256-pinned) staged from the RedMagic 11 Pro, plus the app's own `libprimer_gui.so`. The libs are git-ignored (Qualcomm licence + AGPL repo); a [staging README](src/crates/primer-gui/gen/android/app/src/main/jniLibs/arm64-v8a/README.md) documents the manifest + the `adb pull` / QAIRT-SDK routes. `libcdsprpc.so` is intentionally not bundled (device system/vendor lib). A CI drift-guard cross-compiles `primer-gui --features qnn` for Android.
- [x] **Android path-resolution + on-device bring-up (sub-project 3, 2026-06-11).** The QNN APK now **installs, boots, and renders** on the RedMagic 11 Pro, and a QNN session drives the full stack onto the NPU up to `GenieDialog_create`. Landed (PR TBD): (1) **per-platform path resolution** — `primer-gui` resolves config / session-DB / voice-cache from `app_data_dir()` (Tauri path API) via a `mobile`-gated `.setup()` hook instead of `$HOME`; desktop is byte-identical. (2) **seed corpus** — `resource_dir()` is `asset://localhost/` (not `std::fs`-readable), so the seed is staged from `<app_data>/seed` (graceful empty-KB fallback). (3) **QNN basename dlopen** — `primer-qnn-sys` loads `libGenie.so` by basename on Android (linker resolves it + deps from `nativeLibraryDir`); `primer-engine::resolve_qairt_lib_dir` returns empty on Android so `qnn_qairt_lib_dir` is unnecessary on-device. (4) **logcat tracing** via `paranoid-android` (Android discards stdout). (5) **`ADSP_LIBRARY_PATH`** set to `nativeLibraryDir` (discovered from `/proc/self/maps`) so the DSP can find the bundled v79 skel. All new logic is pure + host-tested; the mobile-cfg path cross-compiles in CI. **On-device findings:** the model bundle must live in app *internal* storage (`/data/user/0/<pkg>/files/qnn-bundle`) — scoped storage hides `adb`-written `/sdcard/Android/data/<pkg>` files from the app; `logcat` is dead on this ROM; v79 binaries are confirmed to run on this Gen-5/V81 part (step 1.2.0), so the HTP arch is not the blocker.
- [ ] **Sub-project 4 — first on-device NPU token (device-gated).** `GenieDialog_create` returns **status -1** after the whole stack (path resolution → basename load → config parse/absolutize → skel-path setup) succeeds. Cause is past `ADSP_LIBRARY_PATH` (the -1 persists with it set) — likely DSP signing / unsigned-PD or a Genie-internal failure that the generic -1 hides. **Next step: wire a Genie log callback to a file** (logcat is dead on this ROM) to read the error behind -1, then address it (unsigned-PD enablement, signed PD, or a native V81 export). Acceptance: ≥1 coherent token from `QnnBackend` on the Hexagon NPU.
- [ ] Display: colour E Ink or repurposed tablet; child-friendly UI; text + illustrated modes.
- [ ] Audio: MEMS mic array (beam-forming), speaker, physical volume knob.
- [ ] Power/enclosure: 5-7h battery, passive thermal, drop-resistant, physical power button.
- [ ] Compute integration: mount SBC/phone, wire display+audio+power, boot-to-Primer.

**Phase 3 exit criteria:** turn on the device and have the Phase 2 conversation with no other equipment.

---

## Phase 4 — Pedagogical Depth

**Goal:** a genuinely effective learning companion, not just a Socratic chatbot.

- [ ] Curriculum alignment (Australian Curriculum, IB PYP).
- [ ] Multi-session learning arcs.
- [ ] Read-only parental dashboard (no surveillance).
- [ ] Collaborative mode (two children sharing a Primer).
- [ ] Assessment through conversation, never quizzes/scores.
- [ ] Opt-in, parent-consented age-calibrated language corpus; on-device PII scrubbing before any export.

---

## Who works on what

| Contributor | Natural focus areas |
|---|---|
| **Horst Herb** | `primer-pedagogy`, `primer-cli`, `primer-knowledge`, integration testing, coordination |
| **Bernd Brinkmann** *(EE/ML)* | `primer-inference` (local backends), Phase 3 hardware, thermal design |
| **Frithjof Herb** *(ML/math)* | `primer-inference` (quantisation, benchmarks), `primer-pedagogy` (comprehension, learner model), `primer-knowledge` (semantic search) |
| **Claude** *(via Claude Code)* | Implementation pairing, code review, refactoring, test scaffolding, docs — under Horst's direction; `Co-Authored-By` trailer on AI commits |

Bernd and Frithjof are planned contributors not yet committing code; Phase 1 hardware/local-inference work is genuinely waiting on them. Phase 0 is pure Rust and needs no special hardware.

---

## Principles that don't change

1. **The Primer asks more questions than it answers.**
2. **The Primer does not maximise engagement.** It suggests breaks, lets children stop, never guilt-trips.
3. **All learner data stays on-device.** Cloud inference is stateless; the learner model never leaves without explicit parental consent.
4. **The pedagogical engine is testable without hardware.** Every phase keeps the stub backend working.
