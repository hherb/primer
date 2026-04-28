# The Primer

A Socratic AI learning companion for children — inspired by the Young Lady's Illustrated Primer in Neal Stephenson's *The Diamond Age*.

The Primer doesn't teach by telling. It teaches by asking. When a child says "Why is the sky blue?", the Primer doesn't recite Rayleigh scattering — it asks "What colour does the sky turn at sunset? Why do you think it changes?" and walks the child toward discovering the answer themselves.

## Status

**Early skeleton** — the trait architecture and module boundaries are in place, the code compiles, and you can run a text-mode REPL against either a stub backend (canned Socratic responses) or the Anthropic Claude API. There is no local inference, no speech pipeline, and no hardware integration yet. See [ROADMAP.md](ROADMAP.md) for what comes next.

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

Two backends today:

- **StubBackend** — returns canned Socratic responses. No model, no network, no dependencies. Use this for developing and testing the dialogue engine.
- **CloudBackend** — calls the Anthropic Messages API. Requires an API key. This is the path to a working proof of concept before local models are integrated.

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
cargo run --bin primer -- --backend cloud --api-key $ANTHROPIC_API_KEY --name Binti --age 8
```

Or set the environment variable:

```bash
export ANTHROPIC_API_KEY=sk-ant-...
cargo run --bin primer -- --backend cloud --name Binti --age 8
```

### CLI options

```
--backend <stub|cloud>     Inference backend (default: stub)
--name <name>              Child's name for the learner profile (default: Explorer)
--age <age>                Child's age in years (default: 8)
--knowledge-db <path>      Path to SQLite knowledge base (default: in-memory)
--api-key <key>            Anthropic API key (or set ANTHROPIC_API_KEY)
```

## Design Principles

**The Primer never gives a direct answer when it can ask a guiding question instead.** If the child asks a pure factual question ("How far is the moon?"), it answers directly, then pivots: "Now that you know it's 384,000 km — how long would it take to drive there?"

**The Primer does not try to maximise engagement.** If a child wants to stop, the Primer says "That's enough for today" without guilt. It detects frustration and disengagement from response patterns and adjusts — offering scaffolding, suggesting a topic change, or closing the session.

**All data is local.** The learner model (what the child knows, how deeply they understand it, what topics sustain their attention) never leaves the device without explicit parental consent. Cloud inference sends conversation turns per-request; nothing is stored server-side.

**Comprehension is verified, not assumed.** The Primer probes understanding through transfer questions ("Can you explain it to someone who's never heard of it?"), application challenges ("What would happen if gravity were twice as strong?"), and contradiction probing ("Someone told me plants eat soil — what would you say to them?").

## Team

- **Horst** — emergency physician, software developer, ongoing maintainer
- **Son-in-law** — PhD in electrical engineering, teaches ML at UTAS, two children (the Primer's first users)
- **Younger son** — mathematician/chemist, PhD in ML for analytical chemistry

## License

AGPL-3.0 — see [LICENSE](LICENSE).
