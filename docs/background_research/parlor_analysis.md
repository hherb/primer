# Parlor: what it does well, and what the Primer should take from it

Analysis of [fikrikarim/parlor](https://github.com/fikrikarim/parlor) (Apache-2.0,
commit `da1ddf1`) against the Primer's voice and inference stack.

Parlor is an on-device real-time voice + vision assistant — the author's attempt
to match OpenAI's GPT-Live on a MacBook M3 Pro with no cloud in the loop. It is
about 2,400 lines of Python across nine source files, and it reaches **~0.7 s
from end-of-utterance to first audio** on a short question with Gemma 4 E2B
(~1.3 s with the default E4B). That number is the reason it is worth studying:
the Primer's voice mode is architecturally similar and materially slower.

Parlor is *not* a model for the Primer's product. It is an engagement-maximising
assistant with timers, web research, and translation modes — much of it directly
contrary to [the Primer's pedagogical principles](../../CLAUDE.md). What it has
is a set of hard-won latency and robustness techniques, several of them backed by
checked-in measurements, in a system shaped almost exactly like ours.

## Their stack vs ours

| Layer | Parlor | Primer |
| --- | --- | --- |
| VAD | Silero, **in the browser** | Silero (`primer-speech`), on-device |
| End-of-turn | **smart-turn-v3 ONNX** (Pipecat, BSD-2), ~20 ms CPU | silence timer (`--mic-silence-ms`) |
| STT | **none — the LLM hears the audio** | Whisper / SFSpeechRecognizer / Android SODA |
| LLM | Gemma 4 E2B/E4B/12B, QAT q4_0, llama.cpp `llama-server` (HTTP) | llama.cpp in-process, QNN, Ollama, cloud |
| Structured output | **grammar-forced JSON** (`response_format: json_schema`) | free-text JSON + `extract_first_json_object` |
| TTS | Kokoro-82M (MLX on Mac, ONNX on Linux) | Piper / Supertonic / AVSpeech |
| Transport | FastAPI + WebSocket to a browser | Tauri desktop / CLI REPL |

Two structural differences drive most of what follows: parlor's LLM is
**audio-native** (no STT stage at all), and parlor talks to llama.cpp over HTTP
so it can exploit **`llama-server`'s prefix cache** across requests.

---

## What to take, ranked by value to the Primer

### 1. Sentence-level LLM → TTS pipelining

**The single biggest latency win available to us, and we already built the
infrastructure for it.**

Parlor's `run_turn` (`src/parlor/pipeline.py`) runs the LLM stream on a producer
thread, feeds deltas through a `StreamParser` that returns *complete sentences*
as they close, pushes each onto a `sentence_q`, and a `tts_worker` synthesises
and ships them while generation continues. First audio therefore lands after the
first sentence, not after the last.

The Primer awaits `responder.respond(...)` to completion, then synthesises
`accumulated` in one call. The `on_chunk` callback exists but only fills a buffer
for GUI replay. Yet `SynthesisSession::push_text` is documented as *"the
synthesiser invokes `on_event` for each PCM chunk and phrase boundary as soon as
it's available"*, and `PiperSession`'s own doc-comment says the design *"gives
the audio pipeline audio to play while the LLM is still generating the next
phrase."* The stateful `PhraseSplitter` inside Piper and Supertonic already holds
back partial phrases. We built the pipeline and then wired it serially.

On a 4-sentence Socratic reply at 15 tok/s this costs roughly **4 seconds of
silence per turn**, and the gap widens on exactly the slow on-device hardware
Phase 1.2 targets.

Filed as **#325**, with the four real complications noted there: incremental
markdown stripping (a delta can split `**bold**`), cancellation once audio is
already committed, the `handle_llm_err` fallback becoming an append rather than a
replace, and the macOS-native main-thread constraint on `push_text`.

### 2. Semantic end-of-turn detection

Parlor does not decide "has the child finished?" with a silence timer. Silero
segments on ~200 ms of silence, and then **smart-turn-v3** — a small acoustic
classifier from Pipecat — judges whether the utterance is a *complete thought*.
If not, the audio is held, the client is told `turn_incomplete`, and the speaker
is allowed to continue; a client-side flush timer answers anyway if they stay
quiet.

This matters more for us than it does for them. A Socratic tutor's whole job is
to provoke the pause where a child is working something out — *"because... um...
because the water goes up?"* A silence timer punishes exactly the behaviour we
are trying to elicit. It cuts the child off, and the Primer answers half a
thought.

Feasibility is good: the model is ONNX and we already carry `ort` pinned at
`=2.0.0-rc.10`; inference is ~20 ms on CPU; it slots in as a
`TurnCompletionDetector` trait beside `VoiceActivityDetector`, consulted at
`SpeechEnd`, with a hold/flush path in the state machine. The cost is porting
parlor's numpy-only Whisper log-mel feature extractor (~200 lines, vendored from
`transformers` to avoid the dep) to Rust.

Two caveats parlor states honestly and we should not gloss: their `turnbench.py`
runs on smart-turn's *own* test split, so it is in-domain and optimistic, and
LiveKit's `eot-bench` scores smart-turn v3.2 considerably harder. And the model
is English-centric — German and Hindi coverage needs its own evaluation before we
would ship it as the default.

`benchmarks/turnbench.py` also settles a question we would otherwise be tempted
to try: asking Gemma itself to judge turn completeness from audio scores **at
chance**. Don't prompt for this; use the classifier.

### 3. Grammar-forced structured output

Parlor's action head asks llama.cpp for
`response_format: {"type": "json_schema", "json_schema": {...}}`. llama.cpp
compiles the schema to a grammar, so the output is *structurally guaranteed to
parse*.

The Primer's three structured-output crates — `primer-classifier`,
`primer-extractor`, `primer-comprehension` — instead coax JSON out of free text
and recover with `extract_first_json_object` plus a soft-fail to
`ConceptExtraction::empty()`. Every soft-fail is a silently dropped signal, and
on the small local models Phase 1 targets the malformed-output rate is not small.

This is a cheap, contained upgrade with no architectural risk:

- `LlamaCppBackend` runs llama.cpp in-process, and `llama-cpp-2` exposes
  `LlamaGrammar`.
- `OpenAiCompatBackend` can pass `response_format` straight through.
- `OllamaBackend` has the `format` field.
- `CloudBackend` has tool-use / structured outputs.
- QNN's Genie surface would need checking; it can keep the current path.

It would want a new optional field on `GenerationParams` (a JSON schema), ignored
by backends that can't honour it — the same shape as the existing
`routing: Option<RoutingSignals>` field.

**Counter-evidence, from parlor's own measurements:** they tried grammar-forcing
the *main* reply into `{transcript, response}` and it was worse — *"format breaks
1-3/3 on degraded audio and 3/3 on chunked."* Grammar helps a short side-call at
temp 0; it hurts long-form streaming prose. Apply it to the three classifiers,
never to the Primer's spoken turn.

### 4. Speculative prefix-cache priming during speech

While the user is still talking, parlor's browser ships ~3 s speech chunks and
the camera frame to the server, and each one triggers a fire-and-discard
`max_tokens=1` request whose only purpose is to push that prefix through
`llama-server`'s cache. By the time the utterance ends, only the tail needs
prefill. `user_content()` carries an explicit comment that part order is
*"canonical (cache-stable)"* — the trick collapses the moment the prefix varies.

The Primer's prompt is *larger* than parlor's: system prompt, pedagogical intent
section, engagement state, rolling summary, retrieved older turns, due-vocabulary
section, and retrieved KB passages. Priming it at `SpeechStart` rather than
`SpeechEnd` would hide most of that prefill under the child's own speech.

The blocker is on our side and worth naming: per CLAUDE.md, `LlamaCppBackend`
creates a **fresh `LlamaContext` per `infer`** (its `'a` lifetime borrows the
model, so it cannot be stored beside it; a `Mutex<()>` serialises callers). There
is no KV reuse across turns at all today, so there is no cache to prime. Making
priming possible means revisiting that — which would *also* pay off for #3, since
parlor's action-head docstring is emphatic that the head must share the speech
request's cache: *"a separate model would pay full prefill of history + audio
every turn — the shared prefix cache is what makes deciding cheap."* Our
subsystem backends already `Arc::clone` the main backend by default, so the
sharing is there in principle and worth nothing in practice.

Sequencing note: this is a real architecture change and should follow #1, which
is cheaper and larger in effect.

### 5. Robustness findings we can adopt directly

Small, concrete, each one earned by a production failure:

- **Pad silence inside the WAV.** `TAIL_SILENCE_S = 0.3` — audio that stops
  abruptly at the VAD cutoff makes the encoder hallucinate a confident
  completion of the last word. It must be in the *same* WAV; a separate silence
  segment does not fix it. Directly applicable to our Whisper path.
- **Guard against hallucinated transcripts.** Parlor's `NO_SPEECH_RE` rejects a
  transcript that is entirely a bracketed annotation, and a rejected turn is
  never stored — *"one stored echo loop came back as invented or copied user
  words on every turn after it."* Our voice loop's only filter is
  `transcript_so_far.is_empty()`, which Whisper's silence hallucinations pass.
  Filed as **#326**; the learner-model corruption path there is the serious part,
  because depth promotion is monotonic-max by design and cannot self-correct.
- **Never store a degenerate turn.** Parlor's `remember()` drops turns the model
  produced nothing for. Ours keeps the child turn and drops the Primer turn on a
  mid-stream error, which is the right call for a partial reply — but there is no
  equivalent guard for a turn whose *input* was junk.
- **Cancellation must actually cancel.** `ChatStream.cancel()` shuts the socket
  down so llama-server observes it and stops generating. Our in-process backend
  drops a future instead; worth confirming that a dropped `generate_stream`
  future actually halts llama.cpp decode rather than letting it run to
  completion in the background.

### 6. Barge-in — worth reconsidering our position

The Primer currently **cannot be interrupted while speaking**: `is_speaking`
gates the audio thread, and mic samples are drained and discarded throughout
SPEAK. CLAUDE.md frames this as an invariant ("never lets the child speak over
the Primer"), and as echo protection it is correct.

Parlor keeps the mic live and filters instead:

- raise Silero's `positiveSpeechThreshold` from 0.5 to **0.92** during TTS rather
  than muting;
- require **6 of the last 10** frames above p=0.85 before counting it a barge-in
  (a single loud frame is echo);
- an 800 ms grace window after TTS starts;
- a phantom-capture watchdog that resets if barge-in fired but real speech never
  followed within ~1 s.

Pedagogically this is not a small thing. A child saying *"wait — no, I get it!"*
mid-explanation is one of the most valuable signals in the whole session, and we
currently discard it by construction. The mechanism to act on it already exists —
`cancel_response_tx` is plumbed for the GUI Stop button — only the mic gate
stands in the way. Worth a deliberate decision rather than an inherited default.

### 7. Audio-native LLM — the strategic one

Parlor has **no STT**. Gemma 4's mmproj takes audio directly, and audio arrives
as `input_audio` parts in the chat message. That deletes Whisper from the
pipeline: one model resident instead of two, no transcription stage in the
latency budget, and — the part that matters for us — **prosody reaches the
model**.

`LlmEngagementClassifier` currently infers frustration from text, and
`update_learner_model`'s engagement heuristic is a documented crude word-count
placeholder. A child's hesitation, excitement, and frustration are *in the
audio* and are being thrown away at the STT boundary. That is a pedagogical
capability, not a latency optimisation.

It is also a large change: `InferenceBackend` takes text `Prompt`s, so audio
parts are a trait-surface change; QNN/Genie bundles are text-only; and `--speech`
works well today. Phase 2/3 strategic note, not a now-item — but it should be on
the roadmap explicitly, because it changes what the Primer can perceive.

Related finding, should we ever go there: parlor makes the model emit
`###TRANSCRIPT: <what I heard>` as the **first** line before responding, and
measured **WER 0.39 → 0.00** on a 33-word utterance versus transcribing after the
reply. Committing to what was heard *before* answering stops the transcript
becoming a paraphrase from memory. It also gives the UI something to show while
the response is still decoding.

---

## Process ideas worth stealing

**Architecture benchmarks as checked-in decision records.** Parlor's `benchmarks/`
directory holds runnable arguments for design decisions, each with the measured
result in its docstring:

- `archbench.py` — in-band control tags vs a decoupled JSON action head. Tags
  scored recall 0.955; the single miss was an *ack-without-action* — the model
  said it would do something and didn't. The decoupled head scored 1.0.
- `camerabench.py` — attach the camera frame every turn vs fetch it as a tool
  call. Verdict: keep attaching, with numbers (a frame costs 50 context tokens,
  not the ~300 they had estimated; the tool-call variant adds ~2.2 s to *every*
  turn; the speak-first variant hallucinates scenes it never saw).
- `timerprobe.py` — can a turn-based model announce into silence? No. Hence a
  server-owned clock.
- `turnbench.py` — smart-turn vs LLM-judged turn completion.

We have the same instinct applied to *parameters* — `retrieval_sweep*.rs`,
`qnn_bench`, `llamacpp_bench` — but not to *architecture*. The ack-without-action
finding in particular has a direct Primer analogue: the Primer promising a break,
or promising to remember something, and the machinery never firing.

**Before/after latency diffing as routine.** `bench.py --label before` →
`compare.py before.json after.json`, run around every change. Cheap discipline.

**Degraded-audio end-to-end tests in CI.** Parlor's suite spawns the real server
and drives it over WebSocket with *locally synthesised* speech, including clipped
word endings, noise, and competing voices. Ours (`whisper_stream_reuse.rs`) is
`#[ignore]`'d and owner-gated on real WAVs the developer must supply. Synthesising
fixtures locally is what makes theirs runnable by anyone, including CI.

## What not to take

- **The browser/WebSocket architecture.** Tauri is the right shape for us, and
  `localhost`-only secure-context gymnastics are a browser problem we don't have.
- **Server-owned timers and background research delegation.** Off-mission, and
  the reasoner breaks `[[project_strict_offline_first]]` outright.
- **Kokoro.** 82M and English-first (`af_heart`); no German or Hindi. Piper +
  Supertonic + AVSpeech already cover more ground.
- **`MAX_OUTPUT_TOKENS = 256` and "1-4 short sentences".** Our turns are short for
  pedagogical reasons, not budgetary ones; don't import the constraint as if it
  were the same thing.
- **The engagement framing generally.** Parlor optimises for a satisfying
  assistant. Several of its best mechanisms serve that goal, and adopting the
  mechanism without re-deriving it from our principles is how a Socratic tutor
  quietly becomes a chatbot.

## Suggested sequencing

1. **#325** — sentence-level TTS pipelining. Largest win, infrastructure exists.
2. **#326** — transcript hallucination guard, plus `TAIL_SILENCE_S` padding.
   Small, and it is corrupting the learner model today.
3. **Grammar-forced JSON** for the three classifier crates. Contained, and it
   directly improves signal quality on Phase 1 local models.
4. **smart-turn-v3** evaluation — including a German/Hindi assessment before it
   could become a default.
5. **Barge-in policy** — a decision to make deliberately, not a default to
   inherit.
6. **KV-cache reuse in `LlamaCppBackend`**, unlocking speculative priming and
   cheap shared-prefix classifier calls.
7. **Audio-native inference** — roadmap item, Phase 2/3, justified by perception
   rather than latency.
