#![cfg(all(target_os = "macos", feature = "macos-native"))]

use primer_speech::macos::permissions::{
    SpeechAuthStatus, current_speech_authorization_status, request_speech_authorization,
};

/// Verify that the `From<SFSpeechRecognizerAuthorizationStatus>` conversion
/// maps every known Apple variant to the correct `SpeechAuthStatus`.
///
/// This exercises the actual conversion logic in `permissions.rs`, unlike a
/// pure enum-construction test which the compiler already enforces.
#[test]
fn from_impl_maps_all_four_known_apple_variants() {
    use objc2_speech::SFSpeechRecognizerAuthorizationStatus;

    assert_eq!(
        SpeechAuthStatus::from(SFSpeechRecognizerAuthorizationStatus::NotDetermined),
        SpeechAuthStatus::NotDetermined
    );
    assert_eq!(
        SpeechAuthStatus::from(SFSpeechRecognizerAuthorizationStatus::Denied),
        SpeechAuthStatus::Denied
    );
    assert_eq!(
        SpeechAuthStatus::from(SFSpeechRecognizerAuthorizationStatus::Restricted),
        SpeechAuthStatus::Restricted
    );
    assert_eq!(
        SpeechAuthStatus::from(SFSpeechRecognizerAuthorizationStatus::Authorized),
        SpeechAuthStatus::Authorized
    );
}

/// Verify that reading the *current* authorization status (which does NOT
/// trigger the OS consent prompt and does NOT require
/// `NSSpeechRecognitionUsageDescription` in Info.plist) returns a known
/// variant.
///
/// On a fresh user / CI bot this will be `NotDetermined`. On a machine that
/// previously granted or denied speech recognition it will reflect that
/// decision. The test is intentionally agnostic — any known variant is a pass.
#[test]
fn current_speech_authorization_returns_a_known_status() {
    let status = current_speech_authorization_status();
    assert!(matches!(
        status,
        SpeechAuthStatus::Authorized
            | SpeechAuthStatus::Denied
            | SpeechAuthStatus::Restricted
            | SpeechAuthStatus::NotDetermined
    ));
}

/// Confirm `request_speech_authorization` is exported and callable in a
/// context where the OS will honour it (i.e. an app bundle with a valid
/// `NSSpeechRecognitionUsageDescription`). We only assert the symbol exists
/// here; exercising the consent flow from a plain test binary is not possible
/// without an Info.plist.
///
/// Marked `#[ignore]` so it is skipped in the default `cargo test` run.
/// Run with `-- --ignored` inside a proper app bundle if needed.
#[tokio::test]
#[ignore = "requires NSSpeechRecognitionUsageDescription in Info.plist"]
async fn request_speech_authorization_returns_a_known_status() {
    let status = request_speech_authorization().await;
    assert!(matches!(
        status,
        SpeechAuthStatus::Authorized
            | SpeechAuthStatus::Denied
            | SpeechAuthStatus::Restricted
            | SpeechAuthStatus::NotDetermined
    ));
}
