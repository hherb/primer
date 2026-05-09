# Gemma 4 E2B / E4B — Suitability Evaluation for the Primer

*Background research note. May 2026.*

This document evaluates whether Google DeepMind's Gemma 4 edge models (**E2B** ≈ 2.3 B effective, **E4B** ≈ 4.5 B effective, released 2 April 2026) are a good fit for the Primer — both as the dialogue/classifier/extractor/comprehension backbone and as a possible replacement for the Whisper-based STT path in `--speech` mode.

The headline: **Gemma 4 E2B/E4B are strong candidates for the Primer's local-tier dialogue and classification work today, and they are a serious — but not yet drop-in — candidate for unifying STT + dialogue once two infrastructure gaps close.**

---

## 1. What the E-series actually is

| Spec                    | E2B                              | E4B                              |
|-------------------------|----------------------------------|----------------------------------|
| Effective params        | ~2.3 B                           | ~4.5 B                           |
| Context window          | 128 K                            | 128 K                            |
| Modalities              | text, image, **audio**           | text, image, **audio**           |
| Audio max length        | 30 s per request                 | 30 s per request                 |
| Audio encoder           | redesigned, 50 % smaller than Gemma 3N, 40 ms frame duration | same |
| Languages (text)        | natively trained on 140+         | same                             |
| Audio tasks             | ASR + speech translation + spoken Q&A + audio summarisation | same |
| RAM (Q4)                | ~1.5–3 GB                        | ~4–5 GB                          |
| RAM (BF16)              | ~5 GB combined family floor; ~15 GB high end | same |
| License                 | Apache 2.0                       | Apache 2.0                       |

These are not standalone ASR models. The E-series is an LLM with a bolted-on audio encoder that emits embeddings consumed by the same backbone that does text generation. Audio is just another input channel into the prompt; the model can transcribe, translate, answer questions about what was said, or continue a conversation that started as speech — all via prompting.

---

## 2. Audio quality vs. Whisper (the backbone of our current `--speech` path)

Public Open ASR Leaderboard benchmarks (twango.dev, May 2026):

- **LibriSpeech clean**: E4B WER **4.17 %** beats Whisper-base.en (4.25 %). E2B is a few points behind but in the same league for clean read speech.
- **Conversational / noisy** (AMI, Earnings22, GigaSpeech): the gap to specialised ASR widens. E4B AMI ≈ 41 % WER, E2B ≈ 202 % (the model writes ~2× as many words as the reference — pathological insertion behaviour).
- **Short clips** (< 1 s): both edge models hallucinate badly. E2B sub-1 s mean WER ~2 200 %, E4B ~220 %. Anything ≥ 3 s is roughly fine.
- **30-second hard cap**: any clip longer than 30 s must be chunked client-side, including segmenting on VAD boundaries.

**Implication for the Primer.** Children's speech is short, often noisy, and frequently sub-3 s ("um, the sun?"). The reported short-clip and conversational behaviour is exactly the failure mode we cannot ship to a child:

- Stub-only "uh-huh" or "yes" turns risk hallucinated verbose transcripts.
- Living-room background noise (siblings, TV, kitchen) lands closer to AMI than to LibriSpeech.
- Children's voices are out-of-distribution for most adult-speech corpora; neither Whisper nor Gemma 4 publishes a kid-speech eval, so this needs in-house testing either way.

Whisper-small/medium plus our Silero VAD gating (see `primer-speech::vad_debounce`) currently handles these conditions reasonably. Until we have first-party evidence that Gemma 4 E4B holds up on short, noisy, child-spoken phrases, **we should not retire Whisper**.

---

## 3. Runtime / deployment status

This is where the practical answer diverges sharply by feature.

### Text-only chat / classifier / extractor / comprehension
- **Ollama**: ✅ first-class. Tags include `gemma4:e2b`, `gemma4:e4b`, `gemma4:e2b-it-q4_K_M`, etc. Works today via our existing `OllamaBackend` with `--backend ollama --model gemma4:e2b` (or `e4b`). We are *already smoke-testing against `gemma4:e4b`* — that is what drove the 3000 ms classifier and 5000 ms extractor `blocking_timeout` defaults documented in CLAUDE.md.
- **llama.cpp / GGUF**: ✅ available, full quantisation matrix.
- **MLX, vLLM**: ✅ supported.

### Audio input
- **`llama-mtmd-cli`**: ✅ supported. `-hf ggml-org/gemma-4-E2B-it-GGUF --mmproj <bf16-mmproj> --audio <file.wav>` works for transcription and spoken Q&A. The combined vision+audio mmproj is **BF16-only** — the projector tensors don't satisfy K-quant block alignment, so there is no Q4/Q8 mmproj.
- **`llama-server`**: 🟡 partially supported. The OpenAI-compatible `/chat/completions` endpoint accepts `input_audio` content blocks per the docs, but a recent server-side issue (#21868) reported missing routing for audio inputs and was closed as "not planned". State of play is muddy as of May 2026 — needs verification with a current build before betting the architecture on it.
- **Ollama**: ❌ no audio. Ollama's API surface today is text + images for the models that support image input; audio is not exposed. Our `OllamaBackend` is therefore **text-only against Gemma 4** for the foreseeable future.

**Implication.** If we want native audio in the Primer, the path runs through **llama.cpp directly** (or a binding such as `llama-cpp-rs`), not through our existing `OllamaBackend`. That is a new backend in `primer-inference`, with its own retry semantics and its own multipart prompt assembly (audio embeddings interleaved with text turns).

---

## 4. Mapping to the Primer's architecture

The CLAUDE.md trait-based design makes most of these decisions cleanly separable. Walking the existing crates:

### `primer-inference` — dialogue backbone
- **E4B at Q4_K_M is a strong fit for the Tier-1 local dialogue role** that `inference_architecture.md` already nominates (the doc currently lists "Gemma 4 E2B" as a candidate). 4.5 B effective, dense, ~5 GB at Q4, 128 K context.
- E2B is the better fit for phone-class hardware (Pixel 9 Pro range gets ~10–25 tok/s). On a Raspberry Pi 5 / 8 GB E2B is ~3–8 tok/s, E4B is ~2–4 tok/s — borderline acceptable for E4B, comfortable for E2B.
- Adopting them via the **existing `OllamaBackend` is zero-effort** for text. The path that becomes a real research project is the local llama.cpp backend already on the Phase 1 roadmap; Gemma 4 strengthens that direction.

### `primer-classifier`, `primer-extractor`, `primer-comprehension`
- These already run today against `gemma4:e4b` and the soft-fail policies + bumped timeouts (3 s / 5 s / 5 s) were calibrated against it. So this is **production-validated**, not speculative.
- 128 K context is overkill for these one-shot structured-output calls — E2B at Q4 would handle them with comfortable headroom and free memory for the dialogue model. Once the local llama.cpp path lands, **E2B as the classifier/extractor/comprehension model alongside E4B for dialogue** is a natural split.

### `primer-speech` — the interesting case
The current speech loop is:

```
mic → Silero VAD → Whisper STT → text → DialogueManager → Piper TTS → speaker
```

Gemma 4 E-series advertises ASR + spoken Q&A + audio understanding as a single model. In principle that lets us collapse two boxes:

```
mic → Silero VAD → Gemma 4 E4B (ASR + dialogue in one prompt) → text → Piper TTS → speaker
```

**Why this is attractive:**
- One model in memory instead of two (Whisper-medium is ~1.5 GB, removing it freezes a slot for a larger dialogue model).
- The transcription has the same priors as the dialogue model — fewer "the model heard X but reasons about Y" failures.
- Spoken-Q&A capability means the model can use prosody / hesitation cues that pure text loses. This matters for engagement classification: a hesitant "I... think... it's gravity?" reads as low-confidence in audio in a way the transcribed text doesn't capture.
- "Did you really mean photosynthesis or were you guessing?" becomes a query the model can answer from the audio itself rather than from text post-hoc.

**Why we should not retire Whisper yet:**
1. **Ollama can't carry audio.** Users on the default backend lose voice mode entirely. We'd need the local llama.cpp backend before this even compiles.
2. **Short-clip hallucination is a child-speech showstopper.** Until we eval E4B against actual children speaking ≥ 3 s phrases at known SNRs, we cannot guarantee the model won't invent words a child didn't say. Whisper has known, bounded failure modes on this distribution; Gemma 4 doesn't.
3. **30-s window forces VAD anyway.** We don't escape the VAD-debounce machine — we still need to chunk on speech boundaries. So `primer-speech::vad_debounce` stays. The savings are smaller than they first look.
4. **mmproj is BF16-only.** The audio projector adds ~600 MB–1 GB on top of the Q4 LLM weights. On a 4 GB-RAM SBC that's the difference between fitting and OOM.
5. **Inter-phrase latency tightens.** The dialogue manager today has a comfortable inter-turn pause (where the classifier+extractor+comprehension chain runs in the background — see CLAUDE.md "10 s budget"). If the same model is also doing the ASR pass synchronously, that latency is on the critical path. We need to measure this before redesigning around it.

### `primer-knowledge`, `primer-storage`, `primer-pedagogy`
- No interaction. These are model-agnostic. Locale-keyed FTS5 tables, the Leitner-box scheduler, and `decide_intent` care about strings and timestamps, not model architecture. **Zero changes** to swap to Gemma 4 here.

### `primer-cli`
- `--model gemma4:e2b` / `--model gemma4:e4b` works today against Ollama with no code change.
- A future `--backend llama-cpp` flag is the natural place to expose the audio-capable path. Probably worth gating audio multimodal under a `primer-cli/multimodal-llm` feature so the default build stays small.

---

## 5. Recommendation

### Adopt now (no code changes)
- **Promote `gemma4:e4b` to a documented "recommended local Ollama model" in README/CLAUDE.md** alongside the current llama3.2 examples. We already test against it; making that visible saves users an experiment.
- **Document `gemma4:e2b` for the classifier/extractor/comprehension trio** when running on memory-constrained hardware. Same Ollama plumbing, smaller residency.

### Build next (Phase 1 work, already on the roadmap)
- **`LlamaCppBackend`** in `primer-inference` (already a TODO per CLAUDE.md). When it lands, Gemma 4 E4B at Q4_K_M is the default test target. This unblocks the audio path without committing to it.

### Investigate before committing (gate behind a measurement spike)
- **Native audio dialogue via Gemma 4** is genuinely promising but has three blockers: Ollama can't host it, short-clip hallucination is unproven for children's speech, and BF16 mmproj eats the RAM budget on small SBCs. The cleanest next step is a **2-day spike**: feed `llama-mtmd-cli` a corpus of 30–50 child-spoken Primer-style prompts (3–10 s each, household noise, ages 6–10) and measure WER + hallucination rate against our current Whisper-small baseline. Decide on real numbers, not vendor benchmarks on adult LibriSpeech.

### Do not retire yet
- **Whisper STT** stays the default `--speech` ASR backend until the spike above shows otherwise. The Silero VAD + phrase-split state machine stays regardless — Gemma 4's 30-s ceiling needs it just as much as Whisper does.
- **Cloud Sonnet** stays the development-default and Tier-2 supervisor (per `inference_architecture.md`). Gemma 4 changes the Tier-1 picture, not the cloud-supervisor picture.

### Specifically not recommended
- Routing the **dialogue** path through `gemma4:e2b`. E2B is too small for the Socratic-question-quality bar (`decide_intent` outputs need to be coherent across 20+ turns; that's where 4–7 B starts paying off). Keep E2B for the structured-output side-tasks and E4B (or a 7 B-class model) for dialogue.
- Treating "native audio" as a reason to drop Silero VAD or the phrase splitter. Both stay needed.

---

## 6. Concrete experiments worth running (in priority order)

1. **Child-speech ASR spike** (above). Measure WER, hallucination rate, and median latency for E4B audio vs. Whisper-small on representative Primer audio.
2. **Quality A/B on dialogue**: 50 Primer turns through `gemma4:e4b` vs. our current `claude-sonnet-4-6` defaults, scored against the `prompt_builder` characterization tests for intent fidelity. Free, runs over Ollama today.
3. **Memory residency on a target SBC**: simultaneously load E4B Q4 (dialogue) + E2B Q4 (classifier/extractor/comprehension) on an Orange Pi 6 Plus / 8 GB and measure peak RSS through a 20-turn session. This is the Phase 1 "does the local-first plan actually fit" gate.
4. **Translation-as-a-feature spike**: E-series does speech-to-translated-text in one shot. For non-English locales (the i18n design doc anticipates these) this could replace a separate translation step. Worth knowing whether the translation quality is good enough that locale packs can lean on it.

---

## Sources

- [Gemma 4 — Google DeepMind](https://deepmind.google/models/gemma/gemma-4/)
- [Gemma 4: Byte for byte, the most capable open models](https://blog.google/innovation-and-ai/technology/developers-tools/gemma-4/)
- [Gemma 4 model overview | Google AI for Developers](https://ai.google.dev/gemma/docs/core)
- [Gemma 4 E2B vs E4B: edge models that run audio and vision on your phone](https://www.mindstudio.ai/blog/gemma-4-e2b-vs-e4b-edge-models-audio-vision-phone)
- [What Is Gemma 4's Audio Encoder?](https://www.mindstudio.ai/blog/gemma-4-audio-encoder-e2b-e4b-speech-recognition)
- [How Good is Gemma 4 at ASR Tasks? Benchmarking Against the Open ASR Leaderboard](https://twango.dev/writing/gemma4-asr-benchmark)
- [llama.cpp multimodal docs](https://github.com/ggml-org/llama.cpp/blob/master/docs/multimodal.md)
- [llama.cpp PR #21421 — mtmd: add Gemma 4 audio conformer encoder support](https://github.com/ggml-org/llama.cpp/pull/21421)
- [llama.cpp issue #21868 — server: add input_audio content type routing for Gemma 4 audio inference](https://github.com/ggml-org/llama.cpp/issues/21868)
- [Ollama library — gemma4](https://ollama.com/library/gemma4)
- [Unsloth: Gemma 4 — How to Run Locally](https://unsloth.ai/docs/models/gemma-4)
- [NVIDIA: Bringing AI Closer to the Edge and On-Device with Gemma 4](https://developer.nvidia.com/blog/bringing-ai-closer-to-the-edge-and-on-device-with-gemma-4/)
