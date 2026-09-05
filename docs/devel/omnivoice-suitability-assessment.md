# OmniVoice (k2-fsa) — suitability assessment for the Primer's TTS path

**Date:** 2026-08-11
**Scope:** [`k2-fsa/OmniVoice`](https://github.com/k2-fsa/OmniVoice) evaluated as a candidate `TextToSpeech` /
`StreamingTextToSpeech` backend for the Primer, against the same bar the Supertonic evaluation used
([Stage A.5 spike](supertonic3-stage-a5-spike.md), [OpenRAIL-M licence read](supertonic-openrail-license-assessment.md)).
**Verdict:** ❌ **Not suitable today — do not adopt.** Three of the Primer's four hard TTS requirements are
structurally unmet (no non-Python runtime, no streaming, ~8× the on-disk footprint at far worse CPU RTF), and
the licence story is *worse* than Supertonic's, not better. **Keep Supertonic.** There is one clean re-evaluation
trigger (see [Watch items](#watch-items)) and one narrow off-runtime use it could serve.

> This is an engineering assessment for the maintainer, not legal advice. `huggingface.co` is blocked by this
> environment's egress proxy, so the HF model card and its `LICENSE` files were **not** read first-hand —
> licence statements below are sourced from the GitHub repo, PyPI metadata, and the public HF discussion
> threads as indexed. **Re-read the model card directly before acting on the licence section.**

## What OmniVoice is

Released 2026-03-31 by the Next-gen Kaldi / k2-fsa team (the same group behind `sherpa-onnx`). A **zero-shot
voice-cloning TTS** model — TTS only, no ASR — built on a diffusion-language-model architecture with a
Higgs-Audio-2-derived audio tokenizer. Headline claims: **600+ languages** (646 listed, 581k training hours),
RTF **0.025** (40× realtime), voice cloning from a short reference clip plus attribute-based "voice design"
(gender / age / pitch / accent). Apache-2.0 code, PyPI package `omnivoice` 0.2.1, ~3.27 GB checkpoint. It has
had real traction (≈3.8k GitHub stars and 460k+ HF downloads in its first three weeks).

The language coverage is genuinely the broadest of any open zero-shot TTS, and the quality on high-resource
languages is well regarded. Nothing below disputes that — the mismatch is with *our* deployment envelope.

## The bar: what the Primer actually needs from a TTS

Set by [`primer-speech`](../../src/crates/primer-speech/) and the voice loop, not by taste:

1. **In-process, callable from Rust, no Python at runtime.** Every shipped backend is either ONNX-via-`ort`
   (Piper, Supertonic) or an OS-native API (AVSpeechSynthesizer, Android `TextToSpeech`). The product ships no
   Python interpreter and no torch.
2. **Per-phrase streaming with sub-second time-to-first-audio.** `StreamingTextToSpeech` + `PhraseSplitter`
   exist precisely because the macOS-native path's >5 s perceived latency was unacceptable in the SPEAK state.
3. **Runs on the target hardware.** Android ARM64 (RedMagic 11 Pro, with the 4B model already occupying the
   NPU) and eventually RK3588-class boards — CPU-only, alongside the LLM, offline
   ([[project_strict_offline_first]]).
4. **A licence a children's product can ship under**, cleanly separable from the AGPL tree.

Supertonic 3 clears all four: ~396 MB of ONNX, CPU RTF 0.17–0.23 on Apple Silicon, 32 languages including
Hindi and Japanese, OpenRAIL-M weights fetched at runtime (conditionally cleared).

## Requirement-by-requirement

| Requirement | Supertonic 3 (shipping) | OmniVoice | |
|---|---|---|---|
| Rust-callable, no Python | ONNX via vendored `supertonic-rs` + `ort` | **PyTorch ≥2.4 + `transformers` ≥5.3 + `accelerate`**; no ONNX/GGML export, no C/C++/Rust bindings, no mobile | ❌ |
| Streaming / TTFA | Per-phrase, few-hundred-ms first audio | **No streaming.** Issue [#6](https://github.com/k2-fsa/OmniVoice/issues/6) closed with no plan; chunking is 15 s segments for *VRAM*, not latency | ❌ |
| On-device CPU footprint | ~396 MB total, 4 ONNX sessions | **3.27 GB checkpoint**; 32 iterative unmasking steps (16 "for faster") × classifier-free guidance | ❌ |
| CPU RTF | 0.17–0.23 measured | 0.025 is **H100** (0.0115 with FlashInfer @ batch 8). Maintainers: *"the current PyTorch version is rather slow on CPU"* | ❌ |
| Languages incl. hi/ja | 32, Hindi verified working | 646 — but **Hindi = 117 h** of training data vs English 206,061 h, German 21,927 h | ⚠️ |
| Weights licence | OpenRAIL-M, assessed and cleared | Apache-2.0 headline, **but the bundled tokenizer is Higgs-Audio-2-derived** → Boson Community License | ⚠️ |
| Maturity | v3 assets, stable | 5 months old; open issues include a VRAM leak, audio truncation, cross-generation pronunciation drift | ⚠️ |

## The three blockers, in detail

**1. Runtime shape.** This is the decisive one and it is not a tuning problem. Adopting OmniVoice means either
embedding a Python+torch runtime in a Tauri Android APK (a non-starter) or waiting for an ONNX/GGML export that
does not exist. The export request is [issue #151](https://github.com/k2-fsa/OmniVoice/issues/151) (opened
2026-05-08, no maintainer commitment); on the HF `ONNX / GGML inference` discussion the team said they are *not
sure whether an ONNX or GGML implementation can resolve* the CPU-speed problem, that they will look into it,
and that contributions are welcome. That is an open research question upstream, not a scheduled deliverable.

**2. Latency architecture.** A diffusion LM with iterative unmasking + CFG is structurally the opposite of what
the voice loop wants: it is a batch-oriented, whole-utterance, many-forward-passes decode. Supertonic's
single-pass ONNX pipeline is what makes per-phrase TTFA possible. Even a hypothetical ONNX export of OmniVoice
would still be 32 (or 16) sequential passes over a multi-GB graph per utterance on an ARM CPU — the RTF gap
versus 0.17 is orders of magnitude, not percentages.

**3. Footprint.** 3.27 GB of TTS weights next to a 4B QNN bundle on a phone is not a budget the Phase 1.2/2
device story has room for.

## Licence read

The headline is Apache-2.0 for both code and the k2-fsa weights, which would be *better* than Supertonic's
OpenRAIL-M. But HF discussion #1 ("Higgs Audio Tokenizer Licensing Issue") established that the bundled audio
tokenizer derives from Higgs Audio 2 and remains under the **Boson Higgs Audio 2 Community License** — the team
resolved the thread by adding Boson's `LICENSE` into the tokenizer directory rather than by replacing the
component. That licence reportedly carries: commercial use only **under 100k annual active users** (above which
a paid Boson licence is required), a **derivative-naming requirement** (names must begin "Higgs Audio 2"),
mandatory attribution to **Boson and Meta**, a prohibition on using outputs to improve other LLMs, and
termination on IP litigation.

For the Primer specifically:

- The <100k-AAU ceiling is a **growth-gated dependency** — exactly the kind of term the Supertonic assessment
  was written to avoid walking into unexamined. OpenRAIL-M's restrictions are *behavioural* (and the Primer's
  pedagogy already satisfies them); this one is *scale-based*, and it becomes a blocker precisely on success.
- The "no outputs to improve other LLMs" clause sits awkwardly beside the Phase 4 anonymisation/contribution
  path in the roadmap.
- A mixed-licence bundle presented under a single Apache-2.0 headline needs a component-by-component read
  before use, which is more diligence than the alternative requires for a strictly worse outcome.

**Net: the licence posture is worse than the incumbent's, not better.** That alone would not decide it — but
it removes the one axis on which OmniVoice might have beaten Supertonic.

## Two product-level notes

**Hindi is the wrong reason to want this.** Hindi is *why* Supertonic entered the stack (Piper has no Hindi).
OmniVoice's Hindi is 117 hours out of 581k — 0.02% of the corpus, roughly 1/1800th of English. Broad language
*coverage* is not the same as usable *quality* in the tail, and the tail is where our need is. Any adoption
argument resting on "600+ languages" has to survive an actual Hindi listen test first.

**Zero-shot voice cloning is a liability here, not a feature.** The Supertonic licence read explicitly noted
that its fixed catalogue voices (F1–F5 / M1–M5) mean *no impersonation/consent exposure* under the
deepfake clause. OmniVoice's headline capability is cloning an arbitrary voice from a short reference clip — on
a device a child operates. That is a surface we deliberately do not have today, and adding it would want its
own safety review regardless of the engineering merits.

## Watch items

The verdict flips if **k2-fsa ships OmniVoice through `sherpa-onnx`**. They own that project; it already has a
C API (Rust-callable), Android/iOS support, and ONNX-Runtime-based on-device inference — it is the natural
destination, and its *absence* today is itself informative. If an official `sherpa-onnx` OmniVoice recipe
appears with quantised weights, re-run the Stage-A.5-style spike: CPU RTF on ARM, footprint, per-phrase TTFA,
Hindi quality, and a fresh tokenizer-licence read. Secondary signals: movement on
[#151](https://github.com/k2-fsa/OmniVoice/issues/151), or a credible community INT8 ONNX port.

**Separately and more promising:** `sherpa-onnx` itself is worth a look on the **STT** side (SenseVoice /
Zipformer / Whisper, Rust-callable, Android+iOS, offline) as an alternative to our `whisper-rs` path. That is a
different question from the one asked here and is not a recommendation to act — just noting that the useful
k2-fsa avenue for the Primer probably runs through `sherpa-onnx`, not through OmniVoice.

## Narrow use that would be defensible

**Offline studio asset generation** — running OmniVoice on a workstation to *produce* fixed WAV assets (an
onboarding line, a language-sample demo, a voice for a locale Supertonic's 32 do not cover) that ship as audio
files rather than as a runtime dependency. This sidesteps every engineering blocker above. It does **not**
sidestep the tokenizer licence: generated-output terms would need the same component-level read first.

## Conclusion

**Do not adopt OmniVoice as a Primer TTS backend.** It is a good model aimed at a different deployment
envelope — GPU-served, batch, whole-utterance, breadth-over-depth — where the Primer needs on-device, streaming,
small, and deep on four languages. Supertonic 3 remains the right choice, and the open Supertonic work
(Stage E in-loop A/B numbers, Stage F Hindi promotion) is a better use of the same effort. Revisit only on the
`sherpa-onnx` trigger.

## Sources

- Repo + README: <https://github.com/k2-fsa/OmniVoice> · `pyproject.toml` (deps, Apache-2.0, v0.2.1)
- Language table: <https://github.com/k2-fsa/OmniVoice/blob/master/docs/languages.md> (646 languages / 581k h;
  en 206,061.1 h, de 21,927.13 h, hi 117.17 h)
- Generation params: <https://github.com/k2-fsa/OmniVoice/blob/master/docs/generation-parameters.md>
  (`num_step=32`, `guidance_scale=2.0`, 15 s/30 s chunking for VRAM)
- Streaming: <https://github.com/k2-fsa/OmniVoice/issues/6> (closed, no plan) ·
  ONNX/Android: <https://github.com/k2-fsa/OmniVoice/issues/151> (open)
- CPU speed + ONNX/GGML maintainer position: HF discussion `k2-fsa/OmniVoice` #2 (via search index; HF blocked
  from this environment)
- Tokenizer licence: HF discussion `k2-fsa/OmniVoice` #1 · Boson Higgs Audio 2 Community License,
  <https://github.com/boson-ai/higgs-audio> (terms via search index — **verify against the `LICENSE` file
  directly before relying on them**)
- Primer baseline: [`supertonic3-stage-a5-spike.md`](supertonic3-stage-a5-spike.md) ·
  [`supertonic-openrail-license-assessment.md`](supertonic-openrail-license-assessment.md)
