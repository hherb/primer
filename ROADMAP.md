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
- [ ] Handle API errors gracefully (rate limits, network drops, invalid key) with clear user-facing messages — partial. Mid-stream errors propagate cleanly and the partial Primer turn is dropped. Full retry/backoff on rate limits is still TODO.
- ✅ Add conversation persistence — sessions persist to SQLite via `primer-storage` (per-turn save). Default location is `~/.primer/<slug-of-name>.db`; explicit path via `--session-db <path>`. See [docs/superpowers/specs/2026-04-30-session-persistence-sqlite-design.md](docs/superpowers/specs/2026-04-30-session-persistence-sqlite-design.md)
- ✅ Resume past session — `--resume <uuid>` CLI flag loads a stored `Session` and picks up without a greeting. Errors clearly when the file or id is missing.
- ✅ Long-term memory across the context window — schema v2 added a rolling LLM-generated `Session.summary` (refreshed on resume + every 20 turns) plus FTS5 retrieval (`turn_text_fts`) of relevant older turns; both inject into the system prompt while the chat-message timeline stays = `recent_turns(window)`. Context budget is bounded across hours of conversation.

### 0.2 — Knowledge base bootstrapping

- [ ] Write an ingestion script (Python is fine) that downloads Simple English Wikipedia dumps and inserts passages into the SQLite FTS5 database
- [ ] Add a curated seed corpus — 50-100 hand-picked passages covering topics children commonly ask about (space, dinosaurs, weather, the human body, animals, how things work)
- [ ] Test RAG retrieval quality: does "why is the sky blue" retrieve Rayleigh scattering passages? Does "how do plants eat" retrieve photosynthesis?
- [ ] Tune `RetrievalParams` defaults (top_k, min_score) based on real queries

### 0.3 — Pedagogical engine refinement

- [ ] Implement concept extraction from child responses (currently a placeholder — even simple keyword matching against a concept taxonomy would be a major improvement)
- [ ] Add LLM-based comprehension assessment: after the child responds, use a short classifier prompt to gauge whether the response demonstrates genuine understanding, parroting, or confusion
- [ ] Implement learner model persistence (SQLite) so the Primer remembers what a child knows across sessions
- [ ] Per-child vocabulary tracking with spaced repetition — when the Primer introduces a new technical word ("repel", "plasma", "insulator"), record it in the same SQLite store as concepts (vocabulary as a flavour of `ConceptState`, e.g. `concept_id = "vocab:repel"`, with the plain-language explanation given and the child's apparent grasp). The dialogue manager uses last-encountered timestamps and encounter counts to weave recently-introduced words back into the prompt at expanding intervals — within-session, next-session, then a week later — so children leave the conversation having *gained* new words rather than having them quietly avoided. **Schema constraint:** capture vocabulary entries in a form that can later be anonymised (no child name, no session-specific personal details inside the term record itself) so the Phase 4 corpus-contribution path remains open
- [ ] Add session time tracking with gentle break suggestions ("We've been exploring for 25 minutes — want to take a break and come back to this?")
- ✅ Write unit tests for `decide_intent()` — characterization tests landed (18 tests in `prompt_builder::tests`); they pin down current behaviour and document the gaps below
- ✅ Make `Encouragement` reachable from `decide_intent` — landed in the engagement-classifier work (Phase 0.3): `LlmEngagementClassifier` supplies `EngagementAssessment` into `decide_intent`; "frustrated but still trying" now routes to `Encouragement`, "frustrated and stuck" to `Scaffolding`
- ✅ Detect factual-question patterns in `decide_intent` so "what is X?" / "how does Y work?" route to `DirectAnswer`, with `AnswerThenPivot` on the next turn — landed in the engagement-classifier work; both intents are now reachable
- ✅ Make `Disengaging` session-length-aware — route to `Encouragement` early in a session, `SessionClose` only after a meaningful duration — landed in the engagement-classifier work using session turn count as the duration proxy

### 0.4 — Developer experience

- [ ] Add `cargo test` coverage for all crates (core trait contracts, prompt builder output, dialogue manager state transitions, knowledge base retrieval) — partial. 180 tests across the workspace after the engagement-classifier landing (Phase 0.3): streaming parsers + stub-summarize (19 in `primer-inference`), `decide_intent`/prompt builder including long-term-memory injection (24 in `primer-pedagogy::prompt_builder`), `dialogue_manager` including resume + summary refresh + classifier integration (16+ in `primer-pedagogy::dialogue_manager`), session persistence + v2/v3 migration + load + FTS retrieval + `turn_classifications` (41+ in `primer-storage`), classifier trait + `LlmEngagementClassifier` + `StubEngagementClassifier` (in `primer-classifier`), CLI slug + path resolution (7 in `primer-cli`), and the enum-variant arrays (2 in `primer-core`). `primer-knowledge` retrieval is the only crate still without coverage.
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

- [ ] Implement `WhisperBackend` for STT (whisper.cpp or candle-whisper)
- [ ] Add voice activity detection (VAD) so the Primer knows when the child has finished speaking
- [ ] Handle ambient noise gracefully (these are children's rooms, not recording studios)

### 2.2 — Text-to-speech

- [ ] Implement `PiperBackend` for TTS
- [ ] Select/create a warm, patient voice profile (the Primer should sound like a favourite teacher, not a GPS)
- [ ] Implement streaming TTS: begin speaking before generation is complete (sentence-boundary chunking)

### 2.3 — Conversation flow with speech

- [ ] Echo cancellation (the Primer's own voice shouldn't trigger STT)
- [ ] Interrupt handling (if the child starts talking while the Primer is speaking, stop and listen)
- [ ] Silence handling (long pauses during thinking are normal for children — don't rush them)

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
