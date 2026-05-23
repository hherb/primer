//! End-to-end smoke for the macos-native-26 path. Not in CI; gated
//! `#[ignore]` because it requires macOS 26 + an installed locale model
//! (en-US ships with the OS; de-DE downloads on first run).
//!
//! Run with:
//!   cargo test -p primer-speech --features macos-native-26 \
//!       --test macos26_smoke -- --ignored --nocapture

#![cfg(all(target_os = "macos", feature = "macos-native-26"))]

use primer_core::i18n::Locale;
use primer_speech::macos26::Macos26Stt;

#[tokio::test]
#[ignore = "requires macOS 26 host; not in CI"]
async fn construct_en_us() {
    let stt = Macos26Stt::new(Locale::English)
        .await
        .expect("en-US Macos26Stt constructs");
    assert_eq!(stt.locale(), Locale::English);
}

#[tokio::test]
#[ignore = "requires macOS 26 host; first run downloads de-DE model (~hundreds of MB)"]
async fn construct_de_de_triggers_download_if_missing() {
    let stt = Macos26Stt::new(Locale::German)
        .await
        .expect("de-DE Macos26Stt constructs");
    assert_eq!(stt.locale(), Locale::German);
}

#[tokio::test]
#[ignore = "requires macOS 26 host"]
async fn hindi_rejected_with_helpful_error() {
    let err = Macos26Stt::new(Locale::Hindi).await.unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("Hindi"));
    assert!(msg.contains("Whisper"));
}
