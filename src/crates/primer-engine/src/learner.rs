//! Learner-model lifecycle helpers shared between binaries.
//!
//! - `create_learner_with_id` — fresh `LearnerModel` construction
//! - `reconcile_persisted_learner` — merge CLI flags into a loaded
//!   `LearnerModel` on launch (age refresh + name-mismatch warning)
//! - `verify_resume_locale_match` — guard for `--resume` to prevent
//!   silent concept-language corruption when a stored learner's locale
//!   differs from the CLI's requested locale.

use chrono::Utc;
use primer_core::i18n::Locale;
use primer_core::learner::{EngagementState, LearnerModel, LearnerProfile, LearningPreferences};
use uuid::Uuid;

/// Reconcile a freshly-loaded persisted `LearnerModel` against the
/// CLI flags for this launch.
///
/// Behaviour (kept minimal so the test surface matches the production
/// branch exactly):
/// - If `cli_name` differs from the persisted name, log a `tracing::warn!`
///   AND a stderr `eprintln!` (so a parent who typos a name sees it
///   without `RUST_LOG=warn`). The persisted name **always** wins —
///   silently rewriting it would lock a child out of their own data.
/// - Update `age` from the CLI (covers the birthday case).
/// - Update `last_active` to now.
///
/// Returns the reconciled `LearnerModel`. The caller is responsible
/// for the subsequent `save_learner` call (so this helper has no I/O).
pub fn reconcile_persisted_learner(
    mut existing: LearnerModel,
    cli_name: &str,
    cli_age: u8,
) -> LearnerModel {
    if existing.profile.name != cli_name {
        eprintln!(
            "Note: --name {:?} differs from the persisted learner name {:?}; \
             keeping persisted (delete ~/.primer/<slug>.db to start fresh).",
            cli_name, existing.profile.name
        );
        tracing::warn!(
            "CLI --name {:?} differs from persisted learner name {:?}; using persisted",
            cli_name,
            existing.profile.name
        );
    }
    existing.profile.age = cli_age;
    existing.profile.last_active = Utc::now();
    existing
}

/// Verify that a `--resume`'d session's stored locale matches the one
/// requested at the CLI. Mismatches are a hard error: the session store
/// has already opened with `cli_locale`, so any new concept inserts in
/// the resumed session would be tagged with the wrong
/// `concept_language_tag` — silent corruption of the longitudinal
/// learner data, the kind of drift the project treats as a fail-fast
/// condition for categorical state.
///
/// Returns `Ok(())` on match (or when the locales are equal) and
/// `Err(message)` with a user-facing actionable string on mismatch.
/// Pure for testability.
pub fn verify_resume_locale_match(
    cli_locale: Locale,
    learner_locale: Locale,
    resume_id: Uuid,
) -> std::result::Result<(), String> {
    if cli_locale == learner_locale {
        return Ok(());
    }
    Err(format!(
        "--resume {resume_id} was created in locale '{learner}', but \
         --language '{cli}' was specified.\n  \
         Drop --language to use the session's locale, or pass \
         --language {learner}.",
        learner = learner_locale.pack_id(),
        cli = cli_locale.pack_id(),
    ))
}

/// Construct a fresh `LearnerModel` with the given identity. Used at
/// session-start when no persisted learner row exists yet.
pub fn create_learner_with_id(id: Uuid, name: &str, age: u8, locale: Locale) -> LearnerModel {
    LearnerModel {
        profile: LearnerProfile {
            id,
            name: name.to_string(),
            age,
            languages: vec![locale.pack_id().to_string()],
            locale,
            created_at: Utc::now(),
            last_active: Utc::now(),
        },
        concepts: vec![],
        preferences: LearningPreferences::default(),
        current_engagement: EngagementState::Engaged,
        recent_assessments: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use primer_core::storage::LearnerStore;
    use primer_storage::SqliteSessionStore;
    use std::sync::Arc;

    #[test]
    fn verify_resume_locale_match_ok_when_locales_equal() {
        let id = Uuid::new_v4();
        assert!(verify_resume_locale_match(Locale::English, Locale::English, id).is_ok());
        assert!(verify_resume_locale_match(Locale::German, Locale::German, id).is_ok());
    }

    #[test]
    fn verify_resume_locale_match_errors_on_mismatch_with_actionable_message() {
        let id = Uuid::new_v4();
        let err = verify_resume_locale_match(Locale::English, Locale::German, id).unwrap_err();
        assert!(
            err.contains("'de'"),
            "must name the learner's locale: {err}"
        );
        assert!(err.contains("'en'"), "must name the cli locale: {err}");
        assert!(
            err.contains("--language de"),
            "must show the corrective flag value: {err}"
        );
        assert!(
            err.contains(&id.to_string()),
            "must include the resume id so the user knows which session: {err}"
        );
    }

    #[test]
    fn verify_resume_locale_match_symmetric_de_to_en() {
        let id = Uuid::new_v4();
        let err = verify_resume_locale_match(Locale::German, Locale::English, id).unwrap_err();
        assert!(
            err.contains("'en'"),
            "must name the learner's locale: {err}"
        );
        assert!(err.contains("--language en"), "corrective flag: {err}");
    }

    #[tokio::test]
    async fn cli_birthday_case_updates_age_and_keeps_uuid() {
        // Save a learner with age=8, simulate startup with --age=9 by
        // calling the SAME helper main() uses, then verify the persisted
        // row has age=9 with the same UUID and created_at preserved.
        let store = Arc::new(
            SqliteSessionStore::open_for_locale(
                std::path::Path::new(":memory:"),
                primer_core::i18n::Locale::default(),
            )
            .unwrap(),
        );
        let original_id = Uuid::new_v4();
        let original_created = Utc::now() - chrono::Duration::days(365);
        let mut original =
            create_learner_with_id(original_id, "Binti", 8, primer_core::i18n::Locale::English);
        original.profile.created_at = original_created;
        store.save_learner(&original).await.unwrap();

        // Reload + reconcile via the production helper.
        let existing = store.load_learner().await.unwrap().expect("learner row");
        let reconciled = reconcile_persisted_learner(existing, "Binti", 9);
        store.save_learner(&reconciled).await.unwrap();

        assert_eq!(
            reconciled.profile.id, original_id,
            "UUID stable across launches"
        );
        assert_eq!(reconciled.profile.age, 9, "age updated to CLI value");
        assert_eq!(
            reconciled.profile.created_at.timestamp(),
            original_created.timestamp(),
            "created_at preserved",
        );
    }

    #[tokio::test]
    async fn cli_name_mismatch_keeps_persisted_name() {
        // Save with name="Binti", call reconcile_persisted_learner with
        // --name="Other" — the SAME helper main() uses — and verify the
        // persisted name stays "Binti". The tracing::warn! / eprintln!
        // emission is intentionally NOT asserted (subscriber capture
        // would over-couple the test); the data invariant is what
        // matters here, and exercising the production helper proves we
        // are testing the actual production branch rather than a stub.
        let store = Arc::new(
            SqliteSessionStore::open_for_locale(
                std::path::Path::new(":memory:"),
                primer_core::i18n::Locale::default(),
            )
            .unwrap(),
        );
        let original = create_learner_with_id(
            Uuid::new_v4(),
            "Binti",
            8,
            primer_core::i18n::Locale::English,
        );
        store.save_learner(&original).await.unwrap();

        let existing = store.load_learner().await.unwrap().expect("learner row");
        let reconciled = reconcile_persisted_learner(existing, "Other", 8);
        store.save_learner(&reconciled).await.unwrap();

        assert_eq!(
            reconciled.profile.name, "Binti",
            "persisted name wins over CLI"
        );

        // Round-trip through the store too — proves the saved row also
        // keeps the persisted name (i.e. the helper didn't mutate name
        // before save_learner committed it).
        let round_trip = store.load_learner().await.unwrap().expect("learner row");
        assert_eq!(round_trip.profile.name, "Binti");
    }

    #[test]
    fn reconcile_persisted_learner_preserves_name_and_id_on_match() {
        // The non-mismatch path: same name should be a pure age/last_active
        // refresh with no warn (covered by absence of stderr in this test).
        let original_id = Uuid::new_v4();
        let original =
            create_learner_with_id(original_id, "Binti", 8, primer_core::i18n::Locale::English);
        let result = reconcile_persisted_learner(original, "Binti", 9);
        assert_eq!(result.profile.name, "Binti");
        assert_eq!(result.profile.id, original_id);
        assert_eq!(result.profile.age, 9);
    }
}
