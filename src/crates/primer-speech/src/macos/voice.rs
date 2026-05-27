//! AVSpeechSynthesisVoice probing and selection.

use objc2::rc::Retained;
use objc2_avf_audio::{AVSpeechSynthesisVoice, AVSpeechSynthesisVoiceQuality};
use objc2_foundation::NSString;
use primer_core::i18n::Locale;

/// Voice-quality tier, mirroring `AVSpeechSynthesisVoiceQuality` so
/// callers don't import the objc2 type.
///
/// Variant declaration order encodes the ranking: `Default` < `Enhanced`
/// < `Premium`. This matches Apple's own ordering. Both Enhanced and
/// Premium are downloadable neural voices; if the user took the effort
/// to install Premium, we honour their choice. The original ordering
/// reversed Premium and Enhanced under the assumption that Premium is
/// "rarely installed", but Enhanced is also a download and the inversion
/// just discarded a better voice when both were present — that's now
/// fixed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum VoiceQuality {
    Default,  // lowest — robotic, always-bundled
    Enhanced, // middle — neural, downloadable, broad locale coverage
    Premium,  // highest — neural, downloadable, top tier when available
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

/// Pick the best available voice for `locale`. Preference is `Premium`
/// over `Enhanced` over `Default`, matching the `VoiceQuality` enum
/// ordering and Apple's own `AVSpeechSynthesisVoiceQuality` ranking:
/// Premium are top-tier neural voices (~500 MB, opt-in); Enhanced are
/// good neural voices (~100 MB, downloadable); Default is the always-
/// bundled robotic-edge fallback.
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
            "only Default-quality voice available; install a Premium or Enhanced voice via System Settings → Accessibility → Spoken Content → System Voice → Manage Voices for substantially better quality"
        );
    } else if quality == VoiceQuality::Enhanced {
        tracing::info!(
            target: "primer::speech::macos",
            locale = %want_lang,
            "selected Enhanced-quality voice; a Premium-quality voice (top tier, neural) is available via System Settings → Accessibility → Spoken Content → System Voice → Manage Voices if you want best-in-class quality"
        );
    }
    Some(VoiceSelection {
        identifier,
        language,
        quality,
        voice,
    })
}

#[cfg(test)]
mod tests {
    use super::VoiceQuality;

    /// Pin the ordering so a future refactor can't silently reverse it again.
    /// `Default < Enhanced < Premium` matches Apple's AVSpeechSynthesisVoiceQuality
    /// ranking; reversing this regresses the quality of the selected voice
    /// when a user has installed Premium for their locale.
    #[test]
    fn voice_quality_ordering_matches_apple() {
        assert!(VoiceQuality::Default < VoiceQuality::Enhanced);
        assert!(VoiceQuality::Enhanced < VoiceQuality::Premium);
        assert!(VoiceQuality::Default < VoiceQuality::Premium);
    }
}
