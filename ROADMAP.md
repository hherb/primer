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

- ✅ Hybrid retrieval pipeline (BM25 + dense-vector RRF) — landed (PR #34, 2026-05-05; specs at [docs/superpowers/specs/2026-05-05-hybrid-retrieval-design.md](docs/superpowers/specs/2026-05-05-hybrid-retrieval-design.md) and [docs/superpowers/specs/2026-05-05-kb-bootstrapping-design.md](docs/superpowers/specs/2026-05-05-kb-bootstrapping-design.md)). New `primer-core::embedder::Embedder` trait, new `primer-core::rrf::fuse` Reciprocal Rank Fusion helper (`k = 60`), new `primer-embedding` crate (`StubEmbedder` always built; `FastEmbedBackend` with BGE-M3 default behind the `fastembed` feature; `OllamaEmbedder` behind the `ollama` feature). `KnowledgeBase::retrieve_hybrid` and `SessionStore::retrieve_session_turns_hybrid` both run BM25 + cosine in parallel and fuse via RRF; default trait impls fall back to BM25-only when no embedder is wired so every consumer can call them unconditionally. Knowledge-base schema bumped to v3 (`embedding_models` lookup table + per-locale `embeddings_<pack>` BLOB column); session-store schema bumped to v8 (mirror tables for per-turn vectors). Mismatched dim under an existing model name is a hard error in both stores. Configurable via `--embedder-backend none|stub|fastembed|ollama` (default `none`), `--embedder-model`, `--embedder-ollama-url`. Embedding-on-save runs in a `tokio::spawn` after `save_session`, fire-and-forget; idempotent at the storage layer.
- ✅ JSONL ingestion + auto-seed + `--reembed` infrastructure — landed in PR #34. New `primer-kb-load` crate exposes `load_jsonl(kb, path)` (idempotent on passage id), `auto_seed_if_empty(kb, locale)` (discovers `seed_passages.<pack>.jsonl` via `$PRIMER_SEED_DIR` → `$XDG_DATA_HOME/primer/seed/` → walking up from `CARGO_MANIFEST_DIR`), and `reembed_kb(kb, embedder, force, batch_size)` (backfills embeddings for passages missing one; `--force` re-embeds everything under a new model). Standalone binary `primer-kb-load --reembed --knowledge-db <path> --locale <pack>` is feature-gated on `fastembed`.
- ✅ Initial seed corpus — 55 hand-drafted CC0-1.0 passages across all five planned clusters at `data/seed/seed_passages.en.jsonl` (space: sun, moon, planets, stars, galaxies, black holes, comets, eclipses, tides, big bang, day/night; body: heart, lungs, brain, digestion, skeleton, muscles, skin, immune system, eyes, ears, sleep/dreams, where babies come from; how-things-work: magnets, electricity, batteries, simple machines, sound, light, mirrors/lenses, fire, ice/water, salt, soap; life: photosynthesis, seeds, food chains, cells, evolution, dinosaurs, mammals, birds, reptiles/amphibians, insects; earth/weather: rain, snow, clouds, wind, lightning, rainbows, water cycle, volcanoes, earthquakes, mountains, climate vs weather). Auto-loads on first run when the KB is empty.
- ✅ Retrieval-quality test in CI — `primer-kb-load/tests/retrieval_quality.rs` loads both seed files via `auto_seed_if_empty` into a temp DB and runs 58+ canonical child queries across every cluster (incl. wiki-targeted probes like "what is gravity", "what is a virus", "what is climate change"), asserting at least one top-5 passage mentions the expected key terms. Runs on every push so corpus edits that regress retrieval get caught early.
- ✅ Simple English Wikipedia ingestion (Phase 0.2 MVP, 2026-05-06; PR #36 merged at commit `44a70b8`; spec at [docs/superpowers/specs/2026-05-06-wikipedia-ingestion-design.md](docs/superpowers/specs/2026-05-06-wikipedia-ingestion-design.md), plan at [docs/superpowers/plans/2026-05-06-wikipedia-ingestion.md](docs/superpowers/plans/2026-05-06-wikipedia-ingestion.md)). Python pipeline at `data/ingest/` ingests 35 Simple English Wikipedia Science articles via the MediaWiki extracts API (lead-only chunking, batched 20-titles-per-request fetch, disambiguation-page detection, hand-curated whitelist seeded from Wikipedia Vital Articles). Output JSONL ships in-repo at `data/seed/wiki_passages.en.jsonl` (CC-BY-SA-3.0; per-passage `source_url` carries the canonical Wikipedia URL through to the `sources` table). `primer-kb-load::auto_seed_if_empty` extended to load all `*.<pack>.jsonl` files in the seed dir, so the wiki layer auto-loads alongside the existing CC0 seed corpus on a fresh KB. A future locale's wiki layer (e.g. `wiki_passages.de.jsonl`) auto-loads on a same-locale session without code change. Deferred follow-ups for the ingest tooling: #37 (real User-Agent contact identifier), #38 (429/5xx backoff in `fetch_leads`), #40 (aggregate per-source attribution for the wiki layer), #41 (tighten the disambiguation regex if false positives appear).
- ✅ Tune `RetrievalParams` defaults against the broader corpus — landed (Phase 0.2 close, 2026-05-06; plan at [docs/superpowers/plans/2026-05-06-retrieval-tuning.md](docs/superpowers/plans/2026-05-06-retrieval-tuning.md)). 87-query benchmark with 20 strict-subset canonical-id mappings drives an `#[ignore]`'d 24-cell `(top_k × min_score)` BM25 sweep at `src/crates/primer-kb-load/tests/retrieval_sweep.rs`; tuned production defaults are `KB_FINAL_TOP_K = 5` and `KB_BM25_ONLY_MIN_SCORE = 0.5`, with `RetrievalParams::default()` now reading from `primer_core::consts` as the single source of truth. The `retrieval_quality.rs` regression test pins production defaults with both loose-substring and strict-canonical-id assertions. A 54-cell hybrid-sweep scaffold at `src/crates/primer-kb-load/tests/retrieval_sweep_hybrid.rs` (`--features fastembed --ignored`) is staged for the eventual `HybridParams` defaults exercise; companion hybrid-retrieval canonical-query test is tracked at #39. One known corpus gap surfaced by the strict tier ("how does the sun shine") is filed as #42.

### 0.3 — Pedagogical engine refinement

- ✅ Implement concept extraction from child responses — landed via the `primer-extractor` crate (`ConceptExtractor` trait, `LlmConceptExtractor` + `StubConceptExtractor` impls). One LLM call per completed (child, primer) exchange returns separated `{ child_concepts, primer_concepts }`; spawned async after primer save, persisted via `SessionStore::update_turn_concepts`, applied to in-memory `LearnerModel.concepts` at the next-turn boundary
- ✅ Add LLM-based comprehension assessment: after the child responds, use a short classifier prompt to gauge whether the response demonstrates genuine understanding, parroting, or confusion — landed as `primer-comprehension` crate (Phase 0.3): `LlmComprehensionClassifier` produces per-concept `{depth, confidence, evidence}` assessments against the extractor's candidate set; results promote `LearnerModel.concepts.depth` via monotonic max (threshold-gated) AND persist to schema-v5 `turn_comprehensions` for offline depth-trajectory analysis.
- ✅ Implement learner model persistence (SQLite) so the Primer remembers what a child knows across sessions — landed via the `LearnerStore` trait + schema v4 migration; CLI startup adopts existing `sessions.learner_id` on first-run-after-upgrade
- ✅ Per-child vocabulary tracking with spaced repetition — landed (Phase 0.3, 2026-05-05; spec at [docs/superpowers/specs/2026-05-05-vocabulary-spaced-repetition-design.md](docs/superpowers/specs/2026-05-05-vocabulary-spaced-repetition-design.md)). New `primer-core::vocab` module with three pure functions (`apply_box_transition`, `is_due`, `due_concepts`) implements a Leitner-box scheduler with intervals `[1d, 3d, 7d, 14d, 30d]`. `apply_comprehension` advances the box on every accepted assessment; `build_turn_prompt` injects the top-K most-overdue concepts as a passive prompt-section that the LLM weaves in only if topically relevant. Schema v7 adds `learner_concepts.box_level INTEGER NOT NULL DEFAULT 0`. Configurable via `--vocab-max-per-prompt N` (default 4). All learner data stays per-DB. **Schema constraint preserved:** vocabulary entries continue to live as `ConceptState` rows so the Phase 4 anonymisation/contribution path is unchanged
- ✅ Add session time tracking with gentle break suggestions — landed (Phase 0.3 close, 2026-05-05; spec at [docs/superpowers/specs/2026-05-05-session-break-suggestion-design.md](docs/superpowers/specs/2026-05-05-session-break-suggestion-design.md)). New `PedagogicalIntent::SuggestBreak` (id=9) is seeded automatically into existing DBs on next open. `decide_intent_at_with_pack` consults `primer_core::session_timing::should_suggest_break_now` after engagement-state overrides (a frustrated child past the threshold gets `Scaffolding`, not `SuggestBreak`). Cadence resets on each fire. Locale-aware `{minutes}` interpolation via `pack.break_suggestion_intro(minutes)`. Configurable via `--session-break-after-mins N` (default 30; must be ≥1). The Primer never enforces a session halt — children can keep going through any number of nudges. `PedagogyConfig.break_suggest_after_minutes` is the renamed field (was `max_session_minutes`).
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

**Phase 0 exit criteria:** You can sit a child in front of a terminal, type `cargo run --bin primer -- --backend cloud --name Binti --age 8`, and have a 15-minute Socratic conversation about a topic of their choosing that feels qualitatively different from ChatGPT. The Primer asks more questions than it answers. It catches parroting. It suggests breaks. It remembers what was discussed last time. Phase 0.1, Phase 0.2, and Phase 0.3 are all complete; the only remaining Phase 0 work is the developer-experience items in 0.4 (CI setup, broader retrieval test coverage). Next milestone is Phase 1 (local llama.cpp inference).

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
