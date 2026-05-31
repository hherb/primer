# Supertonic 3 — issue #170 Stage A.5 spike result

**Date:** 2026-05-31
**Status:** PASS on metrics; quality listen-test is a human gate (see below)
**Gates:** Supertonic Stage C (voice-loop wiring + CLI/GUI backend selector). With this
spike passing, Stage C is unblocked.
**Host:** macOS (Apple Silicon), CPU-only inference (`use_gpu = false`).

## Why this spike exists

`SupertonicTts` (the `TextToSpeech` + `StreamingTextToSpeech` impl, Stage B, merged in
#175) wraps the vendored **v2 fork** at `src/vendor/supertonic-rs/`. The open question
that gated Stage C was: **does that v2 fork load Supertonic _3_ (v3) assets unchanged**,
and are the per-language latency numbers good enough to replace the macOS-native TTS
(which is slow — Enhanced neural voices, >5 s perceived latency) while covering the
languages Piper lacks (notably **Hindi**, part of the EN/DE/JA/HI cohort)?

## Method

```bash
# Assets (~396 MB, git-lfs via the hf CLI)
hf download Supertone/supertonic-3 --local-dir /tmp/supertonic-3

# Build the smoke binary (first build also compiles vendored supertonic-rs + ort rc.10)
cd src && ~/.cargo/bin/cargo build --example tts_supertonic_hello --features supertonic

# Per-language synthesis (voice style F1, one of 10: F1–F5, M1–M5)
target/debug/examples/tts_supertonic_hello \
  --onnx-dir /tmp/supertonic-3/onnx \
  --voice-style /tmp/supertonic-3/voice_styles/F1.json \
  --lang <en|de|hi|ja> --text "<phrase>" --out /tmp/supertonic-<lang>.wav
```

## Findings

### 1. The v2 fork loads v3 assets unchanged — the load-bearing assumption holds ✅

The v3 `onnx/` directory has exactly the layout `tts_supertonic_hello` /
`supertonic_tts::helper::load_text_to_speech` expects, with no code change:

```
onnx/duration_predictor.onnx   3.5M
onnx/text_encoder.onnx          35M
onnx/vector_estimator.onnx     245M
onnx/vocoder.onnx               97M
onnx/tts.json                   (config; sample_rate = 44100, "opensource-multilingual" v1.7.3)
onnx/unicode_indexer.json
voice_styles/{F1..F5,M1..M5}.json   (10 voices)
```

All four ONNX sessions load; synthesis runs end-to-end for every language tried.

### 2. Latency is Piper-class on CPU ✅

One-time model load ≈ **290–305 ms**. Per-utterance:

| Lang | Synth time | Audio length | **RTF** | Speed vs realtime |
|------|-----------|--------------|---------|-------------------|
| en   | 729 ms    | 4.32 s       | **0.169** | 5.9× |
| de   | 871 ms    | 4.78 s       | **0.182** | 5.5× |
| hi ⭐ | 756 ms    | 4.14 s       | **0.183** | 5.5× |
| ja   | 527 ms    | 2.27 s       | **0.232** | 4.3× |

RTF ≈ 0.17–0.23 means a 4 s phrase synthesises in well under a second. With the
per-phrase streaming the `SupertonicTts` backend already does (the `PhraseSplitter`
glue mirroring Piper), the first (short) phrase's time-to-first-audio is a few hundred
ms — the opposite of the macOS-native >5 s problem that motivated this whole thread.

### 3. Language coverage ✅

The model exposes **32 languages**:
`en, ko, ja, ar, bg, cs, da, de, el, es, et, fi, fr, hi, hr, hu, id, it, lt, lv, nl, pl,
pt, ro, ru, sk, sl, sv, tr, uk, vi, na`. This covers the full EN/DE/JA/HI test cohort —
**Hindi and Japanese are the unlock Piper cannot provide**, and the Hindi run succeeded
cleanly.

### 4. Quality — human gate (not auto-judgeable)

WAVs were written to `/tmp/supertonic-{en,de,hi,ja}.wav` (44.1 kHz, 16-bit mono) for an
A/B listen against Piper. The repo also ships per-voice reference samples at
`/tmp/supertonic-3/audio_samples/*_supertonic3.wav`. Quality sign-off is Horst's call;
the metrics above are the objective half.

## Conclusion

The A.5 spike **passes on every objective axis**: v3 assets load on the v2 fork
unchanged, latency is Piper-class on CPU, and the language set covers the cohort
including Hindi. Pending Horst's quality sign-off on the WAVs, **Supertonic 3 dominates
the TTS choice** — Piper-class speed *and* the missing languages — and Stage C is
unblocked.

## Next session — Stage C scope (not yet done)

1. Wire `SupertonicTts` into the voice loop (a `build_local_backends_supertonic`-style
   builder, mirroring the Piper path; 44.1 kHz → device-rate output resampler already
   handled by the existing `SpeakerPipeline`).
2. Add a `supertonic` arm to the `SpeechBackend` selector (CLI flag + GUI Settings
   dropdown), feature-gated like the others.
3. Asset auto-download + consent (Stage D), mirroring the voice-asset download flow.
4. Capture real in-loop TTFA/RTF and the Hindi preview→stable promotion (Stages E/F).

## Reproduce

The spike is fully reproducible from the commands in **Method** above. Assets live at
`/tmp/supertonic-3` (re-`hf download` if cleaned). The smoke binary prints model-load
time and RTF per run.
