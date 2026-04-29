# Speculations and Ideas

Open-ended technical considerations that aren't yet roadmap commitments. Entries here are "evaluate before adopting" — the goal is to capture the thinking so it isn't lost between sessions, not to commit the project to anything.

Each entry should record: what the idea is, where it might fit (and where it doesn't), what would need to be verified before adopting it, and a status. When something graduates to "we're doing this", move the relevant bits into [ROADMAP.md](ROADMAP.md) and mark the entry here as adopted.

---

## tract — Rust-native ML inference

**Status:** under consideration; evaluation experiment proposed for the speech and embedding paths.
**Originated:** April 2026 conversation about local-inference options.

[tract](https://github.com/sonos/tract) is Sonos's pure-Rust inference engine. Originally built for ONNX/NNEF and traditional ML deployment on edge devices. No C/C++ dependencies. CPU-focused (no mainstream GPU/NPU acceleration).

### Where it does NOT fit: the 7B LLM

The Primer hardware plan depends on the RK1828 NPU (or QNN on Snapdragon) hitting 60+ tok/s on a quantised 7B model. tract cannot reach those NPUs — they need vendor runtimes (RKLLM, QNN SDK). On the bare RK3588 CPU, any inference engine is in the ~2 tok/s ballpark on a 7B model, which is too slow for conversational use per the technical spec. tract's quantisation story for transformer-class LLMs is also less mature than llama.cpp's. Conclusion: do not use tract for `InferenceBackend`. Stick with the planned llama.cpp / RKLLM / QNN backends.

### Where it might fit

The supporting models around the LLM are smaller, more classical, and benefit from a clean pure-Rust stack:

- **Whisper STT** (Phase 2.1). tract has Whisper support. Compared to whisper-rs / whisper.cpp it eliminates C/C++ build-system complexity and gives predictable cross-compilation from MacBook → Orange Pi → embedded enclosure. Same target binary size story.
- **Piper or similar small TTS** (Phase 2.2). Same argument as STT.
- **Sentence-embedding model for the knowledge base.** `primer-knowledge` is currently FTS5/BM25 only. A small embedding model (e5-small, MiniLM-class) running via tract would enable hybrid lexical + semantic retrieval, fully on-device, with no Python or C deps. This naturally extends the existing `KnowledgeBase` trait without a new dependency layer.

This is also a nice fit with the project's trait architecture: `SpeechToText`, `TextToSpeech`, and a future embedding trait can each have a `TractBackend` independent of whatever serves `InferenceBackend`.

### What needs to be verified before adopting

The recommendation is contingent — don't refactor anything based on the above without these checks:

1. **Whisper-small latency via tract** on the MacBook and on a representative ARM target (Orange Pi 5 or comparable RK3588). Acceptance: streaming STT with <1s latency-to-first-word for utterances <10s.
2. **Op coverage** for the specific Whisper / Piper / embedding model variants we want. tract's coverage is good but not universal — confirm the chosen model loads and produces correct outputs against a reference implementation.
3. **Memory footprint** under sustained operation. Concurrent STT + embedding lookup + LLM context shouldn't push the 32GB device into swap.

A 1–2 day spike is enough to answer all three. Should happen before Phase 2.1 begins.

### Open questions

- Does tract benefit from RK3588's Mali-G610 GPU at all (via Vulkan compute or similar), or is it strictly CPU? If GPU is reachable that changes the calculus for medium-sized models.
- Is there a sensible quantisation path for the embedding model under tract, or would we need to keep it FP16/FP32 only? Affects RAM budget.
