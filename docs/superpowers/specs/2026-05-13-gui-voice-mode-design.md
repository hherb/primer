# GUI voice mode — wiring `--speech` into the Tauri desktop app

**Status:** approved design, ready for implementation plan
**Phase:** Phase 0.3+ (closes the voice loop on the desktop GUI; mirrors the CLI `--speech` mode shipped in spec [2026-05-02](2026-05-02-voice-roundtrip-poc-design.md))
**Author:** brainstorming session, 2026-05-13

## Goal

Voice mode for the desktop GUI. A header toggle ends the current text session and starts a voice session in its place. Mic input → silero VAD → Whisper STT → existing `DialogueManager` → Piper TTS → speakers, with the no-barge-in invariants the CLI already enforces. The transcript and the evaluation sidebar (intent / engagement / concepts / comprehension) stay visible throughout, because the developer is still evaluating Primer behavior in this phase. The voice state (LISTEN / LATENT_THINK / SPEAK) is shown in a composer-zone widget that replaces the text input while voice mode is active.

This is **Phase A** of voice-in-GUI. **Phase B** — the big central ear / thinking / mouth child-facing illustration — is a separate later spec, deferred until the Primer's voice loop has proven consistently reliable.

## Scope

### In scope

- Lift `speech_loop` out of `primer-cli/src/speech_loop.rs` into `primer-speech::voice_loop` behind a new `voice-loop` cargo feature, with a `LoopObserver` trait carrying side-effects (state-change events, transcripts, response chunks, exit). CLI's existing module becomes a ~50-line adapter.
- New `speech` cargo feature on `primer-gui` that pulls `primer-speech/{silero,whisper,piper,cpal,voice-loop}`. Default GUI build stays light — no cpal in the dep tree.
- Tauri commands: `start_voice_mode`, `stop_voice_mode`, `cancel_voice_response`, `download_voice_assets`.
- Tauri events: `primer://voice/state_change`, `primer://voice/transcript`, `primer://voice/response_chunk`, `primer://voice/response_complete`, `primer://voice/exit`, `primer://voice/download_progress`.
- Frontend: header "Voice mode" toggle (sticky per device, persisted in `gui-config.json`), composer-zone state widget showing animated mic / thinking dots / speaker pulse keyed by state, a Stop button, Esc keyboard shortcut.
- Settings → Speech group: per-locale voice override (auto-default set by locale pack mapping), Whisper model path override, asset paths persisted. First-run consent dialog explains the ~500 MB Whisper + Piper download to `~/.cache/primer/models/` with explicit URLs visible.
- Locale-pack default-voice mapping in `primer-speech::voice_loop::locale_defaults` — `en` → `en_GB-alba-medium` + `Whisper small.en`; `de` → `de_DE-thorsten-medium` + `Whisper small` (multilingual). Voice selection auto-follows learner locale unless overridden.
- Voice-keyword exit ("goodbye" / "bye primer" / "stop primer") inherits the CLI's substring-match logic, now executed in the shared loop.
- `GuiConfig` schema bump 2 → 3 (additive: new `speech: SpeechSettings` block; old configs load with defaults).
- Tests: mock-driven coverage of the new observer wiring (CLI and GUI adapters produce equivalent state-change sequences for the same scripted VAD/STT/TTS input); existing `speech_loop` tests migrate to the shared crate; GUI adapter unit-tests for the Tauri-event emission path.

### Out of scope (explicit follow-ups)

- **Phase B production visuals** — the big central ear / thinking / mouth illustration. Composer-zone widget only for this iteration. Filed as a separate spec once Primer reliability is validated.
- Auto-download UI for the FastEmbed BGE-M3 embedding model. Same `~/.cache/primer/models/` directory but the embedding flow already has its own first-run path; out of voice-mode scope.
- Push-to-talk or dictation-into-text-composer modes. Voice-only session is the only shape.
- Multi-voice runtime switching (one `PiperTts` per voice loop instance, mirroring the CLI invariant).
- Wake-word activation. Strict offline-first rules out Picovoice; out of scope today.
- Hardware-button integration (dedicated mic-mute / Stop hardware button). Phase 3 hardware work.
- Free-form barge-in. No-barge-in is pedagogical, not a deferral.
- Visual partial-transcript rendering. The composer widget shows state, not the in-flight ASR. Finalized transcript lands in the chat bubble after VAD `SpeechEnd`, same as CLI today.
- Live monitoring of the audio device list / mic gain / latency dashboards.
- Speculative-commit `DialogueManager` API (the cleaner fix for the two-consecutive-child-turns artefact noted in the [2026-05-02 spec](2026-05-02-voice-roundtrip-poc-design.md)). Still future work; inherited as-is.

## Key decisions

### Voice-only session mode, header toggle, sticky per device

The "Voice mode" toggle in the header ends the current text session and starts a fresh voice session in its place. The composer area swaps; the chat history and sidebar stay. The toggle persists to `gui-config.json` so re-launching the GUI returns the user to whichever mode they last used.

Persistence is **per device, not per learner**. The voice toggle is a UI preference of the room/device, not a longitudinal property of the child. A child whose tablet is in a quiet room today and a noisy room tomorrow might toggle voice off for the noisy session — that should not affect any pedagogical record. The [[project_personal_device_model]] memory pins one-child-per-device, so device-scoped is also user-scoped today.

### Locale-default voice; auto-follow learner locale

Each locale pack ships a default-voice mapping. Switching the learner's locale in settings switches the voice. A per-locale override block in `SpeechSettings` lets the user pin a different voice once they've used the system enough to have a preference; the override is keyed by locale `pack_id` so switching locales doesn't clobber the override the user typed in for the other one.

### Auto-download with explicit consent; strict-offline escape hatch

First voice-mode launch resolves the locale-default assets at `~/.cache/primer/models/voice/<locale>/` and `~/.cache/primer/models/whisper/`. If any file is missing, `start_voice_mode` returns a structured `asset_missing` error rather than starting the loop, and the frontend renders a consent dialog: locale name, total approximate size, per-asset table (name / size / source URL / destination), Cancel and Download buttons.

A `disable_auto_download` setting honours the [[project_strict_offline_first]] posture: when set, missing assets surface as a plain error pointing at Settings → Speech instead of the consent dialog.

### Composer-zone widget for Phase A; big central visual deferred to Phase B

See [[project_gui_voice_phased_visual]]. Phase A keeps the chat transcript and sidebar visible alongside an animated voice-state widget in the composer area. Phase B replaces it with a child-facing big-central-ear-/-mouth illustration in a separate spec, gated on voice-mode reliability validation.

### Belt-and-suspenders cancel/exit controls

Four converging paths for "make it stop":

1. **Stop button** in the composer-zone widget — aborts in-flight LLM + synthesis, returns to LISTEN, partial Primer turn dropped (same semantic as text-mode cancel-mid-stream).
2. **Esc keyboard shortcut** — same effect as Stop button.
3. **VAD `SpeechStart` during LATENT_THINK** — internal to the loop, no Tauri round-trip; aborts synthesis so the Primer never barges in over a resumed child utterance.
4. **Voice keyword exit** — "goodbye" / "bye primer" / "stop primer" substring-match on the finalized transcript ends voice mode entirely (not just the current response). Inherited from the CLI loop.

Plus the header toggle, which while voice mode is active reads "End voice mode" and tears the loop down.

### Shared voice loop in `primer-speech`, not duplicated in `primer-gui`

The [2026-05-02 spec](2026-05-02-voice-roundtrip-poc-design.md) deferred this decision: *"state machine lives in primer-cli, not primer-speech... if a future hardware integration test wants to drive the loop headlessly, we extract it then."* The GUI is that moment. Both CLI and GUI consume `primer_speech::voice_loop::run_loop` with their own `LoopObserver` implementation; the state machine and no-barge-in invariants have one source of truth.

## Architecture

### Crate boundary changes

| Module                                     | Crate           | Behind feature                                                                              |
|--------------------------------------------|-----------------|---------------------------------------------------------------------------------------------|
| `voice_loop` (state machine + observer)    | `primer-speech` | new `voice-loop` feature                                                                    |
| `voice_loop::locale_defaults`              | `primer-speech` | `voice-loop`                                                                                |
| `speech_loop` CLI adapter (~50 lines)      | `primer-cli`    | existing `speech` feature → pulls `primer-speech/voice-loop`                                |
| `commands/voice.rs` Tauri commands         | `primer-gui`    | new `speech` feature → pulls `primer-speech/{silero,whisper,piper,cpal,voice-loop}`         |
| `ui/voice.js` frontend module              | `primer-gui`    | always present; runtime no-op when `voice_mode_available` is false                          |
| `SpeechSettings` on `GuiConfig`            | `primer-gui`    | always built (struct lives even when feature is off; values are inert without speech)       |

### `primer-speech::voice_loop` public API

```rust
pub trait LoopObserver: Send + 'static {
    fn on_state_change(&mut self, state: VoiceState, hint: Option<&str>);
    fn on_transcript_finalized(&mut self, text: &str);
    fn on_response_chunk(&mut self, primer_turn_index: usize, chunk: &str);
    fn on_response_complete(&mut self, payload: TurnComplete);
    fn on_inference_error(&mut self, err: &InferenceError);
    fn on_exit(&mut self, reason: ExitReason);
}

pub enum VoiceState { Listen, LatentThink, Speak, Exit }
pub enum ExitReason { UserKeyword, ExternalStop, MicError, SpeakerError }

pub struct LoopBackends { /* DialogueManager, VAD, STT, TTS, MicCapture, SpeakerSink */ }
pub struct LoopConfig { /* mic_silence_ms, voice profile, exit_keywords, ... */ }

pub struct LoopHandle {
    pub stop_tx: oneshot::Sender<()>,
    pub cancel_response_tx: mpsc::Sender<()>,
}

pub async fn run_loop(
    backends: LoopBackends,
    cfg: LoopConfig,
    observer: impl LoopObserver,
) -> (LoopHandle, JoinHandle<Result<(), VoiceLoopError>>);
```

A trait, not a channel, because (a) the CLI wants synchronous side-effects (printlns happen now, not when a consumer drains), and (b) the GUI's Tauri `AppHandle::emit` is itself synchronous on the calling thread; threading a channel between the loop and an event-pump task adds a hop for no benefit. `Send + 'static` so the observer can move into the state-machine task.

`LoopHandle` is what both adapters use for shutdown and mid-stream cancel. The CLI wires Ctrl+C to `stop_tx`; the GUI wires the header "End voice mode" button to the same. The Stop button and Esc keyboard shortcut route to `cancel_response_tx`. VAD-driven cancel-on-resumed-speech happens inside the loop without going through the channel.

### `primer-gui` integration

`AppState` gains an `Option<ActiveVoiceLoop>` slot sibling to `session`:

```rust
pub struct ActiveVoiceLoop {
    pub join: JoinHandle<Result<(), VoiceLoopError>>,
    pub handle: LoopHandle,
    pub info: SessionInfo,
}
```

Three Tauri commands in `primer-gui/src/commands/voice.rs` (gated by `#[cfg(feature = "speech")]`):

```rust
#[tauri::command]
pub async fn start_voice_mode(
    state: tauri::State<'_, AppState>,
    app: AppHandle,
) -> Result<SessionInfo, StartVoiceModeError>;

#[tauri::command]
pub async fn stop_voice_mode(state: tauri::State<'_, AppState>) -> Result<(), String>;

#[tauri::command]
pub async fn cancel_voice_response(state: tauri::State<'_, AppState>) -> Result<(), String>;

#[tauri::command]
pub async fn download_voice_assets(
    state: tauri::State<'_, AppState>,
    app: AppHandle,
    missing: Vec<MissingAsset>,
) -> Result<(), String>;
```

`StartVoiceModeError` is the structured error type the frontend unpacks to decide between "show a generic error banner" and "show the asset-consent dialog":

```rust
#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StartVoiceModeError {
    AssetMissing { entries: Vec<MissingAsset> },
    NotBuilt,            // non-speech build
    Other { message: String },
}
```

When `primer-gui` is built without `--features speech`, the four commands are replaced by stubs returning `Err(StartVoiceModeError::NotBuilt)` or the moral equivalent. The frontend reads a `voice_mode_available: bool` field from `current_session_info` and disables the header toggle when false, matching the existing pattern for the embedder.

### Tauri event surface

| Event                              | Payload                                                          | When                                                                                  |
|------------------------------------|------------------------------------------------------------------|---------------------------------------------------------------------------------------|
| `primer://voice/state_change`      | `{ state: "listen"|"latent_think"|"speak", hint?: string }`      | Every state transition. `hint` carries cancel-reason distinction (`"user_cancel"` vs. `"child_resumed"`). |
| `primer://voice/transcript`        | `{ text: string }`                                               | Whisper finalize. Child-turn text lands in the chat bubble alongside this.            |
| `primer://voice/response_chunk`    | `{ primer_turn_index: usize, text: string }`                     | Per-token; mirrors text-mode `primer://chunk` shape so chat-bubble code reuses.       |
| `primer://voice/response_complete` | `{ session_id, child_turn_index, primer_turn_index }`            | Mirrors text-mode `primer://turn_complete`. Triggers sidebar refresh.                 |
| `primer://voice/exit`              | `{ reason: "user"|"keyword"|"mic_error"|"speaker_error" }`       | Loop has cleanly exited.                                                              |
| `primer://voice/download_progress` | `{ asset_id, bytes_done, bytes_total }`                          | Per-asset chunk during `download_voice_assets`.                                       |

Namespacing under `voice/` (rather than reusing `primer://chunk`) lets the frontend filter cleanly when both modes coexist in the same chat history — e.g., a chat bubble can carry a "🎙" badge when its turn arrived through voice.

### Asset management

A new module `primer-speech::voice_loop::locale_defaults`:

```rust
pub struct VoiceDefault {
    pub piper_voice_id: &'static str,
    pub piper_onnx_url: &'static str,
    pub piper_config_url: &'static str,
    pub whisper_model_id: &'static str,
    pub whisper_url: &'static str,
    pub approx_total_mb: u32,
}

pub fn voice_default_for(locale: &Locale) -> Option<&'static VoiceDefault>;
```

Today's table:

| Pack id | Piper voice                | Whisper model      | ≈ size |
|---------|----------------------------|--------------------|--------|
| `en`    | `en_GB-alba-medium`        | `ggml-small.en.bin`| 530 MB |
| `de`    | `de_DE-thorsten-medium`    | `ggml-small.bin`   | 540 MB |

Both Piper and Whisper assets resolved from official Hugging Face mirrors (`rhasspy/piper-voices` and `ggerganov/whisper.cpp` respectively). `small.en` vs. `small` distinguishes English-only (smaller, slightly more accurate for EN) from multilingual (needed for DE).

`primer-gui::wiring::resolve_voice_assets(cfg, locale)`:

1. If the user set explicit paths in `SpeechSettings.overrides[locale.pack_id()]`, use those.
2. Otherwise look up `LOCALE_DEFAULTS[locale.pack_id()]` and resolve to `~/.cache/primer/models/voice/<locale>/<voice_id>.{onnx,onnx.json}` and `~/.cache/primer/models/whisper/<whisper_model_id>`.
3. Check existence. Missing files → return `AssetMissing { entries }` for the frontend's consent dialog.
4. All present → hand the resolved paths to `LoopBackends::open`.

`download_voice_assets` streams each missing file to `<dest>.partial` via `reqwest::get`, atomically renames on success, and retries transient 5xx / network errors via `primer_core::retry::retry_with_backoff`. A killed download leaves no half-files for the existence check to falsely accept. 4xx errors return an inline message ("download URL returned 404 — model may have been renamed upstream; pick a model manually in Settings"). Progress events fire per chunk so the modal can render a bar.

### `GuiConfig` schema bump 2 → 3

Additive on the root:

```rust
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SpeechSettings {
    pub voice_mode_enabled: bool,
    pub disable_auto_download: bool,
    pub mic_silence_ms: u32,
    pub overrides: BTreeMap<String, SpeechLocaleOverride>,
}

impl Default for SpeechSettings {
    fn default() -> Self {
        Self {
            voice_mode_enabled: false,
            disable_auto_download: false,
            mic_silence_ms: 600, // mirrors CLI --mic-silence-ms default
            overrides: BTreeMap::new(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct SpeechLocaleOverride {
    pub piper_onnx_path: Option<PathBuf>,
    pub piper_config_path: Option<PathBuf>,
    pub whisper_model_path: Option<PathBuf>,
    pub voice_id: Option<String>,
}
```

Manual `Default` impl (not derived) because `mic_silence_ms` needs a non-zero default; `#[derive(Default)]` would zero-init it. The default value lives in `primer_core::consts::speech::DEFAULT_MIC_SILENCE_MS` per [[feedback_no_magic_numbers]] (and is what the CLI's `--mic-silence-ms` already reads) — the literal `600` above is illustrative; the implementation reads from the consts module.

`#[serde(default)]` on the new `speech` block; older configs get `SpeechSettings::default()` injected silently on load, and the next save writes them out at v3. The same forward-compatible pattern the embedder block followed.

### Frontend (HTML / CSS / JS)

Header gains a "Voice mode" toggle button to the left of "Sessions" / "Settings", `aria-pressed` follows active state, label flips "Voice mode" ↔ "End voice mode".

The composer area gains a sibling `<div class="composer composer-voice">` containing the voice-state widget. CSS toggles which composer is visible based on `voiceMode.active`. The widget is keyed off a `[data-state="listen"|"latent_think"|"speak"]` attribute updated from `primer://voice/state_change`:

- LISTEN: blue ring + pulsing animation, "Listening…" / "take your time"
- LATENT_THINK: gray dot + thinking animation, "Thinking…" / "the Primer is working on a reply"
- SPEAK: green ring + speaker pulse, "Speaking…" / "let the Primer finish"

Per-state copy lives in a single object in `ui/voice.js` (`VOICE_STATE_COPY[state] = { label, hint }`), with the four short strings locale-aware via the existing pack-system TOML templates (a ride-along addition for EN and DE).

A Stop button is always visible in the widget; it's a no-op outside LATENT_THINK and SPEAK (clicking during LISTEN does nothing, with no error). The Esc key follows the same routing via a document-level `keydown` listener guarded by `voiceMode.active`.

A new `ui/voice.js` module loaded after `app.js` owns: header-toggle wiring, composer swap on activate/deactivate, the five `primer://voice/*` event subscriptions, sticky-toggle restoration on launch, asset-consent modal management.

Settings → Speech replaces today's `is-coming-soon` placeholder block with: backend availability hint, `mic_silence_ms` input, per-locale override sub-table (one row per locale pack with file pickers for voice + config + Whisper, plus a voice-id text field), `disable_auto_download` checkbox. When `voice_mode_available === false`, the form fields are hidden and replaced with a "build with `--features primer-gui/speech`" hint.

### Sidebar refresh

The existing sidebar refresh trigger (`primer://turn_complete`) is duplicated on `primer://voice/response_complete`. One line added to the event-subscription block in `app.js`; no other sidebar code change. Intent / engagement / concepts / comprehension / vocab all populate identically for voice turns.

### No `primer-storage` or `primer-knowledge` schema changes

Voice mode produces the same `Session` / `Turn` records as text mode. The speaker, intent, concepts, and comprehension columns are populated identically. The only delta is *how the input arrived*: as Whisper transcript instead of as composer text. That's not a property of the turn worth recording in the DB; the storage layer stays untouched.

The KB retrieval call site uses the finalized transcript verbatim as the query — same FTS5 sanitization, same hybrid retrieval if the embedder is wired.

### Sticky-toggle interaction with asset-missing path

`voice_mode_enabled` is flipped server-side: `start_voice_mode` flips it to `true` only on success (an `AssetMissing` return leaves the flag unchanged); `stop_voice_mode` flips it to `false` unconditionally and is idempotent when no loop is active.

If voice mode was sticky-enabled but voice models are missing on next launch, the start command returns `AssetMissing` → the asset-consent dialog appears. If the user accepts and the download succeeds, the retry of `start_voice_mode` succeeds and the user is in voice mode — exactly where they expected to be. If they cancel the dialog, the frontend invokes `stop_voice_mode` (which is the same idempotent off-flip used by the header "End voice mode" button); the flag persists as `false` and the next GUI launch falls through to text mode without further prompts.

## Pedagogical invariants preserved

The Primer's pedagogy is encoded in the system prompt and the dialogue manager; voice mode adds an input/output channel without touching either. The CLI's invariants ([2026-05-02 spec](2026-05-02-voice-roundtrip-poc-design.md)) carry over verbatim:

- The Primer never speaks over the child (LATENT_THINK keeps the mic open; VAD `SpeechStart` aborts in-flight synthesis).
- The child never speaks over the Primer (mic closed during SPEAK; speaker drain awaited before reopening).
- No barge-in is pedagogical, not a deferral ([[project_no_barge_in_pedagogy]]).
- All learner data stays local; cloud inference still sends turns per-request only.
- Comprehension is verified through transfer/application/contradiction probes, not voice-mode confidence signals.

## Testing

Five layers, each with a concrete pin:

1. **`primer-speech::voice_loop` migration sanity** (mock-driven, gated by `voice-loop`). The existing speech-loop tests in `primer-cli/src/speech_loop.rs` move into `primer-speech::voice_loop::tests` verbatim. A scripted `MockObserver` records the `(state, transcript, response_chunk, response_complete, exit)` sequence; tests assert the exact sequence the production observers will see. Same coverage, same red-test guarantee on state-machine refactors.
2. **CLI adapter parity** (smoke, behind `primer-cli/speech`). One test scripts the same mock backends through the pre-refactor code path (snapshot from git) and the post-refactor `CliObserver`-driven path, then asserts byte-for-byte equivalent stdout output. Retired after landing — it's a one-shot migration confidence test.
3. **GUI observer wiring** (unit, in `primer-gui/src/commands/voice.rs::tests`). `TauriEventObserver` constructed against a `tauri::test::mock_app()` and stepped through the `LoopObserver` callbacks; assert each callback produces the expected `primer://voice/*` event with the expected JSON payload. No real audio, no real backends.
4. **GUI lifecycle unit tests** mirror today's `commands/session.rs::tests`: `start_voice_mode_creates_active_loop_and_returns_info`, `stop_voice_mode_drains_loop_and_clears_state`, `cancel_voice_response_routes_to_loop_cancel_channel`, `asset_missing_returns_structured_error`, `start_when_already_active_closes_first`. Mocks the `LoopBackends` builder so no audio hardware is touched.
5. **Manual smoke matrix** (no CI gate — documented in the PR description). Developer wallclock validation on macOS before merge:
   - First-time launch, voice mode off → text mode still works exactly as before.
   - Click Voice mode → consent dialog → cancel → flag flips back to false, text mode resumes.
   - Click Voice mode → consent dialog → accept → download → loop starts → speak a question → hear a reply → say "goodbye" → voice mode exits cleanly.
   - Stop button mid-Primer-response → response aborts, partial Primer turn dropped, child turn preserved, returns to LISTEN.
   - Esc keypress during SPEAK → same result as Stop button.
   - Header "End voice mode" mid-session → drains background tasks, returns to picker.
   - Switch locale to `de` while voice mode is on → loop tears down, restarts with German voice (or asset-missing dialog appears for DE).
   - Sidebar refreshes on every `primer://voice/response_complete` — intent, engagement, concepts, comprehension all populate just like in text mode.

The current CI matrix already excludes `--features speech` per the CLI's existing posture (the cdn.pyke.io ort-runtime download and audio dep linkage doesn't run in GitHub Actions runners). The `voice-loop` feature on `primer-speech` is similarly excluded; CI builds `primer-gui` with default features only. Lint + unit tests for the new code live in the default-feature CI surface because the `TauriEventObserver` wiring tests don't need cpal/silero/whisper/piper.

## Rollout sequencing

Six steps. Each independently mergeable; earlier steps can ride to main without later ones blocking on hardware-bound testing.

1. **Extract `voice_loop` into `primer-speech`** behind the `voice-loop` feature with `LoopObserver` trait. CLI adapter ships in the same PR; CLI behavior unchanged. Migration-sanity test gates merge.
2. **Add `SpeechSettings` to `GuiConfig`**, bump `schema_version` 2 → 3, default-fill on older configs. Settings → Speech form renders but is non-functional yet. No-op in default-feature build.
3. **Wire `start_voice_mode` / `stop_voice_mode` / `cancel_voice_response` Tauri commands + `TauriEventObserver`** behind `primer-gui/speech` feature. The frontend toggle is wired but the loop is the only consumer.
4. **Asset resolver + `download_voice_assets` command + consent modal.** First end-to-end smoke is possible at this step.
5. **Composer-zone widget + sticky toggle + Esc shortcut + voice-keyword exit.** Smoke matrix runs.
6. **Documentation + README update** — the "Status" section mentions voice mode in CLI today; add a one-liner for GUI voice mode mirroring the existing pattern.

## Future work

- **Phase B production visuals.** Big central ear / thinking / mouth illustration, child-facing, replacing the composer-zone widget. Separate spec, gated on voice-mode reliability validation. See [[project_gui_voice_phased_visual]].
- **Speculative-commit `DialogueManager` API** — the cleaner fix for the two-consecutive-child-turns artefact ([2026-05-02 spec](2026-05-02-voice-roundtrip-poc-design.md)).
- **FastEmbed BGE-M3 first-run download in the GUI consent flow.** Today the embedder downloads silently on first construction; folding it into the same consent dialog as voice assets would surface the disk-space cost upfront.
- **Hardware mic-mute / Stop button integration.** Phase 3.
- **Wake-word activation.** Pending a strict-offline option (Picovoice ruled out by [[project_strict_offline_first]]).
- **Push-to-talk mode** for environments where always-on mic is wrong (classrooms, shared rooms).
- **Live partial-transcript display** in the composer widget for users who want to see the ASR working.

## References

- [2026-05-02 voice roundtrip POC spec](2026-05-02-voice-roundtrip-poc-design.md) — the CLI `--speech` mode this design extends.
- `primer-cli/src/speech_loop.rs` — current state-machine implementation, source for the lift.
- `primer-speech/src/{cpal_io,silero,whisper,piper,vad_debounce,phrase_split}.rs` — building blocks the loop assembles.
- `primer-gui/src/commands/session.rs` — Tauri command patterns the voice commands mirror.
- `primer-gui/ui/{index.html,app.js,styles.css}` — frontend integration points.
- [[project_gui_voice_phased_visual]] — Phase A composer-zone / Phase B big-central decision.
- [[project_no_barge_in_pedagogy]] — pedagogical basis for LISTEN → THINK → SPEAK.
- [[project_strict_offline_first]] — basis for the `disable_auto_download` escape hatch.
- [[project_personal_device_model]] — basis for device-scoped (not learner-scoped) sticky toggle.
