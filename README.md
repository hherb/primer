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

**Phase 0.1 done; Phase 0.2 done; Phase 0.3 done.** The trait architecture and module boundaries are in place, the text REPL holds real Socratic conversations against either the Anthropic Claude API or a local Ollama model, a hand-drafted CC0 seed corpus plus a 35-article Simple-English-Wikipedia layer (CC-BY-SA-3.0) and a 66-article German Klexikon layer (CC-BY-SA-4.0) auto-load on a fresh KB (locale-keyed), the hybrid (BM25 + dense-vector) retrieval pipeline is wired through, and both BM25-only AND hybrid retrieval defaults have been tuned via diagnostic sweeps over the 91-passage English corpus (hybrid achieves 100% loose / 100% strict recall on all 91 benchmark queries / 24 strict-subset canonical mappings; the issue #45 paraphrase queries that remained after the initial closure have been closed by adding a `seed:en:flowers` passage and a stomach-growl sentence to `seed:en:digestion`).

**What works today:**

- **Streaming generation** — tokens arrive in the terminal as the model produces them.
- **Conversation persistence** — every turn writes through to a normalised SQLite store (one DB per child under `~/.primer/`, kept separate from the RAG corpus on privacy grounds).
- **Session resume by UUID** — `--resume <uuid>` picks up a past conversation; no greeting is emitted on resume.
- **Long-term memory** — once a conversation grows past the active context window, a rolling LLM-generated summary plus FTS5 retrieval over older turns are injected into the system prompt, so the chat-message timeline stays bounded but the model has access to the whole history.
- **Engagement classifier** — runs one model behind the chat (configurable via `--classifier-backend` / `--classifier-model`), persisting per-turn assessments to `turn_classifications` for cross-session analysis.
- **Concept extractor** — runs after each completed exchange (configurable via `--extractor-backend` / `--extractor-model`), extracts the topics the child surfaced and the topics the Primer introduced, and writes them atomically to `turn_concepts` while updating the in-memory learner model.
- **Comprehension classifier** — chained after the concept extractor (configurable via `--comprehension-backend` / `--comprehension-model`), assesses the depth of the child's understanding for each concept the exchange touched. Per-concept `{depth, confidence, evidence}` rows persist to `turn_comprehensions`; the in-memory `LearnerModel.concepts.depth` is promoted via monotonic max (threshold-gated, never demoted by a single weak exchange).
- **Spaced-repetition vocabulary review** — concepts the child has previously encountered are gently surfaced back into the conversation at expanding intervals (1d / 3d / 7d / 14d / 30d) via a Leitner-box scheduler driven by the existing comprehension classifier (no extra LLM call). Strong re-confirmation advances the box; an `Aware` reading or sub-confidence resets it. The Primer's system prompt receives a passive hint list — the LLM weaves words in only if topically relevant; no drilling, no quizzing. Configurable via `--vocab-max-per-prompt N` (default 4). Schema v7 persists `box_level` alongside the existing depth/confidence/last-encountered state.
- **Session break suggestions** — after a configurable wallclock interval (default 30 minutes), the Primer's next utterance is phrased as a gentle, in-character break suggestion. Cadence resets on each suggestion so nudges stay gentle and spaced. Engagement-state overrides win: a frustrated child past the threshold gets `Scaffolding` or `Encouragement`, not a break nudge. The Primer never enforces a session halt — children can keep going through any number of suggestions. Configurable via `--session-break-after-mins N`.
- **Hybrid retrieval (BM25 + dense vector)** — the knowledge base and the long-term-memory layer both run a lexical leg (FTS5/BM25) and a semantic leg (cosine over per-passage f32 vectors) in parallel and fuse the two ranked lists via Reciprocal Rank Fusion (`k = 60`). When no embedder is wired the path falls back to BM25-only — every consumer can call the hybrid API unconditionally. Embedder identity is recorded in a per-DB `embedding_models` lookup table; mismatched dim or unrecorded model is a hard error, preventing silent quality regressions when a user re-opens a DB with a different embedder. Opt-in via `--embedder-backend none|stub|fastembed|ollama` (default `none`); `fastembed` uses BGE-M3 (1024-dim multilingual, ~570 MB on first run) behind the `embedding` cargo feature.
- **Knowledge-base bootstrapping (Phase 0.2)** — three corpora ship in-repo and auto-load on a fresh KB, locale-keyed: (1) A hand-drafted CC0 seed corpus of 56 passages across all five planned clusters (space, body, how-things-work, life, earth/weather) at `data/seed/seed_passages.en.jsonl`. (2) A 35-article Simple English Wikipedia layer at `data/seed/wiki_passages.en.jsonl` (CC-BY-SA-3.0; physics fundamentals, chemistry, biology, earth science, health) covering concepts the seed corpus does not. (3) A 66-article German children's-wiki layer from [Klexikon](https://klexikon.zum.de/) at `data/seed/wiki_passages.de.jsonl` (CC-BY-SA-4.0) — auto-loads when the CLI is started with `--language de`. The Python ingestion pipeline at `data/ingest/` re-generates each wiki layer from its own hand-curated whitelist via a `WikiSource`-parameterised pipeline; Simple English uses the MediaWiki TextExtracts API while Klexikon uses `parse&prop=wikitext&section=0` + an in-house wikitext stripper because Klexikon's MediaWiki has no TextExtracts extension. See `data/ingest/README.md`. A standalone `primer-kb-load` binary supports JSONL ingestion and `--reembed` backfill for re-embedding passages under a new model. The retrieval-quality integration test exercises 91 canonical child queries across the English layers (with 24 strict-subset canonical-id mappings on top of loose-substring assertions) and pins production behaviour against regression in CI. A parallel German benchmark (31 child-style German queries, 25 strict canonical-id mappings against the Klexikon corpus) ships alongside as `tests/retrieval_quality_de.rs` + `tests/retrieval_quality_hybrid_de.rs`. 5 of those queries are stress paraphrases authored to use child-style vocabulary deliberately mis-aligned with the canonical article's lead — BM25 misses 3 of the 5, the dense leg lifts 1 of those 3 (`bauch komische geräusche` → `verdauung`), and the other 2 are documented corpus-coverage gaps (`gänsehaut` and `ebbe/flut` — Klexikon's `haut` article doesn't describe the goosebumps reflex, and its `mond` article doesn't discuss tides; no embedding can bridge content that isn't there). The 3 BM25 misses live in `KNOWN_FAILING_QUERIES_DE` and the 2 hybrid corpus gaps in `KNOWN_FAILING_QUERIES_DE_HYBRID`, both with documented rationales mirroring the EN issue #45 option-(b) resolution.
- **Tuned hybrid retrieval defaults via diagnostic sweeps over the 91-passage corpus** — a 24-cell `(top_k × min_score)` BM25 sweep at `src/crates/primer-kb-load/tests/retrieval_sweep.rs` selected production defaults `KB_FINAL_TOP_K = 5` and `KB_BM25_ONLY_MIN_SCORE = 0.5`. A 54-cell hybrid sweep at `src/crates/primer-kb-load/tests/retrieval_sweep_hybrid.rs` (`--features fastembed --ignored`) tuned `HybridParams::default()` to `(bm25_top_k=30, vector_top_k=30, final_top_k=5, rrf_k=60)`, achieving 100% loose / 100% strict recall on all 91 benchmark queries / 24 strict-subset canonical mappings, and lifting the BM25-only strict miss for "how does the sun shine" (former issue #42). All 4 paraphrase queries re-added in issue #45 are now satisfied by the hybrid path — 2 via the dense leg ("what is inside a tiny bug" → seed:en:insects, "why does the brain need oxygen from the lungs" → seed:en:brain) and 2 via the seed-corpus expansion landed in the issue #45 closure ("what makes my tummy growl when I am hungry" → seed:en:digestion (stomach-growl sentence added); "why do flowers smell nice" → seed:en:flowers (new hand-drafted passage)). `KNOWN_FAILING_QUERIES_HYBRID` is therefore empty; new entries should be investigated before adding. Both `RetrievalParams::default()` and `HybridParams::default()` read from a single source of truth in `primer_core::consts`, with drift-prevention tests pinning the alignment. The hybrid path's regression test (`retrieval_quality_hybrid.rs`) ships in two flavours — a structural always-built check using `StubEmbedder`, and a recall floor under `--features fastembed` against real BGE-M3. The defensive `KB_BM25_ONLY_MIN_SCORE = 0.5` floor (a no-op for recall on the current corpus) is itself defended by an `#[ignore]`'d tripwire diagnostic at `src/crates/primer-kb-load/tests/bm25_floor_tripwire.rs` — fires loudly if future corpus expansion dilutes BM25 scores enough that the floor would start filtering genuine top-K hits (former issue #44). Parallel German diagnostic sweeps (`tests/retrieval_sweep_de.rs`, `tests/retrieval_sweep_hybrid_de.rs`) measure the same grid against the 66-passage Klexikon corpus + 31-query German benchmark — production defaults clear all non-paraphrase queries; the 5 stress paraphrases (issue #64) are excluded from the regression assertions via the `KNOWN_FAILING_QUERIES_DE*` lists and serve as the documented BM25-vs-hybrid-lift demonstration plus 2 corpus-coverage gaps the dense leg cannot bridge.
- **Graceful inference-error handling** — typed `InferenceError` variants, bounded jittered retry on transient conditions (rate limits, 5xx, network flap), single i18n-ready render boundary. A child whose API key is wrong sees an actionable message instead of a raw 401.
- **Learner-model persistence** — profile, concept-mastery state, learning preferences, and latest engagement snapshot persist across sessions via a `LearnerStore` trait + schema v4. A returning child carries forward their identity and progress.
- **Voice round-trip POC** (`--speech`, behind a Cargo feature) — full LISTEN → THINK → SPEAK → LISTEN loop wired end-to-end: Silero VAD opens the mic, Whisper transcribes, the dialogue manager generates the response, Piper synthesises it phrase-by-phrase. No barge-in by design (the Primer never speaks over the child and the child never speaks over the Primer); cancel-on-resume preserves Socratic etiquette without freezing the loop.
- **Desktop GUI (Tauri 2)** — a small native window (`primer-gui` crate) that exposes the Primer to non-CLI users (parents, older children, the developer during evaluation). Launch shows a session picker — past sessions for the configured learner, clickable to resume — alongside a "Start new session" button that opens a settings modal mirroring every CLI flag (backend, model, locale, embedder, classifier/extractor/comprehension subsystems, vocab and break tuning, persistence). Chat surface is markdown-rendered bubbles with streaming caret; a right-hand sidebar (collapsible, default-open) surfaces the per-turn signals the engine produces (intent badge, engagement state with confidence bar, extractor-surfaced concepts split by speaker, per-concept comprehension depth) plus a longitudinal Learner panel (vocab review queue with Leitner-box dots, concept-depth distribution bar, recent-engagement strip) and a Session timeline with click-to-scroll to any past turn. Vanilla HTML/CSS/JS (no npm framework); the Tauri backend embeds a long-lived `DialogueManager` and streams response tokens via `primer://chunk` / `primer://turn_complete` events.
- **Multilingual prompt packs + preview-locale gate** — every user-facing string is keyed off a `Locale` enum; per-locale prompt packs (`prompts/en.toml`, `prompts/de.toml`, `prompts/hi.toml`) carry the system prompt, age-band language guidance, intent instructions, engagement notes, and voice-mode UI copy. English and German are production-ready; Hindi is in preview (machine-translated, awaiting native-speaker review). The preview gate is two firewalls: a closed `PackStatus` enum exposed via `[meta] status = "preview"` (loader emits a one-time `tracing::warn!`) and exclusion from `Locale::ALL` (CLI/GUI pickers iterate that slice, so end users don't see preview locales). Developers can still exercise a preview locale via `--language hi`. See [`docs/localisation/`](docs/localisation/) for the contributor manual and per-locale status pages.
- **OpenAI-compatible inference backend** — `OpenAiCompatBackend` speaks `/v1/chat/completions` with SSE streaming, error classification, and bounded jittered retry. Talks to any OpenAI-API-shaped server: oMLX, LM Studio, vLLM, llama.cpp `--server`, Together, Groq, OpenRouter. Unlocks ~20–40% throughput gains on Apple Silicon via MLX-native servers. See [docs/superpowers/specs/2026-05-15-openai-compat-backend-design.md](docs/superpowers/specs/2026-05-15-openai-compat-backend-design.md).
- **Voice mode in the desktop GUI** (Phase A — composer-zone widget) — header "Voice mode" toggle ends the current text session and starts a voice session in its place. The composer area swaps to an animated state widget (mic-pulse on LISTEN, thinking-spin on LATENT_THINK, speaker-pulse on SPEAK); the transcript and the evaluation sidebar stay visible alongside. Belt-and-suspenders cancel/exit: Stop button, Esc key, "goodbye"/"bye primer"/"stop primer" voice keywords, and the header toggle itself. Locale-default Whisper + Piper assets auto-download to `~/.cache/primer/models/` on first launch via a consent dialog (or `disable_auto_download` for strict-offline). State machine is the same `primer-speech::voice_loop` shared with the CLI's `--speech` mode — one source of truth for the no-barge-in invariants. Build with `cargo run -p primer-gui --features speech` and click "Voice mode" in the header. The big-central-ear/mouth child-facing visual is Phase B, a separate spec gated on voice-mode reliability validation.

**Phase 0.2 and Phase 0.3 are both complete.** Hybrid retrieval, the hand-drafted CC0 seed corpus across all five planned clusters, a 35-article Simple-English-Wikipedia layer (CC-BY-SA-3.0), and tuned BM25-only AND hybrid retrieval defaults all ship today. Still ahead (see [ROADMAP.md](ROADMAP.md)): local llama.cpp inference, ongoing hardening of the speech loop (macOS-native Apple-platform path ships now; a macOS 26+ SpeechAnalyzer variant is in flight), hardware integration.

**Validated platforms (2026-05-26):** Phase 0 text REPL runs end-to-end on a RedMagic 11 Pro (Snapdragon 8 Elite, 24 GB RAM) via Termux — cloud backend against Anthropic, session persistence, full classifier/extractor/comprehension chain. See [docs/devel/redmagic-termux-quickstart.md](docs/devel/redmagic-termux-quickstart.md). On-device Ollama at 4B Q4 on CPU is functionally correct but **too slow for conversational use** — the standalone-phone path is dependent on Phase 1.2 (`QnnBackend` for the Hexagon NPU).

## Architecture

The codebase is a Rust workspace under `src/`, organised into fifteen crates. The core design principle is **trait-based hardware abstraction**: the pedagogical engine doesn't know or care whether it's talking to a local 7B model on a phone's NPU, llama.cpp on a laptop, or Claude over the network. Backend selection is a runtime config choice, not a code change.

```
src/
├── Cargo.toml                  # workspace root
└── crates/
    ├── primer-core/            # traits + shared types (everyone depends on this)
    ├── primer-inference/       # LLM backends (stub, cloud, ollama; later: llama.cpp, QNN, RKNN)
    ├── primer-qnn-sys/         # Phase 1.2 FFI scaffold: dlopen + raw Genie C API decls (Android-only path)
    ├── primer-speech/          # VAD + STT + TTS backends (Silero, Whisper, Piper, cpal; macos-native: SFSpeechRecognizer + AVSpeechSynthesizer; macos-native-26: SpeechAnalyzer + AVSpeechSynthesizer)
    ├── primer-knowledge/       # SQLite FTS5 + dense-vector hybrid knowledge base
    ├── primer-storage/         # SQLite session + learner-model persistence
    ├── primer-classifier/      # per-turn engagement classifier (LLM-backed + stub)
    ├── primer-extractor/       # per-exchange concept extractor (LLM-backed + stub)
    ├── primer-comprehension/   # per-exchange comprehension classifier (LLM-backed + stub)
    ├── primer-embedding/       # embedder backends (stub, fastembed/BGE-M3, ollama)
    ├── primer-kb-load/         # JSONL ingestion + auto-seed-on-empty + --reembed backfill
    ├── primer-pedagogy/        # Socratic dialogue engine (prompt builder + dialogue manager)
    ├── primer-engine/          # shared wiring helpers (backend construction, path resolution)
    ├── primer-cli/             # text-mode REPL binary
    └── primer-gui/              # Tauri 2 desktop app (chat bubbles + settings modal + sidebar)
```

### primer-core

Defines the trait contracts that all backends implement:

- `InferenceBackend` — text generation (streaming and non-streaming)
- `KnowledgeBase` — passage retrieval with BM25 ranking + optional hybrid (BM25 + dense-vector RRF) when an `Embedder` is wired
- `Embedder` — text → fixed-dimension f32 vector for the dense-vector retrieval leg
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

- **QnnBackend** *(behind the `qnn` cargo feature on `primer-inference`; wired into `primer-cli` behind matching `--features qnn`)* — Qualcomm Hexagon NPU via the Genie SDK, lazy-dlopened from `primer-qnn-sys`. Steps 1.2.2–1.2.5 of the Phase 1.2 plan land the safe wrapper, CLI wiring, and the 4K context budget: `primer-meta.json` parsing, chat-template renderer via `minijinja`, mutex-serialised `GenieDialog` session, ABI smoke check at construction, per-token streaming via a `Box::into_raw` C-ABI callback feeding an `mpsc` channel (the receiver is wrapped as a `TokenStream` and the `GenieDialog_query` call runs inside `tokio::task::spawn_blocking` so the runtime stays healthy under multi-second decode latencies), and `--backend qnn --qnn-bundle-dir <path>` on the REPL with env-var fallbacks (`PRIMER_QNN_BUNDLE_DIR`, `PRIMER_QNN_QAIRT_LIB_DIR`) and a startup warning when every subsystem (classifier / extractor / comprehension) inherits the NPU backend — that configuration serialises all background LLM work behind the chat turn through the dialog mutex. Step 1.2.5 adds a per-backend context budget: because the QnnBackend's `name()` begins `qnn:`, the dialogue manager automatically shrinks the recent-turn window (20 → 12) and the KB retrieval top-K (5 → 3) so the assembled prompt fits a 4K-token model. Step 1.2.6 ships the device benchmark harness — `cargo run --release --example qnn_bench --features qnn` loops a 30-prompt Socratic corpus, measures TTFT and steady-state decode tok/s, samples `/sys/class/thermal`, and emits a pass/fail verdict against the targets (≥ 15 tok/s sustained decode, < 3 s TTFT, ≤ 70 °C); its aggregation logic is host-tested while the numbers themselves await a device run. Real on-device run-through still gated on Phase 1.2 step 1.2.0 (QAIRT install + chatapp_android device validation).

Future backends (not yet implemented): `LlamaCppBackend` (CPU/Vulkan), `RknnBackend` (Rockchip RK1828 NPU).

### primer-pedagogy

The Socratic engine — where the Primer's personality lives. Two modules:

- **prompt_builder** — constructs system prompts that vary by the child's age, engagement state, and what the dialogue manager wants to accomplish next (ask a guiding question, check comprehension, scaffold with an analogy, extend a concept, close the session).
- **dialogue_manager** — orchestrates the conversation loop: receive child input, decide pedagogical intent, retrieve knowledge, build prompt, generate response, update the learner model.

### primer-knowledge

SQLite-backed hybrid knowledge base. The lexical leg is FTS5 with BM25 ranking; the semantic leg is an in-Rust cosine over per-passage f32 vectors (1:1 with FTS5 `content_rowid`, BLOB column). When an `Embedder` is wired, `KnowledgeBase::retrieve_hybrid` runs both legs in parallel and fuses the ranked lists via Reciprocal Rank Fusion (`primer_core::rrf::fuse`, `k = 60`); when no embedder is wired, the same call falls back transparently to BM25-only. Tables are per-locale so BM25 statistics stay locale-pure and tokenizers can diverge per language. Schema v3 added an `embedding_models` lookup table that records `(model_name, dim)` on first vector write — subsequent writes that report a different dim under the same name fail loudly, so silently swapping embedders on an existing DB is impossible. The pedagogical engine uses retrieved passages to ground the LLM's responses in verified information.

### primer-embedding

Concrete `Embedder` backends. `StubEmbedder` (deterministic FNV+xorshift hash → L2-normalised f32 vector; default dim 384; model id `"stub-fxhash-v1"`) is always built and is the runtime fallback when a real-model download fails. Behind the `fastembed` cargo feature: `FastEmbedBackend` wraps `fastembed-rs` with BGE-M3 as the default model (1024-dim multilingual, ~570 MB int8 — covers EN/DE/JA/HI in one model so the initial test cohort never hits a model swap). Behind the `ollama` cargo feature: `OllamaEmbedder` calls `/api/embeddings`. All three implement the `Embedder` trait and report a stable model name for the per-DB identity check.

### primer-kb-load

JSONL ingestion + auto-seed-on-empty + `--reembed` backfill for the knowledge base. `load_jsonl(kb, path)` ingests passages (idempotent: skips already-present ids). `auto_seed_if_empty(kb, locale)` discovers `seed_passages.<pack_id>.jsonl` via `$PRIMER_SEED_DIR` → `$XDG_DATA_HOME/primer/seed/` → walking up from `CARGO_MANIFEST_DIR` and loads it on a freshly-empty KB. `reembed_kb(kb, embedder, force, batch_size)` backfills embeddings for passages missing one, or re-embeds everything under a new model when `force` is set. A standalone binary `primer-kb-load --reembed --knowledge-db <path> --locale <pack>` is feature-gated on `fastembed`. A retrieval-quality integration test asserts that canonical child queries surface passages with the expected key terms — corpus edits that regress retrieval get caught in CI.

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

Apple-platform alternatives sit behind two mutually-exclusive cargo features:

- **`macos-native`** — `SFSpeechRecognizer` for STT (with `requiresOnDeviceRecognition = true`; never falls back to network) and `AVSpeechSynthesizer.writeUtterance:toBufferCallback:` for phrase-by-phrase streaming TTS via `PhraseSplitter`. Silero stays as the VAD on this path because macOS-26-only `SpeechDetector` would break the macOS 13 floor. en-US + de-DE only — Hindi is deferred until Apple ships on-device `hi-IN`. PCM chunks stream into the speaker ringbuf as `AVSpeechSynthesizer` emits them, so audio begins playing while later phrases are still being synthesised. (PRs #95, #112, #122, #123.)
- **`macos-native-26`** — landed (PR #134, 2026-05-23). Replaces Whisper + Silero + ONNX runtime with `SpeechAnalyzer` + `SpeechTranscriber` + `SpeechDetector` via a Swift sidecar bridged through `swift-bridge`. Motivated by an empirical latency probe (PR #131): SpeechAnalyzer is ~100× faster to first partial (~30 ms vs ~3.8 s) and ~2× faster to final (~800 ms vs ~1.8 s) than Whisper `ggml-small.en` on macOS 26.5. Mutually exclusive with `macos-native` at compile time. en-US + de-DE only; Hindi `hi-IN` errors loudly at construction (not on-device for `SpeechTranscriber` yet).

A vendored **Supertonic TTS** backend (PRs #127, #128) is also available behind a `supertonic` feature for A/B evaluation. As of PR #175 it implements the `TextToSpeech` + `StreamingTextToSpeech` traits (`SupertonicTts`, one voice per instance like `PiperTts`, 44.1 kHz native output, streaming via `PhraseSplitter`), so it can stand in for Piper in a voice loop — but it is still gated behind the `supertonic` feature and not wired onto any default code path (Stage C voice-loop wiring is pending the #170 v2/v3 ONNX-compat spike).

Two pure helper modules (`vad_debounce`, `phrase_split`) carry the streaming state machines so they can be unit-tested without any backend dep. The `--speech` flag in `primer-cli` is gated by a top-level `speech` feature that pulls all four. See the [Voice mode](#voice-mode-experimental-poc) section below for setup.

### primer-engine

Small internal library that holds the shared wiring helpers — `build_backend`, `build_classifier`, `build_extractor`, `build_comprehension`, `build_fastembed_embedder`, `build_ollama_embedder`, `resolve_session_db_path`, learner reconciliation, locale-mismatch guards. Both `primer-cli` and `primer-gui` import them so the REPL and the desktop app construct identical session stacks without copy-pasting setup code. Behaviour-neutral by design — the engine doesn't add policy, only shares plumbing.

### primer-cli

A text-mode REPL for developing and testing the dialogue without any hardware. This is the primary development interface for now. With `--features speech` it gains a `--speech` flag that swaps the text REPL for a voice loop driven by `primer-speech`.

### primer-gui

A Tauri 2 desktop app that exposes the Primer to non-CLI users. Backend is Rust (embeds a long-lived `DialogueManager` via `primer-engine`'s wiring helpers; streams response tokens to the frontend through `primer://chunk` / `primer://turn_complete` events; persists settings to `~/.primer/gui-config.json` with mode 0600 so an inline API key can live there safely). Frontend is vanilla HTML/CSS/JS in Tauri's WebView — no npm framework. Three surfaces: a session picker at launch (lists past sessions for the configured learner; click to resume by UUID), a chat window with markdown-rendered bubbles + cancel-mid-stream, and a settings modal mirroring every CLI flag with two save modes ("Save & start new session" vs. "Save (next session only)"). A collapsible right-hand sidebar surfaces the same per-turn signals the CLI's `--verbose` flag prints, plus a longitudinal Learner panel (vocab review queue with Leitner-box dots, concept-depth distribution, recent-engagement strip) and a Session timeline with click-to-scroll to any past turn. Text-only in v1 — voice mode is a placeholder pending the speech-loop hardening pass.

## Building

Requires Rust 1.88+ (edition 2024). No system dependencies for the default build — SQLite is bundled, TLS uses rustls. The `--speech` voice loop additionally needs system `espeak-ng` (see [Voice mode](#voice-mode-experimental-poc)); the desktop GUI needs the platform's standard WebView (already present on macOS and Windows; on Debian/Ubuntu run `apt install libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev`).

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

### Desktop GUI mode

A native window with chat bubbles, a session picker on launch, a settings modal mirroring every CLI flag, and a collapsible sidebar showing the pedagogical signals the engine produces per turn (intent, engagement, concepts, comprehension, vocab review queue). Aimed at parents and older children who want to evaluate or monitor the system without touching the CLI.

```bash
cargo run --bin primer-gui
```

Settings persist to `~/.primer/gui-config.json` (mode 0600). The first launch ships with the stub backend so you can click through the surface without an API key; pick **Settings** in the launch picker to switch to cloud or ollama and then **Save & start new session**. Sessions persist to the same per-learner SQLite file the CLI uses (`~/.primer/<slugified-name>.db`), so the GUI and the CLI share data — a session started in either can be resumed in the other.

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
--backend <stub|cloud|ollama|openai-compat>
                                Inference backend (default: stub)
--model <id>                    Model id (cloud default: claude-sonnet-4-6;
                                required for ollama and openai-compat)
--ollama-url <url>              Ollama server URL (default: http://localhost:11434)
--openai-compat-url <url>       OpenAI-compatible server URL
                                (default: http://localhost:8000; or env OPENAI_COMPAT_URL).
                                Works with oMLX, LM Studio, vLLM, llama.cpp --server,
                                Together, Groq, OpenRouter.
--openai-compat-api-key <key>   Bearer token for the OpenAI-compatible server
                                (or env OPENAI_COMPAT_API_KEY). Optional for local
                                servers; required for remote providers.
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
--session-break-after-mins N    Minutes between break-suggestion nudges (default 30; must be ≥1).
--embedder-backend <name>       Embedder backend for hybrid retrieval:
                                none|stub|fastembed|ollama|openai-compat
                                (default: none = BM25-only, the pre-Phase-0.2.5 behaviour). `stub`
                                is for testing the hybrid pipeline only; `fastembed` requires
                                --features primer-cli/embedding (downloads BGE-M3, ~570 MB on
                                first run); `ollama` requires --features primer-cli/ollama-embedding;
                                `openai-compat` requires --features primer-cli/openai-compat-embedding.
--embedder-model <id>           Embedder model name. Defaults: `bge-m3` for fastembed,
                                `nomic-embed-text` for ollama.
--embedder-ollama-url <url>     Ollama endpoint for `--embedder-backend ollama`
                                (default: http://localhost:11434).
--embedder-openai-compat-url <url>
                                OpenAI-compatible /v1/embeddings endpoint
                                (falls back to --openai-compat-url).
--embedder-openai-compat-model <name>
                                Required when --embedder-backend openai-compat.
```

Voice-mode flags (only when built with `--features primer-cli/speech`):

```
--speech                        Run the voice REPL instead of the text REPL. On the
                                whisper+piper build (the default speech path) requires
                                --whisper-model, --voice-onnx, --voice-config. On the
                                macOS-native build (--features speech,macos-native on
                                macOS) those three flags are not declared at all —
                                SFSpeechRecognizer + AVSpeechSynthesizer replace them.
--whisper-model <path>          Path to the whisper.cpp GGML/GGUF model file
                                (e.g. ~/models/ggml-small.en.bin). Not present on
                                the macOS-native build.
--voice-onnx <path>             Path to the Piper voice ONNX file
                                (e.g. ~/models/voices/en_GB-alba-medium.onnx). Not
                                present on the macOS-native build.
--voice-config <path>           Path to the matching Piper voice JSON sidecar
                                (e.g. ~/models/voices/en_GB-alba-medium.onnx.json).
                                Not present on the macOS-native build.
--voice <id>                    VoiceProfile.model_id; must match the file stem of
                                --voice-onnx (default: en_GB-alba-medium). Not
                                present on the macOS-native build.
--mic-silence-ms <ms>           Override Silero's min_silence_ms (default: 600,
                                bounded to [50, 5000]). Used on both speech builds —
                                Silero remains the VAD on macOS-native too.
```

### Resuming a past session

Sessions are persisted automatically. To pick one up later, copy its UUID out of the session DB and pass it via `--resume`:

```bash
sqlite3 ~/.primer/explorer.db 'SELECT id FROM sessions ORDER BY started_at DESC LIMIT 1;'
cargo run --bin primer -- --resume <uuid>
```

When the resumed session has more than `context_window_turns` (default 20) turns, the Primer maintains long-term memory in two complementary ways: a rolling LLM-generated summary (refreshed on resume only when the loaded one is stale, then every 20 further pre-window turns during active conversation) and FTS5-based retrieval of relevant older turns based on the current child input. Both are injected into the system prompt — the chat-message timeline the model sees stays equal to the last 20 turns, so context budget is bounded even across hours of conversation. (Small-context backends such as the Qualcomm NPU use a 12-turn window instead — see the QnnBackend note above.)

### macOS evaluation build

For evaluators on macOS 13+ who want zero external dependencies and the
fastest install path:

```bash
cd src
~/.cargo/bin/cargo tauri build --features "primer-gui/speech primer-gui/macos-native"
```

See [docs/macos_native_speech.md](docs/macos_native_speech.md) for details.

## Building the macOS DMG

Produces a signed and notarized `.dmg` for the desktop GUI, ready to hand to evaluators with no Gatekeeper friction. Apple Silicon only.

**One-time prerequisites:**

- Install the Tauri 2 CLI:
  ```bash
  ~/.cargo/bin/cargo install tauri-cli --version "^2.0"
  ```
- A `Developer ID Application` certificate from the Apple Developer Program in your login keychain. Verify with `security find-identity -p codesigning -v` — you should see a line matching `Developer ID Application: <Your Name> (TEAMID)`. If missing, create at developer.apple.com → Certificates → + → Developer ID Application, then double-click the downloaded `.cer` to install.
- An App Store Connect API key with the "Developer" role (re-use the one you already have for App Store submission if applicable). At appstoreconnect.apple.com → Users and Access → Keys → +; download the `.p8` file (you only get one chance) and note the Key ID and Issuer ID. Either export the three variables in your shell profile:
  ```bash
  export APPLE_API_ISSUER="<Issuer ID>"
  export APPLE_API_KEY="<Key ID>"
  export APPLE_API_KEY_PATH="$HOME/.appstoreconnect/AuthKey_XXXXXX.p8"
  ```
  or — easier — copy `scripts/apple-notarize-env.sh.example` to `scripts/apple-notarize-env.sh`, fill in your three values, and `scripts/build-dmg.sh` auto-sources it on every run. The real file is gitignored so your credentials never land in version control.

**Build:**
```bash
./scripts/build-dmg.sh
```

Output: `src/target/aarch64-apple-darwin/release/bundle/dmg/Primer_0.1.0_aarch64.dmg`. Notarization typically takes 3–10 minutes; the script blocks until stapling completes.

**Installing on an evaluator's Mac:** double-click the DMG, drag `Primer.app` to Applications, launch — no Gatekeeper warning expected. The notary stamp is stapled to the bundle so Gatekeeper accepts it offline.

**Updating the app icon:** the source is [assets/curious_childs_primer_icon.png](assets/curious_childs_primer_icon.png). Regenerate the full set with:
```bash
cp assets/curious_childs_primer_icon.png src/crates/primer-gui/icons/source.png
cd src/crates/primer-gui
~/.cargo/bin/cargo tauri icon icons/source.png
```

## Contributing

**Developer manual:** see [docs/devel/](docs/devel/) for the full contributor manual — getting started, architecture, subsystem deep-dives, and how-to recipes (add a new backend, schema migration, locale, …).

## License

AGPL-3.0 — see [LICENSE](LICENSE).
