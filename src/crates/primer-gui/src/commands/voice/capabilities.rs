//! Compile-time capability flags the frontend queries at launch to
//! enable / disable voice-mode controls. None touch session state.

/// Whether voice mode is built into this binary.
///
/// Returns `cfg!(feature = "speech")` — a compile-time constant. Independent
/// of any session state so the frontend can enable / disable the voice
/// toggle at launch without waiting for a session to start. Previously the
/// frontend read this flag off `current_session_info`, which left the
/// toggle permanently disabled on the session-picker screen (no active
/// session at launch → `null` → `state.available = false` → tooltip
/// incorrectly says "Voice mode is not built into this binary").
#[tauri::command]
pub async fn voice_mode_available() -> Result<bool, String> {
    Ok(cfg!(feature = "speech"))
}

/// Whether this binary was compiled with the Android-native speech stack
/// (`android-native`). The frontend uses this to choose the
/// `*_voice_mode_android` commands over the desktop `*_voice_mode`
/// commands and to enable the voice toggle on an Android build (where
/// `voice_mode_available` is false — `android-native` is independent of
/// the cpal `speech` feature). A pure compile-time flag, no session state.
#[tauri::command]
pub async fn android_voice_available() -> Result<bool, String> {
    Ok(cfg!(feature = "android-native"))
}

/// Whether this binary was compiled with a macOS-native speech stack
/// (`macos-native` or `macos-native-26`). The settings modal uses this
/// to enable/disable the "macOS Native" speech-backend option: selecting
/// it on a build without the feature would silently fall through to
/// whisper/piper (see `voice::backends::build_loop_backends`), so the
/// option is shown-but-disabled with a hint instead. Mirrors
/// `voice_mode_available` — a pure compile-time flag, no session state.
#[tauri::command]
pub async fn macos_native_speech_available() -> Result<bool, String> {
    Ok(cfg!(all(
        target_os = "macos",
        any(feature = "macos-native", feature = "macos-native-26"),
    )))
}

/// Whether this binary was compiled with the `supertonic` feature. The
/// settings modal uses this to enable/disable the "Supertonic" option in
/// the TTS-backend dropdown: selecting it on a build without the feature
/// would fail at session start with a "rebuild with --features supertonic"
/// error (see `voice_loop::build_tts`), so the option is shown-but-disabled
/// with a hint instead. Mirrors `macos_native_speech_available` — a pure
/// compile-time flag, no session state.
#[tauri::command]
pub async fn supertonic_tts_available() -> Result<bool, String> {
    Ok(cfg!(feature = "supertonic"))
}
