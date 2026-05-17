#![cfg(all(target_os = "macos", feature = "macos-native"))]

use primer_core::i18n::Locale;
use primer_speech::macos::voice::{VoiceQuality, select_voice};

#[test]
fn en_us_resolves_to_a_voice() {
    let selection = select_voice(&Locale::English).expect("en-US must have at least one voice");
    assert!(!selection.identifier.is_empty());
    assert!(matches!(
        selection.quality,
        VoiceQuality::Default | VoiceQuality::Enhanced | VoiceQuality::Premium
    ));
}

#[test]
fn de_de_resolves_to_a_voice() {
    let selection = select_voice(&Locale::German).expect("de-DE must have at least one voice");
    assert!(
        selection.language.starts_with("de"),
        "language must be a de-* tag, got {}",
        selection.language
    );
}
