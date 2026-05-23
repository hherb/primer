//! Supertonic TTS — vendored library entrypoint.
//!
//! Upstream ships `helper.rs` as a sibling of `example_onnx.rs`'s bin target.
//! Primer needs to consume the synthesis API from `primer-speech`, so this
//! `lib.rs` re-exposes the `helper` module's public surface as a library.
//!
//! See `Cargo.toml` for the upstream commit sha + the rc.7 → rc.10 ort delta.

pub mod helper;

pub use helper::{
    is_valid_lang, load_cfgs, load_text_to_speech, load_voice_style, AEConfig, Config, Style,
    TTLConfig, TextToSpeech, UnicodeProcessor, VoiceStyleData,
};
