//! Voice-mode wiring for the GUI.
//!
//! - `observer` — `TauriEventObserver` impl that emits `primer://voice/*`
//!   events for the frontend.
//! - `assets` — voice-asset path resolver + `AssetMissing` shape.
//! - `backends` — thin wrapper around `primer_speech::voice_loop::build_tts`
//!   + `build_voice_backends` (cpal + VAD + STT + injected TTS).
//! - `backends_android` — cpal-free Android-native backend builder (OS-owned
//!   mic/speaker).
//! - `download` — streaming voice-asset downloader powering the
//!   `download_voice_assets` Tauri command.
//! - `responder` — `ArcDmResponder`: a `Responder` impl that locks a
//!   shared `Arc<Mutex<DialogueManager>>` per turn.
//!
//! `assets`, `backends` and `download` are gated by
//! `#[cfg(feature = "speech")]`; `observer` and `responder` by
//! `any(feature = "speech", feature = "android-native")`; `backends_android`
//! by `feature = "android-native"`. When neither feature is on, the module is
//! empty and the Tauri command stubs in `commands/voice/` return
//! `Err(StartVoiceModeError::NotBuilt)`.

#[cfg(feature = "speech")]
pub mod assets;
#[cfg(feature = "speech")]
pub mod backends;
#[cfg(feature = "speech")]
pub mod download;
// `observer` + `responder` are cpal-free (they only touch the voice-loop
// traits + Tauri + the DM), so they are shared by both the cpal speech
// build and the cpal-free `android-native` build.
#[cfg(any(feature = "speech", feature = "android-native"))]
pub mod observer;
#[cfg(any(feature = "speech", feature = "android-native"))]
pub mod responder;
// Android-native voice backend builder (OS-owned mic/speaker; no cpal).
#[cfg(feature = "android-native")]
pub mod backends_android;
