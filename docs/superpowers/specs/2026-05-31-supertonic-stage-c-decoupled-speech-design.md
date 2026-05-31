# Supertonic 3 Stage C — decoupled STT/TTS voice-loop wiring + CLI/GUI selectors

**Date:** 2026-05-31
**Issue:** #170 (Stage C)
**Status:** design approved; implementation plan next
**Gates:** Stage D (Supertonic asset auto-download + consent), Stage E (in-loop A/B
numbers), Stage F (Hindi `preview → stable` promotion).

## Background

Stages A (smoke example), B (`SupertonicTts` full `TextToSpeech` +
`StreamingTextToSpeech` impl, PR #175), and A.5 (v3-on-v2-fork spike, PR #190) are
done. The spike passed on every objective axis: v3 assets load unchanged on the
vendored v2 fork, CPU RTF is Piper-class (~0.17–0.23), and the model covers 32
languages including the **Hindi** and Japanese that Piper/AVSpeech lack.

Stage C makes the TTS backend a **runtime choice** in the voice loop and exposes it
through the CLI and GUI. Today the TTS is hard-wired per voice-loop builder:

- `voice_loop/backends.rs` → Whisper STT + **Piper** TTS
- `voice_loop/backends_macos_native.rs` → SFSpeechRecognizer STT + **AVSpeech** TTS
- `voice_loop/backends_macos_native_26.rs` → SpeechAnalyzer STT + **AVSpeech** TTS

All three share the identical injection point:
`let tts: Arc<dyn StreamingTextToSpeech> = Arc::new(SomeTts::new(...)); tts.sample_rate()`.

## Goals

1. STT and TTS are **orthogonal** runtime choices, not a coupled "stack".
2. Deliver **Whisper STT + Supertonic TTS** end-to-end — the Hindi unlock — on the
   portable (Linux/Android/desktop default) build.
3. GUI Settings exposes two independent dropdowns (STT, TTS), each disabling options
   not compiled into the running binary.
4. No regression to the existing default Whisper+Piper path or the macOS-native paths.

## Non-goals (explicitly deferred)

- **Asset auto-download / consent for Supertonic** — Stage D. In Stage C the user
  supplies Supertonic asset paths explicitly (CLI flags / GUI per-locale fields).
- **In-loop A/B latency numbers, Hindi promotion** — Stages E/F.
- **Full runtime STT/TTS decoupling in the CLI** — see decision D2 below; the CLI
  gets `--tts piper|supertonic` on the portable build only.

## Decisions (signed off)

- **D1 — Inject TTS into the builders (not a 4th sibling file).** Each builder takes a
  pre-built `Arc<dyn StreamingTextToSpeech>` + `VoiceProfile`. Reuses the existing
  `backends_common` plumbing; no duplication. (Issue #170's recommendation; overrides
  the spike note's "build_local_backends_supertonic sibling" suggestion.)
- **D2 — CLI macos-native build stays AVSpeech (compile-time), unchanged.** Hindi needs
  Whisper STT (handles `hi`) + Supertonic TTS — the portable build. macOS-native STT/TTS
  have no `hi-IN`, so a CLI `--tts` override there buys nothing. The CLI gets
  `--tts piper|supertonic` on the portable build; full runtime decoupling lives in the
  GUI. YAGNI for the CLI.
- **D3 — Legacy `backend` config field is migrated, not dropped.** PR #189's
  `SpeechSettings.backend: SpeechBackend` shipped this week. The new fields default via
  `#[serde(default)]`; additionally a legacy optional `backend` is deserialized and
  mapped so a #189 user's choice survives the upgrade.

## Architecture

### Two orthogonal selectors

```
SttBackend  { Whisper, MacosNative }              // which builder skeleton runs
TtsBackend  { Piper, Supertonic, MacosNative }    // which Arc<dyn StreamingTextToSpeech> is injected
```

STT selects the builder (audio-thread / VAD / transcription shape); TTS selects the
synthesiser injected into it. They vary independently. Feature gates constrain which
values are *reachable* in a given binary (see "Feature gating").

### `primer-speech` — uniform TTS injection

Each builder loses its internal TTS construction and gains two parameters:

| Builder | Removed params | Added params |
| --- | --- | --- |
| `build_local_backends` (Whisper) | `piper_onnx`, `piper_config`, `voice_id` | `tts: Arc<dyn StreamingTextToSpeech>`, `voice: VoiceProfile` |
| `build_local_backends_macos_native` | — | `tts`, `voice` |
| `build_local_backends_macos_native_26` | — | `tts`, `voice` |

`build_local_backends` loses its `feature = "piper"` gate (it becomes purely STT-side:
`silero` + `whisper` + `cpal`). The `piper` / `supertonic` / `macos-native` features now
gate only **TTS construction** at the call site.

Two shared helpers centralise construction so CLI and GUI share one path:

- `build_tts(choice: TtsBackend, assets: &TtsAssets) -> Result<(Arc<dyn StreamingTextToSpeech>, VoiceProfile)>`
  — feature-gated per arm. An uncompiled choice returns a
  `PrimerError::Speech("…rebuild with --features <X>…")` error, deliberately distinct
  from a generic failure (mirrors the qnn "rebuild with --features qnn" pattern).
  `TtsAssets` is a neutral input struct carrying the optional path sets each backend
  needs (piper onnx+config; supertonic onnx-dir+voice-style; macos-native: none, locale
  supplies the voice).
- `build_voice_backends(stt: SttBackend, tts: Arc<…>, voice, whisper_model, locale, mic_silence_ms, verbose)`
  — matches `stt` and calls the right builder (cfg-gated arms). Centralises the dispatch
  the CLI/GUI shims do today.

`SttBackend` / `TtsBackend` live in `primer-speech` (the layer that owns the builders)
and are re-used by the GUI config rather than redefined, so there is one source of truth.

### Voice / `VoiceProfile` ownership

`voice_id` moves out of the Whisper builder signature: the caller that constructs the
TTS also constructs the matching `VoiceProfile` (Piper → `.onnx` stem; Supertonic →
`supertonic-<style-stem>`; macOS-native → ignored). `build_tts` returns both so the pair
is always consistent — a Supertonic `Arc` never travels with a Piper `model_id`.

## CLI surface

Portable (non-macos-native) build only:

- `--tts piper|supertonic` (default `piper`).
- `--supertonic-dir <DIR>` — the Supertonic `onnx/` asset directory.
- `--supertonic-voice-style <FILE>` — e.g. `voice_styles/F1.json`.
- clap `requires`: `--supertonic-dir` and `--supertonic-voice-style` are required when
  `--tts supertonic`; the existing `--voice-onnx`/`--voice-config` stay required for
  `--tts piper` (the default). `--whisper-model` is always required (STT is Whisper).

`SpeechLoopConfig` (portable cfg) gains `tts: TtsBackend` + the two optional Supertonic
paths; the dispatch in `speech_loop/mod.rs` calls `build_tts` then the Whisper builder.

macos-native / macos-native-26 CLI builds: unchanged (AVSpeech, compile-time).

## GUI surface

- `SpeechSettings`: replace `backend: SpeechBackend` with
  `stt_backend: SttBackend` + `tts_backend: TtsBackend` (both `#[serde(default)]`).
  Keep an optional legacy `backend` for the D3 migration mapping (applied in `Default`/
  `into_config` resolution, then the legacy field is not re-serialised).
- `SpeechLocaleOverride`: add `supertonic_onnx_dir: Option<PathBuf>` +
  `supertonic_voice_style_path: Option<PathBuf>` (per-locale, mirroring `piper_onnx_path`).
- Settings → Speech: two `<select>` dropdowns (STT, TTS). Each option that isn't
  compiled in is rendered disabled with the #189 "requires building with --features …"
  hint. New capability commands as needed (mirror `macos_native_speech_available`):
  e.g. `supertonic_tts_available`.
- `voice/assets.rs` `ResolvedAssets`: extend to carry the resolved Supertonic paths;
  `voice/backends.rs::build_loop_backends` calls `build_tts(tts_backend, …)` then
  `build_voice_backends(stt_backend, …)` instead of the hard-coded Piper path.
- The webview IPC DTOs (`*View` / `*Update`) thread the two new enum fields and the two
  new per-locale path fields through `gather()`/`populate()` exactly like the existing
  speech fields. Mandatory-field discipline: any non-`serde(default)` Update field must
  be sent by `gather()` even when its dropdown is hidden (the #188/#189 lesson).

## Feature gating

Reachable matrix depends on compiled features:

| Build | STT options | TTS options |
| --- | --- | --- |
| default (Linux/Win/macOS no native) | Whisper | Piper, Supertonic* |
| + `macos-native` (macOS) | Whisper, MacosNative | Piper, Supertonic*, MacosNative |
| + `macos-native-26` (macOS 26) | Whisper, MacosNative | Piper, Supertonic*, MacosNative |

\* Supertonic requires the `supertonic` cargo feature on `primer-speech` (propagated via
`primer-cli/supertonic` / `primer-gui/supertonic`). On a build without it, the GUI shows
the option disabled and `build_tts(Supertonic, …)` errors with the rebuild hint.

A new `supertonic` feature is added to `primer-cli` and `primer-gui` (forwarding to
`primer-speech/supertonic`), parallel to how `speech` / `macos-native` are forwarded.

## Testing (TDD)

Pure / unit (no assets, always built):

- `SttBackend` / `TtsBackend` serde round-trips (kebab-case).
- D3 legacy-`backend` → `(stt, tts)` migration mapping.
- `build_tts` feature-gate error text per uncompiled arm (assert the rebuild hint names
  the right feature).
- `VoiceProfile` derivation per TTS choice (model_id matches the synthesiser).
- CLI `--tts` parsing + clap `requires` (supertonic flags required iff `--tts supertonic`).
- GUI config: new fields round-trip through `GuiConfigView` / `into_config`; the
  per-locale Supertonic paths survive a save.

Behaviour-preserving: the entire existing Whisper+Piper voice-loop test suite stays
green after the injection refactor (the default path is exercised by constructing
`PiperTts` in the caller and passing it in).

Asset-gated (`#[ignore]`): the existing Supertonic synth smoke
(`SUPERTONIC_TEST_ONNX_DIR` / `SUPERTONIC_TEST_VOICE_STYLE`) stays; add a builder-level
`#[ignore]` test under `--features supertonic` that runs `build_tts(Supertonic, …)` to a
non-empty session.

Verification matrix for this session:

- Default build: `fmt`, `clippy -D warnings`, full `test --workspace` — always.
- `--features supertonic` compile + ignored smoke — run unsandboxed (cdn.pyke.io ort
  download + ~400 MB assets).
- `--features macos-native` compile-check on the macOS host if it builds.
- `--features macos-native-26`: compile-check only if macOS 26 + swiftc available;
  otherwise note as not-locally-verified.

## Risks

- **macOS-native-26 local verification** may be unavailable (needs macOS 26 + swiftc).
  Mitigation: the refactor is uniform across the three builders; the `_26` change is the
  same two-param injection as the others, reviewed by inspection if it can't be built.
- **Config migration edge case (D3):** a malformed legacy `backend` value should fall
  back to the defaults, not error the whole config load. Covered by a unit test.
- **Files over 500 lines:** `primer-gui/src/config.rs` is already ~1414 lines; the new
  fields add to it. Out-of-scope to split here, but flag if the additions push a
  function past readability — prefer extracting the migration mapping to a small free fn.
- **OpenRAIL-M weights** (Supertonic) — licence read before any *default-path flip* is a
  Stage D/F concern; Stage C only adds an opt-in selector, so no default changes.
