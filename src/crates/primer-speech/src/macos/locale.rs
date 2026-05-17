//! On-device locale-availability probe for SFSpeechRecognizer.
//!
//! Apple does not publish a stable list of locales whose models ship
//! on-device; the answer is per-device, per-OS-version, per-user-installed-
//! language. The only reliable check is to construct a recognizer for the
//! locale and read `supportsOnDeviceRecognition`. We must do this BEFORE
//! starting the voice loop and fail loudly if false — falling back to
//! network would violate the project's strict-offline-first principle.

use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_foundation::{NSLocale, NSString};
use objc2_speech::SFSpeechRecognizer;
use primer_core::i18n::Locale;

/// Returns `true` if `SFSpeechRecognizer` can do on-device recognition for
/// `locale` on this device and OS combination. Returns `false` if the
/// recognizer cannot be constructed (unknown locale) or the OS does not ship
/// the on-device model for this locale.
pub fn is_on_device_available(locale: &Locale) -> bool {
    let bcp47 = locale.bcp47();
    let ns_str = NSString::from_str(bcp47);
    // `localeWithLocaleIdentifier:` is a safe class method — no `unsafe` block
    // needed here. `initWithLocale:` and `supportsOnDeviceRecognition` are
    // marked `#[unsafe(method(...))]` in the generated bindings.
    let ns_locale: Retained<NSLocale> = NSLocale::localeWithLocaleIdentifier(&ns_str);
    let recognizer: Option<Retained<SFSpeechRecognizer>> =
        unsafe { SFSpeechRecognizer::initWithLocale(SFSpeechRecognizer::alloc(), &ns_locale) };

    match recognizer {
        Some(r) => unsafe { r.supportsOnDeviceRecognition() },
        None => false,
    }
}
