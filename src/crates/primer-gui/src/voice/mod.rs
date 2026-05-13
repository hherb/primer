//! Voice-mode wiring for the GUI.
//!
//! - [`observer`] — `TauriEventObserver` impl that emits `primer://voice/*`
//!   events for the frontend.
//!
//! All sub-modules are gated by `#[cfg(feature = "speech")]`; when the
//! feature is off, the module is empty and the Tauri command stubs in
//! `commands/voice.rs` return `Err(StartVoiceModeError::NotBuilt)`.

#[cfg(feature = "speech")]
pub mod observer;
