# Primer — TTS post-step-3 brief

**Audience:** future Claude Code session implementing the unified speech REPL (step 4 of Phase 2).
**Last updated:** 2026-05-02

PR <PR-NUMBER-TBD> added `StreamingTextToSpeech`, a Piper backend, the `Named` super-trait, and `PhraseSplitter`. This brief carries forward the small handful of facts step 4 will care about.

## Carry-forward facts

- **`piper-rs` is vendored at `src/vendor/piper-rs/`** and patched for `ort 2.0.0-rc.10`. The vendor is referenced via `[patch.crates-io] piper-rs = { path = "vendor/piper-rs" }` in `src/Cargo.toml`. If you bump `silero-vad-rust` or `ort`, re-test the patch.
- **The actual upstream API is `VitsModel::new(config_path, onnx_path)` + `PiperSpeechSynthesizer::new(Arc<dyn PiperModel>).synthesize_lazy(text, None)`** — NOT the simpler `Piper::new` + `piper.create` shape that earlier docs assumed. `PiperTts::new(onnx_path, config_path)` hides this; downstream code goes through the trait.
- **piper-rs's synthesis call is one-shot and synchronous; there is no native phrase-boundary callback.** `PhraseSplitter` chunks on `. ! ?` to fake streaming. If step 4 ever wants finer-grained streaming (per-syllable, etc.) it must wait for upstream piper-rs to expose hooks or move to a different synthesiser.
- **`PiperTts` is one voice per instance.** Construct it at startup with the chosen voice ONNX + JSON pair. `open_session(voice)` errors on `model_id` mismatch; if step 4 wants runtime voice switching it has to either build a `PiperTtsRouter` (HashMap of model_id → backend) or wait for a future multi-voice impl.
- **The Piper backend's `sample_rate()` is read from the voice config at construction.** Don't hardcode 22 050 — different voices use different rates.
- **`cargo run --example tts_hello --features piper -- --onnx … --config … --out …`** is the manual smoke. Reuse it as a copy-paste path-validity check before driving real audio out via cpal.
- **ONNX Runtime first-build downloads from `cdn.pyke.io`.** Sandboxed CI environments will fail. Document, don't fight.
- **`primer-cli` does NOT yet have a `--voice` flag**; step 4 owns that.
- **The silero VAD feature has a pre-existing breakage** (`unsigned_is_multiple_of` unstable API in `silero-vad-rust 6.2.1` — unrelated to TTS work and not introduced by this PR). If step 4 wires VAD, expect to chase a different silero pin or upstream fix.

## What step 4 needs to add

Out of scope for this brief (read the spec for step 4 itself), but at a high level:
- cpal capture → SileroVad (after the silero pin issue is resolved) → WhisperStt streaming session → DialogueManager → PiperTts streaming session → cpal playback.
- A new `--speech` flag on `primer-cli` that routes through the speech path instead of the text REPL.
- A way to pick the voice (`--voice` plus a sensible default candidate from `en_US-amy-medium`, `en_GB-jenny_dioco-medium`, `en_GB-alba-medium`).

Delete this brief once step 4 is in.
