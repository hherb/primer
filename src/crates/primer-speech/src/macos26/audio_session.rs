//! Platform-specific audio session setup. The macOS branch is a no-op;
//! macOS has no AVAudioSession. The iOS branch is a placeholder today;
//! it'll need a real impl when iOS host scaffolding lands. Concentrating
//! the divergence here keeps `analyzer.rs` and `stt.rs` Apple-portable.

#![cfg(all(target_vendor = "apple", feature = "macos-native-26"))]

use primer_core::error::Result;

#[cfg(target_os = "macos")]
pub fn configure_for_capture() -> Result<()> {
    // No AVAudioSession on macOS — cpal owns the device.
    Ok(())
}

#[cfg(target_os = "ios")]
pub fn configure_for_capture() -> Result<()> {
    // Placeholder: real iOS impl needs to set the AVAudioSession
    // category to .playAndRecord (.measurement mode), activate it,
    // handle interruption notifications. See the iOS scaffolding work
    // tracked separately. Until then, refuse loudly so a developer
    // doesn't ship an unconfigured iOS build.
    Err(primer_core::error::PrimerError::Speech(
        "macos26::audio_session: iOS session configuration is not yet \
         implemented. Add the AVAudioSession setup before shipping an \
         iOS build.".into()
    ))
}
