//! Android speech-capability diagnostic (Plan 1 go/no-go gate). Surfaced via
//! the frontend; read back over adb/logcat on the device.

use primer_speech::android::SpeechCapabilities;

#[tauri::command]
pub async fn speech_capabilities() -> Result<SpeechCapabilities, String> {
    let caps = primer_speech::android::query_capabilities().map_err(|e| e.to_string())?;
    // Mirror to logcat so it is readable without a UI round-trip on-device.
    tracing::info!(target: "primer::speech::diag", caps = ?caps, "speech capabilities");
    Ok(caps)
}
