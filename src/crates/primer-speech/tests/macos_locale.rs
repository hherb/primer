#![cfg(all(target_os = "macos", feature = "macos-native"))]

use primer_core::i18n::Locale;
use primer_speech::macos::locale::is_on_device_available;

#[test]
fn en_us_is_available_on_device() {
    assert!(is_on_device_available(&Locale::English));
}

/// Verifies that the German on-device STT model is installed and available.
///
/// This test requires the German Siri / dictation language pack to be
/// downloaded on the host machine (System Settings → Keyboard →
/// Dictation → Languages → "Deutsch"). It passes on developer machines
/// where German is configured; it is deliberately skipped in CI and on
/// plain English-only Macs to avoid flakiness.
///
/// The probe itself is the load-bearing [[project_strict_offline_first]]
/// gate — `is_on_device_available` returns `false` on this machine rather
/// than silently allowing a network fallback, which is correct behaviour.
#[test]
#[ignore = "requires German on-device STT model (System Settings → Keyboard → Dictation → Languages → Deutsch)"]
fn de_de_is_available_on_device() {
    assert!(is_on_device_available(&Locale::German));
}

/// CI coverage for the `false` arm. Hindi is the project's preview
/// locale and is not on Apple's on-device list on macOS 13–15 (only
/// added at the SpeechAnalyzer layer on macOS 26+). On a developer
/// machine that has somehow grown Hindi on-device support, this test
/// will fail loudly and tell the maintainer to revisit the contract.
#[test]
fn hindi_is_not_available_on_device() {
    assert!(!is_on_device_available(&Locale::Hindi));
}
