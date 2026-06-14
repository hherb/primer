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
- ✅ Tauri desktop GUI (`primer-gui`): full working app — session picker, streaming chat with mid-stream cancel, settings modal, resume, pedagogy/learner sidebar, voice mode (`--features speech`). Backend + embedder parity with the CLI including `openai-compat` and `qnn` (bundle-dir / QAIRT-lib-dir pickers in Settings; qnn construction needs `--features qnn`). Responsive on phone widths (below 940px the chat goes full-width and the sidebar becomes a slide-in overlay drawer; header condenses to icons).
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

Step 1.2.0 (device validation) ✅ **PASSED on hardware** (RedMagic 11 Pro, 2026-06-09); steps 1.2.1–1.2.5 landed; the Primer's own `QnnBackend` **generated its first tokens on the Hexagon NPU on 2026-06-12 (PR #218)** — via the Tauri-Android APK after clearing the V81-stub / FastRPC-`libcdsprpc` / native-lib-extraction blockers. **2026-06-14: the CMA memory blocker is RESOLVED and the Primer now generates coherent multi-token replies on the NPU, stable across a reboot** — the real Phase 1.2 finish (a working turn, not just one token). Fix: a memory-optimized **`--context-length 2048` re-export** (single value, so the cl3072/cl4096 graphs that drove the ~698 MB NSP buffer — which exceeded the ~637 MB CMA free even right after a reboot — are never baked in; reducing the runtime config `size` can't help, Genie inits every graph in the binary). With cl2048, all 4 context binaries load, all 8 graphs execute, and a real templated turn streams a coherent reply on the DSP. **2026-06-14 (later): a full multi-turn Socratic conversation now runs on the NPU — near-instant, stable across turns, zero context overflow** ("feels like sitting on a MacBook" — owner). The remaining 2K-context blocker turned out to be **not** prompt size but **Genie dialog-context accumulation**: the Primer re-sends the whole prompt each query and one Genie dialog handle is shared by the chat turn *and* the three background subsystems (classifier / extractor / comprehension), so Genie appended every query to the same KV context and saturated 2048 within a turn or two (the constant-1938 "context limit exceeded"). Fix: bind `GenieDialog_reset` (exported by QAIRT 2.45 `libGenie.so`) and reset the dialog before every query, keeping each query independent — verified on-device (3-turn conversation, no overflow in `genie.log`). Shipped alongside: a small-context prompt budget (8-turn window + per-passage KB truncation + a token-ceilinged system-prompt assembly via the pure `primer-core::prompt_budget` helpers, Socratic base never trimmed), a chat-templated construction smoke check (was a raw `"."` that ran to context-full), and graceful "context limit exceeded" (status 4) turn completion. **Remaining follow-ups:** pedagogy/answer-quality tuning on the 4B NPU model, and real `qnn_bench` numbers (now unblocked — turns complete cleanly). **The responsive mobile GUI layout has since landed** (2026-06-14): a 940px breakpoint switches the chat to full-width, turns the evaluation sidebar into a slide-in overlay drawer (backdrop/Esc dismiss), and condenses the header buttons to icons so nothing clips in portrait or landscape — pinned by `primer-gui`'s `responsive_layout_contract` tests.

- [x] **Step 1.2.0 — device validation (RedMagic 11 Pro / SM8850 / Snapdragon 8 Elite Gen 5).** The Genie/QNN NPU pipeline runs on hardware: Qwen3-4B-Instruct-2507 (w4a16, 4096 ctx) generates coherent Socratic text on the Hexagon NPU at **~9.4 tok/s decode (🟡 borderline 8–14 band → proceed), ~190 ms TTFT (✅), ~57 °C peak (✅, ~13 °C headroom)**, NPU-confirmed via a +11 °C rise on the `nsph*` zones. Validated through the `chatapp_android` proxy (the `qai-hub-apps fetch` fast path + a lean-APK + `adb push` of the v79/sm8750 binaries, which are backward-compatible on the Gen-5 part). The `soc_model` 69→87 patch is perf-neutral; a native sm8850/V81 export was network-abandoned (a pure throughput optimization, not gate-blocking). Full report: [docs/handoffs/2026-06-08-qnn-validation-chatapp.md](docs/handoffs/2026-06-08-qnn-validation-chatapp.md); corrected runbook: [docs/devel/qnn-validation-runbook.md](docs/devel/qnn-validation-runbook.md). **Phase 1.2 is unblocked to exercise `primer-inference::qnn` on-device** via `--backend qnn`.
- [x] **`primer-inference::qnn` corrected to the QAIRT 2.45 Genie API + on-device-validated (PR #213, 2026-06-10).** Running the Primer's own `QnnBackend` against the v79 bundle on the RedMagic surfaced three pre-2.45-API bugs no host/mock test could catch — fixed and confirmed against the authoritative QAIRT Community 2.45 headers: (1) `GenieDialog_query` now takes the streaming token callback as a parameter (the separate `setTokenCallback` symbol was removed in 2.45); (2) `GenieDialogConfig_createFromJson` consumes the JSON *content*, not a path; (3) relative bundle paths (`ctx-bins[]`, tokenizer, htp-extensions) are absolutized against the bundle dir via the new pure `absolutize_genie_config` helper (7 host unit tests). With the fixes the backend drives the real `libGenie.so` through dlopen → symbol resolution → config load → backend-extensions → HTP **device-creation entry**. **Remaining gap is deployment, not code:** the Hexagon DSP (`/dev/fastrpc-cdsp`) is SELinux/DAC-gated to properly-packaged apps — a sideloaded `shell`-uid binary AND a Termux app-uid process are both denied (`Failed to create device: 14001`), while the reference chatapp reaches it as a normally-launched app. So the first real on-device Primer NPU token needs the **Tauri-Android APK** (Phase 3 packaging), not a sideload. See [[project_qnn_dsp_needs_app_packaging]].
- [x] `primer-qnn-sys` FFI scaffold (hand-rolled Genie C API + runtime dlopen). **Corrected to QAIRT 2.45 in PR #213** (five resolved symbols, not six — `setTokenCallback` gone).
- [x] `QnnBackend` safe wrapper: trait-abstracted Genie handle, `primer-meta.json` parser, minijinja template, mutex-serialised dialog, ABI smoke check.
- [x] Per-token streaming bridge: C-ABI callback → `mpsc::UnboundedSender`, query in `spawn_blocking`.
- [x] CLI wiring: `--backend qnn`, `--qnn-bundle-dir`, `--qnn-qairt-lib-dir` (+ env fallbacks).
- [x] GUI wiring: QNN backend + bundle-dir / QAIRT-lib-dir pickers in Settings (always shown; selecting qnn on a non-`qnn`-feature build surfaces the "rebuild with --features qnn" hint inline). Host-tested; runtime still device-unverified.
- [x] Per-backend context budget for small-context backends, keyed off `QNN_NAME_PREFIX`: an 8-turn window + 3-passage KB top-K, **plus** (2026-06-14, fit-2K work) per-passage truncation to a relevant lead and a hard system-prompt token ceiling assembled by the pure `primer-core::prompt_budget` helpers (the Socratic base prompt is never trimmed — lower-value optional sections drop first), so prompt + reply fit the 2048-token Genie context the cl2048 bundle runs. The construction smoke check is chat-templated (not raw `"."`) so the model stops promptly instead of running to context-full, and a `GenieDialog_query` "context limit exceeded" (status 4) completes the turn with the streamed reply instead of dropping it.
- [x] **Per-query `GenieDialog_reset` (the on-device 2K-context blocker fix, 2026-06-14).** The single Genie dialog is shared by the chat turn + the three background subsystems and Genie accumulates KV across queries, so it saturated 2048 within a turn or two. Bound `GenieDialog_reset` (QAIRT 2.45 symbol) and reset the dialog before every `query_streaming` (all queries route through `generate_stream`). **Verified on-device: a 3-turn conversation runs near-instant with zero "context limit exceeded" in `genie.log`** — the Primer's `QnnBackend` is functionally complete on the NPU for real conversations.
- [x] Benchmark + thermal harness built: `examples/qnn_bench.rs` + 30-prompt `data/bench/socratic_prompts.jsonl` + pure host-tested metrics/thermal/loading modules + Android CI compile guard. Targets (15+ tok/s decode on Qwen3-4B W4A16, TTFT < 3s, peak < 70°C) are encoded as the verdict.
- [x] **Real on-device throughput numbers captured (2026-06-15) — targets met.** The standalone `qnn_bench` example **cannot** reach the Hexagon DSP on the target RedMagic ROM (the FastRPC node is SELinux-gated to packaged apps; a sideloaded/Termux binary is denied — see [[project_qnn_dsp_needs_app_packaging]]), so throughput is instrumented **inside the APK**: `QnnBackend::generate_stream` records a per-turn JSONL line (TTFT, decode tok/s) to `<app_data>/.primer/qnn_metrics.jsonl` (read via `run-as cat`; the shared `bench::StreamTimer` does the timing; gated by `PRIMER_QNN_METRICS_PATH`, set by the GUI startup hook). Measured over 20 queries on the cl2048 bundle: **decode 25.7 tok/s mean (min 25.0) — ✅ 1.7× the ≥15 target**; **TTFT p50 ≈ 0.78 s, p95 ≈ 2.6 s, max 2.93 s — ✅ under the 3 s target** (full chat turns ~2–2.9 s prefill on the 2048-context prompt; subsystem calls ~0.2–0.8 s); **peak ~52 °C** (one-shot read, not continuously sampled). This is ~2.7× the chatapp-proxy placeholder (~9.4 tok/s) and matches the owner's "near-instant, feels like a MacBook" impression. Shipped in PR #227.

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
- [x] **Sub-project 4 — Genie log callback shipped; -1 cause identified (PR #217, 2026-06-11).** Wired a Genie logging callback to a **file** (`primer-qnn-sys` gains the 3 optional 2.45 logging symbols; `primer-inference::qnn::genie::log` is a process-global file sink + the AArch64 `va_list`→`vsnprintf` bridge; the GUI sets `PRIMER_GENIE_LOG_PATH` → `<app_data>/.primer/genie.log`, read via `run-as cat`). On-device this surfaced the real cause behind the generic `GenieDialog_create` -1: `Failed in loading stub: dlopen failed: library "libQnnHtpV81Stub.so" not found` → `Transport layer setup failed: 14001` → `qnn-api initialization failed!`. **The HTP arch IS the blocker** (this overturns the sub-project-3 "v79 runs on this V81 part" note): our QAIRT **2.45** `libQnnHtp.so` (`v2.45.41.260507231357`) correctly detects the SM8850 as **V81** and demands the host-side `libQnnHtpV81Stub.so`, but only the **V79** per-arch libs were staged. `unsigned PD` is the default in the trace — so signing/unsigned-PD was NOT the cause. **Next step: stage the V81 HTP libs** (`libQnnHtpV81.so` + `libQnnHtpV81Stub.so` host + `libQnnHtpV81Skel.so` DSP) from the same QAIRT 2.45 SDK into `jniLibs/arm64-v8a/`, rebuild + reinstall, re-read `genie.log`. (Firmware has V81 libs in `/vendor/lib64/` + `/vendor/dsp/cdsp/` but non-root `adb pull` is permission-denied and the version may differ.) Acceptance: ≥1 coherent token from `QnnBackend` on the Hexagon NPU. Findings in [docs/devel/qnn-validation-runbook.md](docs/devel/qnn-validation-runbook.md).
- [x] **Sub-project 5 — first NPU token (PR #218, 2026-06-12).** The Primer's own `QnnBackend` **generated tokens on the Hexagon NPU.** Three DSP-bring-up blockers cleared, each read behind a generic status via the sub-project-4 log-to-file path: (1) staged a **coherent QAIRT `2.45.0.260326` V81** set (host `libQnnHtpV81Stub.so` + calculator-stub, DSP `libQnnHtpV81Skel.so`, and matching `libGenie`/`libQnnHtp`/… from the *same* build so the stub↔QnnHtp↔skel triple has zero skew — there is no host-side `libQnnHtpV81.so`, only the DSP skel; the jniLibs README documents the **no-login** Software-Center `/api/download` route since QPM is Linux/Windows-only); (2) `<uses-native-library android:name="libcdsprpc.so">` in `AndroidManifest.xml` — API-31+ refused the public FastRPC vendor lib undeclared (`loadRemoteSymbols err 4000`); (3) `jniLibs.useLegacyPackaging = true` in `build.gradle.kts` — `extractNativeLibs` defaulted false, so the DSP skel lived only inside `base.apk!/lib/…` and FastRPC had no real file to push (`Failed to load skel, error 1002`). Also fixed the diagnostic-logger firehose (VERBOSE → ≈1.4M lines/reply froze the app): Genie log level is now env-driven (`PRIMER_GENIE_LOG_LEVEL`, default WARN) with a callback-side threshold filter. **Remaining gate — a *stable* token across reboots:** the 4th weight-shared context binary's NSP buffers (~698 MB) exceed available CMA (~374 MB free on a settled boot); `spill-fill-bufsize` and a reduced context `size` don't help (Genie initializes every graph in the binary regardless of `size`) — needs a memory-optimized model export or CMA tuning.
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
