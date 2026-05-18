#![cfg(all(target_os = "macos", feature = "macos-native"))]

use primer_core::speech::{Named, StreamingSpeechToText};
use primer_speech::macos::MacosSpeechToText;

/// Smoke test: verify that constructing a recognizer for `en-US` succeeds and
/// that the returned session can be dropped without panicking.
///
/// This test does NOT feed audio or assert on transcripts — end-to-end STT
/// requires microphone permission (which triggers an OS consent prompt that
/// causes `SIGABRT` in test binaries without `NSSpeechRecognitionUsageDescription`
/// in `Info.plist`). Actual transcription is exercised manually via the voice
/// loop (Task 8).
#[test]
fn open_session_returns_a_session() {
    let stt = MacosSpeechToText::new("en-US").expect("en-US recognizer");
    let session = stt.open_session().expect("session opens");
    drop(session); // smoke: drop without panic
}

/// Verify the `Named` impl returns the canonical backend identifier.
#[test]
fn name_is_macos_native_stt() {
    let stt = MacosSpeechToText::new("en-US").unwrap();
    assert_eq!(Named::name(&stt), "macos-native-stt");
}
