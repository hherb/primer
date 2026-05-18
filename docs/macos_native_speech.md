# macOS-Native Speech Backend (Evaluation Builds)

The Primer ships a macOS-only speech backend that uses Apple's
SFSpeechRecognizer + AVSpeechSynthesizer instead of the cross-platform
Whisper + Piper stack. Recommended for macOS evaluators.

## What you get

- Zero external dependencies. No `brew install espeak-ng`. No first-run
  model downloads (saves ~570 MB).
- Native Apple voices: Samantha (en-US) and Anna (de-DE) at minimum.
- Strictly on-device. Audio never leaves your computer.

## Building

```bash
cd src
~/.cargo/bin/cargo tauri build --features "primer-gui/speech primer-gui/macos-native"
```

The resulting `.dmg` is in `target/release/bundle/dmg/`.

## First run

On first entry to Voice mode, macOS will ask for two permissions:

1. **Microphone** — required to hear you.
2. **Speech recognition** — required to turn your voice into text on-device.

Both must be granted. If you accidentally deny either, re-enable under
**System Settings → Privacy & Security → Microphone / Speech Recognition**.

### Installing the German on-device STT model

The German on-device speech recognition model is not pre-installed on
all macOS systems. If voice mode fails to start with locale `de` and
the logs show "on-device recognition unavailable for `de-DE`", install
the model:

**System Settings → Keyboard → Dictation → Languages → +** → choose
German (Germany). The download completes in a few minutes.

The English (`en-US`) model is pre-installed on all macOS 13+ systems.

## Optional: install an Enhanced voice for better TTS

The default Samantha / Anna voices are functional but have an obvious
robotic edge. For substantially better quality:

1. **System Settings → Accessibility → Spoken Content → System Voice → Manage Voices**.
2. Find your language, click the download arrow next to the Enhanced
   voice (Samantha Enhanced, Anna Enhanced).
3. Restart the Primer. It will auto-detect and use the Enhanced voice.

## A/B comparison with Whisper + Piper

`gui-config.json` carries a `speech.backend` field:

```json
{
  "speech": {
    "backend": "macos-native"
  }
}
```

Valid values: `"macos-native"` or `"whisper-piper"`. Default is
`whisper-piper`. Switch and restart to compare.

## Supported locales

| Locale | Voice | On-device STT |
|--------|-------|---------------|
| English (en-US) | Samantha / Ava | yes (pre-installed) |
| German (de-DE) | Anna | yes (manual install) |

Other locales fall through to the Whisper + Piper path even when
`macos-native` is configured.

## Known limitations

- **Hindi is unsupported on the macos-native backend.** Apple's
  SFSpeechRecognizer does not ship an on-device `hi-IN` model on
  macOS 13–15. The newer SpeechAnalyzer API on macOS 26+ adds Hindi
  but is not yet exposed by the Rust bindings we depend on. Hindi
  evaluators should use the cross-platform Whisper + Piper build
  for now.

- **Per-phrase TTS startup ~380–640 ms.** AVSpeechSynthesizer carries
  a fixed per-call startup cost (Samantha ~380 ms, Anna ~640 ms).
  The voice-loop pre-warms with a silent utterance at session open
  to absorb the first hit; subsequent phrases still pay this cost.
  This is acceptable for a Socratic learning tool where pauses are
  natural but worth knowing during evaluation.

- **Finalize blocks the audio capture thread up to 2 s.** When voice
  mode finalizes a child's utterance, the audio thread waits up to
  2 s for SFSpeechRecognizer's final transcript before resuming
  microphone processing. Fast back-and-forth speech may experience
  a noticeable gap. A follow-up PR will move finalize to a helper
  thread.
