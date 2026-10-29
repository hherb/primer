//! Voice-mode Tauri commands.
//!
//! `start_voice_mode` builds the voice loop and stashes its handle in
//! `AppState::voice`. `stop_voice_mode` drains the loop.
//! `cancel_voice_response` aborts the in-flight LLM call + TTS synthesis.
//!
//! All commands are gated by `#[cfg(feature = "speech")]`; the non-speech
//! build provides stubs returning `Err(NotBuilt)` or `Ok(())`.
//!
//! Split by responsibility:
//!
//! - `types` — the asset-`kind` identifiers, `StartVoiceModeError`,
//!   `MissingAsset`, and the pure `missing_to_error` gate.
//! - `start` — `start_voice_mode`.
//! - `stop` — `stop_voice_mode`, `cancel_voice_response`, and the shared
//!   `stop_voice_mode_by_user` / `stop_voice_mode_inner` teardown helpers.
//! - `asset_download` — `download_voice_assets`.
//! - `state_copy` — `VoiceStateCopy` + `get_voice_state_copy`.
//! - `capabilities` — the compile-time `*_available` flags.
//!
//! The flat `pub use` re-export façade keeps every external path
//! (`commands::voice::<name>`, including the hidden `__cmd__*` macros
//! `tauri::generate_handler!` resolves) identical to the pre-split module
//! and lets the shared `tests` module resolve every symbol via `super::*`.

mod asset_download;
mod capabilities;
mod start;
mod state_copy;
mod stop;
mod types;

pub use asset_download::*;
pub use capabilities::*;
pub use start::*;
pub use state_copy::*;
pub use stop::*;
pub use types::*;

#[cfg(test)]
mod tests;
