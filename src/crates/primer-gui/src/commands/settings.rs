//! Settings commands: load, return, persist.
//!
//! `get_settings` returns a redacted view ([`GuiConfigView`]) so the
//! inline API key never crosses the IPC boundary. `update_settings`
//! consumes a [`GuiConfigUpdate`] whose `ApiKeyUpdate::Keep` variant
//! lets the frontend save the rest of the config without ever holding
//! the secret. Validation runs *before* the disk write so a bad config
//! never lands on disk.
//!
//! `list_locales` exposes [`primer_core::i18n::Locale::ALL`] to the
//! settings modal so the locale-picker dropdown is sourced from Rust
//! rather than a hand-mirrored JS list. Preview locales (those not in
//! `Locale::ALL`) are automatically excluded — the IPC surface is the
//! single source of truth for "which locales are advertised to users".

use crate::config::{self, GuiConfigUpdate, GuiConfigView};
use crate::state::AppState;
use crate::validation;
use primer_core::i18n::Locale;
use serde::Serialize;

/// Return the current GUI settings (redacted view — no inline API key).
/// Always succeeds; missing-on-disk returns the in-memory defaults
/// loaded at startup.
#[tauri::command]
pub async fn get_settings(state: tauri::State<'_, AppState>) -> Result<GuiConfigView, String> {
    Ok((&*state.config.lock().await).into())
}

/// Replace the current GUI settings.
///
/// Steps, in order:
/// 1. Resolve the update against the persisted value (so
///    `ApiKeyUpdate::Keep` carries forward the existing secret).
/// 2. Validate — surface obviously-bad configs (unknown backend kind,
///    embedder kind, locale, etc.) here rather than at the next
///    `start_session`.
/// 3. Atomically persist to disk.
/// 4. Swap the in-memory copy.
///
/// **Active-session impact:** the in-memory ActiveSession (if any) is
/// NOT mutated here. Settings that affect the active session (backend,
/// model, locale, embedder) take effect only after the next
/// `start_session` — this matches the "Save & start new session"
/// flow planned for the settings modal in step 8.
#[tauri::command]
pub async fn update_settings(
    state: tauri::State<'_, AppState>,
    config: GuiConfigUpdate,
) -> Result<(), String> {
    let mut guard = state.config.lock().await;
    let resolved = config.into_config(&guard);
    validation::validate(&resolved)?;
    config::save(&state.home, &resolved).map_err(|e| e.to_string())?;
    *guard = resolved;
    Ok(())
}

/// One row of the locale-picker dropdown.
///
/// `id` is the stable pack id (round-trips through `learners.locale` and
/// the `update_settings` validator). `label` is the endonym — the
/// locale's name written in its own language — for direct display in the
/// dropdown. The shape is JSON-serialized as `{"id":"...","label":"..."}`;
/// the frontend reads both fields verbatim.
#[derive(Serialize, Debug, PartialEq, Eq)]
pub struct LocaleChoice {
    pub id: String,
    pub label: String,
}

/// Locales that are advertised to end users in the settings picker.
///
/// Mirrors `Locale::ALL` exactly — preview locales (e.g. Hindi while the
/// machine translation awaits native-speaker review) are excluded from
/// `Locale::ALL` and therefore from this command's output. Pure, no
/// state, never errors.
#[tauri::command]
pub async fn list_locales() -> Result<Vec<LocaleChoice>, String> {
    Ok(Locale::ALL
        .iter()
        .map(|l| LocaleChoice {
            id: l.pack_id().to_string(),
            label: l.endonym().to_string(),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `list_locales` returns exactly `Locale::ALL` in declaration order,
    /// each row carrying `(pack_id, endonym)`. This is the load-bearing
    /// invariant that lets the settings modal drop its hand-mirrored
    /// JS list — drift would re-introduce the divergence #80 was filed
    /// against.
    #[tokio::test]
    async fn list_locales_mirrors_locale_all_with_endonyms() {
        let result = list_locales().await.expect("list_locales never errors");
        let expected: Vec<LocaleChoice> = Locale::ALL
            .iter()
            .map(|l| LocaleChoice {
                id: l.pack_id().to_string(),
                label: l.endonym().to_string(),
            })
            .collect();
        assert_eq!(result, expected);
    }

    /// Preview locales (currently Hindi) must never appear in the
    /// dropdown — they're excluded from `Locale::ALL` by design. Pinning
    /// the specific id here makes a future "oh I'll add Hindi without
    /// reviewing the prompt pack" mistake a hard test failure.
    #[tokio::test]
    async fn list_locales_excludes_preview_hindi() {
        let result = list_locales().await.expect("list_locales never errors");
        assert!(
            result.iter().all(|c| c.id != "hi"),
            "Hindi must stay out of the picker until native-speaker review; got: {result:?}"
        );
    }

    /// Pin the JSON shape — the frontend reads `id` and `label` fields
    /// verbatim. A rename here silently breaks the settings modal.
    #[test]
    fn locale_choice_serialises_with_id_and_label() {
        let c = LocaleChoice {
            id: "en".into(),
            label: "English".into(),
        };
        let json = serde_json::to_value(&c).unwrap();
        assert_eq!(json["id"], "en");
        assert_eq!(json["label"], "English");
    }
}
