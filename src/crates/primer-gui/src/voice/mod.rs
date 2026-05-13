//! Voice-mode wiring for the GUI.
//!
//! - [`observer`] — `TauriEventObserver` impl that emits `primer://voice/*`
//!   events for the frontend.
//! - [`assets`] — voice-asset path resolver + `AssetMissing` shape.
//! - [`responder`] — `ArcDmResponder`: a `Responder` impl that locks a
//!   shared `Arc<Mutex<DialogueManager>>` per turn.
//!
//! All sub-modules are gated by `#[cfg(feature = "speech")]`; when the
//! feature is off, the module is empty and the Tauri command stubs in
//! `commands/voice.rs` return `Err(StartVoiceModeError::NotBuilt)`.

#[cfg(feature = "speech")]
pub mod assets;
#[cfg(feature = "speech")]
pub mod observer;
#[cfg(feature = "speech")]
pub mod responder;
