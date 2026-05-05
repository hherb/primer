# Primer Roadmap

This roadmap is organised around one principle: **get a working conversation loop as fast as possible, then improve every layer independently**. The trait architecture means we can swap inference backends, add speech, integrate hardware, and refine pedagogy without rewriting the core.

---

## Phase 0 — Cloud-Backed Proof of Concept

**Goal:** A text-mode Primer that holds a genuine Socratic conversation with a child, running on any machine with Rust and an internet connection.

**Runs on:** MacBook, Spark DGX, any Linux box, RedMagic 11 Pro (via Termux or adb shell).

**No special hardware required.** This is where all three contributors can work in parallel right now.

### 0.1 — Make the cloud backend conversationally useful

- ✅ Wire up SSE streaming in `CloudBackend` — done. NDJSON streaming for `OllamaBackend` landed in the same pass. Tokens drip through to the terminal as they arrive.
- ✅ Add a `--model` CLI flag to select between Claude models (sonnet for speed, opus for depth) — done. Defaults to `claude-sonnet-4-6`; required for ollama.
- ✅ Handle API errors gracefully — done. `PrimerError::Inference` now carries a typed `InferenceError` (`Auth | RateLimited{retry_after} | ServiceUnavailable | NetworkUnavailable | ModelNotFound{model} | Other`). Both cloud (Anthropic) and Ollama backends classify pre-stream failures and retry transient ones via `primer_core::retry::retry_with_backoff` (default 3 attempts, jittered exponential backoff, honors `Retry-After` ≤ 5 s). User-facing messages render via `primer_core::i18n::render_inference_error` — single i18n boundary; future locale support adds variants without touching producers. Mid-stream errors still propagate cleanly and the partial Primer turn drops. See [docs/superpowers/specs/2026-05-04-graceful-api-errors-design.md](docs/superpowers/specs/2026-05-04-graceful-api-errors-design.md).
- ✅ Add conversation persistence — sessions persist to SQLite via `primer-storage` (per-turn save). Default location is `~/.primer/<slug-of-name>.db`; explicit path via `--session-db <path>`. See [docs/superpowers/specs/2026-04-30-session-persistence-sqlite-design.md](docs/superpowers/specs/2026-04-30-session-persistence-sqlite-design.md)
- ✅ Resume past session — `--resume <uuid>` CLI flag loads a stored `Session` and picks up without a greeting. Errors clearly when the file or id is missing.
- ✅ Long-term memory across the context window — schema v2 added a rolling LLM-generated `Session.summary` (refreshed on resume + every 20 turns) plus FTS5 retrieval (`turn_text_fts`) of relevant older turns; both inject into the system prompt while the chat-message timeline stays = `recent_turns(window)`. Context budget is bounded across hours of conversation.

### 0.2 — Knowledge base bootstrapping

- [ ] Write an ingestion script (Python is fine) that downloads Simple English Wikipedia dumps and inserts passages into the SQLite FTS5 database
- [ ] Add a curated seed corpus — 50-100 hand-picked passages covering topics children commonly ask about (space, dinosaurs, weather, the human body, animals, how things work)
- [ ] Test RAG retrieval quality: does "why is the sky blue" retrieve Rayleigh scattering passages? Does "how do plants eat" retrieve photosynthesis?
- [ ] Tune `RetrievalParams` defaults (top_k, min_score) based on real queries

### 0.3 — Pedagogical engine refinement

- ✅ Implement concept extraction from child responses — landed via the `primer-extractor` crate (`ConceptExtractor` trait, `LlmConceptExtractor` + `StubConceptExtractor` impls). One LLM call per completed (child, primer) exchange returns separated `{ child_concepts, primer_concepts }`; spawned async after primer save, persisted via `SessionStore::update_turn_concepts`, applied to in-memory `LearnerModel.concepts` at the next-turn boundary
- ✅ Add LLM-based comprehension assessment: after the child responds, use a short classifier prompt to gauge whether the response demonstrates genuine understanding, parroting, or confusion — landed as `primer-comprehension` crate (Phase 0.3): `LlmComprehensionClassifier` produces per-concept `{depth, confidence, evidence}` assessments against the extractor's candidate set; results promote `LearnerModel.concepts.depth` via monotonic max (threshold-gated) AND persist to schema-v5 `turn_comprehensions` for offline depth-trajectory analysis.
- ✅ Implement learner model persistence (SQLite) so the Primer remembers what a child knows across sessions — landed via the `LearnerStore` trait + schema v4 migration; CLI startup adopts existing `sessions.learner_id` on first-run-after-upgrade
- ✅ Per-child vocabulary tracking with spaced repetition — landed (Phase 0.3, 2026-05-05; spec at [docs/superpowers/specs/2026-05-05-vocabulary-spaced-repetition-design.md](docs/superpowers/specs/2026-05-05-vocabulary-spaced-repetition-design.md)). New `primer-core::vocab` module with three pure functions (`apply_box_transition`, `is_due`, `due_concepts`) implements a Leitner-box scheduler with intervals `[1d, 3d, 7d, 14d, 30d]`. `apply_comprehension` advances the box on every accepted assessment; `build_turn_prompt` injects the top-K most-overdue concepts as a passive prompt-section that the LLM weaves in only if topically relevant. Schema v7 adds `learner_concepts.box_level INTEGER NOT NULL DEFAULT 0`. Configurable via `--vocab-max-per-prompt N` (default 4). All learner data stays per-DB. **Schema constraint preserved:** vocabulary entries continue to live as `ConceptState` rows so the Phase 4 anonymisation/contribution path is unchanged
- [ ] Add session time tracking with gentle break suggestions ("We've been exploring for 25 minutes — want to take a break and come back to this?")
- ✅ Write unit tests for `decide_intent()` — characterization tests landed (18 tests in `prompt_builder::tests`); they pin down current behaviour and document the gaps below
- ✅ Make `Encouragement` reachable from `decide_intent` — landed in the engagement-classifier work (Phase 0.3): `LlmEngagementClassifier` supplies `EngagementAssessment` into `decide_intent`; "frustrated but still trying" now routes to `Encouragement`, "frustrated and stuck" to `Scaffolding`
- ✅ Detect factual-question patterns in `decide_intent` so "what is X?" / "how does Y work?" route to `DirectAnswer`, with `AnswerThenPivot` on the next turn — landed in the engagement-classifier work; both intents are now reachable
- ✅ Make `Disengaging` session-length-aware — route to `Encouragement` early in a session, `SessionClose` only after a meaningful duration — landed in the engagement-classifier work using session turn count as the duration proxy

### 0.4 — Developer experience

- [ ] Add `cargo test` coverage for all crates (core trait contracts, prompt builder output, dialogue manager state transitions, knowledge base retrieval) — partial. 223 tests across the workspace after the learner-model-persistence landing (Phase 0.3): streaming parsers + stub-summarize (19 in `primer-inference`), `decide_intent`/prompt builder + `dialogue_manager` including resume + summary refresh + classifier integration + learner-store wiring (67 in `primer-pedagogy`), session persistence + v2/v3/v4 migration + load + FTS retrieval + `turn_classifications` + `LearnerStore` round-trip (83 in `primer-storage`), classifier trait + `LlmEngagementClassifier` + `StubEngagementClassifier` (12 in `primer-classifier`), CLI slug + path resolution + adoption-flow round-trip (21 in `primer-cli`), enum-variant arrays + speech traits (9 in `primer-core`, 10 in `primer-speech`). `primer-knowledge` retrieval is the only crate still without coverage.
- [ ] Set up CI (GitHub Actions) — build + test on Linux and macOS
- ✅ Add a `CLAUDE.md` to the repo with codebase conventions — done.
- ✅ Add `--verbose` flag that prints pedagogical decisions (intent chosen, knowledge passages retrieved, engagement state) alongside the conversation — landed in the engagement-classifier work; `[intent]` and `[classifier]` lines emit to stderr when `--verbose` is passed
- ✅ `.env` / `~/.primer_env` auto-loading via dotenvy — done. Secrets can live in either a project-local `.env` or a user-global `~/.primer_env`.

**Phase 0 exit criteria:** You can sit a child in front of a terminal, type `cargo run --bin primer -- --backend cloud --name Binti --age 8`, and have a 15-minute Socratic conversation about a topic of their choosing that feels qualitatively different from ChatGPT. The Primer asks more questions than it answers. It catches parroting. It suggests breaks. It remembers what was discussed last time.

---

## Phase 1 — Local Inference

**Goal:** Run the conversation loop without internet, using a local LLM on whatever hardware is available.

### 1.1 — llama.cpp integration

- [ ] Implement `LlamaCppBackend` via llama-cpp-rs bindings (or direct FFI)
- [ ] Support GGUF model loading from a configurable path
- [ ] Benchmark Qwen3 7B Q4_K_M on: MacBook M-series (Metal), DGX (CUDA), RedMagic 11 Pro (Vulkan)
- [ ] Add automatic backend fallback: try local model first, fall back to cloud if local is too slow or unavailable
- [ ] Add a 3B model fallback path for constrained devices

### 1.2 — Qualcomm NPU (RedMagic / Snapdragon 8 Elite)

- [ ] Implement `QnnBackend` using QNN SDK for Hexagon NPU inference
- [ ] Target: 15+ tok/s on Qwen3 7B W4A16 quantisation
- [ ] Test thermal behaviour under sustained conversation (the phone in an enclosure will need monitoring)

### 1.3 — Hybrid inference

- [ ] Implement the inference router: use local model for routine Socratic turns, escalate to cloud for complex reasoning or knowledge-intensive questions
- [ ] Add latency-aware routing — if local generation is taking >5s, offer to switch to cloud
- [ ] Config-driven: parents can set "local only" (no network), "cloud preferred" (best quality), or "hybrid" (default)

**Phase 1 exit criteria:** The same conversation from Phase 0 works offline, with acceptable latency (<3s to first token on at least one local platform).

---

## Phase 2 — Speech

**Goal:** Talk to the Primer instead of typing.

### 2.1 — Speech-to-text

- [x] Implement `WhisperBackend` for STT — `StreamingSpeechToText` trait + `WhisperStt` backend. Streaming session emits partial transcript chunks via `StreamingSpeechToText::begin_session`. Landed in voice round-trip POC.
- [x] Add voice activity detection (VAD) so the Primer knows when the child has finished speaking — Silero VAD via vendored `silero-vad-rust 6.2.1` (with 2-line `is_multiple_of` patch). Landed in voice round-trip POC.
- [ ] Handle ambient noise gracefully (these are children's rooms, not recording studios)

### 2.2 — Text-to-speech

- [x] Implement `PiperBackend` for TTS
- [ ] Select/create a warm, patient voice profile (the Primer should sound like a favourite teacher, not a GPS)
- [x] Implement streaming TTS: begin speaking before generation is complete (sentence-boundary chunking)
- 2026-05-02 — Streaming `StreamingTextToSpeech` trait + Piper backend (vendored + ort-rc.10 patch). Smoke binary `tts_hello`. PR #6.

### 2.3 — Conversation flow with speech

- [ ] Echo cancellation (the Primer's own voice shouldn't trigger STT)
- [x] Interrupt handling (if the child starts talking while the Primer is speaking, stop and listen) — cancel-on-SpeechStart in `speech_loop`: a `SpeechStart` event mid-LATENT_THINK aborts the in-flight LLM future and waits for the next `SpeechEnd` before retrying. Landed in voice round-trip POC.
- [ ] Silence handling (long pauses during thinking are normal for children — don't rush them)
- [x] Voice round-trip POC (`--speech` mode) — end-to-end LISTEN → LATENT_THINK → SPEAK → LISTEN state machine in `primer-cli::speech_loop`. `primer-speech::cpal_io` (`MicCapture`, `SpeakerSink`, `Resampler`). `rust-toolchain.toml` pins Rust 1.87+. `--speech`, `--whisper-model`, `--voice-onnx`, `--voice-config`, `--voice`, `--mic-silence-ms` CLI flags. 10+ mock-driven `speech_loop` tests + 5 `cpal_io` unit tests. 2026-05-02.

**Phase 2 exit criteria:** A child can have the Phase 0 conversation entirely by voice, with the Primer speaking responses aloud.

---

## Phase 3 — Hardware Enclosure

**Goal:** A physical device a child can hold.

### 3.1 — Display

- [ ] Integrate a colour E Ink display (7-8", capacitive touch) or repurpose a tablet display
- [ ] Design a child-friendly UI: large text, illustrations for concepts, no distracting chrome
- [ ] Support both text and illustrated conversation modes

### 3.2 — Audio hardware

- [ ] MEMS microphone array integration (beam-forming for noise rejection)
- [ ] Speaker selection and integration
- [ ] Physical volume control (no software-only volume — children need a knob)

### 3.3 — Power and enclosure

- [ ] Battery management (target: 5-7 hours active use)
- [ ] Thermal design (passive cooling for the compute module)
- [ ] Drop-resistant enclosure (this will be held by children)
- [ ] Physical power button with a satisfying click

### 3.4 — Compute module integration

- [ ] Mount the chosen SBC or phone-as-compute-module in the enclosure
- [ ] Wire display, audio, and power
- [ ] Boot-to-Primer: the device turns on and is immediately ready for conversation

**Phase 3 exit criteria:** A child can pick up the device, turn it on, and have the Phase 2 conversation without any other equipment.

---

## Phase 4 — Pedagogical Depth

**Goal:** The Primer becomes a genuinely effective learning companion, not just a Socratic chatbot.

- [ ] Curriculum alignment — map the concept graph to real educational standards (Australian Curriculum, IB PYP) so parents and teachers can see what's being explored
- [ ] Multi-session learning arcs — the Primer notices that a child has been circling a concept across several sessions and designs a sequence to deepen understanding
- [ ] Parental dashboard — a simple web UI showing what the child has been exploring, where they're strong, where they're struggling (read-only, no surveillance)
- [ ] Collaborative mode — two children sharing a Primer, with the dialogue engine adapting to multiple learners
- [ ] Assessment without testing — the Primer continuously assesses understanding through conversation, never through quizzes or scores
- [ ] Aggregated age-appropriate language corpus — opt-in, explicitly parent-consented contribution of (age, technical-term, plain-language explanation, child-comprehension signal) tuples to a shared dataset for fine-tuning future small models on genuinely age-calibrated language. PII scrubbing happens on-device before anything leaves: child names, family members, locations, and any free-form notes are stripped. The vocabulary store from Phase 0.3 is the source. No data ever leaves a device without an explicit per-child, per-export consent action by a parent

---

## Who works on what

The crate boundaries are designed so contributors can work in parallel without stepping on each other.

| Contributor | Natural focus areas |
|---|---|
| **Horst Herb** | `primer-pedagogy` (dialogue engine, prompt engineering), `primer-cli` (developer UX), `primer-knowledge` (corpus curation), integration testing, project coordination |
| **Bernd Brinkmann** *(EE/ML)* | `primer-inference` (local backends — llama.cpp, QNN, RKNN), Phase 3 hardware (display, audio, enclosure, power), thermal design |
| **Frithjof Herb** *(ML / math)* | `primer-inference` (model quantisation, benchmark harness), `primer-pedagogy` (comprehension assessment classifier, learner model ML), `primer-knowledge` (embedding-based semantic search) |
| **Claude** *(Anthropic AI pair-programmer, via Claude Code)* | Implementation pairing on Phase 0 work, code review, refactoring, test scaffolding, documentation upkeep — under Horst's direction. Commits that include AI work are tagged with a `Co-Authored-By: Claude …` trailer. |

Bernd and Frithjof are listed as planned contributors — both are currently busy with other commitments and have not yet committed code. The Phase 1 hardware/local-inference work is genuinely waiting on them. Everyone can contribute to Phase 0 immediately: it's pure Rust, runs anywhere, and needs no special hardware.

---

## Principles that don't change

These hold across all phases:

1. **The Primer asks more questions than it answers.** If a phase makes the Primer more answer-y, something went wrong.
2. **The Primer does not maximise engagement.** It suggests breaks. It lets children stop. It never guilt-trips.
3. **All learner data stays on-device.** Cloud inference is stateless. The learner model never leaves the device without explicit parental consent.
4. **The pedagogical engine is testable without hardware.** Every phase must keep the stub backend working. If you can't test the Socratic behaviour in a terminal, the architecture is wrong.
