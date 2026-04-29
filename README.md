# The Primer

A Socratic AI learning companion for children — inspired by the Young Lady's Illustrated Primer in Neal Stephenson's *The Diamond Age*.

The Primer doesn't teach by telling. It teaches by asking. When a child says "Why is the sky blue?", the Primer doesn't recite Rayleigh scattering — it asks "What colour does the sky turn at sunset? Why do you think it changes?" and walks the child toward discovering the answer themselves.

## Status

**Phase 0.1 — cloud-backed proof of concept, mostly working.** The trait architecture and module boundaries are in place, you can hold a real Socratic conversation against either the Anthropic Claude API or a local Ollama model with **tokens streaming progressively** into the terminal. Phase 0.2/0.3 work (knowledge-base bootstrapping, comprehension assessment, learner-model persistence) is still ahead. There is no local llama.cpp inference, no speech pipeline, and no hardware integration yet. See [ROADMAP.md](ROADMAP.md) for what comes next.

## Architecture

The codebase is a Rust workspace under `src/`, organised into six crates. The core design principle is **trait-based hardware abstraction**: the pedagogical engine doesn't know or care whether it's talking to a local 7B model on a phone's NPU, llama.cpp on a laptop, or Claude over the network. Backend selection is a runtime config choice, not a code change.

```
src/
├── Cargo.toml                  # workspace root
└── crates/
    ├── primer-core/            # traits + shared types (everyone depends on this)
    ├── primer-inference/        # LLM backends (stub, cloud; later: llama.cpp, QNN, RKNN)
    ├── primer-speech/           # STT/TTS backends (stub; later: Whisper, Piper)
    ├── primer-knowledge/        # SQLite FTS5 knowledge base for RAG retrieval
    ├── primer-pedagogy/         # Socratic dialogue engine (prompt builder + dialogue manager)
    └── primer-cli/              # text-mode REPL binary
```

### primer-core

Defines the trait contracts that all backends implement:

- `InferenceBackend` — text generation (streaming and non-streaming)
- `KnowledgeBase` — passage retrieval with BM25 ranking
- `SpeechToText` / `TextToSpeech` — audio pipeline (stub for now)

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

### primer-cli

A text-mode REPL for developing and testing the dialogue without any hardware. This is the primary development interface for now.

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
--api-key <key>                 Anthropic API key (or set ANTHROPIC_API_KEY)
```

## Design Principles

**The Primer never gives a direct answer when it can ask a guiding question instead.** If the child asks a pure factual question ("How far is the moon?"), it answers directly, then pivots: "Now that you know it's 384,000 km — how long would it take to drive there?"

**The Primer does not try to maximise engagement.** If a child wants to stop, the Primer says "That's enough for today" without guilt. It detects frustration and disengagement from response patterns and adjusts — offering scaffolding, suggesting a topic change, or closing the session.

**All data is local.** The learner model (what the child knows, how deeply they understand it, what topics sustain their attention) never leaves the device without explicit parental consent. Cloud inference sends conversation turns per-request; nothing is stored server-side.

**Comprehension is verified, not assumed.** The Primer probes understanding through transfer questions ("Can you explain it to someone who's never heard of it?"), application challenges ("What would happen if gravity were twice as strong?"), and contradiction probing ("Someone told me plants eat soil — what would you say to them?").

## License

AGPL-3.0 — see [LICENSE](LICENSE).
