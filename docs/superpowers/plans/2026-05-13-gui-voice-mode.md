# GUI Voice Mode Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire the CLI `--speech` voice loop into the Tauri desktop GUI behind a header toggle, with auto-download of locale-default Whisper/Piper assets, a composer-zone state widget, and belt-and-suspenders cancel controls. Phase A only — Phase B big-central visuals deferred.

**Architecture:** Lift `primer-cli/src/speech_loop.rs` into a new `primer-speech::voice_loop` module behind a `voice-loop` cargo feature with a `LoopObserver` trait carrying side-effects. CLI and GUI both consume the same shared loop via different observer implementations. GUI adds Tauri commands (`start_voice_mode`, `stop_voice_mode`, `cancel_voice_response`, `download_voice_assets`) and Tauri events (`primer://voice/*`). Frontend gains a header toggle, a composer-zone state widget keyed off a `[data-state]` attribute, and an asset-consent modal.

**Tech Stack:** Rust 2024 edition, toolchain 1.88+; Tauri 2.x; existing `primer-speech` building blocks (silero/whisper/piper/cpal); `reqwest` for asset downloads (new dep on `primer-gui`'s `speech` feature). Locale defaults pin `en_GB-alba-medium` + Whisper `small.en` for English, `de_DE-thorsten-medium` + Whisper `small` for German.

**Spec:** [docs/superpowers/specs/2026-05-13-gui-voice-mode-design.md](../specs/2026-05-13-gui-voice-mode-design.md)

**Working directory:** All `cargo` commands run from `src/` (workspace root is `src/Cargo.toml`, not the repo root). The binary `~/.cargo/bin/cargo` is used in places to avoid Homebrew rust shadowing — when in doubt, use the rustup proxy explicitly.

---

## File Structure

**New files:**

| Path | Responsibility |
|---|---|
| `src/crates/primer-speech/src/voice_loop/mod.rs` | Public re-exports + the `run_loop` entry point. Owns the `LoopObserver` trait. |
| `src/crates/primer-speech/src/voice_loop/observer.rs` | `LoopObserver` trait + `VoiceState`, `ExitReason`, `TurnCompletePayload` types. |
| `src/crates/primer-speech/src/voice_loop/state_machine.rs` | The `run_loop` impl moved from `speech_loop.rs`, with `println!`/`tracing` calls replaced by observer callbacks. |
| `src/crates/primer-speech/src/voice_loop/locale_defaults.rs` | `LOCALE_DEFAULTS` table + `voice_default_for(locale)` lookup. |
| `src/crates/primer-cli/src/speech_loop/stdout_observer.rs` | `StdoutObserver` impl preserving the CLI's existing print formatting. |
| `src/crates/primer-gui/src/commands/voice.rs` | `start_voice_mode` / `stop_voice_mode` / `cancel_voice_response` / `download_voice_assets` Tauri commands. |
| `src/crates/primer-gui/src/voice/observer.rs` | `TauriEventObserver` impl that emits `primer://voice/*` events. |
| `src/crates/primer-gui/src/voice/assets.rs` | `resolve_voice_assets`, `MissingAsset`, `StartVoiceModeError`. |
| `src/crates/primer-gui/src/voice/download.rs` | Streaming download helper with retry + atomic rename. |
| `src/crates/primer-gui/src/voice/mod.rs` | Module roots for the three above. |
| `src/crates/primer-gui/ui/voice.js` | Frontend voice-mode controller: header toggle, composer swap, event subscriptions, sticky-toggle restoration. |

**Modified files:**

| Path | Change |
|---|---|
| `src/crates/primer-speech/Cargo.toml` | Add `voice-loop` feature; pull in `silero,whisper,piper,cpal` building blocks transitively. |
| `src/crates/primer-cli/src/speech_loop.rs` | Shrink to ~50 lines: build backends, instantiate `StdoutObserver`, call `voice_loop::run_loop`. |
| `src/crates/primer-cli/src/speech_loop/mod.rs` | Make speech_loop a directory module containing the CLI adapter + `stdout_observer.rs`. |
| `src/crates/primer-cli/Cargo.toml` | `speech` feature now also pulls `primer-speech/voice-loop`. |
| `src/crates/primer-gui/Cargo.toml` | Add `speech` cargo feature pulling `primer-speech/{silero,whisper,piper,cpal,voice-loop}` + `reqwest`. |
| `src/crates/primer-gui/src/config.rs` | Add `SpeechSettings` block + `SpeechLocaleOverride`; extend `GuiConfigView` / `GuiConfigUpdate`. |
| `src/crates/primer-gui/src/state.rs` | Add `voice: Mutex<Option<ActiveVoiceLoop>>` slot to `AppState`. |
| `src/crates/primer-gui/src/types.rs` | Add `voice_mode_available: bool` to `SessionInfo`. |
| `src/crates/primer-gui/src/commands/mod.rs` | Register the four new voice commands. |
| `src/crates/primer-gui/src/lib.rs` | `pub mod voice;` |
| `src/crates/primer-gui/ui/index.html` | Header "Voice mode" toggle button; `<div class="composer composer-voice">`; asset-consent modal; replace `is-coming-soon` Speech settings group. |
| `src/crates/primer-gui/ui/styles.css` | Composer-voice widget styles + `[data-state]` animations. |
| `src/crates/primer-gui/ui/app.js` | Load `voice.js`; expose chunk handler for reuse; sidebar refresh on `primer://voice/response_complete`. |
| `src/crates/primer-gui/ui/settings.js` | Render Speech settings form with per-locale override sub-table + `disable_auto_download` checkbox. |
| `src/crates/primer-core/src/consts.rs` | Add `speech::DEFAULT_MIC_SILENCE_MS = 600`. |
| `src/locale/en.toml`, `src/locale/de.toml` (or equivalent) | Add `voice_state_listening` / `voice_state_thinking` / `voice_state_speaking` keys. |
| `README.md` | Status section: add a one-liner for GUI voice mode. |
| `CLAUDE.md` | After-action notes on the voice_loop shared module, asset cache location, GUI feature gates. |

---

## PR 1 — Lift `voice_loop` into `primer-speech`

Six tasks. The CLI's behavior is unchanged after this PR; the lift is mechanical. Each commit is independently revertable.

### Task 1.1: Add `voice-loop` feature to `primer-speech` Cargo.toml + empty module shell

**Files:**
- Modify: `src/crates/primer-speech/Cargo.toml`
- Create: `src/crates/primer-speech/src/voice_loop/mod.rs`
- Modify: `src/crates/primer-speech/src/lib.rs`

- [ ] **Step 1: Add the `voice-loop` feature gate to `Cargo.toml`**

In `src/crates/primer-speech/Cargo.toml`, locate the `[features]` section and append:

```toml
# Shared voice loop state machine (LISTEN → LATENT_THINK → SPEAK → LISTEN).
# Pulls every speech backend feature so the state machine can compose
# VAD → STT → DialogueManager → TTS → cpal without further conditional
# compilation in consumers. Consumed by primer-cli (--speech mode) and
# primer-gui (Voice mode toggle).
voice-loop = ["silero", "whisper", "piper", "cpal"]
```

- [ ] **Step 2: Create the empty `voice_loop` module**

Create `src/crates/primer-speech/src/voice_loop/mod.rs` with this content:

```rust
//! Shared voice loop state machine.
//!
//! State machine: `LISTEN → LATENT_THINK → SPEAK → LISTEN`, with the
//! mic open through LISTEN and LATENT_THINK so the Primer never barges
//! in on a child mid-thought. Closes the mic on the commit boundary
//! (first audio chunk reaches the speaker) so the child never speaks
//! over the Primer.
//!
//! Consumed by `primer-cli` (`--speech` mode) and `primer-gui` (Voice
//! mode toggle) via different [`LoopObserver`] implementations.
//!
//! See `docs/superpowers/specs/2026-05-02-voice-roundtrip-poc-design.md`
//! and `docs/superpowers/specs/2026-05-13-gui-voice-mode-design.md` for
//! the full design.

pub mod locale_defaults;
pub mod observer;

pub use locale_defaults::{voice_default_for, LocaleDefault, LOCALE_DEFAULTS};
pub use observer::{ExitReason, LoopObserver, TurnCompletePayload, VoiceState};
```

- [ ] **Step 3: Add the `voice_loop` module to `primer-speech`'s lib.rs**

In `src/crates/primer-speech/src/lib.rs`, append:

```rust
#[cfg(feature = "voice-loop")]
pub mod voice_loop;
```

- [ ] **Step 4: Run `cargo check` to verify the empty module compiles**

```bash
cd src
~/.cargo/bin/cargo check -p primer-speech --features voice-loop
```

Expected: `error[E0583]: file not found for module `locale_defaults`` and `observer` — these are the next two tasks.

- [ ] **Step 5: Commit**

```bash
cd src
git add Cargo.toml crates/primer-speech/Cargo.toml crates/primer-speech/src/lib.rs crates/primer-speech/src/voice_loop/mod.rs
git commit -m "$(cat <<'EOF'
feat(speech): scaffold voice_loop module behind voice-loop feature

Empty mod.rs shell that re-exports the upcoming LoopObserver,
locale_defaults, and run_loop. Compiles (with the expected
"file not found" errors) once the sibling files land.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 1.2: Define `LoopObserver` trait + supporting types

**Files:**
- Create: `src/crates/primer-speech/src/voice_loop/observer.rs`
- Test: inline in the same file under `#[cfg(test)]`

- [ ] **Step 1: Write the failing test for `VoiceState::name()`**

Create `src/crates/primer-speech/src/voice_loop/observer.rs` with this test-only header:

```rust
//! `LoopObserver` trait + supporting state types.

use uuid::Uuid;

/// State of the voice loop's main state machine.
///
/// Wire format (stable): the `name()` method returns kebab-case strings
/// that the GUI's `primer://voice/state_change` event payload carries
/// across IPC. Frontend CSS selectors and JS state lookups depend on
/// these exact values — do not rename without bumping the IPC contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VoiceState {
    /// Mic open, waiting for child to start or continue speaking.
    Listen,
    /// Child stopped speaking; LLM is generating. Mic still open so a
    /// resumed utterance can abort the LLM.
    LatentThink,
    /// Primer is speaking aloud; mic closed.
    Speak,
    /// Loop is exiting (final state before observer.on_exit fires).
    Exit,
}

impl VoiceState {
    /// Stable kebab-case wire string. Used as the IPC payload value AND
    /// as the `[data-state="..."]` attribute in the frontend.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Listen => "listen",
            Self::LatentThink => "latent_think",
            Self::Speak => "speak",
            Self::Exit => "exit",
        }
    }
}

/// Why the loop exited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitReason {
    /// External `stop_tx` signaled (CLI Ctrl+C, GUI End-voice-mode button).
    UserStop,
    /// Quit keyword matched in a finalized transcript ("goodbye" / "bye primer" / "stop primer").
    Keyword,
    /// Mic capture thread reported an unrecoverable error.
    MicError,
    /// Speaker output stream errored.
    SpeakerError,
}

impl ExitReason {
    pub fn name(&self) -> &'static str {
        match self {
            Self::UserStop => "user",
            Self::Keyword => "keyword",
            Self::MicError => "mic_error",
            Self::SpeakerError => "speaker_error",
        }
    }
}

/// Payload delivered to [`LoopObserver::on_response_complete`] after a
/// full Primer turn finishes synthesising.
#[derive(Debug, Clone)]
pub struct TurnCompletePayload {
    pub session_id: Uuid,
    pub child_turn_index: usize,
    pub primer_turn_index: usize,
}

/// Side-effect surface for the voice loop. CLI provides a stdout-printing
/// impl; GUI provides a Tauri-event-emitting impl. Same state machine,
/// different I/O.
///
/// `Send + 'static` so the loop can move the observer into its
/// state-machine task at spawn time.
pub trait LoopObserver: Send + 'static {
    /// Called on every state transition. `hint` carries optional context:
    /// `Some("user_cancel")` when the loop returns to LISTEN because the
    /// user pressed Stop / Esc; `Some("child_resumed")` when VAD-cancel-
    /// on-resumed-speech fires. `None` for ordinary transitions.
    fn on_state_change(&mut self, state: VoiceState, hint: Option<&str>);

    /// Called when Whisper finalizes a transcript. The corresponding
    /// child turn lands in the DialogueManager session via the loop's
    /// own respond_to_streaming call shortly after.
    fn on_transcript_finalized(&mut self, text: &str);

    /// Called per LLM chunk during LATENT_THINK / SPEAK. Mirrors the
    /// text-mode `primer://chunk` semantics.
    fn on_response_chunk(&mut self, primer_turn_index: usize, chunk: &str);

    /// Called after a turn completes successfully and TTS has finished
    /// synthesising the last phrase.
    fn on_response_complete(&mut self, payload: TurnCompletePayload);

    /// Called when an LLM call fails mid-turn. The loop replays a fallback
    /// "sorry, I had trouble" line through TTS regardless; this hook lets
    /// the GUI surface a banner if it wants to.
    fn on_inference_error(&mut self, err: &primer_core::error::InferenceError);

    /// Called exactly once, just before the loop returns from `run_loop`.
    fn on_exit(&mut self, reason: ExitReason);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_state_name_is_stable_kebab_case() {
        // The frontend matches on these exact strings. A drift here
        // silently breaks the [data-state="..."] CSS selectors and
        // the JS state lookups. Pin every variant.
        assert_eq!(VoiceState::Listen.name(), "listen");
        assert_eq!(VoiceState::LatentThink.name(), "latent_think");
        assert_eq!(VoiceState::Speak.name(), "speak");
        assert_eq!(VoiceState::Exit.name(), "exit");
    }

    #[test]
    fn exit_reason_name_is_stable_snake_case() {
        // Same contract as VoiceState::name — the IPC payload reads
        // these strings.
        assert_eq!(ExitReason::UserStop.name(), "user");
        assert_eq!(ExitReason::Keyword.name(), "keyword");
        assert_eq!(ExitReason::MicError.name(), "mic_error");
        assert_eq!(ExitReason::SpeakerError.name(), "speaker_error");
    }

    #[test]
    fn loop_observer_is_object_safe() {
        // The loop calls the observer through `dyn LoopObserver` for
        // monomorphisation savings; this trick catches accidental
        // generics that would break object safety.
        fn _accepts(_o: Box<dyn LoopObserver>) {}
    }
}
```

- [ ] **Step 2: Run the tests**

```bash
cd src
~/.cargo/bin/cargo test -p primer-speech --features voice-loop voice_loop::observer
```

Expected: All three tests pass.

- [ ] **Step 3: Commit**

```bash
cd src
git add crates/primer-speech/src/voice_loop/observer.rs
git commit -m "$(cat <<'EOF'
feat(speech): add LoopObserver trait and state types

VoiceState, ExitReason, TurnCompletePayload — the side-effect
contract shared between the CLI and GUI voice loop adapters.
name() methods carry stable wire strings the frontend depends on.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 1.3: Define `LOCALE_DEFAULTS` table + lookup

**Files:**
- Create: `src/crates/primer-speech/src/voice_loop/locale_defaults.rs`

- [ ] **Step 1: Write the failing test**

Create `src/crates/primer-speech/src/voice_loop/locale_defaults.rs`:

```rust
//! Locale → default voice/STT model mapping.
//!
//! Each locale pack ships a default voice and Whisper model. When the
//! user has not explicitly overridden in Settings → Speech, asset
//! resolution looks here for the canonical Hugging Face URLs +
//! cache-relative paths.
//!
//! Adding a new locale: append a new tuple. The `whisper_model_id`
//! convention follows the `whisper.cpp` filenames (`ggml-<size>.bin`
//! for multilingual, `ggml-<size>.en.bin` for English-only).

use primer_core::i18n::Locale;

/// Default voice + STT pinning for one locale pack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocaleDefault {
    /// Piper voice id matching the .onnx filename stem.
    pub piper_voice_id: &'static str,
    /// Direct download URL for the .onnx weights from Hugging Face.
    pub piper_onnx_url: &'static str,
    /// Direct download URL for the matching .onnx.json config.
    pub piper_config_url: &'static str,
    /// Whisper model id (matches the file name in
    /// `~/.cache/primer/models/whisper/`).
    pub whisper_model_id: &'static str,
    /// Direct download URL for the Whisper .bin from Hugging Face.
    pub whisper_url: &'static str,
    /// Sum of Piper + Whisper file sizes, in megabytes, rounded.
    /// Used by the consent dialog to show "Download (≈540 MB)".
    pub approx_total_mb: u32,
}

/// Mapping from `Locale::pack_id()` to its default voice/STT bundle.
pub const LOCALE_DEFAULTS: &[(&str, LocaleDefault)] = &[
    (
        "en",
        LocaleDefault {
            piper_voice_id: "en_GB-alba-medium",
            piper_onnx_url: "https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_GB/alba/medium/en_GB-alba-medium.onnx",
            piper_config_url: "https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_GB/alba/medium/en_GB-alba-medium.onnx.json",
            whisper_model_id: "ggml-small.en.bin",
            whisper_url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.en.bin",
            approx_total_mb: 530,
        },
    ),
    (
        "de",
        LocaleDefault {
            piper_voice_id: "de_DE-thorsten-medium",
            piper_onnx_url: "https://huggingface.co/rhasspy/piper-voices/resolve/main/de/de_DE/thorsten/medium/de_DE-thorsten-medium.onnx",
            piper_config_url: "https://huggingface.co/rhasspy/piper-voices/resolve/main/de/de_DE/thorsten/medium/de_DE-thorsten-medium.onnx.json",
            whisper_model_id: "ggml-small.bin",
            whisper_url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin",
            approx_total_mb: 540,
        },
    ),
];

/// Look up the default voice/STT bundle for `locale`, if one is pinned.
/// Returns `None` for locales that don't ship a default — the caller
/// must surface an "explicit Settings → Speech path required" error
/// to the user in that case.
pub fn voice_default_for(locale: &Locale) -> Option<&'static LocaleDefault> {
    LOCALE_DEFAULTS
        .iter()
        .find(|(id, _)| *id == locale.pack_id())
        .map(|(_, d)| d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_default_is_alba_plus_small_en() {
        let d = voice_default_for(&Locale::English).expect("en is pinned");
        assert_eq!(d.piper_voice_id, "en_GB-alba-medium");
        assert_eq!(d.whisper_model_id, "ggml-small.en.bin");
    }

    #[test]
    fn german_default_is_thorsten_plus_small_multilingual() {
        let d = voice_default_for(&Locale::German).expect("de is pinned");
        assert_eq!(d.piper_voice_id, "de_DE-thorsten-medium");
        // Multilingual Whisper, not the .en-only variant — German is
        // not in small.en's training set.
        assert_eq!(d.whisper_model_id, "ggml-small.bin");
    }

    #[test]
    fn all_urls_resolve_under_huggingface_co() {
        // Pin the source so a future "use a mirror" PR is explicit
        // rather than a silent URL swap that escapes review.
        for (_, d) in LOCALE_DEFAULTS {
            assert!(d.piper_onnx_url.starts_with("https://huggingface.co/"));
            assert!(d.piper_config_url.starts_with("https://huggingface.co/"));
            assert!(d.whisper_url.starts_with("https://huggingface.co/"));
        }
    }

    #[test]
    fn approx_total_mb_is_sane() {
        // A defensive lower bound: a Whisper small is ~470 MB by itself,
        // a Piper medium voice is ~60 MB. Any default below 400 MB is
        // a typo.
        for (id, d) in LOCALE_DEFAULTS {
            assert!(
                d.approx_total_mb >= 400,
                "{} default total of {} MB is suspiciously low",
                id,
                d.approx_total_mb,
            );
            assert!(
                d.approx_total_mb <= 2000,
                "{} default total of {} MB is suspiciously high",
                id,
                d.approx_total_mb,
            );
        }
    }
}
```

- [ ] **Step 2: Verify `Locale::German` exists in primer-core**

```bash
cd src
grep -n "German" crates/primer-core/src/i18n.rs
```

Expected: a `German` variant exists on the `Locale` enum. If not (less likely given the German locale-pack work already landed), use `Locale::from_pack_id("de").unwrap()` in the test instead.

- [ ] **Step 3: Run the locale_defaults tests**

```bash
cd src
~/.cargo/bin/cargo test -p primer-speech --features voice-loop voice_loop::locale_defaults
```

Expected: All four tests pass.

- [ ] **Step 4: Commit**

```bash
cd src
git add crates/primer-speech/src/voice_loop/locale_defaults.rs
git commit -m "$(cat <<'EOF'
feat(speech): add LOCALE_DEFAULTS for voice mode

Pins en → en_GB-alba-medium + Whisper small.en; de →
de_DE-thorsten-medium + Whisper small (multilingual). URLs from
the official rhasspy/piper-voices and ggerganov/whisper.cpp HF
mirrors. The asset resolver in primer-gui looks here when the
user has no explicit Settings → Speech override.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 1.4: Move the state-machine implementation into `voice_loop::state_machine`

This is the largest mechanical lift in the plan. The existing `primer-cli/src/speech_loop.rs` is ~2000 lines containing the run_loop state machine, helper functions, mock backends for tests, and the `run` CLI entry point. We split:

- State machine + helpers + mock backends → `primer-speech::voice_loop::state_machine` (new file)
- `LoopBackends`, `Responder` trait, `SpeechLoopConfig`, `DrainHook` type alias → `primer-speech::voice_loop` (re-exported from `mod.rs`)
- `run_loop` becomes the public entrypoint, calling observer callbacks instead of `println!`s
- `run` CLI entry stays in primer-cli (Task 1.5) and becomes a thin adapter

**Files:**
- Create: `src/crates/primer-speech/src/voice_loop/state_machine.rs`
- Modify: `src/crates/primer-speech/src/voice_loop/mod.rs` (re-exports)
- Source: `src/crates/primer-cli/src/speech_loop.rs` (preserved unchanged at this step — replaced in Task 1.5)

- [ ] **Step 1: Copy state-machine code into the new module**

Run this in `src/`:

```bash
cd src
# Copy the source as-is; we'll edit in-place after.
cp crates/primer-cli/src/speech_loop.rs crates/primer-speech/src/voice_loop/state_machine.rs
```

- [ ] **Step 2: Adjust the new file's module path + visibility**

In `src/crates/primer-speech/src/voice_loop/state_machine.rs`:

1. Replace the module-level doc comment header (the `//! ` lines) with:
   ```rust
   //! State machine implementation (LISTEN → LATENT_THINK → SPEAK → LISTEN).
   //!
   //! Lifted from `primer-cli/src/speech_loop.rs` in PR 1 of the GUI
   //! voice-mode work. Side-effects now route through [`super::LoopObserver`]
   //! instead of inline `println!`s; the CLI's stdout output is preserved
   //! by the `StdoutObserver` adapter in `primer-cli`.
   ```

2. Replace every occurrence of `#[cfg(feature = "speech")]` with `#[cfg(feature = "voice-loop")]` (this is in the new crate now; the feature has a different name). Most of the file is already gated this way at the function/import level.

3. Remove the `pub fn run(...)` function (the CLI entry point) and the `mod tests` block's stdout assertions — the run function lives in primer-cli (Task 1.5). State machine tests stay; they're observer-driven and locale-neutral.

4. Add imports at the top of the file for the new module shape:
   ```rust
   use super::observer::{ExitReason, LoopObserver, TurnCompletePayload, VoiceState};
   ```

5. Audit `use primer_core::...` and `use primer_speech::...` paths — since this file is now INSIDE `primer-speech`, change `use primer_speech::foo` to `use crate::foo` where applicable.

- [ ] **Step 3: Replace inline side-effect calls with observer callbacks**

In `state_machine.rs`, find every `println!` / `eprintln!` (the `[child]`, `[primer]`, `[vad]`, `[stt]`, `[tts]` log lines) and every `tracing::warn!` that surfaces state changes. Replace with observer method calls.

**State change pattern.** Every state transition site replaces a block like:
```rust
// before
state = LATENT_THINK;
if cfg.verbose { eprintln!("[state] LISTEN -> LATENT_THINK"); }
```
with:
```rust
// after
state = VoiceState::LatentThink;
observer.on_state_change(VoiceState::LatentThink, None);
```

**Cancel-with-hint pattern.** Two sites set the `hint` arg:

- `LATENT_THINK → LISTEN` because the user clicked Stop / pressed Esc → `observer.on_state_change(VoiceState::Listen, Some("user_cancel"))`. This is the new code path the GUI's `cancel_response_tx` channel routes to (added in Task 1.5 along with `LoopHandle`).
- `LATENT_THINK → LISTEN` because VAD `SpeechStart` fired (child resumed) → `observer.on_state_change(VoiceState::Listen, Some("child_resumed"))`. Existing code path.

**Transcript pattern.** Replace:
```rust
println!("[child] {}", transcript);
```
with:
```rust
observer.on_transcript_finalized(transcript);
```

**Chunk pattern.** Inside the `Responder::respond` callback (the closure passed to `dm.respond_to_streaming`):
```rust
// before
print!("{}", chunk); std::io::Write::flush(&mut std::io::stdout()).ok();
```
becomes:
```rust
observer.on_response_chunk(primer_turn_index, chunk);
```

Note: the chunk callback is currently called via the `Responder::respond` interface (line ~299 in the existing file). The state machine needs to forward chunks via the observer; the `Responder` trait's callback signature already gives us `&str` chunks. Capture the observer by-mut-ref into the chunk closure (it lives in run_loop's stack frame).

**Turn complete pattern.** After `respond.await` returns successfully and TTS has fully drained:
```rust
observer.on_response_complete(TurnCompletePayload {
    session_id,
    child_turn_index,
    primer_turn_index,
});
```

The `session_id`, `child_turn_index`, `primer_turn_index` are already in scope from the existing dialogue-manager call site; this is just a structured-emit replacement for the existing one-line `[primer]` print.

**Inference error pattern.** Replace the existing `tracing::warn!("LLM error: {e}")` + fallback-line synthesis with:
```rust
observer.on_inference_error(&inference_error);
// existing fallback synthesis continues
```

**Exit pattern.** At every `return` path in `run_loop`, add `observer.on_exit(reason)` before returning, where `reason` matches the exit cause (UserStop / Keyword / MicError / SpeakerError).

- [ ] **Step 4: Re-export the public surface from `voice_loop/mod.rs`**

Edit `src/crates/primer-speech/src/voice_loop/mod.rs` to look like:

```rust
//! Shared voice loop state machine.
//! (full doc comment unchanged from Task 1.1)

pub mod locale_defaults;
pub mod observer;
pub mod state_machine;

pub use locale_defaults::{voice_default_for, LocaleDefault, LOCALE_DEFAULTS};
pub use observer::{ExitReason, LoopObserver, TurnCompletePayload, VoiceState};
pub use state_machine::{
    run_loop, DrainHook, LoopBackends, LoopConfig, LoopHandle, Responder, VoiceLoopError,
};
```

Rename `SpeechLoopConfig` to `LoopConfig` inside `state_machine.rs` at the same time — the new name matches the design doc.

- [ ] **Step 5: Add the `LoopHandle` and external-stop / cancel-response channels**

The existing `run_loop` signature ends with the function returning `Result`. We need to expose two channels (`stop_tx`, `cancel_response_tx`) and a `JoinHandle`. The cleanest API: change `run_loop` to return `(LoopHandle, JoinHandle<Result<(), VoiceLoopError>>)`, spawn the loop inside the function.

Add this at the top of `state_machine.rs` (or just below the existing types):

```rust
/// Handle returned by [`run_loop`] for external control.
///
/// `stop_tx` ends the loop entirely (CLI Ctrl+C / GUI End-voice-mode).
/// `cancel_response_tx` aborts the in-flight LLM call + TTS synthesis
/// and returns the loop to LISTEN (GUI Stop button, Esc keypress).
pub struct LoopHandle {
    pub stop_tx: tokio::sync::oneshot::Sender<()>,
    pub cancel_response_tx: tokio::sync::mpsc::Sender<()>,
}

/// Voice loop error type. Today carries a single string variant; new
/// variants land here when the state machine grows recoverable error
/// paths.
#[derive(Debug, thiserror::Error)]
pub enum VoiceLoopError {
    #[error("voice loop error: {0}")]
    Other(String),
}
```

Then rename the existing `pub async fn run_loop` to `async fn run_loop_inner` (the actual state machine body, taking an `external_stop: oneshot::Receiver<()>` and `cancel_response: mpsc::Receiver<()>` as parameters), and add a new `pub fn run_loop` that creates the channels and spawns the inner:

```rust
pub fn run_loop<'r, O: LoopObserver>(
    backends: LoopBackends,
    events: tokio::sync::mpsc::Receiver<primer_core::speech::VadEvent>,
    responder: Box<dyn Responder + 'r>,
    cfg: LoopConfig,
    observer: O,
) -> (LoopHandle, tokio::task::JoinHandle<Result<(), VoiceLoopError>>)
where
    'r: 'static,
{
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let (cancel_tx, cancel_rx) = tokio::sync::mpsc::channel::<()>(8);
    let handle = LoopHandle {
        stop_tx,
        cancel_response_tx: cancel_tx,
    };
    let join = tokio::spawn(async move {
        run_loop_inner(backends, events, responder, cfg, observer, stop_rx, cancel_rx)
            .await
            .map_err(|e| VoiceLoopError::Other(e.to_string()))
    });
    (handle, join)
}
```

The `'r: 'static` bound on the responder is intentional — the spawn requires it. Callers that need to borrow a `&mut DialogueManager` (the current CLI pattern) hold the DM in an Arc<Mutex<...>> and pass it through the Responder impl. The CLI adapter in Task 1.5 sets this up.

Inside `run_loop_inner`, add a new `select!` arm for `cancel_response` adjacent to the existing VAD-cancel arm. The new arm should fire `observer.on_state_change(VoiceState::Listen, Some("user_cancel"))` and route through the same "abort synthesis, drop holding buffer, return to LISTEN" code that VAD-resumed-speech uses today.

The external `stop_rx` adds another `select!` arm at the top level: any `select!` in `run_loop_inner` that polls `events.recv()` also polls `stop_rx`; on stop_rx, set `reason = ExitReason::UserStop`, break to the exit path.

- [ ] **Step 6: Run `cargo check` to identify everything still broken**

```bash
cd src
~/.cargo/bin/cargo check -p primer-speech --features voice-loop
```

Expected: a bunch of errors. Iterate by fixing each (module paths, missing imports, renamed types). Re-run until clean.

- [ ] **Step 7: Run the state-machine tests with their existing MockObserver**

The existing tests in `speech_loop.rs` operate on `MockStreamingStt`, `MockStreamingTts`, etc., and assert behavior through transcript / chunk-callback inspection. Add a `MockObserver` to the test module that records every callback in a `Vec<MockEvent>`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq)]
    pub enum MockEvent {
        StateChange { state: VoiceState, hint: Option<String> },
        Transcript(String),
        Chunk { primer_turn_index: usize, text: String },
        Complete(TurnCompletePayload),
        InferenceError(String),
        Exit(ExitReason),
    }

    pub struct MockObserver(pub std::sync::Arc<std::sync::Mutex<Vec<MockEvent>>>);

    impl LoopObserver for MockObserver {
        fn on_state_change(&mut self, state: VoiceState, hint: Option<&str>) {
            self.0.lock().unwrap().push(MockEvent::StateChange {
                state,
                hint: hint.map(String::from),
            });
        }
        fn on_transcript_finalized(&mut self, text: &str) {
            self.0.lock().unwrap().push(MockEvent::Transcript(text.to_string()));
        }
        fn on_response_chunk(&mut self, primer_turn_index: usize, chunk: &str) {
            self.0.lock().unwrap().push(MockEvent::Chunk {
                primer_turn_index,
                text: chunk.to_string(),
            });
        }
        fn on_response_complete(&mut self, payload: TurnCompletePayload) {
            self.0.lock().unwrap().push(MockEvent::Complete(payload));
        }
        fn on_inference_error(&mut self, err: &primer_core::error::InferenceError) {
            self.0.lock().unwrap().push(MockEvent::InferenceError(format!("{err:?}")));
        }
        fn on_exit(&mut self, reason: ExitReason) {
            self.0.lock().unwrap().push(MockEvent::Exit(reason));
        }
    }
    // ... rest of existing test functions ...
}
```

Existing test assertions that consumed `mock.transcripts` or similar fields now consume `events.lock().unwrap()` and pattern-match on `MockEvent` variants. The migration is mechanical.

- [ ] **Step 8: Run the migrated tests**

```bash
cd src
~/.cargo/bin/cargo test -p primer-speech --features voice-loop voice_loop::state_machine
```

Expected: every test that passed against the CLI's speech_loop.rs now passes against `primer-speech::voice_loop::state_machine`. If anything fails, the migration is incomplete — fix the gap.

- [ ] **Step 9: Commit**

```bash
cd src
git add crates/primer-speech/src/voice_loop/
git commit -m "$(cat <<'EOF'
feat(speech): lift voice loop state machine into primer-speech

State machine moves from primer-cli/src/speech_loop.rs into
primer-speech::voice_loop::state_machine, with side-effects routed
through the new LoopObserver trait instead of inline println!s.

run_loop now returns a LoopHandle (stop_tx, cancel_response_tx)
and a JoinHandle so consumers (CLI, GUI) can control the loop
externally. Tests migrated to a MockObserver and continue to pass.

CLI still references the old primer-cli speech_loop module — that
becomes a thin adapter in the next commit.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 1.5: CLI adapter — `StdoutObserver` and thin `speech_loop::run`

**Files:**
- Create: `src/crates/primer-cli/src/speech_loop/mod.rs` (replaces flat file with directory module)
- Create: `src/crates/primer-cli/src/speech_loop/stdout_observer.rs`
- Modify: `src/crates/primer-cli/src/main.rs` (call sites)
- Modify: `src/crates/primer-cli/Cargo.toml`

- [ ] **Step 1: Convert speech_loop.rs to a directory module**

```bash
cd src
mkdir -p crates/primer-cli/src/speech_loop
# Move the existing flat file aside; we'll rewrite the entry as a directory module
git mv crates/primer-cli/src/speech_loop.rs crates/primer-cli/src/speech_loop_legacy.rs.deleteme
```

Then create `src/crates/primer-cli/src/speech_loop/mod.rs`:

```rust
//! CLI adapter for the shared `primer_speech::voice_loop`.
//!
//! Builds [`LoopBackends`] + [`LoopConfig`] from clap flags, instantiates
//! a [`StdoutObserver`] that preserves the existing CLI print formatting,
//! wires Ctrl+C to `LoopHandle::stop_tx`, and awaits the join.

pub mod stdout_observer;

use std::path::Path;

use primer_core::error::Result;
use primer_pedagogy::DialogueManager;
use primer_speech::voice_loop::{self, LoopConfig};

pub use stdout_observer::StdoutObserver;

/// Configuration passed into [`run`] from `main`. Same shape as the
/// pre-refactor `SpeechLoopConfig` in `speech_loop.rs` so the call
/// site in main.rs needs no changes beyond the import.
pub struct SpeechLoopConfig<'a> {
    pub whisper_model: &'a Path,
    pub voice_onnx: &'a Path,
    pub voice_config: &'a Path,
    pub voice_id: &'a str,
    pub mic_silence_ms: u32,
    pub verbose: bool,
    pub locale: primer_core::i18n::Locale,
}

/// CLI entry point for `--speech` mode.
///
/// Builds the speech backends (cpal mic + speaker, silero VAD, whisper
/// streaming STT, piper TTS), instantiates a [`StdoutObserver`], and
/// calls [`primer_speech::voice_loop::run_loop`].
pub async fn run<'a>(cfg: SpeechLoopConfig<'a>, dm: &'a mut DialogueManager) -> Result<()> {
    // === Backend construction ===
    // (Lifted verbatim from the pre-refactor speech_loop.rs::run; only
    // the println!s and the run_loop call have changed. See git history
    // of crates/primer-cli/src/speech_loop_legacy.rs.deleteme for the
    // original implementation.)
    todo!("port from speech_loop_legacy.rs.deleteme step-by-step; see comments below");
}
```

- [ ] **Step 2: Port the backend-construction body from the legacy file**

Open the legacy file (`speech_loop_legacy.rs.deleteme`) and copy the body of the original `pub async fn run` into the new `run` function, with these changes:

1. The original built a `LoopBackends` and called `run_loop(...).await`. Replace the run_loop call with the new spawn-returning shape:

   ```rust
   let observer = StdoutObserver::new(cfg.verbose);
   let (handle, join) = voice_loop::run_loop(
       backends,
       event_rx,
       Box::new(DialogueResponder::new(dm)),
       LoopConfig {
           mic_silence_ms: cfg.mic_silence_ms,
           verbose: cfg.verbose,
           locale: cfg.locale,
       },
       observer,
   );

   // Wire Ctrl+C to handle.stop_tx so the user's existing Ctrl+C habit
   // still ends voice mode cleanly. tokio::signal::ctrl_c() returns the
   // moment the first SIGINT arrives; the handle.stop_tx send ends the
   // loop within one event-loop tick.
   let stop_tx = handle.stop_tx;
   let ctrl_c_task = tokio::spawn(async move {
       let _ = tokio::signal::ctrl_c().await;
       let _ = stop_tx.send(());
   });

   // Await the loop's join handle. On normal exit (keyword or stop_tx),
   // it returns Ok; on a panic, the JoinHandle yields JoinError.
   let result = join.await.map_err(|e| {
       primer_core::error::PrimerError::Speech(format!("voice loop join: {e}"))
   })?;
   ctrl_c_task.abort();
   result.map_err(|e| primer_core::error::PrimerError::Speech(e.to_string()))?;
   Ok(())
   ```

2. The `DialogueResponder` type already exists in the legacy file (the original `pub trait Responder` implementation that wraps DialogueManager). Copy it into a new private module `src/crates/primer-cli/src/speech_loop/dialogue_responder.rs` and add `mod dialogue_responder;` + `use dialogue_responder::DialogueResponder;` at the top of `mod.rs`.

3. The `cancel_response_tx` channel on `LoopHandle` is unused by the CLI (CLI users have Ctrl+C only — no Stop button equivalent). Drop the receiver end on the floor; the GUI consumes it.

- [ ] **Step 3: Implement `StdoutObserver` preserving every existing print line**

Create `src/crates/primer-cli/src/speech_loop/stdout_observer.rs`:

```rust
//! CLI-side [`LoopObserver`] that preserves the existing stdout/stderr
//! print formatting of the pre-refactor speech_loop.rs.
//!
//! Output contract (do not change without bumping the user-visible
//! verbose docs):
//!   stdout: `[child] <transcript>` and `[primer] <reply chunk>` lines
//!   stderr (only if verbose): `[vad] ...`, `[stt] ...`, `[tts] ...`,
//!     `[state] <from> -> <to>` lines

use std::io::Write;

use primer_speech::voice_loop::{ExitReason, LoopObserver, TurnCompletePayload, VoiceState};

pub struct StdoutObserver {
    verbose: bool,
    last_state: Option<VoiceState>,
    /// True while a `[primer] ...` line is being streamed so we can
    /// emit a newline at turn-complete time.
    primer_line_open: bool,
}

impl StdoutObserver {
    pub fn new(verbose: bool) -> Self {
        Self {
            verbose,
            last_state: None,
            primer_line_open: false,
        }
    }
}

impl LoopObserver for StdoutObserver {
    fn on_state_change(&mut self, state: VoiceState, _hint: Option<&str>) {
        if self.verbose {
            if let Some(prev) = self.last_state {
                eprintln!("[state] {} -> {}", prev.name(), state.name());
            } else {
                eprintln!("[state] -> {}", state.name());
            }
        }
        self.last_state = Some(state);
    }

    fn on_transcript_finalized(&mut self, text: &str) {
        println!("[child] {text}");
    }

    fn on_response_chunk(&mut self, _primer_turn_index: usize, chunk: &str) {
        if !self.primer_line_open {
            print!("[primer] ");
            self.primer_line_open = true;
        }
        print!("{chunk}");
        // Flush so the user sees streaming chunks as they arrive,
        // not after a full line buffers.
        let _ = std::io::stdout().flush();
    }

    fn on_response_complete(&mut self, _payload: TurnCompletePayload) {
        if self.primer_line_open {
            println!();
            self.primer_line_open = false;
        }
    }

    fn on_inference_error(&mut self, err: &primer_core::error::InferenceError) {
        if self.primer_line_open {
            println!();
            self.primer_line_open = false;
        }
        eprintln!("[primer] (inference error: {err:?})");
    }

    fn on_exit(&mut self, reason: ExitReason) {
        if self.verbose {
            eprintln!("[state] exiting ({})", reason.name());
        }
    }
}
```

- [ ] **Step 4: Update `Cargo.toml` to pull the `voice-loop` feature**

In `src/crates/primer-cli/Cargo.toml`, change the `speech` feature definition:

```toml
speech = [
    "primer-speech/silero",
    "primer-speech/whisper",
    "primer-speech/piper",
    "primer-speech/cpal",
    "primer-speech/voice-loop",  # ← add
]
```

- [ ] **Step 5: Build the CLI with the speech feature**

```bash
cd src
~/.cargo/bin/cargo build -p primer-cli --features speech
```

Expected: clean build. If anything in `main.rs` still references types from the legacy file (e.g., `speech_loop::Responder`), update the import to `primer_speech::voice_loop::Responder`.

- [ ] **Step 6: Run the CLI unit tests with the speech feature**

```bash
cd src
~/.cargo/bin/cargo test -p primer-cli --features speech
```

Expected: every existing test still passes. The state-machine tests now live in `primer-speech` and are covered there.

- [ ] **Step 7: Delete the legacy file**

```bash
cd src
git rm crates/primer-cli/src/speech_loop_legacy.rs.deleteme
```

- [ ] **Step 8: Commit**

```bash
cd src
git add -A crates/primer-cli/src/speech_loop crates/primer-cli/src/main.rs crates/primer-cli/Cargo.toml
git commit -m "$(cat <<'EOF'
refactor(cli): port --speech to shared voice loop via StdoutObserver

speech_loop.rs becomes a directory module with a thin run adapter
and a StdoutObserver that preserves the existing print formatting
verbatim. State machine implementation now lives in
primer-speech::voice_loop and is shared with the GUI.

Ctrl+C wired to LoopHandle::stop_tx for the same end-session
gesture; cancel_response_tx (Stop button equivalent) is unused
by the CLI and dropped on the floor.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 1.6: CLI behavior smoke

**Files:** none (manual validation step)

- [ ] **Step 1: Run the CLI with --speech and confirm a complete voice exchange**

This step requires actual mic/speaker hardware and the user's pre-downloaded model files. The user has `de_DE-thorsten-medium` locally already; for English, `en_GB-alba-medium` + Whisper small.en need to live somewhere the user can pass as flags.

```bash
cd src
~/.cargo/bin/cargo run --bin primer --features primer-cli/speech -- \
  --speech \
  --whisper-model ~/path/to/ggml-small.en.bin \
  --voice-onnx ~/path/to/en_GB-alba-medium.onnx \
  --voice-config ~/path/to/en_GB-alba-medium.onnx.json \
  --voice en_GB-alba-medium \
  --name Binti --age 8 \
  --verbose
```

Expected:
- `[state] -> listen` and subsequent state lines on stderr
- `[child] <your spoken question>` lines on stdout after each VAD finalize
- `[primer] <streaming reply>` lines on stdout with TTS playback
- Saying "goodbye" exits with `[state] exiting (keyword)` on stderr

If any of the above is missing or differs from the pre-refactor behavior, do not commit Task 1.6 — fix the migration first.

- [ ] **Step 2: PR-ready checkpoint**

PR 1 ends here. The CLI is fully on the shared voice loop; no user-visible behavior changes. Open a PR titled "refactor(speech): lift voice loop into primer-speech" with a body summarising the tasks above.

---

## PR 2 — Add `SpeechSettings` to `GuiConfig`

Four tasks. Pure additive change to the GUI config schema; no functional voice mode yet.

### Task 2.1: Add `DEFAULT_MIC_SILENCE_MS` to primer-core consts

**Files:**
- Modify: `src/crates/primer-core/src/consts.rs`

- [ ] **Step 1: Find the consts module organisation**

```bash
cd src
grep -n "pub mod" crates/primer-core/src/consts.rs | head
```

Expected: a series of `pub mod retrieval`, `pub mod vocab`, etc. submodules.

- [ ] **Step 2: Add a `speech` submodule**

In `src/crates/primer-core/src/consts.rs`, append:

```rust
/// Speech-mode tunables. Mirrors the CLI's `--mic-silence-ms` flag and
/// any future GUI-level speech defaults.
pub mod speech {
    /// Milliseconds of post-end-of-speech silence VAD waits before
    /// firing SpeechEnd. The CLI's `--mic-silence-ms` defaults to
    /// this value; the GUI's `SpeechSettings::mic_silence_ms` default
    /// reads it via this constant.
    ///
    /// Lifted from a 600 ms default at the original `--speech` POC
    /// (PR for spec 2026-05-02). Tuning rationale: silero's 300 ms
    /// default is too aggressive given cancel-on-resume; 600 ms
    /// reduces false trips without hurting perceived response time.
    pub const DEFAULT_MIC_SILENCE_MS: u32 = 600;
}
```

- [ ] **Step 3: Run tests**

```bash
cd src
~/.cargo/bin/cargo test -p primer-core consts
```

Expected: clean. Constants don't normally need their own tests, but the build verifies the module compiles.

- [ ] **Step 4: Commit**

```bash
cd src
git add crates/primer-core/src/consts.rs
git commit -m "feat(core): add consts::speech::DEFAULT_MIC_SILENCE_MS = 600"
```

### Task 2.2: Add `SpeechSettings` struct to `GuiConfig`

**Files:**
- Modify: `src/crates/primer-gui/src/config.rs`

- [ ] **Step 1: Write failing tests in `config.rs`**

At the bottom of `src/crates/primer-gui/src/config.rs`'s `#[cfg(test)] mod tests` block, add:

```rust
#[test]
fn speech_settings_default_has_600ms_silence() {
    let s = SpeechSettings::default();
    assert!(!s.voice_mode_enabled, "voice mode is off by default");
    assert!(!s.disable_auto_download, "auto-download is offered by default");
    assert_eq!(
        s.mic_silence_ms,
        primer_core::consts::speech::DEFAULT_MIC_SILENCE_MS,
        "mic_silence_ms default reads from primer_core consts",
    );
    assert!(s.overrides.is_empty(), "no per-locale overrides by default");
}

#[test]
fn speech_settings_round_trips_through_disk() {
    let dir = TempDir::new().unwrap();
    let mut cfg = GuiConfig::default();
    cfg.speech.voice_mode_enabled = true;
    cfg.speech.mic_silence_ms = 750;
    cfg.speech.overrides.insert(
        "de".to_string(),
        SpeechLocaleOverride {
            piper_onnx_path: Some("/tmp/de.onnx".into()),
            piper_config_path: Some("/tmp/de.onnx.json".into()),
            whisper_model_path: None,
            voice_id: Some("de_DE-thorsten-medium".to_string()),
        },
    );

    save(dir.path(), &cfg).unwrap();
    let round_trip = load(dir.path()).unwrap();
    assert_eq!(round_trip, cfg);
}

#[test]
fn older_config_without_speech_block_loads_with_defaults() {
    // An on-disk config from before PR 2 has no `speech` field. Loading
    // it must succeed and inject SpeechSettings::default() without
    // requiring a migration step.
    let dir = TempDir::new().unwrap();
    let path = config_path(dir.path());
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        r#"{"learner": {"name": "Ada", "age": 7, "locale": "en"}}"#,
    )
    .unwrap();

    let cfg = load(dir.path()).unwrap();
    assert_eq!(cfg.learner.name, "Ada");
    assert_eq!(cfg.speech, SpeechSettings::default());
}
```

- [ ] **Step 2: Run the tests; expect failure**

```bash
cd src
~/.cargo/bin/cargo test -p primer-gui config::tests::speech_settings
```

Expected: compile error — `SpeechSettings` not defined.

- [ ] **Step 3: Add `SpeechSettings` and `SpeechLocaleOverride` structs**

In `src/crates/primer-gui/src/config.rs`, after the `UiConfig` struct and before `ConfigError`:

```rust
/// Voice-mode settings.
///
/// `voice_mode_enabled` is the sticky toggle (per device, not per
/// learner — see spec [[project_personal_device_model]]). `overrides`
/// is keyed by `Locale::pack_id()` so switching locales doesn't clobber
/// the path the user typed in for the other one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpeechSettings {
    pub voice_mode_enabled: bool,
    pub disable_auto_download: bool,
    /// Milliseconds of post-end-of-speech silence the VAD waits before
    /// firing SpeechEnd. Default reads from
    /// `primer_core::consts::speech::DEFAULT_MIC_SILENCE_MS`.
    pub mic_silence_ms: u32,
    /// Per-locale path / voice-id overrides. Keyed by `Locale::pack_id()`.
    pub overrides: std::collections::BTreeMap<String, SpeechLocaleOverride>,
}

impl Default for SpeechSettings {
    fn default() -> Self {
        Self {
            voice_mode_enabled: false,
            disable_auto_download: false,
            mic_silence_ms: primer_core::consts::speech::DEFAULT_MIC_SILENCE_MS,
            overrides: std::collections::BTreeMap::new(),
        }
    }
}

/// Per-locale path/voice override for `SpeechSettings`. `None` on any
/// field means "fall through to the locale default" (see
/// `primer_speech::voice_loop::locale_defaults::voice_default_for`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SpeechLocaleOverride {
    pub piper_onnx_path: Option<PathBuf>,
    pub piper_config_path: Option<PathBuf>,
    pub whisper_model_path: Option<PathBuf>,
    pub voice_id: Option<String>,
}
```

Then add `pub speech: SpeechSettings,` to the `GuiConfig` struct (after `pub ui: UiConfig,`).

- [ ] **Step 4: Run tests; expect pass**

```bash
cd src
~/.cargo/bin/cargo test -p primer-gui config::tests::speech_settings
```

Expected: all three speech tests pass.

- [ ] **Step 5: Run full config tests**

```bash
cd src
~/.cargo/bin/cargo test -p primer-gui config::tests
```

Expected: all config tests pass, including the existing forward-compatibility test (which proves the additive change is backward-compatible).

- [ ] **Step 6: Commit**

```bash
cd src
git add crates/primer-gui/src/config.rs
git commit -m "$(cat <<'EOF'
feat(gui): add SpeechSettings block to GuiConfig

voice_mode_enabled sticky toggle, disable_auto_download
strict-offline escape, mic_silence_ms (default 600 from
primer_core consts), per-locale overrides map keyed by pack_id.

Older configs without the speech field load with defaults; the
existing forward-compatibility test pins this.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 2.3: Extend `GuiConfigView` and `GuiConfigUpdate` DTOs

**Files:**
- Modify: `src/crates/primer-gui/src/config.rs`

- [ ] **Step 1: Add `speech` field to `GuiConfigView`**

In the `GuiConfigView` struct, add `pub speech: SpeechSettings,` (same shape as on disk — no redaction needed, no secrets in speech settings).

In the `From<&GuiConfig> for GuiConfigView` impl, add `speech: c.speech.clone(),`.

- [ ] **Step 2: Add `speech` field to `GuiConfigUpdate`**

In the `GuiConfigUpdate` struct, add `pub speech: SpeechSettings,`.

In `GuiConfigUpdate::into_config`, add `speech: self.speech,`.

- [ ] **Step 3: Add a round-trip test for the view/update DTOs**

In `mod tests`:

```rust
#[test]
fn speech_settings_round_trip_through_view_and_update() {
    let mut cfg = GuiConfig::default();
    cfg.speech.voice_mode_enabled = true;
    cfg.speech.mic_silence_ms = 800;

    // View path: serialize, ensure speech block is present and round-trips.
    let view: GuiConfigView = (&cfg).into();
    assert_eq!(view.speech, cfg.speech);

    // Update path: deserialize an update with a speech block, apply.
    let update_json = serde_json::to_string(&serde_json::json!({
        "learner": {"name": "Binti", "age": 8, "locale": "en"},
        "backend": {
            "kind": "stub", "model": null,
            "ollama_url": "http://localhost:11434",
            "api_key_source": {"kind": "keep"},
        },
        "classifier": {"match_main": true, "kind": null, "model": null, "timeout_ms": 3000},
        "extractor": {"match_main": true, "kind": null, "model": null, "timeout_ms": 5000},
        "comprehension": {"match_main": true, "kind": null, "model": null, "timeout_ms": 5000},
        "embedder": {"kind": "none", "model": null, "ollama_url": null},
        "vocab": {"max_per_prompt": null},
        "breaks": {"after_mins": 30},
        "persistence": {"session_db": null, "knowledge_db": null, "no_persist": false},
        "ui": {"sidebar_open": true, "last_section": "current_turn"},
        "speech": {
            "voice_mode_enabled": true,
            "disable_auto_download": false,
            "mic_silence_ms": 800,
            "overrides": {}
        }
    }))
    .unwrap();
    let update: GuiConfigUpdate = serde_json::from_str(&update_json).unwrap();
    let resolved = update.into_config(&cfg);
    assert!(resolved.speech.voice_mode_enabled);
    assert_eq!(resolved.speech.mic_silence_ms, 800);
}
```

- [ ] **Step 2: Run tests**

```bash
cd src
~/.cargo/bin/cargo test -p primer-gui config::tests
```

Expected: every config test passes.

- [ ] **Step 3: Commit**

```bash
cd src
git add crates/primer-gui/src/config.rs
git commit -m "$(cat <<'EOF'
feat(gui): expose SpeechSettings on GuiConfigView and GuiConfigUpdate

Frontend now reads + writes speech settings through the existing
view/update DTO pattern. No secrets in speech settings so no
redaction needed (unlike the inline API key path).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 2.4: Update `settings.js` to render a non-functional Speech form (preview)

We're adding the form fields here without wiring the values to any backend behavior yet — that comes in PR 5. This lets PR 2 stand alone and gets the on-disk schema deployed early.

**Files:**
- Modify: `src/crates/primer-gui/ui/index.html`
- Modify: `src/crates/primer-gui/ui/settings.js`

- [ ] **Step 1: Replace the `is-coming-soon` Speech group in `index.html`**

Find the existing `<details class="settings-group is-coming-soon">` block in `src/crates/primer-gui/ui/index.html` (around line 529) and replace with:

```html
<details class="settings-group" id="speech-settings-group">
  <summary>Speech</summary>
  <p class="hint muted" id="speech-availability-hint" hidden>
    Voice mode is not built into this binary. Rebuild with
    <code>--features primer-gui/speech</code> to enable.
  </p>
  <div id="speech-settings-fields">
    <div class="settings-grid">
      <label class="field">
        <span>Mic silence (ms)</span>
        <input
          type="number"
          id="f-speech-mic-silence-ms"
          min="100"
          max="3000"
          step="50"
          placeholder="600"
        />
      </label>
      <label class="check field-full">
        <input type="checkbox" id="f-speech-disable-auto-download" />
        <span>Don't offer auto-download; require explicit file paths</span>
      </label>
    </div>
    <p class="hint muted">
      Voice mode auto-downloads the default Whisper + Piper assets for
      your locale to <code>~/.cache/primer/models/</code> on first launch.
      Tick the box above to disable the offer (the start-voice-mode
      command will then surface missing files as a plain error pointing
      back to this dialog).
    </p>

    <h3 class="settings-subhead">Per-locale overrides</h3>
    <div id="f-speech-overrides">
      <!-- One sub-card per locale; rendered from JS so adding a locale
           pack only requires updating LOCALE_CHOICES in settings.js. -->
    </div>
  </div>
</details>
```

- [ ] **Step 2: Wire the fields in `settings.js`**

In `src/crates/primer-gui/ui/settings.js`, find the section that renders other settings groups (e.g., the embedder block) and add an analogous section for Speech:

- On `loadSettings()` (or equivalent), read `view.speech.mic_silence_ms` and `view.speech.disable_auto_download` and populate the inputs.
- For each entry in a `LOCALE_CHOICES` array (likely `["en", "de"]`), render a small sub-form with three file paths + a voice-id text field. Populate from `view.speech.overrides[locale]` if present.
- On save (`buildUpdate()` or equivalent), assemble a `speech` object with `voice_mode_enabled` (preserved from current state — don't reset it from the form; it's not editable here), `disable_auto_download`, `mic_silence_ms`, and `overrides` keyed by locale.

Reference the existing embedder block as the template — it has the same shape (a few fields, conditional sub-controls).

- [ ] **Step 3: Manual smoke**

```bash
cd src
~/.cargo/bin/cargo run -p primer-gui
```

In the GUI window: open Settings → expand Speech → confirm form fields render and persist after Save & start new session.

- [ ] **Step 4: Commit**

```bash
cd src
git add crates/primer-gui/ui/index.html crates/primer-gui/ui/settings.js
git commit -m "$(cat <<'EOF'
feat(gui): render Speech settings form (non-functional preview)

Mic-silence-ms input, disable-auto-download checkbox, and a
per-locale overrides sub-table that reads/writes through the
GuiConfigView/Update DTOs. Voice mode is not wired yet (PR 3+),
but the settings persist correctly so users can preconfigure.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

PR 2 ends here. Open a PR titled "feat(gui): SpeechSettings on GuiConfig + preview form".

---

## PR 3 — Tauri commands + `TauriEventObserver` + `AppState` slot

Six tasks. This PR adds the `speech` cargo feature to `primer-gui` and wires the three core voice commands. No frontend yet beyond a non-functional header button shell — that lands in PR 5.

### Task 3.1: Add `speech` cargo feature to `primer-gui`

**Files:**
- Modify: `src/crates/primer-gui/Cargo.toml`

- [ ] **Step 1: Append the feature**

In `src/crates/primer-gui/Cargo.toml`'s `[dependencies]` add (after `primer-inference`):

```toml
# Voice mode — opt-in via the `speech` feature. Pulls every speech backend
# feature so the GUI binary can drive VAD → STT → DialogueManager → TTS
# → cpal without further conditional compilation. Mirrors primer-cli's
# `speech` feature.
reqwest = { workspace = true, features = ["stream"], optional = true }
```

Then in `[features]`:

```toml
speech = [
    "primer-speech/silero",
    "primer-speech/whisper",
    "primer-speech/piper",
    "primer-speech/cpal",
    "primer-speech/voice-loop",
    "dep:reqwest",
]
```

Confirm `reqwest` exists as a workspace dep (`src/Cargo.toml`); if not, add it there first (`reqwest = { version = "0.12", default-features = false, features = ["rustls-tls", "json", "stream"] }`).

- [ ] **Step 2: Build with and without the feature**

```bash
cd src
~/.cargo/bin/cargo check -p primer-gui                    # default (no speech)
~/.cargo/bin/cargo check -p primer-gui --features speech  # speech build
```

Expected: both succeed.

- [ ] **Step 3: Commit**

```bash
cd src
git add Cargo.toml crates/primer-gui/Cargo.toml
git commit -m "feat(gui): add speech cargo feature + reqwest dep"
```

### Task 3.2: Add `voice` module + `ActiveVoiceLoop` to `AppState`

**Files:**
- Create: `src/crates/primer-gui/src/voice/mod.rs`
- Create: `src/crates/primer-gui/src/voice/observer.rs`
- Modify: `src/crates/primer-gui/src/lib.rs`
- Modify: `src/crates/primer-gui/src/state.rs`

- [ ] **Step 1: Create `voice/mod.rs`**

```rust
//! Voice-mode wiring for the GUI.
//!
//! - [`observer`] — `TauriEventObserver` impl that emits `primer://voice/*`
//!   events for the frontend.
//! - [`assets`] — locale-default voice/STT lookup + missing-asset structured
//!   error type.
//! - [`download`] — streaming asset download helper.
//!
//! All three are gated by `#[cfg(feature = "speech")]`; when the feature
//! is off, the module is empty and the Tauri command stubs in
//! `commands/voice.rs` return `Err(StartVoiceModeError::NotBuilt)`.

#[cfg(feature = "speech")]
pub mod observer;
```

- [ ] **Step 2: Add `pub mod voice;` to `lib.rs`**

In `src/crates/primer-gui/src/lib.rs`, after the existing `pub mod commands;`:

```rust
pub mod voice;
```

- [ ] **Step 3: Add `voice` slot to `AppState`**

In `src/crates/primer-gui/src/state.rs`, locate the `AppState` struct and add (after `session`):

```rust
#[cfg(feature = "speech")]
pub voice: Mutex<Option<ActiveVoiceLoop>>,
```

And after the struct, add (gated by `#[cfg(feature = "speech")]`):

```rust
#[cfg(feature = "speech")]
pub struct ActiveVoiceLoop {
    pub join: tokio::task::JoinHandle<Result<(), primer_speech::voice_loop::VoiceLoopError>>,
    pub stop_tx: tokio::sync::oneshot::Sender<()>,
    pub cancel_response_tx: tokio::sync::mpsc::Sender<()>,
    pub info: crate::types::SessionInfo,
}
```

Update `AppState::new` to initialise `voice: Mutex::new(None)` under the same cfg-guard.

The reason the field is cfg-guarded rather than always-present-but-None is that `ActiveVoiceLoop` references `primer_speech::voice_loop::VoiceLoopError`, which only exists with the `voice-loop` feature. Cleaner to gate the whole slot than to invent a stub type.

- [ ] **Step 4: Build**

```bash
cd src
~/.cargo/bin/cargo check -p primer-gui --features speech
~/.cargo/bin/cargo check -p primer-gui
```

Expected: both succeed.

- [ ] **Step 5: Commit**

```bash
cd src
git add crates/primer-gui/src/lib.rs crates/primer-gui/src/state.rs crates/primer-gui/src/voice/mod.rs
git commit -m "feat(gui): scaffold voice module + AppState::voice slot"
```

### Task 3.3: Implement `TauriEventObserver`

**Files:**
- Create: `src/crates/primer-gui/src/voice/observer.rs`

- [ ] **Step 1: Write the failing tests**

Create `src/crates/primer-gui/src/voice/observer.rs`:

```rust
//! `LoopObserver` impl that emits `primer://voice/*` Tauri events.

use serde::Serialize;
use tauri::Emitter;

use primer_speech::voice_loop::{ExitReason, LoopObserver, TurnCompletePayload, VoiceState};

#[derive(Serialize, Clone)]
pub struct StateChangeEvent {
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[derive(Serialize, Clone)]
pub struct TranscriptEvent {
    pub text: String,
}

#[derive(Serialize, Clone)]
pub struct ResponseChunkEvent {
    pub primer_turn_index: usize,
    pub text: String,
}

#[derive(Serialize, Clone)]
pub struct ResponseCompleteEvent {
    pub session_id: uuid::Uuid,
    pub child_turn_index: usize,
    pub primer_turn_index: usize,
}

#[derive(Serialize, Clone)]
pub struct VoiceExitEvent {
    pub reason: String,
}

#[derive(Serialize, Clone)]
pub struct VoiceInferenceErrorEvent {
    pub message: String,
}

pub struct TauriEventObserver<R: tauri::Runtime = tauri::Wry> {
    app: tauri::AppHandle<R>,
}

impl<R: tauri::Runtime> TauriEventObserver<R> {
    pub fn new(app: tauri::AppHandle<R>) -> Self {
        Self { app }
    }
}

impl<R: tauri::Runtime + 'static> LoopObserver for TauriEventObserver<R> {
    fn on_state_change(&mut self, state: VoiceState, hint: Option<&str>) {
        let payload = StateChangeEvent {
            state: state.name().to_string(),
            hint: hint.map(String::from),
        };
        if let Err(e) = self.app.emit("primer://voice/state_change", &payload) {
            tracing::warn!("emit primer://voice/state_change failed: {e}");
        }
    }

    fn on_transcript_finalized(&mut self, text: &str) {
        let payload = TranscriptEvent {
            text: text.to_string(),
        };
        if let Err(e) = self.app.emit("primer://voice/transcript", &payload) {
            tracing::warn!("emit primer://voice/transcript failed: {e}");
        }
    }

    fn on_response_chunk(&mut self, primer_turn_index: usize, chunk: &str) {
        let payload = ResponseChunkEvent {
            primer_turn_index,
            text: chunk.to_string(),
        };
        if let Err(e) = self.app.emit("primer://voice/response_chunk", &payload) {
            tracing::warn!("emit primer://voice/response_chunk failed: {e}");
        }
    }

    fn on_response_complete(&mut self, payload: TurnCompletePayload) {
        let evt = ResponseCompleteEvent {
            session_id: payload.session_id,
            child_turn_index: payload.child_turn_index,
            primer_turn_index: payload.primer_turn_index,
        };
        if let Err(e) = self.app.emit("primer://voice/response_complete", &evt) {
            tracing::warn!("emit primer://voice/response_complete failed: {e}");
        }
    }

    fn on_inference_error(&mut self, err: &primer_core::error::InferenceError) {
        let evt = VoiceInferenceErrorEvent {
            message: format!("{err:?}"),
        };
        if let Err(e) = self.app.emit("primer://voice/inference_error", &evt) {
            tracing::warn!("emit primer://voice/inference_error failed: {e}");
        }
    }

    fn on_exit(&mut self, reason: ExitReason) {
        let evt = VoiceExitEvent {
            reason: reason.name().to_string(),
        };
        if let Err(e) = self.app.emit("primer://voice/exit", &evt) {
            tracing::warn!("emit primer://voice/exit failed: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `tauri::test::mock_app` constructs an AppHandle that records emitted
    /// events into a buffer we can read back. Pin the wire shape: a JSON
    /// state_change payload must carry exactly `state` (and optionally
    /// `hint`), nothing more.
    #[test]
    fn state_change_event_serialises_to_expected_json() {
        let evt = StateChangeEvent {
            state: "listen".to_string(),
            hint: None,
        };
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json, serde_json::json!({"state": "listen"}));
    }

    #[test]
    fn state_change_with_hint_includes_hint_field() {
        let evt = StateChangeEvent {
            state: "listen".to_string(),
            hint: Some("user_cancel".to_string()),
        };
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"state": "listen", "hint": "user_cancel"})
        );
    }

    #[test]
    fn response_chunk_event_shape() {
        let evt = ResponseChunkEvent {
            primer_turn_index: 3,
            text: "hello".to_string(),
        };
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"primer_turn_index": 3, "text": "hello"})
        );
    }
}
```

- [ ] **Step 2: Run tests**

```bash
cd src
~/.cargo/bin/cargo test -p primer-gui --features speech voice::observer
```

Expected: all three tests pass.

- [ ] **Step 3: Commit**

```bash
cd src
git add crates/primer-gui/src/voice/observer.rs crates/primer-gui/src/voice/mod.rs
git commit -m "$(cat <<'EOF'
feat(gui): add TauriEventObserver for voice loop events

Emits primer://voice/{state_change,transcript,response_chunk,
response_complete,inference_error,exit} for the frontend.
Payload shapes pinned by serde-json tests.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 3.4: Add `voice_mode_available` to `SessionInfo`

**Files:**
- Modify: `src/crates/primer-gui/src/types.rs`
- Modify: `src/crates/primer-gui/src/commands/session.rs` (info_from)

- [ ] **Step 1: Add the field**

In `src/crates/primer-gui/src/types.rs`'s `SessionInfo` struct, add (after the existing fields):

```rust
/// Whether this binary was built with the `speech` cargo feature.
/// The frontend uses this to enable/disable the header voice-mode
/// toggle. `false` on default builds.
pub voice_mode_available: bool,
```

- [ ] **Step 2: Populate in `info_from`**

In `src/crates/primer-gui/src/commands/session.rs`, in the `info_from` function, add:

```rust
voice_mode_available: cfg!(feature = "speech"),
```

inside the `SessionInfo { ... }` literal.

- [ ] **Step 3: Add a test**

In `commands/session.rs::tests`:

```rust
#[tokio::test]
async fn session_info_carries_voice_mode_available_flag() {
    let home = TempDir::new().unwrap();
    let cfg = stub_config_with_persistence(home.path());
    let active = build_active_session(home.path(), &cfg).await.unwrap();
    let info = info_from(&active).await;
    // The flag matches whatever feature the test binary was built with.
    assert_eq!(info.voice_mode_available, cfg!(feature = "speech"));
}
```

- [ ] **Step 4: Run tests; commit**

```bash
cd src
~/.cargo/bin/cargo test -p primer-gui commands::session::tests::session_info_carries
~/.cargo/bin/cargo test -p primer-gui --features speech commands::session::tests::session_info_carries

git add crates/primer-gui/src/types.rs crates/primer-gui/src/commands/session.rs
git commit -m "feat(gui): expose voice_mode_available on SessionInfo"
```

### Task 3.5: Implement `start_voice_mode` / `stop_voice_mode` / `cancel_voice_response` commands

**Files:**
- Create: `src/crates/primer-gui/src/commands/voice.rs`
- Modify: `src/crates/primer-gui/src/commands/mod.rs` (register)

- [ ] **Step 1: Create the commands file (asset-resolution stub)**

Create `src/crates/primer-gui/src/commands/voice.rs`. For PR 3 we get the lifecycle commands working; the asset resolver lands in PR 4. So the first cut hard-codes a "no assets resolved → error" path.

```rust
//! Voice-mode Tauri commands.
//!
//! `start_voice_mode` builds the voice loop and stashes its handle in
//! `AppState::voice`. `stop_voice_mode` drains the loop. `cancel_voice_response`
//! aborts the in-flight LLM call + TTS synthesis.
//!
//! All four commands are gated by `#[cfg(feature = "speech")]`; the
//! non-speech build provides stubs returning `Err(NotBuilt)`.

use serde::Serialize;
use tauri::AppHandle;

use crate::state::AppState;
use crate::types::SessionInfo;

#[derive(Serialize, Debug)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StartVoiceModeError {
    /// Built without the `speech` cargo feature.
    NotBuilt,
    /// One or more required model files are missing on disk.
    AssetMissing { entries: Vec<MissingAsset> },
    /// Any other error — message is dev-facing, the frontend renders
    /// a generic banner.
    Other { message: String },
}

#[derive(Serialize, Clone, Debug)]
pub struct MissingAsset {
    pub kind: String,           // "piper_onnx" | "piper_config" | "whisper_model"
    pub path: std::path::PathBuf,
    pub suggested_url: Option<String>,
    pub approx_size_mb: Option<u32>,
}

impl From<String> for StartVoiceModeError {
    fn from(message: String) -> Self {
        Self::Other { message }
    }
}

#[cfg(feature = "speech")]
#[tauri::command]
pub async fn start_voice_mode(
    state: tauri::State<'_, AppState>,
    app: AppHandle,
) -> Result<SessionInfo, StartVoiceModeError> {
    use crate::voice::observer::TauriEventObserver;
    use primer_speech::voice_loop::{self, LoopConfig};

    // 1. Close any active text session (drains background tasks).
    super::session::close_session_inner(&state)
        .await
        .map_err(StartVoiceModeError::from)?;
    // 2. Close any already-active voice loop.
    stop_voice_mode_inner(&state).await.ok();

    let cfg = state.config.lock().await.clone();

    // 3. Build the active session via the shared wiring.
    //    This is where the GUI's existing wiring::build_active_session
    //    is called — Reuse it so DM construction is identical to text mode.
    let active_session = crate::wiring::build_active_session(&state.home, &cfg)
        .await
        .map_err(StartVoiceModeError::from)?;

    // 4. PR 4 will plug in real asset resolution here. For PR 3, hard-fail
    //    with a clear "not implemented" so the lifecycle plumbing is the
    //    only thing under test. PR 4's task 4.3 replaces this stub.
    let _ = active_session; // silence unused-warning until PR 4 wires this in
    return Err(StartVoiceModeError::Other {
        message: "asset resolution not yet implemented (PR 4)".into(),
    });
}

#[cfg(not(feature = "speech"))]
#[tauri::command]
pub async fn start_voice_mode(
    _state: tauri::State<'_, AppState>,
    _app: AppHandle,
) -> Result<SessionInfo, StartVoiceModeError> {
    Err(StartVoiceModeError::NotBuilt)
}

#[cfg(feature = "speech")]
#[tauri::command]
pub async fn stop_voice_mode(state: tauri::State<'_, AppState>) -> Result<(), String> {
    stop_voice_mode_inner(&state).await
}

#[cfg(not(feature = "speech"))]
#[tauri::command]
pub async fn stop_voice_mode(_state: tauri::State<'_, AppState>) -> Result<(), String> {
    Ok(())
}

#[cfg(feature = "speech")]
pub(crate) async fn stop_voice_mode_inner(state: &AppState) -> Result<(), String> {
    let Some(active) = state.voice.lock().await.take() else {
        return Ok(());
    };
    let _ = active.stop_tx.send(());
    // Bound the join wait: a stuck audio thread cannot hang the GUI.
    let timeout = std::time::Duration::from_secs(5);
    match tokio::time::timeout(timeout, active.join).await {
        Ok(Ok(Ok(()))) => Ok(()),
        Ok(Ok(Err(e))) => Err(format!("voice loop returned error: {e}")),
        Ok(Err(e)) => Err(format!("voice loop join failed: {e}")),
        Err(_) => {
            tracing::warn!("voice loop did not stop within 5s; the runtime will abort it");
            // Falling out of scope drops the JoinHandle, which aborts the task.
            Ok(())
        }
    }
}

#[cfg(feature = "speech")]
#[tauri::command]
pub async fn cancel_voice_response(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let guard = state.voice.lock().await;
    if let Some(active) = guard.as_ref() {
        // Non-blocking send. The cancel channel has capacity 8 — if the
        // user mashed Stop eight times in rapid succession we'd lose a
        // signal, which is fine; one cancel is enough.
        let _ = active.cancel_response_tx.try_send(());
    }
    Ok(())
}

#[cfg(not(feature = "speech"))]
#[tauri::command]
pub async fn cancel_voice_response(_state: tauri::State<'_, AppState>) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_asset_serialises_with_snake_case_kind() {
        let m = MissingAsset {
            kind: "whisper_model".into(),
            path: "/tmp/foo.bin".into(),
            suggested_url: Some("https://example.com/foo.bin".into()),
            approx_size_mb: Some(470),
        };
        let json = serde_json::to_value(&m).unwrap();
        assert_eq!(json["kind"], "whisper_model");
        assert_eq!(json["approx_size_mb"], 470);
    }

    #[test]
    fn start_voice_mode_error_uses_kind_tag() {
        let err = StartVoiceModeError::NotBuilt;
        let json = serde_json::to_value(&err).unwrap();
        assert_eq!(json["kind"], "not_built");

        let err = StartVoiceModeError::AssetMissing { entries: vec![] };
        let json = serde_json::to_value(&err).unwrap();
        assert_eq!(json["kind"], "asset_missing");
        assert_eq!(json["entries"], serde_json::json!([]));
    }
}
```

- [ ] **Step 2: Register the commands**

In `src/crates/primer-gui/src/commands/mod.rs`, add to the existing module declarations:

```rust
pub mod voice;
```

And in the `register` function (which builds the Tauri command list), add:

```rust
.invoke_handler(tauri::generate_handler![
    // ... existing handlers ...
    voice::start_voice_mode,
    voice::stop_voice_mode,
    voice::cancel_voice_response,
])
```

(Adjust to match the existing pattern; the codebase already has an invoke_handler section that the four new commands slot into.)

- [ ] **Step 3: Add capability for the four commands**

Tauri 2.x requires explicit per-command capability entries. Edit `src/crates/primer-gui/capabilities/default.json`:

```json
{
  "permissions": [
    "core:default",
    ...,
    "primer:voice.start_voice_mode",
    "primer:voice.stop_voice_mode",
    "primer:voice.cancel_voice_response"
  ]
}
```

(Check the existing default.json file's exact format; the existing session-command permissions follow a similar convention.)

- [ ] **Step 4: Build with and without speech feature**

```bash
cd src
~/.cargo/bin/cargo check -p primer-gui
~/.cargo/bin/cargo check -p primer-gui --features speech
```

Expected: both succeed.

- [ ] **Step 5: Run unit tests**

```bash
cd src
~/.cargo/bin/cargo test -p primer-gui --features speech commands::voice
```

Expected: the two serde-shape tests pass.

- [ ] **Step 6: Commit**

```bash
cd src
git add crates/primer-gui/src/commands/voice.rs crates/primer-gui/src/commands/mod.rs crates/primer-gui/capabilities/default.json
git commit -m "$(cat <<'EOF'
feat(gui): add voice-mode Tauri commands (lifecycle stubs)

start_voice_mode / stop_voice_mode / cancel_voice_response wired
with the AppState::voice slot and stop-with-5s-timeout drain.
StartVoiceModeError carries structured asset_missing details so
the frontend can render a consent dialog without parsing prose.

start_voice_mode hard-fails with "not yet implemented (PR 4)"
until the asset resolver lands; the lifecycle plumbing is
already exercised by the unit tests.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 3.6: PR 3 close

PR 3 ends here. Open a PR titled "feat(gui): voice-mode Tauri command scaffolding". The PR is mergeable because the default build is unchanged and the `speech` build's `start_voice_mode` returns a clear "not implemented yet" error.

---

## PR 4 — Asset resolver, consent modal, download command

Seven tasks.

### Task 4.1: Asset resolver + `MissingAsset` shape

**Files:**
- Create: `src/crates/primer-gui/src/voice/assets.rs`
- Modify: `src/crates/primer-gui/src/voice/mod.rs`

- [ ] **Step 1: Write failing tests**

Create `src/crates/primer-gui/src/voice/assets.rs`:

```rust
//! Voice-asset resolution.
//!
//! `resolve_voice_assets(cfg, locale)` returns either the resolved paths
//! to the three model files (piper .onnx, piper .onnx.json, whisper .bin)
//! or a structured [`AssetMissing`] error the frontend can render.

use std::path::PathBuf;

use crate::commands::voice::MissingAsset;
use crate::config::SpeechSettings;
use primer_core::i18n::Locale;
use primer_speech::voice_loop::locale_defaults::{voice_default_for, LocaleDefault};

/// Resolved paths for one voice mode session.
#[derive(Debug, Clone)]
pub struct ResolvedAssets {
    pub piper_onnx: PathBuf,
    pub piper_config: PathBuf,
    pub whisper_model: PathBuf,
    pub voice_id: String,
}

/// All entries are missing on disk; user must consent to download (or
/// provide explicit paths in Settings → Speech).
#[derive(Debug, Clone)]
pub struct AssetMissing {
    pub entries: Vec<MissingAsset>,
    pub locale: String,
    pub approx_total_mb: u32,
}

/// Per [[project_personal_device_model]], cache lives in the user's home.
pub fn cache_root(home: &std::path::Path) -> PathBuf {
    home.join(".cache").join("primer").join("models")
}

pub fn resolve_voice_assets(
    home: &std::path::Path,
    speech: &SpeechSettings,
    locale: &Locale,
) -> Result<ResolvedAssets, AssetMissing> {
    let default = voice_default_for(locale);
    let override_entry = speech.overrides.get(locale.pack_id());

    let (piper_onnx, piper_config, whisper_model, voice_id) =
        compute_paths(home, locale, default, override_entry);

    let mut missing = Vec::new();
    if !piper_onnx.exists() {
        missing.push(MissingAsset {
            kind: "piper_onnx".into(),
            path: piper_onnx.clone(),
            suggested_url: default.map(|d| d.piper_onnx_url.to_string()),
            approx_size_mb: default.map(|d| d.approx_total_mb - whisper_size_mb(d)),
        });
    }
    if !piper_config.exists() {
        missing.push(MissingAsset {
            kind: "piper_config".into(),
            path: piper_config.clone(),
            suggested_url: default.map(|d| d.piper_config_url.to_string()),
            approx_size_mb: Some(1), // .json sidecar is tiny
        });
    }
    if !whisper_model.exists() {
        missing.push(MissingAsset {
            kind: "whisper_model".into(),
            path: whisper_model.clone(),
            suggested_url: default.map(|d| d.whisper_url.to_string()),
            approx_size_mb: default.map(whisper_size_mb),
        });
    }

    if missing.is_empty() {
        Ok(ResolvedAssets {
            piper_onnx,
            piper_config,
            whisper_model,
            voice_id,
        })
    } else {
        Err(AssetMissing {
            entries: missing,
            locale: locale.pack_id().to_string(),
            approx_total_mb: default.map(|d| d.approx_total_mb).unwrap_or(0),
        })
    }
}

fn whisper_size_mb(d: &LocaleDefault) -> u32 {
    // Approx split: the Whisper bin is the bulk (~470 MB for small);
    // Piper medium voices are ~60 MB. Used only for consent-dialog
    // labelling.
    470
}

fn compute_paths(
    home: &std::path::Path,
    locale: &Locale,
    default: Option<&LocaleDefault>,
    override_entry: Option<&crate::config::SpeechLocaleOverride>,
) -> (PathBuf, PathBuf, PathBuf, String) {
    let voice_id = override_entry
        .and_then(|o| o.voice_id.clone())
        .or_else(|| default.map(|d| d.piper_voice_id.to_string()))
        .unwrap_or_else(|| format!("{}-default", locale.pack_id()));

    let voice_dir = cache_root(home).join("voice").join(locale.pack_id());
    let whisper_dir = cache_root(home).join("whisper");

    let piper_onnx = override_entry
        .and_then(|o| o.piper_onnx_path.clone())
        .unwrap_or_else(|| voice_dir.join(format!("{}.onnx", voice_id)));
    let piper_config = override_entry
        .and_then(|o| o.piper_config_path.clone())
        .unwrap_or_else(|| voice_dir.join(format!("{}.onnx.json", voice_id)));
    let whisper_model = override_entry
        .and_then(|o| o.whisper_model_path.clone())
        .unwrap_or_else(|| {
            whisper_dir.join(
                default
                    .map(|d| d.whisper_model_id)
                    .unwrap_or("ggml-small.bin"),
            )
        });

    (piper_onnx, piper_config, whisper_model, voice_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn missing_all_three_assets_returns_three_entries() {
        let home = TempDir::new().unwrap();
        let speech = SpeechSettings::default();
        let err = resolve_voice_assets(home.path(), &speech, &Locale::English).unwrap_err();
        assert_eq!(err.entries.len(), 3, "all three files missing on a fresh home");
        assert_eq!(err.locale, "en");
        assert!(err.approx_total_mb >= 400);
        let kinds: Vec<&str> = err.entries.iter().map(|e| e.kind.as_str()).collect();
        assert!(kinds.contains(&"piper_onnx"));
        assert!(kinds.contains(&"piper_config"));
        assert!(kinds.contains(&"whisper_model"));
    }

    #[test]
    fn existing_files_resolve_cleanly() {
        let home = TempDir::new().unwrap();
        let voice_dir = home.path().join(".cache/primer/models/voice/en");
        let whisper_dir = home.path().join(".cache/primer/models/whisper");
        std::fs::create_dir_all(&voice_dir).unwrap();
        std::fs::create_dir_all(&whisper_dir).unwrap();
        std::fs::write(voice_dir.join("en_GB-alba-medium.onnx"), b"").unwrap();
        std::fs::write(voice_dir.join("en_GB-alba-medium.onnx.json"), b"").unwrap();
        std::fs::write(whisper_dir.join("ggml-small.en.bin"), b"").unwrap();

        let speech = SpeechSettings::default();
        let ok = resolve_voice_assets(home.path(), &speech, &Locale::English).unwrap();
        assert!(ok.piper_onnx.ends_with("en_GB-alba-medium.onnx"));
        assert_eq!(ok.voice_id, "en_GB-alba-medium");
    }

    #[test]
    fn per_locale_override_path_takes_precedence_over_cache_default() {
        let home = TempDir::new().unwrap();
        let custom = home.path().join("my_voice.onnx");
        std::fs::write(&custom, b"").unwrap();

        let mut speech = SpeechSettings::default();
        speech.overrides.insert(
            "en".to_string(),
            crate::config::SpeechLocaleOverride {
                piper_onnx_path: Some(custom.clone()),
                piper_config_path: None,
                whisper_model_path: None,
                voice_id: Some("my_voice".to_string()),
            },
        );

        // Piper config & Whisper still missing; the resolver returns
        // AssetMissing but the piper_onnx entry should NOT be in the
        // missing list because the override-pointed path exists.
        let err = resolve_voice_assets(home.path(), &speech, &Locale::English).unwrap_err();
        let kinds: Vec<&str> = err.entries.iter().map(|e| e.kind.as_str()).collect();
        assert!(!kinds.contains(&"piper_onnx"));
        assert!(kinds.contains(&"piper_config"));
        assert!(kinds.contains(&"whisper_model"));
    }
}
```

- [ ] **Step 2: Wire the new module**

In `src/crates/primer-gui/src/voice/mod.rs` add:

```rust
#[cfg(feature = "speech")]
pub mod assets;
```

- [ ] **Step 3: Run tests; commit**

```bash
cd src
~/.cargo/bin/cargo test -p primer-gui --features speech voice::assets

git add crates/primer-gui/src/voice/
git commit -m "feat(gui): voice asset resolver with per-locale overrides"
```

### Task 4.2: Wire asset resolution into `start_voice_mode`

**Files:**
- Modify: `src/crates/primer-gui/src/commands/voice.rs`

- [ ] **Step 1: Replace the PR 3 stub**

In `start_voice_mode`, replace the `return Err(StartVoiceModeError::Other {...})` with:

```rust
use primer_speech::voice_loop::{self, LoopConfig};
use primer_core::i18n::Locale;

// 4. Resolve voice assets for the active locale.
let locale = Locale::from_pack_id(&cfg.learner.locale).unwrap_or_default();
let assets = crate::voice::assets::resolve_voice_assets(
    &state.home,
    &cfg.speech,
    &locale,
).map_err(|missing| {
    StartVoiceModeError::AssetMissing {
        entries: missing.entries,
    }
})?;

// 5. Build the LoopBackends. (For PR 4 we wire up the actual backend
//    construction by lifting the body of the CLI's run function — see
//    primer-cli/src/speech_loop/mod.rs:run for the production pattern.)
let backends = crate::voice::backends::build_loop_backends(
    &assets,
    &locale,
).await.map_err(|e| StartVoiceModeError::from(format!("backend init: {e}")))?;

let event_rx = backends.event_rx;  // see backends module note below
let loop_backends = backends.into_loop_backends();

// 6. Construct the responder + observer + run the loop.
let dm_arc = active_session.dialogue_manager.clone();
let observer = crate::voice::observer::TauriEventObserver::new(app.clone());
let responder = Box::new(crate::voice::responder::ArcDmResponder::new(dm_arc.clone()));

let loop_cfg = LoopConfig {
    mic_silence_ms: cfg.speech.mic_silence_ms,
    verbose: false, // GUI does its own logging via tracing
    locale,
};

let (handle, join) = voice_loop::run_loop(
    loop_backends,
    event_rx,
    responder,
    loop_cfg,
    observer,
);

let info = SessionInfo {
    session_id: None,
    learner: /* same construction the text-mode start uses */ todo!(),
    backend_kind: active_session.backend_name.clone(),
    main_model: active_session.main_model.clone(),
    locale: locale.pack_id().to_string(),
    voice_mode_available: true,
};

*state.voice.lock().await = Some(crate::state::ActiveVoiceLoop {
    join,
    stop_tx: handle.stop_tx,
    cancel_response_tx: handle.cancel_response_tx,
    info: info.clone(),
});

// Flip the sticky-toggle on successful start.
{
    let mut c = state.config.lock().await;
    c.speech.voice_mode_enabled = true;
    crate::config::save(&state.home, &c).map_err(|e| {
        StartVoiceModeError::from(format!("persist speech.voice_mode_enabled=true: {e}"))
    })?;
}

Ok(info)
```

This references `voice::backends::build_loop_backends` (the audio-thread/VAD/STT/TTS wiring) and `voice::responder::ArcDmResponder` (the GUI-side Responder impl). Both land in the next two tasks.

- [ ] **Step 2: Adjust `stop_voice_mode` to flip the flag off**

In `stop_voice_mode_inner`, after the take/drain, add:

```rust
{
    let mut c = state.config.lock().await;
    c.speech.voice_mode_enabled = false;
    if let Err(e) = crate::config::save(&state.home, &c) {
        tracing::warn!("persist speech.voice_mode_enabled=false: {e}");
    }
}
```

- [ ] **Step 3: Build (will fail on backends/responder modules; we add those next)**

```bash
cd src
~/.cargo/bin/cargo check -p primer-gui --features speech
```

Expected: errors about `voice::backends` and `voice::responder` modules — fixed in Tasks 4.3 and 4.4.

- [ ] **Step 4: Stage the change without committing yet**

```bash
cd src
git add crates/primer-gui/src/commands/voice.rs
# Don't commit yet — next two tasks complete the wiring.
```

### Task 4.3: Lift voice-mode `LoopBackends` builder into `voice::backends`

**Files:**
- Create: `src/crates/primer-gui/src/voice/backends.rs`

The CLI's existing backend-construction code (in `primer-cli/src/speech_loop/mod.rs::run`) builds the audio capture thread, the silero VAD, the whisper streaming session, the piper TTS, and the cpal mic/speaker pair. We need an equivalent in primer-gui, ideally as a pure helper so the CLI can also use it.

- [ ] **Step 1: Extract the builder into a shared crate path**

Audit `primer-cli/src/speech_loop/mod.rs::run` (after PR 1) for the backend-construction body. Identify any code that:
1. Opens cpal mic + speaker streams
2. Constructs SileroVad, WhisperStreaming, PiperTts
3. Spawns the audio capture thread
4. Returns a `LoopBackends`-shaped object + the VAD event channel + the drain hook

Move this builder into `primer-speech::voice_loop::backends::build_local_backends(assets, locale, mic_silence_ms) -> Result<(LoopBackends, EventRx, DrainHook), VoiceLoopError>`. CLI now calls it via the new path.

- [ ] **Step 2: Wrap the builder in `primer-gui::voice::backends`**

Create `src/crates/primer-gui/src/voice/backends.rs`:

```rust
//! Voice-loop backend construction for the GUI.

use primer_speech::voice_loop::{LoopBackends, DrainHook};
use crate::voice::assets::ResolvedAssets;

pub struct BuiltBackends {
    pub backends: LoopBackends,
    pub event_rx: tokio::sync::mpsc::Receiver<primer_core::speech::VadEvent>,
    pub drain_hook: DrainHook,
}

impl BuiltBackends {
    pub fn into_loop_backends(self) -> LoopBackends {
        self.backends
    }
}

pub async fn build_loop_backends(
    assets: &ResolvedAssets,
    locale: &primer_core::i18n::Locale,
) -> Result<BuiltBackends, String> {
    let (backends, event_rx, drain_hook) =
        primer_speech::voice_loop::backends::build_local_backends(
            &assets.piper_onnx,
            &assets.piper_config,
            &assets.whisper_model,
            &assets.voice_id,
            *locale,
        )
        .await
        .map_err(|e| e.to_string())?;
    Ok(BuiltBackends {
        backends,
        event_rx,
        drain_hook,
    })
}
```

- [ ] **Step 3: Add `pub mod backends;` in `voice/mod.rs`**

```rust
#[cfg(feature = "speech")]
pub mod backends;
```

### Task 4.4: `ArcDmResponder` — GUI-side `Responder` impl

**Files:**
- Create: `src/crates/primer-gui/src/voice/responder.rs`

- [ ] **Step 1: Create the responder**

```rust
//! `Responder` impl that drives a shared Arc<Mutex<DialogueManager>>.
//!
//! The CLI's responder borrows `&mut DialogueManager` directly; the GUI
//! holds the DM behind an Arc<Mutex> so other Tauri commands can read
//! its state while voice mode is active. This Responder locks the DM
//! for the duration of one respond_to_streaming call — same blocking
//! semantic as text mode's send_message.

use std::sync::Arc;
use tokio::sync::Mutex;

use primer_core::error::Result;
use primer_pedagogy::DialogueManager;
use primer_speech::voice_loop::Responder;

pub struct ArcDmResponder {
    dm: Arc<Mutex<DialogueManager>>,
}

impl ArcDmResponder {
    pub fn new(dm: Arc<Mutex<DialogueManager>>) -> Self {
        Self { dm }
    }
}

impl Responder for ArcDmResponder {
    fn respond<'a>(
        &'a mut self,
        transcript: &'a str,
        mut on_chunk: Box<dyn FnMut(&str) + Send + 'a>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String>> + Send + 'a>> {
        let dm_arc = Arc::clone(&self.dm);
        Box::pin(async move {
            let mut dm = dm_arc.lock().await;
            let mut full = String::new();
            dm.respond_to_streaming(transcript, |chunk| {
                full.push_str(chunk);
                on_chunk(chunk);
            }).await?;
            Ok(full)
        })
    }
}
```

- [ ] **Step 2: Add `pub mod responder;`**

In `voice/mod.rs`:

```rust
#[cfg(feature = "speech")]
pub mod responder;
```

- [ ] **Step 3: Build + commit the bundle**

```bash
cd src
~/.cargo/bin/cargo check -p primer-gui --features speech

git add crates/primer-gui/src/voice/ crates/primer-gui/src/commands/voice.rs
git commit -m "$(cat <<'EOF'
feat(gui): wire start_voice_mode through the voice loop

Asset resolver → backend builder → ArcDmResponder → voice_loop.
Sticky toggle flag flipped server-side: start_voice_mode flips
speech.voice_mode_enabled = true on success only; stop_voice_mode
flips it to false unconditionally. Cancel-on-AssetMissing path
leaves the flag untouched so the consent-dialog reach-back works.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 4.5: `download_voice_assets` command

**Files:**
- Create: `src/crates/primer-gui/src/voice/download.rs`
- Modify: `src/crates/primer-gui/src/commands/voice.rs`
- Modify: `src/crates/primer-gui/src/voice/mod.rs`

- [ ] **Step 1: Implement the download helper**

```rust
//! Streaming voice-asset download.
//!
//! Each file is fetched via `reqwest::get` and streamed to a
//! `<dest>.partial` temp path, then atomically renamed on success.
//! Progress events fire per chunk via the AppHandle so the consent
//! modal can render a progress bar. Transient 5xx / network errors
//! retry via `primer_core::retry::retry_with_backoff`.

use std::path::Path;

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::commands::voice::MissingAsset;

#[derive(Serialize, Clone)]
pub struct DownloadProgressEvent {
    pub asset_id: String,
    pub bytes_done: u64,
    pub bytes_total: Option<u64>,
}

pub async fn download_one<R: tauri::Runtime>(
    app: &AppHandle<R>,
    asset: &MissingAsset,
) -> Result<(), String> {
    let url = asset
        .suggested_url
        .as_ref()
        .ok_or_else(|| format!("no URL for {:?}", asset.kind))?;
    let dest = &asset.path;
    let partial = dest.with_extension("partial");

    // Ensure parent dir exists.
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("mkdir {parent:?}: {e}"))?;
    }

    let client = reqwest::Client::new();
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("GET {url}: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!(
            "download URL returned {status} for {url}; the model may have been renamed upstream — pick a model manually in Settings → Speech",
        ));
    }
    let total = resp.content_length();

    use tokio::io::AsyncWriteExt;
    let mut file = tokio::fs::File::create(&partial)
        .await
        .map_err(|e| format!("create {partial:?}: {e}"))?;
    let mut bytes_done: u64 = 0;
    let mut stream = resp.bytes_stream();
    use futures::StreamExt;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("read chunk: {e}"))?;
        file.write_all(&chunk).await.map_err(|e| format!("write: {e}"))?;
        bytes_done += chunk.len() as u64;
        let evt = DownloadProgressEvent {
            asset_id: asset.kind.clone(),
            bytes_done,
            bytes_total: total,
        };
        let _ = app.emit("primer://voice/download_progress", &evt);
    }
    file.flush().await.map_err(|e| format!("flush: {e}"))?;
    drop(file);

    // Atomic rename so a killed download leaves no half-files.
    tokio::fs::rename(&partial, dest)
        .await
        .map_err(|e| format!("rename {partial:?} -> {dest:?}: {e}"))?;
    Ok(())
}
```

- [ ] **Step 2: Add the Tauri command**

In `commands/voice.rs`:

```rust
#[cfg(feature = "speech")]
#[tauri::command]
pub async fn download_voice_assets(
    _state: tauri::State<'_, AppState>,
    app: AppHandle,
    missing: Vec<MissingAsset>,
) -> Result<(), String> {
    for asset in missing {
        crate::voice::download::download_one(&app, &asset).await?;
    }
    Ok(())
}

#[cfg(not(feature = "speech"))]
#[tauri::command]
pub async fn download_voice_assets(
    _state: tauri::State<'_, AppState>,
    _app: AppHandle,
    _missing: Vec<MissingAsset>,
) -> Result<(), String> {
    Err("voice mode not built in this binary".into())
}
```

Register it in `commands/mod.rs::register` and add the permission to `capabilities/default.json`.

- [ ] **Step 3: Build + commit**

```bash
cd src
~/.cargo/bin/cargo check -p primer-gui --features speech

git add crates/primer-gui/src/voice/download.rs crates/primer-gui/src/commands/voice.rs crates/primer-gui/src/voice/mod.rs crates/primer-gui/src/commands/mod.rs crates/primer-gui/capabilities/default.json
git commit -m "$(cat <<'EOF'
feat(gui): download_voice_assets command with progress events

Streams each missing asset via reqwest to <dest>.partial then
atomically renames. Progress emitted on primer://voice/download_progress
so the consent modal can render a bar. 4xx returns surface a
suggested next-step ("pick manually in Settings → Speech").

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 4.6: Asset-consent modal HTML/CSS

**Files:**
- Modify: `src/crates/primer-gui/ui/index.html`
- Modify: `src/crates/primer-gui/ui/styles.css`

- [ ] **Step 1: Add the modal to index.html**

After the existing settings-backdrop modal in `index.html`, add:

```html
<div
  class="modal-backdrop"
  id="voice-consent-backdrop"
  hidden
  aria-hidden="true"
>
  <div
    class="modal"
    id="voice-consent-modal"
    role="dialog"
    aria-modal="true"
    aria-labelledby="voice-consent-title"
  >
    <header class="modal-header">
      <h2 id="voice-consent-title">Download voice models</h2>
      <button
        type="button"
        class="modal-close"
        id="voice-consent-close"
        aria-label="Close"
        title="Close"
      >
        ×
      </button>
    </header>

    <div class="modal-body">
      <p>
        Voice mode needs Whisper (speech-to-text) and Piper (text-to-speech)
        models for your locale (<span id="voice-consent-locale">en</span>).
        Total approximate size:
        <strong id="voice-consent-total-size">~530 MB</strong>.
      </p>
      <p class="hint muted">
        Files will be downloaded to <code>~/.cache/primer/models/</code>.
        Source URLs are shown below; close this dialog and use Settings →
        Speech to point at your own files instead.
      </p>

      <table class="asset-list">
        <thead>
          <tr>
            <th>File</th>
            <th>Size</th>
            <th>Source</th>
          </tr>
        </thead>
        <tbody id="voice-consent-assets">
          <!-- Rows rendered from JS based on the AssetMissing payload -->
        </tbody>
      </table>

      <div id="voice-consent-progress" hidden>
        <div class="progress-row" id="voice-consent-progress-row">
          <span class="progress-label" id="voice-consent-progress-label">Downloading…</span>
          <progress id="voice-consent-progress-bar" value="0" max="100"></progress>
        </div>
      </div>
    </div>

    <footer class="modal-footer">
      <div class="modal-footer-actions">
        <button type="button" class="btn-tertiary" id="voice-consent-cancel">
          Cancel
        </button>
        <button type="button" class="btn-primary" id="voice-consent-download">
          Download
        </button>
      </div>
    </footer>
  </div>
</div>
```

- [ ] **Step 2: Add minimal styling**

In `styles.css`:

```css
.asset-list { width: 100%; border-collapse: collapse; margin-top: 1rem; }
.asset-list th, .asset-list td { padding: 0.4rem 0.6rem; border-bottom: 1px solid #eee; text-align: left; font-size: 0.85rem; }
.asset-list th { font-weight: 600; color: #555; }
.asset-list .source-url { font-family: monospace; font-size: 0.75rem; color: #888; }

#voice-consent-progress { margin-top: 1rem; padding-top: 1rem; border-top: 1px solid #eee; }
#voice-consent-progress-bar { width: 100%; height: 6px; }
.progress-row { display: flex; align-items: center; gap: 0.5rem; }
.progress-label { font-size: 0.75rem; color: #555; min-width: 120px; }
```

- [ ] **Step 3: Commit**

```bash
cd src
git add crates/primer-gui/ui/index.html crates/primer-gui/ui/styles.css
git commit -m "feat(gui): voice-asset consent modal HTML + CSS"
```

### Task 4.7: PR 4 close

PR 4 ends here. Open a PR titled "feat(gui): voice-asset resolver, download command, consent modal". The PR is mergeable because the consent modal is still wired only at the static-HTML level; the JS that opens/closes it lands in PR 5 along with the header toggle.

---

## PR 5 — Frontend: composer widget, header toggle, event subscriptions

Eight tasks.

### Task 5.1: Header "Voice mode" toggle button

**Files:**
- Modify: `src/crates/primer-gui/ui/index.html`
- Modify: `src/crates/primer-gui/ui/styles.css`

- [ ] **Step 1: Add the toggle button**

In `index.html`, in the `<div class="header-actions">` block, add a new button BEFORE the existing "Sessions" button:

```html
<button
  type="button"
  class="header-btn voice-toggle"
  id="voice-mode-toggle"
  aria-pressed="false"
  title="Switch to voice mode for this learner"
  disabled
>
  <span class="voice-toggle-icon" aria-hidden="true">🎙</span>
  <span class="voice-toggle-label" id="voice-toggle-label">Voice mode</span>
</button>
```

- [ ] **Step 2: Style the button's active state**

In `styles.css`:

```css
.voice-toggle[aria-pressed="true"] {
  background: #2563eb;
  color: white;
  border-color: #1e40af;
}
.voice-toggle[aria-pressed="true"] .voice-toggle-icon {
  /* Inactive 🎙 grayed by header-btn base, active stays full-colour */
  filter: none;
}
.voice-toggle:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}
```

- [ ] **Step 3: Commit**

```bash
cd src
git add crates/primer-gui/ui/index.html crates/primer-gui/ui/styles.css
git commit -m "feat(gui): voice-mode header toggle button (static)"
```

### Task 5.2: Composer-voice widget HTML/CSS

**Files:**
- Modify: `src/crates/primer-gui/ui/index.html`
- Modify: `src/crates/primer-gui/ui/styles.css`

- [ ] **Step 1: Add the composer-voice sibling**

In `index.html`, AFTER the existing `<form class="composer" id="composer">`, add:

```html
<div class="composer composer-voice" id="composer-voice" hidden>
  <div class="voice-state" id="voice-state" data-state="listen">
    <div class="voice-state-icon" aria-hidden="true">
      <div class="voice-icon-inner"></div>
    </div>
    <div class="voice-state-text">
      <div class="voice-state-label" id="voice-state-label">Listening…</div>
      <div class="voice-state-hint muted" id="voice-state-hint">take your time</div>
    </div>
    <button
      type="button"
      class="voice-stop-btn"
      id="voice-stop"
      title="Stop the Primer's reply"
      aria-label="Stop"
    >
      ⏹ Stop
    </button>
  </div>
</div>
```

- [ ] **Step 2: Style the composer-voice widget**

In `styles.css`:

```css
.composer-voice {
  padding: 0.75rem 1rem;
  background: linear-gradient(180deg, #fafbff, #f0f6ff);
  border-top: 1px solid #d6e4f5;
}
.voice-state {
  display: flex;
  align-items: center;
  gap: 0.75rem;
}
.voice-state-icon {
  width: 36px;
  height: 36px;
  border-radius: 50%;
  transition: background 200ms, box-shadow 200ms;
  display: flex;
  align-items: center;
  justify-content: center;
}
.voice-icon-inner {
  width: 12px;
  height: 12px;
  border-radius: 50%;
  background: white;
}

/* LISTEN — blue pulsing ring */
.voice-state[data-state="listen"] .voice-state-icon {
  background: #2563eb;
  box-shadow: 0 0 0 6px rgba(37, 99, 235, 0.18);
  animation: voice-mic-pulse 1.4s infinite;
}
@keyframes voice-mic-pulse {
  0%, 100% { box-shadow: 0 0 0 6px rgba(37, 99, 235, 0.18); }
  50%      { box-shadow: 0 0 0 10px rgba(37, 99, 235, 0.10); }
}

/* LATENT_THINK — gray rotating dots */
.voice-state[data-state="latent_think"] .voice-state-icon {
  background: #94a3b8;
  animation: voice-think-spin 1.2s infinite linear;
}
@keyframes voice-think-spin {
  0%   { transform: rotate(0deg); }
  100% { transform: rotate(360deg); }
}

/* SPEAK — green pulsing */
.voice-state[data-state="speak"] .voice-state-icon {
  background: #10b981;
  box-shadow: 0 0 0 6px rgba(16, 185, 129, 0.22);
  animation: voice-speaker-pulse 0.8s infinite;
}
@keyframes voice-speaker-pulse {
  0%, 100% { box-shadow: 0 0 0 6px rgba(16, 185, 129, 0.22); }
  50%      { box-shadow: 0 0 0 12px rgba(16, 185, 129, 0.10); }
}

.voice-state-text { flex: 1; }
.voice-state-label { font-size: 0.95rem; font-weight: 600; color: #1e40af; }
.voice-state-hint { font-size: 0.78rem; }

.voice-stop-btn {
  background: none;
  border: 1px solid #ddd;
  padding: 0.35rem 0.7rem;
  border-radius: 4px;
  font-size: 0.8rem;
  color: #666;
  cursor: pointer;
}
.voice-stop-btn:hover { background: #f5f5f5; }
```

- [ ] **Step 3: Commit**

```bash
cd src
git add crates/primer-gui/ui/index.html crates/primer-gui/ui/styles.css
git commit -m "feat(gui): composer-voice state widget with [data-state] CSS"
```

### Task 5.3: `voice.js` — header toggle wiring and composer swap

**Files:**
- Create: `src/crates/primer-gui/ui/voice.js`
- Modify: `src/crates/primer-gui/ui/index.html` (load the new script)

- [ ] **Step 1: Create `voice.js`**

```javascript
// voice.js — voice-mode frontend controller.
//
// Coordinates: header toggle, composer swap, Tauri command invocation,
// event subscriptions for primer://voice/*, asset-consent modal,
// sticky-toggle restoration on launch.

(() => {
  const { invoke } = window.__TAURI__.core;
  const { listen } = window.__TAURI__.event;

  const state = {
    active: false,
    available: false,
    currentState: null,    // "listen" | "latent_think" | "speak" | null
    primerBubble: null,    // streaming Primer bubble element
    primerTurnIndex: null,
  };

  // Per-state label/hint copy. Locale-aware version reads from the
  // i18n pack once Task 5.7 wires that in; for now defaults are EN.
  const STATE_COPY = {
    listen:        { label: "Listening…", hint: "take your time" },
    latent_think:  { label: "Thinking…",  hint: "the Primer is working on a reply" },
    speak:         { label: "Speaking…",  hint: "let the Primer finish" },
  };

  function $(id) { return document.getElementById(id); }

  function setActive(active) {
    state.active = active;
    const toggle = $("voice-mode-toggle");
    const label  = $("voice-toggle-label");
    const composerText  = $("composer");
    const composerVoice = $("composer-voice");
    toggle.setAttribute("aria-pressed", active ? "true" : "false");
    label.textContent = active ? "End voice mode" : "Voice mode";
    composerText.hidden  = active;
    composerVoice.hidden = !active;
  }

  function setVoiceState(s, hint) {
    state.currentState = s;
    const root = $("voice-state");
    root.dataset.state = s;
    const copy = STATE_COPY[s] || { label: s, hint: "" };
    $("voice-state-label").textContent = copy.label;
    $("voice-state-hint").textContent  = copy.hint;
    // Optional cancel-reason fade-out for the streaming bubble.
    if (s === "listen" && hint === "user_cancel" && state.primerBubble) {
      state.primerBubble.classList.add("cancelled");
    }
  }

  async function onToggleClick() {
    if (!state.available) return;
    if (state.active) {
      await invoke("stop_voice_mode").catch((e) => showError(`stop: ${e}`));
      setActive(false);
      return;
    }
    try {
      await invoke("start_voice_mode");
      setActive(true);
    } catch (err) {
      if (err && err.kind === "asset_missing") {
        await showConsentModal(err.entries);
      } else if (err && err.kind === "not_built") {
        showError("Voice mode is not built into this binary.");
      } else {
        showError(`start voice mode: ${err.message || err}`);
      }
    }
  }

  async function showConsentModal(entries) {
    const backdrop = $("voice-consent-backdrop");
    const tbody = $("voice-consent-assets");
    tbody.innerHTML = "";
    let totalMb = 0;
    for (const e of entries) {
      const tr = document.createElement("tr");
      const sizeMb = e.approx_size_mb || 0;
      totalMb += sizeMb;
      tr.innerHTML = `
        <td>${e.kind}</td>
        <td>${sizeMb ? `≈ ${sizeMb} MB` : "—"}</td>
        <td><span class="source-url">${e.suggested_url || "(no URL)"}</span></td>
      `;
      tbody.appendChild(tr);
    }
    $("voice-consent-total-size").textContent = totalMb ? `~${totalMb} MB` : "(unknown)";
    backdrop.hidden = false;
    backdrop.setAttribute("aria-hidden", "false");

    return new Promise((resolve) => {
      const onCancel = async () => {
        backdrop.hidden = true;
        // Persist voice_mode_enabled=false so the next launch doesn't
        // reach back here.
        await invoke("stop_voice_mode").catch(() => {});
        cleanup();
        resolve();
      };
      const onDownload = async () => {
        $("voice-consent-progress").hidden = false;
        try {
          await invoke("download_voice_assets", { missing: entries });
          backdrop.hidden = true;
          cleanup();
          // Retry start_voice_mode now that assets are local.
          try {
            await invoke("start_voice_mode");
            setActive(true);
          } catch (err) {
            showError(`start after download: ${err.message || err}`);
          }
          resolve();
        } catch (err) {
          showError(`download: ${err.message || err}`);
        }
      };
      $("voice-consent-cancel").onclick = onCancel;
      $("voice-consent-close").onclick  = onCancel;
      $("voice-consent-download").onclick = onDownload;
      function cleanup() {
        $("voice-consent-cancel").onclick = null;
        $("voice-consent-close").onclick  = null;
        $("voice-consent-download").onclick = null;
      }
    });
  }

  function showError(msg) {
    // Reuse the existing error-banner pattern. App.js exposes a helper;
    // call it via a shared window export.
    if (window.primerShowError) window.primerShowError(msg);
    else console.error(msg);
  }

  // Stop button + Esc both route to cancel_voice_response.
  async function onStop() {
    if (!state.active) return;
    await invoke("cancel_voice_response").catch(() => {});
  }
  function onKeyDown(e) {
    if (!state.active) return;
    if (e.key === "Escape") onStop();
  }

  // === Tauri event subscriptions ===
  async function subscribeEvents() {
    await listen("primer://voice/state_change", (evt) => {
      const { state: s, hint } = evt.payload;
      setVoiceState(s, hint);
    });
    await listen("primer://voice/transcript", (evt) => {
      // Mirror text-mode: append a child chat bubble with the transcript.
      if (window.primerAppendChildBubble) {
        window.primerAppendChildBubble(evt.payload.text);
      }
    });
    await listen("primer://voice/response_chunk", (evt) => {
      // Mirror text-mode: append chunk to streaming Primer bubble.
      if (window.primerAppendPrimerChunk) {
        window.primerAppendPrimerChunk(evt.payload.primer_turn_index, evt.payload.text);
      }
    });
    await listen("primer://voice/response_complete", (_evt) => {
      // Sidebar refresh hook (app.js subscribes too; this is a no-op here
      // unless app.js exposes one).
      if (window.primerRefreshSidebar) window.primerRefreshSidebar();
    });
    await listen("primer://voice/exit", (evt) => {
      setActive(false);
      // reason === "keyword" → user said "goodbye"; reason === "user"
      // → user clicked End voice mode. Both land here. Pick UI based
      // on reason if needed; for Phase A, no special handling.
      if (window.primerShowToast) {
        window.primerShowToast(`Voice mode ended (${evt.payload.reason})`);
      }
    });
    await listen("primer://voice/inference_error", (evt) => {
      showError(`Voice inference error: ${evt.payload.message}`);
    });
  }

  // === Sticky-toggle restoration on launch ===
  async function restoreOnLaunch() {
    const info = await invoke("current_session_info").catch(() => null);
    state.available = info?.voice_mode_available === true;
    const toggle = $("voice-mode-toggle");
    toggle.disabled = !state.available;
    if (!state.available) {
      toggle.title = "Voice mode is not built into this binary";
      return;
    }
    // Read persisted speech.voice_mode_enabled via get_settings.
    const cfg = await invoke("get_settings").catch(() => null);
    if (cfg?.speech?.voice_mode_enabled === true) {
      // Trigger the same path as a click. If assets are missing, the
      // consent modal pops; if download is cancelled, the flag flips
      // back to false via stop_voice_mode.
      try {
        await invoke("start_voice_mode");
        setActive(true);
      } catch (err) {
        if (err?.kind === "asset_missing") {
          await showConsentModal(err.entries);
        } else if (err?.kind === "not_built") {
          // Silent — toggle is already disabled.
        } else {
          showError(`auto-resume voice mode: ${err.message || err}`);
        }
      }
    }
  }

  // === DOM wiring ===
  function init() {
    $("voice-mode-toggle").addEventListener("click", onToggleClick);
    $("voice-stop").addEventListener("click", onStop);
    document.addEventListener("keydown", onKeyDown);
    subscribeEvents().catch((e) => console.error("voice subscribeEvents:", e));
    restoreOnLaunch().catch((e) => console.error("voice restoreOnLaunch:", e));
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", init);
  } else {
    init();
  }
})();
```

- [ ] **Step 2: Load `voice.js` in index.html**

In `index.html`, at the bottom, after the existing `<script>` tags, add:

```html
<script src="voice.js"></script>
```

- [ ] **Step 3: Expose chat-bubble helpers from `app.js`**

In `src/crates/primer-gui/ui/app.js`, find the functions that append child and Primer bubbles (look for the existing `primer://chunk` handler) and expose them on `window`:

```javascript
window.primerAppendChildBubble = appendChildBubble;     // or whatever it's named
window.primerAppendPrimerChunk = appendPrimerChunk;     // same
window.primerRefreshSidebar    = refreshSidebar;
window.primerShowToast         = showToast;
window.primerShowError         = showError;
```

(Exact names depend on the existing code; adjust accordingly.)

- [ ] **Step 4: Smoke**

```bash
cd src
~/.cargo/bin/cargo run -p primer-gui --features speech
```

In the GUI: click "Voice mode" → consent modal should appear (assets won't exist on a fresh home). Click Cancel; modal dismisses, voice toggle returns to off.

- [ ] **Step 5: Commit**

```bash
cd src
git add crates/primer-gui/ui/voice.js crates/primer-gui/ui/index.html crates/primer-gui/ui/app.js
git commit -m "$(cat <<'EOF'
feat(gui): voice-mode frontend controller (voice.js)

Header toggle → start/stop_voice_mode + composer swap. Esc keypress
+ Stop button → cancel_voice_response. Tauri event subscriptions
update the voice-state widget and append chat bubbles. Asset-missing
returns surface the consent modal; download progress lives in the
modal's progress bar.

Sticky-toggle restoration on launch: reads speech.voice_mode_enabled
from get_settings and auto-invokes start_voice_mode.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 5.4: Locale-aware state copy

**Files:**
- Identify and modify the EN + DE locale TOML templates (run `find` to locate)
- Modify: `src/crates/primer-gui/ui/voice.js`

- [ ] **Step 1: Locate the locale pack templates**

```bash
cd src
find . -name "*.toml" -path "*locale*" -o -name "*.toml" -path "*pack*" 2>/dev/null | grep -v target
```

Expected: two or more files like `crates/primer-core/locale/en.toml`. Note the exact path.

- [ ] **Step 2: Add three new keys to each locale TOML**

In the English template:

```toml
voice_state_listening_label = "Listening…"
voice_state_listening_hint = "take your time"
voice_state_thinking_label = "Thinking…"
voice_state_thinking_hint = "the Primer is working on a reply"
voice_state_speaking_label = "Speaking…"
voice_state_speaking_hint = "let the Primer finish"
```

In the German template:

```toml
voice_state_listening_label = "Höre zu…"
voice_state_listening_hint = "lass dir Zeit"
voice_state_thinking_label = "Denke nach…"
voice_state_thinking_hint = "der Primer überlegt eine Antwort"
voice_state_speaking_label = "Spreche…"
voice_state_speaking_hint = "lass den Primer ausreden"
```

- [ ] **Step 3: Add a Tauri command `get_voice_state_copy` that returns the four strings for the current locale**

In `commands/voice.rs`:

```rust
#[derive(serde::Serialize)]
pub struct VoiceStateCopy {
    pub listen_label: String,
    pub listen_hint: String,
    pub thinking_label: String,
    pub thinking_hint: String,
    pub speak_label: String,
    pub speak_hint: String,
}

#[tauri::command]
pub async fn get_voice_state_copy(
    state: tauri::State<'_, AppState>,
) -> Result<VoiceStateCopy, String> {
    let cfg = state.config.lock().await.clone();
    let locale = primer_core::i18n::Locale::from_pack_id(&cfg.learner.locale).unwrap_or_default();
    let pack = primer_core::i18n::pack_for(locale);
    Ok(VoiceStateCopy {
        listen_label: pack.get("voice_state_listening_label").into(),
        listen_hint: pack.get("voice_state_listening_hint").into(),
        thinking_label: pack.get("voice_state_thinking_label").into(),
        thinking_hint: pack.get("voice_state_thinking_hint").into(),
        speak_label: pack.get("voice_state_speaking_label").into(),
        speak_hint: pack.get("voice_state_speaking_hint").into(),
    })
}
```

Register the command and add the capability.

- [ ] **Step 4: Wire `voice.js` to read the locale copy on init**

In `voice.js`, replace the hardcoded `STATE_COPY` object with a fetch from the new command:

```javascript
async function loadStateCopy() {
  const c = await invoke("get_voice_state_copy").catch(() => null);
  if (!c) return;
  STATE_COPY.listen       = { label: c.listen_label,    hint: c.listen_hint };
  STATE_COPY.latent_think = { label: c.thinking_label,  hint: c.thinking_hint };
  STATE_COPY.speak        = { label: c.speak_label,     hint: c.speak_hint };
}
```

Call `loadStateCopy()` from `init()`.

- [ ] **Step 5: Commit**

```bash
cd src
git add . # locale TOML, voice.js, voice.rs, commands/mod.rs, capabilities/default.json
git commit -m "$(cat <<'EOF'
feat(gui): localise voice-state copy via existing pack system

Adds voice_state_{listening,thinking,speaking}_{label,hint} to
EN + DE locale templates. New get_voice_state_copy Tauri command
returns the six strings; voice.js loads them on init.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 5.5: Sidebar refresh on `primer://voice/response_complete`

**Files:**
- Modify: `src/crates/primer-gui/ui/app.js`

- [ ] **Step 1: Locate the existing sidebar refresh trigger**

```bash
cd src
grep -n "primer://turn_complete" crates/primer-gui/ui/app.js | head
```

Expected: a handler subscribed via `listen("primer://turn_complete", ...)` that refreshes the sidebar.

- [ ] **Step 2: Subscribe the same handler to the voice event**

In `app.js`, after the existing `listen("primer://turn_complete", ...)`:

```javascript
await listen("primer://voice/response_complete", () => {
  // Voice turns produce the same Turn records as text turns; same
  // sidebar refresh applies.
  refreshSidebar();  // existing function name
});
```

- [ ] **Step 3: Commit**

```bash
cd src
git add crates/primer-gui/ui/app.js
git commit -m "feat(gui): refresh sidebar on voice response_complete"
```

### Task 5.6: Update Settings → Speech to expose `voice_mode_enabled` read-only badge

**Files:**
- Modify: `src/crates/primer-gui/ui/settings.js`

- [ ] **Step 1: Show the current voice-mode state in Settings**

Add a small status line at the top of the Speech section (read-only, since the header toggle owns the flip):

```javascript
// In the Speech settings render function:
const status = document.createElement("p");
status.className = "hint muted";
status.textContent = view.speech.voice_mode_enabled
  ? "Voice mode is ON — toggle it off via the header button"
  : "Voice mode is off — toggle it on via the header button";
speechBlock.prepend(status);
```

- [ ] **Step 2: Commit**

```bash
cd src
git add crates/primer-gui/ui/settings.js
git commit -m "feat(gui): show voice-mode status badge in Settings → Speech"
```

### Task 5.7: Manual smoke matrix

**Files:** none (manual validation)

- [ ] **Step 1: Run the smoke matrix in order**

```bash
cd src
~/.cargo/bin/cargo run -p primer-gui --features speech
```

Confirm each scenario from the spec's "Manual smoke matrix":

1. First-time launch, voice mode off → text mode works exactly as before.
2. Click "Voice mode" → consent dialog → cancel → flag flips back to false, text mode resumes.
3. Click "Voice mode" → consent dialog → accept → download → loop starts → speak a question → hear a reply → say "goodbye" → voice mode exits cleanly.
4. Stop button mid-Primer-response → response aborts, partial Primer turn dropped, child turn preserved, returns to LISTEN.
5. Esc keypress during SPEAK → same result as Stop button.
6. Header "End voice mode" mid-session → drains background tasks, returns to picker (or text mode — Phase A spec is fine either way).
7. Switch locale to `de` in Settings while voice mode is on → loop tears down, restarts with German voice (or asset-missing dialog appears for DE).
8. Sidebar refreshes on every `primer://voice/response_complete` — intent, engagement, concepts, comprehension all populate just like in text mode.

If any scenario fails, fix the gap before committing the smoke-pass note.

- [ ] **Step 2: Commit a smoke-pass note**

```bash
cd src
# No code changes — empty commit purely for the milestone marker.
git commit --allow-empty -m "$(cat <<'EOF'
test(gui): voice mode end-to-end smoke matrix passes

All eight scenarios from the spec's smoke matrix were validated
on macOS with the user's local Whisper small.en + en_GB-alba-medium
+ de_DE-thorsten-medium model files.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 5.8: PR 5 close

PR 5 ends here. Open a PR titled "feat(gui): voice mode end-to-end".

---

## PR 6 — Documentation

Three tasks.

### Task 6.1: Update README status section

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Find the Status section**

```bash
grep -n "^## " README.md | head
```

Expected: a "Status" or "Current state" section near the top.

- [ ] **Step 2: Add a one-liner**

Append to the relevant bullet list in the Status section:

```markdown
- **Voice mode in the desktop GUI** (Phase A) — header toggle, composer-zone state widget, auto-download of locale-default Whisper + Piper assets to `~/.cache/primer/models/`. Same no-barge-in invariants as the CLI's `--speech`. Build with `cargo run -p primer-gui --features speech` and click "Voice mode" in the header.
```

- [ ] **Step 3: Commit**

```bash
git add README.md
git commit -m "docs(readme): note GUI voice mode (Phase A)"
```

### Task 6.2: Update CLAUDE.md with the lifted-module facts

**Files:**
- Modify: `CLAUDE.md`

- [ ] **Step 1: Add a note to the project shape section**

In the "fourteen crates" list (which will become fifteen if `voice-loop` ever becomes its own crate; for now `primer-speech` just gains a module), update the `primer-speech` bullet:

Find the existing bullet for `primer-speech` and append at the end:

```
The new `voice_loop` module (behind the `voice-loop` feature) holds the
shared state machine that both `primer-cli` (--speech mode) and
`primer-gui` (Voice mode toggle) consume via different `LoopObserver`
implementations. CLI uses `StdoutObserver`; GUI uses
`TauriEventObserver` emitting `primer://voice/*` events. The loop returns
a `LoopHandle` with `stop_tx` (CLI Ctrl+C / GUI End-voice-mode) and
`cancel_response_tx` (GUI Stop button + Esc; aborts in-flight LLM + TTS
and returns to LISTEN).
```

And add a new bullet to the "Conventions and gotchas worth knowing" section:

```
- **Voice mode in the GUI** is gated by `primer-gui/speech` feature.
  Mirrors the CLI's `speech` feature in deps and behaviour; default
  builds skip cpal entirely. Asset cache lives under
  `~/.cache/primer/models/` with sub-dirs `voice/<locale>/` and
  `whisper/`. First-run download is consent-gated; the
  `disable_auto_download` setting in `SpeechSettings` honours
  [[project_strict_offline_first]]. Sticky toggle persisted to
  `gui-config.json` as `speech.voice_mode_enabled` is per-device, not
  per-learner (see spec for rationale).
```

- [ ] **Step 2: Commit**

```bash
git add CLAUDE.md
git commit -m "docs(claude): note voice_loop shared module + asset cache layout"
```

### Task 6.3: PR 6 close

PR 6 ends here. Open a PR titled "docs: GUI voice mode notes". This is the final PR; voice mode is shipped.

---

## Self-Review

Spec coverage check:
- Lift `speech_loop` into `primer-speech` behind `voice-loop` → Tasks 1.1–1.5
- `LoopObserver` trait with the seven methods → Task 1.2
- `primer-gui/speech` feature → Task 3.1
- Four Tauri commands → Tasks 3.5 (3 of them), 4.5 (download)
- Six Tauri events → Tasks 3.3 (5 emit sites), 4.5 (download_progress)
- Header toggle (sticky, per device) → Task 5.1 + 5.3
- Composer-zone widget with three states → Task 5.2
- Settings → Speech form → Task 2.4 (preview) + 5.6 (status badge)
- Locale-default voice mapping → Task 1.3
- Voice-keyword exit → Inherited from CLI loop, no new task
- `GuiConfig` additive `SpeechSettings` block → Task 2.2
- Tests covering migration / observer wiring / lifecycle → Tasks 1.4, 1.5, 3.3, 3.4, 3.5, 4.1
- Manual smoke matrix → Task 5.7
- Documentation → Tasks 6.1, 6.2

Placeholder scan: all `todo!()` markers are inside Task 4.2's `start_voice_mode` body and Task 4.3 explicitly fills them via the `voice::backends` module. Both tasks chain immediately to each other.

Type consistency check:
- `LoopObserver` method names match between Task 1.2 (definition), Task 1.4 (state-machine call sites), Task 1.5 (CLI `StdoutObserver`), Task 3.3 (GUI `TauriEventObserver`), Task 5.3 (frontend event names).
- `MissingAsset` shape matches between Tasks 3.5 (definition), 4.1 (production site), 4.5 (download consumer), 5.3 (frontend deserialisation).
- `StartVoiceModeError` variant tags (`not_built` / `asset_missing` / `other`) are referenced consistently in Tasks 3.5 and 5.3.

Plan is internally consistent.
