# The Primer

<p align="center">
  <img src="assets/curious_childs_primer.png" alt="Primer logo" width="380">
</p>

A Socratic AI learning companion for children — inspired by the Young Lady's Illustrated Primer in Neal Stephenson's *The Diamond Age*.

The Primer doesn't teach by telling. It teaches by asking. When a child says "Why is the sky blue?", the Primer doesn't recite Rayleigh scattering — it asks "What colour does the sky turn at sunset? Why do you think it changes?" and walks the child toward discovering the answer themselves.

## Design Principles

- **The Primer can run on local hardware without internet dependence.**
 While the Primer can make use of cloud services (AI API, web search) it is designed to work autonomously and airgapped if that is the user's preference or no connectivity available

- **The Primer never gives a direct answer when it can ask a guiding question instead.** 
If the child asks a pure factual question ("How far is the moon?"), it answers directly, then pivots: "Now that you know it's 384,000 km — how long would it take to drive there?"

- **The Primer does not try to maximise engagement.** 
If a child wants to stop, the Primer says "That's enough for today" without guilt. It detects frustration and disengagement from response patterns and adjusts — offering scaffolding, suggesting a topic change, or closing the session.

- **Comprehension is verified, not assumed.** The Primer probes understanding through transfer questions ("Can you explain it to someone who's never heard of it?"), application challenges ("What would happen if gravity were twice as strong?"), and contradiction probing ("Someone told me plants eat soil — what would you say to them?").

- **Voice-first is a pedagogical choice, not a hardware constraint.** The Primer treats voice interaction as its primary interface. A screen is available for text, diagrams, and code — but it is never required, and for younger children (roughly under 8) it is actively undesirable. The research basis is strong: children who gesture while explaining concepts are significantly more likely to transfer learning to novel problems (Goldin-Meadow, 2009), and a voice-only device frees the child's body to move, gesture, and manipulate objects while thinking. Conversational speech also demands active construction — you cannot skim a conversation the way you can skim text — which produces exactly the kind of effortful cognitive processing that drives deep learning. Screen-based interaction, by contrast, pins attention to a visual surface and displaces the parent-child interaction that remains the most powerful learning environment available. The Primer should feel like a conversation with a thoughtful adult, not like an app.

- **All data is local.** 
The learner model (what the child knows, how deeply they understand it, what topics sustain their attention) never leaves the device without explicit parental consent. Cloud inference sends conversation turns per-request; nothing is stored server-side.


## Status

**Phase 0.1 done; Phase 0.3 substantially progressed.** The trait architecture and module boundaries are in place, and the text REPL holds real Socratic conversations against either the Anthropic Claude API or a local Ollama model.

**What works today:**

- **Streaming generation** — tokens arrive in the terminal as the model produces them.
- **Conversation persistence** — every turn writes through to a normalised SQLite store (one DB per child under `~/.primer/`, kept separate from the RAG corpus on privacy grounds).
- **Session resume by UUID** — `--resume <uuid>` picks up a past conversation; no greeting is emitted on resume.
- **Long-term memory** — once a conversation grows past the active context window, a rolling LLM-generated summary plus FTS5 retrieval over older turns are injected into the system prompt, so the chat-message timeline stays bounded but the model has access to the whole history.
- **Engagement classifier** — runs one model behind the chat (configurable via `--classifier-backend` / `--classifier-model`), persisting per-turn assessments to `turn_classifications` for cross-session analysis.
- **Concept extractor** — runs after each completed exchange (configurable via `--extractor-backend` / `--extractor-model`), extracts the topics the child surfaced and the topics the Primer introduced, and writes them atomically to `turn_concepts` while updating the in-memory learner model.
- **Comprehension classifier** — chained after the concept extractor (configurable via `--comprehension-backend` / `--comprehension-model`), assesses the depth of the child's understanding for each concept the exchange touched. Per-concept `{depth, confidence, evidence}` rows persist to `turn_comprehensions`; the in-memory `LearnerModel.concepts.depth` is promoted via monotonic max (threshold-gated, never demoted by a single weak exchange).
- **Spaced-repetition vocabulary review** — concepts the child has previously encountered are gently surfaced back into the conversation at expanding intervals (1d / 3d / 7d / 14d / 30d) via a Leitner-box scheduler driven by the existing comprehension classifier (no extra LLM call). Strong re-confirmation advances the box; an `Aware` reading or sub-confidence resets it. The Primer's system prompt receives a passive hint list — the LLM weaves words in only if topically relevant; no drilling, no quizzing. Configurable via `--vocab-max-per-prompt N` (default 4). Schema v7 persists `box_level` alongside the existing depth/confidence/last-encountered state.
- **Graceful inference-error handling** — typed `InferenceError` variants, bounded jittered retry on transient conditions (rate limits, 5xx, network flap), single i18n-ready render boundary. A child whose API key is wrong sees an actionable message instead of a raw 401.
- **Learner-model persistence** — profile, concept-mastery state, learning preferences, and latest engagement snapshot persist across sessions via a `LearnerStore` trait + schema v4. A returning child carries forward their identity and progress.
- **Voice round-trip POC** (`--speech`, behind a Cargo feature) — full LISTEN → THINK → SPEAK → LISTEN loop wired end-to-end: Silero VAD opens the mic, Whisper transcribes, the dialogue manager generates the response, Piper synthesises it phrase-by-phrase. No barge-in by design (the Primer never speaks over the child and the child never speaks over the Primer); cancel-on-resume preserves Socratic etiquette without freezing the loop.

**Still ahead** (see [ROADMAP.md](ROADMAP.md)): knowledge-base bootstrapping (Phase 0.2), session-time-based break suggestions (rest of Phase 0.3), local llama.cpp inference, hardening of the speech loop, hardware integration.

## Architecture

The codebase is a Rust workspace under `src/`, organised into ten crates. The core design principle is **trait-based hardware abstraction**: the pedagogical engine doesn't know or care whether it's talking to a local 7B model on a phone's NPU, llama.cpp on a laptop, or Claude over the network. Backend selection is a runtime config choice, not a code change.

```
src/
├── Cargo.toml                  # workspace root
└── crates/
    ├── primer-core/            # traits + shared types (everyone depends on this)
    ├── primer-inference/       # LLM backends (stub, cloud, ollama; later: llama.cpp, QNN, RKNN)
    ├── primer-speech/          # VAD + STT + TTS backends (Silero, Whisper, Piper, cpal)
    ├── primer-knowledge/       # SQLite FTS5 knowledge base for RAG retrieval
    ├── primer-storage/         # SQLite session + learner-model persistence
    ├── primer-classifier/      # per-turn engagement classifier (LLM-backed + stub)
    ├── primer-extractor/       # per-exchange concept extractor (LLM-backed + stub)
    ├── primer-comprehension/   # per-exchange comprehension classifier (LLM-backed + stub)
    ├── primer-pedagogy/        # Socratic dialogue engine (prompt builder + dialogue manager)
    └── primer-cli/             # text-mode REPL binary
```

### primer-core

Defines the trait contracts that all backends implement:

- `InferenceBackend` — text generation (streaming and non-streaming)
- `KnowledgeBase` — passage retrieval with BM25 ranking
- `VoiceActivityDetector` — frame-by-frame speech-vs-silence classification
- `SpeechToText` / `StreamingSpeechToText` — audio → text (one-shot or chunked)
- `TextToSpeech` / `StreamingTextToSpeech` — text → audio (one-shot or phrase-by-phrase)

All speech traits inherit from a small `Named` trait so a backend that implements both the one-shot and streaming variant of a trait writes its `name()` impl exactly once.

Also defines the learner model types: `LearnerProfile`, `ConceptState` (Bloom's taxonomy depth tracking), `EngagementState`, `LearningPreferences`, and the conversation types (`Session`, `Turn`, `PedagogicalIntent`).

### primer-inference

Three backends today, all implementing `InferenceBackend::generate_stream()`:

- **StubBackend** — returns canned Socratic responses. No model, no network, no dependencies. Use this for developing and testing the dialogue engine.
- **CloudBackend** — streams from the Anthropic Messages API via SSE (`event:`/`data:` framing). Requires an API key.
- **OllamaBackend** — streams from a local Ollama server via NDJSON (one JSON object per `\n`-terminated line). Useful for prototype testing against real local models without integrating llama.cpp directly.

Future backends (not yet implemented): `LlamaCppBackend` (CPU/Vulkan), `QnnBackend` (Qualcomm Hexagon NPU), `RknnBackend` (Rockchip RK1828 NPU).

### primer-pedagogy

The Socratic engine — where the Primer's personality lives. Two modules:

- **prompt_builder** — constructs system prompts that vary by the child's age, engagement state, and what the dialogue manager wants to accomplish next (ask a guiding question, check comprehension, scaffold with an analogy, extend a concept, close the session).
- **dialogue_manager** — orchestrates the conversation loop: receive child input, decide pedagogical intent, retrieve knowledge, build prompt, generate response, update the learner model.

### primer-knowledge

SQLite FTS5-backed knowledge base. Stores passages from Wikipedia, curated encyclopedias, and curriculum materials. Retrieves relevant passages via full-text search with BM25 ranking. The pedagogical engine uses retrieved passages to ground the LLM's responses in verified information.

### primer-storage

SQLite-backed conversation + learner-model persistence. Every child turn, every Primer turn, and every `close_session` writes through to disk in an append-only schema (turn rowids stay stable across saves). Categorical text columns (`speaker`, `pedagogical_intent`, `concept`, engagement state, understanding depth) are normalised into integer-keyed lookup tables; the matching Rust enums are the canonical source of truth and the storage layer validates the on-disk lookup tables against them on every `open()` — drift is a hard error. Schema version 2 added rolling-summary fields on `sessions` and a `turn_text_fts` FTS5 virtual table for retrieval of older turns. Version 3 added `turn_classifications` for the engagement classifier. Version 4 added the `learners` table + `learner_concepts` junction (one row per child per concept with depth, confidence, encounter count, last-encountered timestamp, notes) plus a `LearnerStore` trait (`save_learner` / `load_learner`) — concepts are monotonic on disk (a concept dropped from the in-memory `Vec` survives, mirroring real cognition), and `INSERT … ON CONFLICT DO UPDATE` upserts avoid the cascade-wipe footgun. Each migration runs inside a single transaction so a partial failure rolls back to the pre-migration state. Adoption of an existing session's `learner_id` on first-run-after-upgrade is a CLI-level concern via `SessionStore::most_recent_session_learner_id`, not part of the migration. The session DB is intentionally separate from the RAG corpus on privacy grounds (different file, different lifecycle); existing v1/v2/v3 databases are migrated in place on first open.

### primer-classifier

Per-turn engagement classifier. The `EngagementClassifier` trait has two implementations: `LlmEngagementClassifier` (wraps any `InferenceBackend` and prompts it for a structured engagement assessment) and `StubEngagementClassifier` (deterministic, for tests). Soft-fail policy throughout — classifier errors never propagate up; they degrade to "unknown / low confidence" and the conversation continues. Classification of turn N runs on a separate model in the natural inter-turn pause; its result is applied at the start of turn N+1's intent decision. Every assessment is persisted to `turn_classifications` for cross-session analysis and future training data.

### primer-extractor

Per-exchange concept extractor. The `ConceptExtractor` trait mirrors `EngagementClassifier`: an LLM-backed implementation (`LlmConceptExtractor`) plus a stub. One LLM call per completed (child, primer) exchange returns the topics the child surfaced and the topics the Primer introduced as separate lists; both are persisted atomically into `turn_concepts` and merged into the in-memory `LearnerModel.concepts`. Same soft-fail policy as the classifier.

### primer-comprehension

Per-concept comprehension classifier. The `ComprehensionClassifier` trait mirrors `EngagementClassifier` and `ConceptExtractor`: an LLM-backed implementation (`LlmComprehensionClassifier`) plus a stub. After each completed exchange, the dialogue manager chains the comprehension classifier behind the concept extractor: the deduped union of `child_concepts ∪ primer_concepts` (capped at `max_concepts_per_call`) becomes the candidate set the comprehension classifier assesses. The classifier returns a JSON array of per-concept `{depth, confidence, evidence}` rows (depth values from the existing `UnderstandingDepth` enum: `Aware | Recall | Comprehension | Application | Analysis`), persisted atomically to schema-v5 `turn_comprehensions`. Above-threshold assessments promote the corresponding `LearnerModel.concepts.depth` via monotonic max — never demoted by a single weak exchange. Same soft-fail policy as the classifier and extractor.

### primer-speech

The voice pipeline. Stub backends are always available; real backends sit behind Cargo features:

- **Silero VAD** (`silero` feature) — frame-by-frame voice-activity detection over a 16 kHz capture stream.
- **Whisper STT** (`whisper` feature) — `whisper.cpp` GGML/GGUF model, used for both streaming partial transcripts and the final phrase commit.
- **Piper TTS** (`piper` feature) — neural text-to-speech, synthesising one phrase at a time (`PhraseSplitter` chunks the LLM stream on `. ! ?` boundaries) so the Primer can begin speaking before generation has fully finished.
- **cpal I/O** (`cpal` feature) — cross-platform mic capture + speaker playback, gated by an `is_speaking` flag so the Primer's own audio never leaks back through the mic.

Two pure helper modules (`vad_debounce`, `phrase_split`) carry the streaming state machines so they can be unit-tested without any backend dep. The `--speech` flag in `primer-cli` is gated by a top-level `speech` feature that pulls all four. See the [Voice mode](#voice-mode-experimental-poc) section below for setup.

### primer-cli

A text-mode REPL for developing and testing the dialogue without any hardware. This is the primary development interface for now. With `--features speech` it gains a `--speech` flag that swaps the text REPL for a voice loop driven by `primer-speech`.

## Building

Requires Rust 1.85+ (edition 2024). No system dependencies — SQLite is bundled, TLS uses rustls.

```bash
cd src
cargo build
```

## Running

### Stub mode (no model, no API key needed)

```bash
cargo run --bin primer
```

The stub backend returns canned Socratic responses. Useful for testing the dialogue flow, the learner model updates, and the pedagogical intent decisions without any model.

### Cloud mode (Anthropic Claude)

```bash
cargo run --bin primer -- --backend cloud --name Binti --age 8
# uses ANTHROPIC_API_KEY from the environment
```

Or override the model:

```bash
cargo run --bin primer -- --backend cloud --model claude-opus-4-7 --name Binti --age 8
```

### Ollama mode (local model via Ollama)

```bash
cargo run --bin primer -- --backend ollama --model llama3.2 --name Binti --age 8
# defaults to http://localhost:11434; override with --ollama-url
```

`--model` is required for ollama (e.g. `llama3.2`, `qwen2.5:7b`). The model must already be pulled (`ollama pull llama3.2`).

### Voice mode (experimental POC)

The voice loop is gated by the `speech` Cargo feature on `primer-cli`, so default builds stay light:

```bash
cargo build --features primer-cli/speech
```

System prerequisite: **espeak-ng** must be installed for Piper to phonemise text. The bundled `espeak-rs` ships an incomplete subset that fails on most voices.

```bash
# macOS
brew install espeak-ng
# Debian / Ubuntu
sudo apt install espeak-ng-data
```

Then download a whisper.cpp model and a Piper voice (the matching `.onnx` and `.onnx.json` sidecar):

```bash
cargo run --features primer-cli/speech --bin primer -- \
    --backend cloud --name Binti --age 8 \
    --speech \
    --whisper-model ~/models/ggml-small.en.bin \
    --voice-onnx ~/models/voices/en_GB-alba-medium.onnx \
    --voice-config ~/models/voices/en_GB-alba-medium.onnx.json \
    --voice en_GB-alba-medium
```

`--voice` is the `VoiceProfile.model_id` and **must** match the file stem of `--voice-onnx` — Piper rejects mismatches at session open. Piper voices are at <https://huggingface.co/rhasspy/piper-voices>; whisper.cpp models are at <https://huggingface.co/ggerganov/whisper.cpp>.

Build with `~/.cargo/bin/cargo` rather than a Homebrew rust if both are on `PATH` — silero requires a recent rustc that the project's `rust-toolchain.toml` pins, and Homebrew's older toolchain will fail to compile. The first build downloads ONNX Runtime via `ort`'s `download-binaries` feature; subsequent builds are cached.

This is a working POC, not production-grade. Latency, voice quality, and edge-case handling are all under active iteration.

### Configuring secrets

Both `.env` (project-local) and `~/.primer_env` (user-global) are auto-loaded at startup. Drop your `ANTHROPIC_API_KEY` into either:

```
ANTHROPIC_API_KEY=sk-ant-...
```

Project-local `.env` wins over the home file. Both are gitignored. See `.env.example` for the format.

### CLI options

```
--backend <stub|cloud|ollama>   Inference backend (default: stub)
--model <id>                    Model id (cloud default: claude-sonnet-4-6; required for ollama)
--ollama-url <url>              Ollama server URL (default: http://localhost:11434)
--name <name>                   Child's name for the learner profile (default: Explorer)
--age <age>                     Child's age in years (default: 8)
--knowledge-db <path>           Path to SQLite knowledge base (default: in-memory)
--session-db <path>             Path to SQLite session store
                                (default: ~/.primer/<slugified-name>.db, created if missing)
--resume <uuid>                 Resume a past session by UUID. Reads from --session-db; errors if
                                the file or the id is missing. No greeting is emitted on resume.
--no-persist                    Run in-memory only — nothing is written to disk and the conversation
                                evaporates on exit. Mutually exclusive with --resume and --session-db.
--api-key <key>                 Anthropic API key (or set ANTHROPIC_API_KEY)
--classifier-backend <name>     Backend for the engagement classifier (default: same as --backend;
                                pass `stub` to force deterministic empty assessments).
--classifier-model <id>         Model for the engagement classifier (default: same as --model).
--classifier-timeout-ms <ms>    Bounded wait for the previous turn's classification before the next
                                intent decision (default: 500ms).
--extractor-backend <name>      Backend for the concept extractor (default: same as --backend).
--extractor-model <id>          Model for the concept extractor (default: same as --model). Useful
                                for haiku-as-extractor + sonnet-as-chat on bigger machines.
--extractor-timeout-ms <ms>     Bounded wait for the previous turn's extraction (default: 1500ms).
--verbose                       Print pedagogical decisions ([intent], [classifier], [extractor]) to
                                stderr alongside the conversation. Stdout stays clean.
```

Voice-mode flags (only when built with `--features primer-cli/speech`):

```
--speech                        Run the voice REPL instead of the text REPL. Requires
                                --whisper-model, --voice-onnx, --voice-config.
--whisper-model <path>          Path to the whisper.cpp GGML/GGUF model file
                                (e.g. ~/models/ggml-small.en.bin).
--voice-onnx <path>             Path to the Piper voice ONNX file
                                (e.g. ~/models/voices/en_GB-alba-medium.onnx).
--voice-config <path>           Path to the matching Piper voice JSON sidecar
                                (e.g. ~/models/voices/en_GB-alba-medium.onnx.json).
--voice <id>                    VoiceProfile.model_id; must match the file stem of
                                --voice-onnx (default: en_GB-alba-medium).
--mic-silence-ms <ms>           Override Silero's min_silence_ms (default: 600,
                                bounded to [50, 5000]).
```

### Resuming a past session

Sessions are persisted automatically. To pick one up later, copy its UUID out of the session DB and pass it via `--resume`:

```bash
sqlite3 ~/.primer/explorer.db 'SELECT id FROM sessions ORDER BY started_at DESC LIMIT 1;'
cargo run --bin primer -- --resume <uuid>
```

When the resumed session has more than `context_window_turns` (default 20) turns, the Primer maintains long-term memory in two complementary ways: a rolling LLM-generated summary (refreshed on resume only when the loaded one is stale, then every 20 further pre-window turns during active conversation) and FTS5-based retrieval of relevant older turns based on the current child input. Both are injected into the system prompt — the chat-message timeline the model sees stays equal to the last 20 turns, so context budget is bounded even across hours of conversation.

## License

AGPL-3.0 — see [LICENSE](LICENSE).
