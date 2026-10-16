//! Locale-aware voice-state widget copy: [`VoiceStateCopy`] and the
//! `get_voice_state_copy` command.

use serde::Serialize;

use crate::state::AppState;

/// Locale-aware copy strings for the voice-state widget.
///
/// Returns the six display strings (label + hint for each of the three
/// voice states) in the learner's current locale. Not feature-gated — it
/// is just a locale table lookup and works in default (non-speech) builds
/// too, so the Settings → Speech badge can show the right language even
/// when the voice loop isn't compiled in.
#[derive(Serialize, Debug)]
pub struct VoiceStateCopy {
    pub listen_label: String,
    pub listen_hint: String,
    pub thinking_label: String,
    pub thinking_hint: String,
    pub speak_label: String,
    pub speak_hint: String,
}

impl VoiceStateCopy {
    /// Build the locale-aware copy by reading the active prompt pack's
    /// `[voice_state]` table. The embedded packs are validated at build
    /// time so a load failure here would be a structural codebase bug,
    /// not a user-recoverable condition — mirrors the established pattern
    /// at `dialogue_manager::lifecycle::DialogueManager::new`.
    pub(super) fn for_locale(locale: &primer_core::i18n::Locale) -> Self {
        let pack = primer_pedagogy::prompt_pack::load_cached(*locale)
            .expect("prompt pack load failed; this should be impossible at runtime");
        let labels = pack.voice_state_labels();
        Self {
            listen_label: labels.listen_label.clone(),
            listen_hint: labels.listen_hint.clone(),
            thinking_label: labels.thinking_label.clone(),
            thinking_hint: labels.thinking_hint.clone(),
            speak_label: labels.speak_label.clone(),
            speak_hint: labels.speak_hint.clone(),
        }
    }
}

#[tauri::command]
pub async fn get_voice_state_copy(
    state: tauri::State<'_, AppState>,
) -> Result<VoiceStateCopy, String> {
    let cfg = state.config.lock().await.clone();
    let locale = primer_core::i18n::Locale::from_pack_id(&cfg.learner.locale).unwrap_or_default();
    Ok(VoiceStateCopy::for_locale(&locale))
}
