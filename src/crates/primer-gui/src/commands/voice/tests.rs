use super::*;

/// Pin the `MissingAsset` serialisation shape. The frontend switches on
/// `asset.kind` and reads `approx_size_mb` to estimate download budget —
/// a field rename here silently breaks the asset-consent modal.
#[test]
fn missing_asset_serialises_with_snake_case_kind() {
    let m = MissingAsset {
        kind: super::kind::WHISPER_MODEL.into(),
        path: "/tmp/foo.bin".into(),
        suggested_url: Some("https://example.com/foo.bin".into()),
        approx_size_mb: Some(470),
    };
    let json = serde_json::to_value(&m).unwrap();
    assert_eq!(json["kind"], "whisper_model");
    assert_eq!(json["approx_size_mb"], 470);
}

#[cfg(feature = "speech")]
#[test]
fn supertonic_kind_constants_match_the_asset_table() {
    use primer_speech::locale_defaults::supertonic_assets;
    let table_kinds: std::collections::BTreeSet<&str> =
        supertonic_assets().iter().map(|a| a.kind).collect();
    let const_kinds: std::collections::BTreeSet<&str> = [
        kind::SUPERTONIC_VECTOR_ESTIMATOR,
        kind::SUPERTONIC_VOCODER,
        kind::SUPERTONIC_TEXT_ENCODER,
        kind::SUPERTONIC_DURATION_PREDICTOR,
        kind::SUPERTONIC_TTS_CONFIG,
        kind::SUPERTONIC_UNICODE_INDEXER,
        kind::SUPERTONIC_VOICE_STYLE,
    ]
    .into_iter()
    .collect();
    assert_eq!(const_kinds, table_kinds);
}

// Trust-boundary invariant: `MissingAsset` must NEVER implement
// `Deserialize`. The IPC direction is server→webview only — if a
// future contributor re-derives `Deserialize` (e.g. "just to
// round-trip through a frontend cache"), the trust-boundary
// hardening from #90 silently regresses. This compile-time check is
// the load-bearing structural guarantee; the doc comment on
// `MissingAsset` itself explains the rationale. If you genuinely
// need to round-trip the type back, define a separate echoed-
// identity DTO instead of re-deriving `Deserialize` here.
static_assertions::assert_not_impl_any!(MissingAsset: serde::de::DeserializeOwned);

/// Pin the existing English `VoiceStateCopy` strings byte-identically.
/// This is the regression witness for the i18n refactor that moves the
/// six display strings into `primer_pedagogy::prompt_pack`. The pack
/// values must reproduce these strings exactly; any drift here would
/// silently change UI copy.
#[test]
fn voice_state_copy_english_strings_pinned() {
    let copy = VoiceStateCopy::for_locale(&primer_core::i18n::Locale::English);
    assert_eq!(copy.listen_label, "Listening…");
    assert_eq!(copy.listen_hint, "take your time");
    assert_eq!(copy.thinking_label, "Thinking…");
    assert_eq!(copy.thinking_hint, "the Primer is working on a reply");
    assert_eq!(copy.speak_label, "Speaking…");
    assert_eq!(copy.speak_hint, "let the Primer finish");
}

/// Pin the existing German `VoiceStateCopy` strings byte-identically.
/// Sibling of [`voice_state_copy_english_strings_pinned`] — see that
/// test's doc for rationale.
#[test]
fn voice_state_copy_german_strings_pinned() {
    let copy = VoiceStateCopy::for_locale(&primer_core::i18n::Locale::German);
    assert_eq!(copy.listen_label, "Höre zu…");
    assert_eq!(copy.listen_hint, "lass dir Zeit");
    assert_eq!(copy.thinking_label, "Denke nach…");
    assert_eq!(copy.thinking_hint, "der Primer überlegt eine Antwort");
    assert_eq!(copy.speak_label, "Spreche…");
    assert_eq!(copy.speak_hint, "lass den Primer ausreden");
}

/// `voice_mode_available` must mirror `cfg!(feature = "speech")` exactly.
/// The frontend uses this flag at launch (independent of session state)
/// to enable / disable the voice toggle. Drift here would silently
/// break the consent-modal flow or the never-enabled regression that
/// motivated splitting this off from `current_session_info`.
#[tokio::test]
async fn voice_mode_available_matches_cfg_feature_speech() {
    let result = voice_mode_available().await.expect("command never errors");
    assert_eq!(result, cfg!(feature = "speech"));
}

/// The macOS-native speech capability reflects the compiled feature
/// set exactly. Written against the same `cfg!` expression as the
/// command so it holds on both a default build (false) and a
/// `--features macos-native` build (true on macOS).
#[tokio::test]
async fn macos_native_speech_available_matches_cfg() {
    let expected = cfg!(all(
        target_os = "macos",
        any(feature = "macos-native", feature = "macos-native-26"),
    ));
    assert_eq!(
        macos_native_speech_available()
            .await
            .expect("command never errors"),
        expected
    );
}

/// The Supertonic TTS capability reflects the compiled feature set
/// exactly. Holds on a default build (false) and a `--features
/// supertonic` build (true).
#[tokio::test]
async fn supertonic_tts_available_matches_cfg() {
    assert_eq!(
        supertonic_tts_available()
            .await
            .expect("command never errors"),
        cfg!(feature = "supertonic")
    );
}

/// Pin the `StartVoiceModeError` tag format. The frontend branches on
/// `err.kind` — a rename or format change here silently breaks the
/// banner rendering and the asset-missing detection path.
#[test]
fn start_voice_mode_error_uses_kind_tag() {
    let err = StartVoiceModeError::NotBuilt;
    let json = serde_json::to_value(&err).unwrap();
    assert_eq!(json["kind"], "not_built");

    let err = StartVoiceModeError::AssetMissing { entries: vec![] };
    let json = serde_json::to_value(&err).unwrap();
    assert_eq!(json["kind"], "asset_missing");
    assert_eq!(json["entries"], serde_json::json!([]));

    let err = StartVoiceModeError::Other {
        message: "test message".into(),
    };
    let json = serde_json::to_value(&err).unwrap();
    assert_eq!(json["kind"], "other");
    assert_eq!(json["message"], "test message");
}

#[cfg(feature = "speech")]
#[test]
fn missing_to_error_offers_download_when_auto_download_enabled() {
    let missing = crate::voice::assets::AssetMissing {
        entries: vec![MissingAsset {
            kind: kind::SUPERTONIC_VOCODER.into(),
            path: std::path::PathBuf::from("/x/vocoder.onnx"),
            suggested_url: Some("https://example/vocoder.onnx".into()),
            approx_size_mb: Some(97),
        }],
        locale: "en".into(),
        approx_total_mb: 97,
    };
    let err = missing_to_error(false, missing);
    match err {
        StartVoiceModeError::AssetMissing { entries } => assert_eq!(entries.len(), 1),
        other => panic!("expected AssetMissing, got {other:?}"),
    }
}

#[cfg(feature = "speech")]
#[test]
fn missing_to_error_blocks_download_when_disabled() {
    let missing = crate::voice::assets::AssetMissing {
        entries: vec![MissingAsset {
            kind: kind::SUPERTONIC_VOCODER.into(),
            path: std::path::PathBuf::from("/x/vocoder.onnx"),
            suggested_url: Some("https://example/vocoder.onnx".into()),
            approx_size_mb: Some(97),
        }],
        locale: "en".into(),
        approx_total_mb: 97,
    };
    let err = missing_to_error(true, missing);
    match err {
        StartVoiceModeError::AutoDownloadDisabled { entries } => {
            assert_eq!(
                entries.len(),
                1,
                "entries carried for the informational banner"
            );
        }
        other => panic!("expected AutoDownloadDisabled, got {other:?}"),
    }
}

// ─── Issue #102 polished follow-up: preserve sticky toggle ──────

/// Synthesize an `ActiveVoiceLoop` whose task completes the moment
/// `stop_tx` is signaled — mirrors the production join contract
/// without spinning up cpal/whisper/piper. Returned ready to stash
/// into `state.voice`.
#[cfg(feature = "speech")]
fn fake_voice_loop(info: crate::types::SessionInfo) -> crate::state::ActiveVoiceLoop {
    use primer_speech::voice_loop::VoiceLoopError;
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let (cancel_tx, _cancel_rx) = tokio::sync::mpsc::channel::<()>(8);
    let join: tokio::task::JoinHandle<Result<(), VoiceLoopError>> = tokio::spawn(async move {
        let _ = stop_rx.await;
        Ok(())
    });
    crate::state::ActiveVoiceLoop {
        join,
        stop_tx,
        cancel_response_tx: cancel_tx,
        info,
    }
}

#[cfg(feature = "speech")]
fn make_synthetic_session_info() -> crate::types::SessionInfo {
    use crate::types::LearnerSummary;
    crate::types::SessionInfo {
        session_id: None,
        learner: LearnerSummary {
            id: uuid::Uuid::nil(),
            name: "test".into(),
            age: 8,
            concept_count: 0,
        },
        backend_kind: "stub".into(),
        main_model: "stub".into(),
        backend_notice: None,
        retrieval_notice: None,
        locale: "en".into(),
        voice_mode_available: true,
    }
}

/// User pressed Stop (or a start-error teardown happened) →
/// `preserve_toggle = false` → the sticky toggle is durably
/// flipped off so the next launch doesn't auto-resume voice mode.
#[cfg(feature = "speech")]
#[tokio::test]
async fn stop_voice_mode_inner_flips_toggle_off_by_default() {
    use crate::config::GuiConfig;
    use crate::state::AppState;
    use tempfile::TempDir;

    let home = TempDir::new().unwrap();
    let mut cfg = GuiConfig::default();
    cfg.speech.voice_mode_enabled = true;
    let state = AppState::new(home.path().to_path_buf(), cfg);
    *state.voice.lock().await = Some(fake_voice_loop(make_synthetic_session_info()));

    stop_voice_mode_inner(&state, false).await.unwrap();

    assert!(state.voice.lock().await.is_none(), "loop must be cleared");
    assert!(
        !state.config.lock().await.speech.voice_mode_enabled,
        "preserve_toggle=false must flip the sticky toggle off"
    );
}

/// Session switch → `preserve_toggle = true` → the sticky toggle
/// stays at its current value so the frontend can auto-restart
/// voice mode under the new locale (closes #102 polished
/// follow-up).
#[cfg(feature = "speech")]
#[tokio::test]
async fn stop_voice_mode_inner_preserves_toggle_when_requested() {
    use crate::config::GuiConfig;
    use crate::state::AppState;
    use tempfile::TempDir;

    let home = TempDir::new().unwrap();
    let mut cfg = GuiConfig::default();
    cfg.speech.voice_mode_enabled = true;
    let state = AppState::new(home.path().to_path_buf(), cfg);
    *state.voice.lock().await = Some(fake_voice_loop(make_synthetic_session_info()));

    stop_voice_mode_inner(&state, true).await.unwrap();

    assert!(state.voice.lock().await.is_none(), "loop must be cleared");
    assert!(
        state.config.lock().await.speech.voice_mode_enabled,
        "preserve_toggle=true must leave the sticky toggle untouched so the frontend's \
             post-`start_session` `primerRestoreVoiceMode` sees true and auto-restarts"
    );
}

// ─── Issue #366: a failed start keeps the text session ──────────

/// A mock Tauri app whose `AppState` holds an active stub text session, a
/// sticky voice toggle that is on (the auto-start path), and an empty home
/// (so no voice assets are cached). The `TempDir` must outlive the app.
#[cfg(any(feature = "speech", feature = "android-native"))]
async fn app_with_text_session() -> (tempfile::TempDir, tauri::App<tauri::test::MockRuntime>) {
    use crate::config::GuiConfig;
    use crate::state::AppState;
    use tauri::Manager;

    let home = tempfile::TempDir::new().unwrap();
    let mut cfg = GuiConfig::default();
    cfg.persistence.no_persist = true;
    cfg.embedder.kind = "none".to_string();
    cfg.speech.voice_mode_enabled = true;
    let active = crate::wiring::build_active_session(home.path(), &cfg)
        .await
        .unwrap();

    let app = tauri::test::mock_app();
    app.manage(AppState::new(home.path().to_path_buf(), cfg));
    *app.state::<AppState>().session.lock().await = Some(active);
    (home, app)
}

#[cfg(any(feature = "speech", feature = "android-native"))]
type SharedDm = std::sync::Arc<tokio::sync::Mutex<primer_pedagogy::DialogueManager>>;

/// The dialogue manager of the app's current text session, to check after a
/// failed start that the *same* session survived (not a rebuilt one).
#[cfg(any(feature = "speech", feature = "android-native"))]
async fn current_dm(app: &tauri::App<tauri::test::MockRuntime>) -> SharedDm {
    use tauri::Manager;
    let state = app.state::<crate::state::AppState>();
    let guard = state.session.lock().await;
    std::sync::Arc::clone(&guard.as_ref().expect("a text session").dialogue_manager)
}

/// After a failed start, the same text session must still be active and the
/// sticky toggle untouched. (No voice loop runs in these tests, so the
/// toggle half guards against a new unconditional flip, not the reorder.)
#[cfg(any(feature = "speech", feature = "android-native"))]
async fn assert_text_session_untouched(
    app: &tauri::App<tauri::test::MockRuntime>,
    dm_before: &SharedDm,
) {
    use tauri::Manager;
    let state = app.state::<crate::state::AppState>();
    let session = state.session.lock().await;
    let active = session
        .as_ref()
        .expect("a failed start must leave the text session alive");
    assert!(
        std::sync::Arc::ptr_eq(&active.dialogue_manager, dm_before),
        "a failed start must keep the original text session, not rebuild it"
    );
    assert!(
        state.config.lock().await.speech.voice_mode_enabled,
        "a failed start must not touch the sticky toggle"
    );
}

/// Voice-model consent path: voice assets are not on disk, so
/// `start_voice_mode` must fail with `AssetMissing` *before* it tears
/// anything down. The text session then survives the consent-modal
/// "Cancel".
#[cfg(feature = "speech")]
#[tokio::test]
async fn failed_start_keeps_the_text_session_and_toggle() {
    use tauri::Manager;
    let (_home, app) = app_with_text_session().await;
    let dm_before = current_dm(&app).await;

    let err = super::start_voice_mode(app.state(), app.handle().clone())
        .await
        .expect_err("assets are missing, so start must fail");

    assert!(
        matches!(err, StartVoiceModeError::AssetMissing { .. }),
        "expected AssetMissing, got {err:?}"
    );
    assert_text_session_untouched(&app, &dm_before).await;
}

/// Backend-init path: the asset files exist but are garbage, so asset
/// resolution passes and `build_loop_backends` fails. That is the last
/// fallible step before the teardown, so this pins the teardown below it.
#[cfg(feature = "speech")]
#[tokio::test]
async fn failed_backend_init_keeps_the_text_session_and_toggle() {
    use primer_core::i18n::Locale;
    use tauri::Manager;
    let (home, app) = app_with_text_session().await;
    let dm_before = current_dm(&app).await;

    let cfg = app
        .state::<crate::state::AppState>()
        .config
        .lock()
        .await
        .clone();
    let (stt, tts) = cfg.speech.resolve_backends();
    let missing = crate::voice::assets::resolve_voice_assets(
        home.path(),
        &cfg.speech,
        &Locale::from_pack_id(&cfg.learner.locale).unwrap_or_default(),
        stt,
        tts,
    )
    .expect_err("a fresh home has no voice assets");
    for entry in &missing.entries {
        std::fs::create_dir_all(entry.path.parent().unwrap()).unwrap();
        std::fs::write(&entry.path, b"not a model").unwrap();
    }

    let err = super::start_voice_mode(app.state(), app.handle().clone())
        .await
        .expect_err("garbage model files, so the backend build must fail");

    assert!(
        matches!(&err, StartVoiceModeError::Other { message } if message.starts_with("backend init")),
        "expected a backend-init error, got {err:?}"
    );
    assert_text_session_untouched(&app, &dm_before).await;
}

/// Android path: on a host build the JNI bridge cannot be constructed, which
/// stands in for any bridge-init failure. It must fail before the teardown,
/// too. (The permission check right after it shares the same position.)
#[cfg(feature = "android-native")]
#[tokio::test]
async fn failed_android_start_keeps_the_text_session_and_toggle() {
    use tauri::Manager;
    let (_home, app) = app_with_text_session().await;
    let dm_before = current_dm(&app).await;

    let err =
        crate::commands::voice_android::start_voice_mode_android(app.state(), app.handle().clone())
            .await
            .expect_err("no JNI bridge on a host build, so start must fail");

    assert!(
        matches!(&err, StartVoiceModeError::Other { message } if message.starts_with("android bridge init")),
        "expected the bridge-init error, got {err:?}"
    );
    assert_text_session_untouched(&app, &dm_before).await;
}

// ─── Issue #372: consent-modal Cancel persists the toggle off ───

/// The consent modal's "Cancel" calls `stop_voice_mode` after an
/// `AssetMissing` start, when no voice loop ever ran. The off-state must
/// still be persisted to `gui-config.json`, or the modal re-appears on every
/// launch. With no loop to stop, the text session is left alone (#352 item 3
/// only drops it after a real voice stop).
#[cfg(feature = "speech")]
#[tokio::test]
async fn stop_without_a_loop_persists_the_toggle_off() {
    use tauri::Manager;
    let (home, app) = app_with_text_session().await;
    let dm_before = current_dm(&app).await;

    super::stop_voice_mode(app.state()).await.unwrap();

    let state = app.state::<crate::state::AppState>();
    assert!(!state.config.lock().await.speech.voice_mode_enabled);
    let on_disk = crate::config::load(home.path()).unwrap();
    assert!(
        !on_disk.speech.voice_mode_enabled,
        "the off-state must survive a relaunch"
    );
    let session = state.session.lock().await;
    assert!(
        std::sync::Arc::ptr_eq(
            &session
                .as_ref()
                .expect("text session kept")
                .dialogue_manager,
            &dm_before
        ),
        "no loop ran, so the text session must survive"
    );
}
