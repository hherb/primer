//! Long-lived Tauri-managed app state.
//!
//! `AppState` is the single value registered with `Builder::manage()`;
//! every Tauri command takes `tauri::State<'_, AppState>` and locks
//! the appropriate field. The state holds:
//!
//! 1. The persisted `GuiConfig` (mutable across "Save" actions).
//! 2. The home directory path so config save/load are filesystem-pure.
//! 3. An optional `ActiveSession` holding the constructed long-lived
//!    `DialogueManager` once `start_session` succeeds.
//!
//! The `DialogueManager` is held long-lived (behind `Arc<Mutex<…>>`)
//! so the natural CLI latency design carries over: the previous turn's
//! classifier / extractor / comprehension tasks are awaited at the
//! TOP of the next `respond_to_streaming` rather than at the END of
//! the current one. That keeps the composer re-enable instantaneous
//! once the stream finishes and absorbs the ~13 s background-task
//! wallclock in the natural inter-turn pause.
//!
//! Sharing across tasks: the DM is held in `Arc<Mutex<DialogueManager>>`
//! so a command can clone the Arc out of the session guard, release
//! the session guard, and lock the DM independently. To keep
//! `current_session_info` / `update_settings` / future sidebar
//! commands free to run while a turn is streaming, an
//! [`SessionSnapshot`] mirror of the DM-owned fields the frontend
//! needs lives behind its own short-lived `Mutex` — readers never
//! touch the DM lock. The snapshot is refreshed after each successful
//! turn from within `send_message`.

use std::path::PathBuf;
use std::sync::Arc;

use primer_core::i18n::Locale;
use primer_core::storage::SessionStore;
use primer_pedagogy::DialogueManager;
use tokio::sync::Mutex;
use tokio::task::AbortHandle;
use uuid::Uuid;

use crate::config::GuiConfig;

/// Tauri-managed application state.
pub struct AppState {
    /// User's home directory (resolved at startup). Used by config
    /// load/save and the default session-DB path resolver. Held as
    /// owned so commands don't need to re-read `$HOME` on every call.
    pub home: PathBuf,

    /// Persisted settings, kept in memory so `get_settings` doesn't
    /// hit disk and `update_settings` can stage the new value before
    /// the JSON save completes.
    pub config: Mutex<GuiConfig>,

    /// The currently open session, if any. `None` between
    /// `close_session` and the next `start_session` / `resume_session`.
    pub session: Mutex<Option<ActiveSession>>,

    /// The currently active voice loop, if any. Only present when the
    /// binary was built with `--features speech` AND the user started
    /// voice mode via `start_voice_mode`. `None` on default builds and
    /// when voice mode is inactive.
    #[cfg(feature = "speech")]
    pub voice: Mutex<Option<ActiveVoiceLoop>>,
}

impl AppState {
    /// Build a fresh state from a home directory and an initial config.
    /// The config is taken by value so callers can mutate their own
    /// `GuiConfig` separately from what gets registered.
    pub fn new(home: PathBuf, config: GuiConfig) -> Self {
        Self {
            home,
            config: Mutex::new(config),
            session: Mutex::new(None),
            #[cfg(feature = "speech")]
            voice: Mutex::new(None),
        }
    }
}

/// Handle to an active voice loop — wraps the spawned task handle plus
/// the channels that let the GUI stop the loop or cancel an in-flight
/// LLM call.
///
/// The reason this struct is cfg-guarded rather than always-present-but-
/// None is that `VoiceLoopError` only exists when the `voice-loop` speech
/// feature is compiled in. Gating the whole slot is cleaner than inventing
/// a stub type.
#[cfg(feature = "speech")]
pub struct ActiveVoiceLoop {
    /// Join handle for the spawned voice-loop task. Dropping it aborts
    /// the task (tokio semantics); the `stop_voice_mode` command sends
    /// to `stop_tx` first to let the loop exit cleanly, then joins with
    /// a 5-second timeout before dropping.
    pub join: tokio::task::JoinHandle<
        Result<(), primer_speech::voice_loop::VoiceLoopError>,
    >,
    /// One-shot sender that signals the loop to exit at the next
    /// LISTEN→LATENT_THINK boundary. The loop drains TTS and saves the
    /// session before returning.
    pub stop_tx: tokio::sync::oneshot::Sender<()>,
    /// Multi-message sender (capacity 8) that cancels the in-flight LLM
    /// call and TTS synthesis mid-turn. Non-blocking send: if the channel
    /// is full (user mashed Cancel repeatedly) one cancel is still enough.
    pub cancel_response_tx: tokio::sync::mpsc::Sender<()>,
    /// Snapshot of the session info at the time voice mode was started.
    /// Used by `start_voice_mode` to return a `SessionInfo` to the caller.
    pub info: crate::types::SessionInfo,
}

/// The active session — wraps a long-lived `DialogueManager` plus the
/// small chunk of display-only metadata commands need without locking
/// the DM (so `current_session_info` doesn't block on an in-flight
/// `send_message`).
///
/// The DM owns all its collaborators (inference + knowledge as
/// `Arc<dyn …>` since the Phase 0.3+ refactor; classifier / extractor
/// / comprehension already were). Putting it behind an `Arc<Mutex<…>>`
/// lets `send_message` clone the Arc and run its turn outside the
/// session-state lock.
pub struct ActiveSession {
    /// The dialogue manager — owns the active `Session`, the loaded
    /// `LearnerModel`, and every subsystem trait object the turn
    /// needs. Long-lived across `send_message` calls so the previous
    /// turn's background tasks are awaited at the top of the next
    /// turn (CLI-style), not synchronously at end-of-turn.
    pub dialogue_manager: Arc<Mutex<DialogueManager>>,

    /// The session's locale (matches the learner's stored locale and
    /// the knowledge base's per-locale partition). Kept outside the
    /// DM mutex so `current_session_info` can read it without locking.
    pub locale: Locale,

    /// Display string for the inference backend kind (e.g. "cloud",
    /// "ollama", "stub"). Kept outside the DM mutex so the frontend
    /// header can render without contention.
    pub backend_name: String,

    /// Display string for the main model id (e.g. "claude-sonnet-4-6",
    /// "llama3.2"). Kept outside the DM mutex for the same reason.
    pub main_model: String,

    /// Snapshot of the DM-owned fields the frontend reads via
    /// `current_session_info`. Refreshed by `send_message` after each
    /// successful turn so readers never have to lock the DM (and queue
    /// behind an in-flight stream that can take tens of seconds).
    pub snapshot: Arc<Mutex<SessionSnapshot>>,

    /// Handle to the underlying session store. Cloned out of wiring so
    /// the resume_session command can call `load_session(uuid)` after
    /// the fresh `ActiveSession` is built — DM itself doesn't expose
    /// `load_session`, and we don't want to re-open the SQLite file
    /// just to read one row.
    pub session_store: Arc<dyn SessionStore>,

    /// Abort handle for the in-flight turn, if any. `send_message`
    /// spawns the turn into a dedicated tokio task and stashes its
    /// abort handle here; `cancel_response` calls `.abort()` on it to
    /// drop the in-progress stream. The DM's existing "partial Primer
    /// turn is not recorded on mid-stream error" invariant cleans up
    /// the state correctly when the spawned future is dropped.
    pub current_turn_abort: Mutex<Option<AbortHandle>>,
}

/// Read-mostly mirror of the DM-owned fields the frontend renders via
/// `current_session_info`. Kept separate from the DM mutex so the
/// sidebar can read while a turn is streaming. Refreshed by
/// `send_message` after every successful turn.
#[derive(Debug, Clone)]
pub struct SessionSnapshot {
    /// `None` until the first send_message completes — at that point
    /// a real `Session` row exists on disk and the UUID can be
    /// round-tripped through `load_session`.
    pub session_id: Option<Uuid>,
    /// Stable learner UUID; written once at construction.
    pub learner_id: Uuid,
    /// Learner display name from the resolved profile.
    pub learner_name: String,
    /// Learner age from the resolved profile.
    pub learner_age: u8,
    /// Concept count from the in-memory learner model. Grows as the
    /// extractor surfaces new concepts; refreshed at each turn boundary.
    pub concept_count: usize,
}

impl std::fmt::Debug for ActiveSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ActiveSession")
            .field("locale", &self.locale)
            .field("backend_name", &self.backend_name)
            .field("main_model", &self.main_model)
            .finish_non_exhaustive()
    }
}
