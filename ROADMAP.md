# Primer Roadmap

**Principle:** get a working conversation loop running fast, then improve every layer independently. The trait architecture lets us swap inference backends, add speech, integrate hardware, and refine pedagogy without rewriting the core.

Status key: ✅ done · 🟡 in progress · [ ] not started.

---

## Phase 0 — Cloud-Backed Proof of Concept

**Goal:** a text-mode Primer that holds a genuine Socratic conversation with a child on any machine with Rust + internet. **Phases 0.1–0.3 complete; 0.4 nearly done.** RedMagic 11 Pro validated 2026-05-26 (cloud REPL usable; on-device CPU Ollama too slow — needs NPU, Phase 1.2).

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

Steps 1.2.1–1.2.5 landed; 1.2.6's harness is built + host-tested (device numbers pending); 1.2.0 (QAIRT install + device validation) remains.

- [x] `primer-qnn-sys` FFI scaffold (hand-rolled Genie C API + runtime dlopen).
- [x] `QnnBackend` safe wrapper: trait-abstracted Genie handle, `primer-meta.json` parser, minijinja template, mutex-serialised dialog, ABI smoke check.
- [x] Per-token streaming bridge: C-ABI callback → `mpsc::UnboundedSender`, query in `spawn_blocking`.
- [x] CLI wiring: `--backend qnn`, `--qnn-bundle-dir`, `--qnn-qairt-lib-dir` (+ env fallbacks).
- [x] GUI wiring: QNN backend + bundle-dir / QAIRT-lib-dir pickers in Settings (always shown; selecting qnn on a non-`qnn`-feature build surfaces the "rebuild with --features qnn" hint inline). Host-tested; runtime still device-unverified.
- [x] Per-backend 4K context budget for small-context backends (12-turn window, 3-passage top-K), keyed off `QNN_NAME_PREFIX`.
- [x] Benchmark + thermal harness built: `examples/qnn_bench.rs` + 30-prompt `data/bench/socratic_prompts.jsonl` + pure host-tested metrics/thermal/loading modules + Android CI compile guard. Targets (15+ tok/s decode on Qwen3-4B W4A16, TTFT < 3s, peak < 70°C) are encoded as the verdict; **the actual numbers still need a device run** (gated on 1.2.0).

### 1.3 — Hybrid inference 🟡

- [x] Inference router (`RouterBackend` decorator + `--router-mode local-only|cloud-preferred|hybrid`; pure composite-complexity policy in `primer_core::router`; CLI + GUI). Routes routine turns to the local primary and complex/knowledge-intensive turns to the cloud secondary, self-failing-over at the pre-stream boundary; reuses the `--fallback-*` secondary leg. **Latency-aware switching is a designed extension point, deferred** until the owner-gated bench numbers exist.

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
