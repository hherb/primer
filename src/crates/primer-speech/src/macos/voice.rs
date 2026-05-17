//! AVSpeechSynthesisVoice probing and selection.

use objc2::rc::Retained;
use objc2_avf_audio::{AVSpeechSynthesisVoice, AVSpeechSynthesisVoiceQuality};
use objc2_foundation::NSString;
use primer_core::i18n::Locale;

/// Voice-quality tier, mirroring `AVSpeechSynthesisVoiceQuality` so
/// callers don't import the objc2 type.
///
/// Variant declaration order encodes the ranking: `Default` < `Premium` <
/// `Enhanced`. `Premium` ranks below `Enhanced` because Premium voices are
/// optional ~500 MB downloads that most users will not have installed. We
/// prefer the reliably-available Enhanced (~100 MB) neural voice over an
/// absent Premium, and any neural voice over the always-bundled Default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum VoiceQuality {
    Default,  // lowest — robotic, always-bundled
    Premium,  // middle — neural, ~500 MB optional download, fewer voices available
    Enhanced, // highest — neural, ~100 MB optional download, broader coverage; preferred default
}

impl VoiceQuality {
    fn from_raw(raw: AVSpeechSynthesisVoiceQuality) -> Self {
        match raw {
            AVSpeechSynthesisVoiceQuality::Enhanced => VoiceQuality::Enhanced,
            AVSpeechSynthesisVoiceQuality::Premium => VoiceQuality::Premium,
            _ => VoiceQuality::Default,
        }
    }
}

/// A selected voice ready to assign to an `AVSpeechUtterance`.
pub struct VoiceSelection {
    pub identifier: String,
    pub language: String,
    pub quality: VoiceQuality,
    /// Retained pointer — keep alive for the lifetime of the utterance.
    pub(crate) voice: Retained<AVSpeechSynthesisVoice>,
}

impl VoiceSelection {
    /// Borrow the underlying AVFoundation voice for use with an
    /// `AVSpeechUtterance::setVoice`. Crate-internal callers can also
    /// `clone()` the field directly via `pub(crate)`.
    pub fn voice(&self) -> &AVSpeechSynthesisVoice {
        &self.voice
    }
}

/// Pick the best available voice for `locale`. Preference is `Enhanced`
/// over `Premium` over `Default`: Enhanced voices are good neural voices
/// in the ~100 MB range; Premium are ~500 MB and optional; Default is
/// the always-bundled robotic-edge fallback.
///
/// Returns `None` if no voice matches the locale's BCP-47 language tag at all.
pub fn select_voice(locale: &Locale) -> Option<VoiceSelection> {
    let want_lang = locale.bcp47();

    // SAFETY: `speechVoices()` is a thread-safe class method that returns a
    // snapshot of the system's installed voice list. The `Retained<NSArray<_>>`
    // wrapper ensures the array stays alive for the duration of this function.
    let all_voices = unsafe { AVSpeechSynthesisVoice::speechVoices() };
    // Convert to an owned Vec so we can iterate without needing the
    // NSEnumerator feature. Each element is a `Retained<AVSpeechSynthesisVoice>`.
    let voices_vec = all_voices.to_vec();

    let mut best: Option<(
        VoiceQuality,
        Retained<AVSpeechSynthesisVoice>,
        String,
        String,
    )> = None;

    for voice in &voices_vec {
        // SAFETY: `language()` is documented as "not atomic" (may race across
        // threads) but we hold each voice alive via `Retained` in `voices_vec`
        // and call it from a single thread with no concurrent mutation. The
        // returned `NSString` is retained for the duration of this scope.
        let lang: Retained<NSString> = unsafe { voice.language() };
        let lang_str = lang.to_string();
        if lang_str != want_lang {
            continue;
        }

        // SAFETY: same thread-safety rationale as `language()` above.
        let identifier: Retained<NSString> = unsafe { voice.identifier() };
        let identifier_str = identifier.to_string();

        // SAFETY: same thread-safety rationale as `language()` above.
        let quality = VoiceQuality::from_raw(unsafe { voice.quality() });

        let take = match &best {
            None => true,
            Some((current_q, _, _, _)) => quality > *current_q,
        };
        if take {
            // `voice` is a `&Retained<AVSpeechSynthesisVoice>` from the vec;
            // `clone()` bumps the ObjC retain count so our stored copy stays
            // valid independently of `voices_vec`.
            best = Some((quality, voice.clone(), identifier_str, lang_str));
        }
    }

    let (quality, voice, identifier, language) = best?;
    if quality == VoiceQuality::Default {
        tracing::warn!(
            target: "primer::speech::macos",
            locale = %want_lang,
            "only Default-quality voice available; user should install Enhanced via System Settings → Accessibility → Spoken Content → System Voice → Manage Voices for substantially better quality"
        );
    }
    Some(VoiceSelection {
        identifier,
        language,
        quality,
        voice,
    })
}
