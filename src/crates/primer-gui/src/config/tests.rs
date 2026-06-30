use std::fs;

use super::*;
use tempfile::TempDir;

#[test]
fn load_missing_returns_defaults() {
    let dir = TempDir::new().unwrap();
    let cfg = load(dir.path()).unwrap();
    assert_eq!(cfg, GuiConfig::default());
    // Missing file does NOT create one — pure read.
    assert!(!config_path(dir.path()).exists());
}

#[test]
fn load_malformed_surfaces_parse_error() {
    let dir = TempDir::new().unwrap();
    let path = config_path(dir.path());
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, b"{ this is not json").unwrap();

    let err = load(dir.path()).unwrap_err();
    match err {
        ConfigError::Parse { path: p, .. } => {
            assert_eq!(p, path, "error must name the offending path");
        }
        other => panic!("expected Parse error, got {other:?}"),
    }
}

#[test]
fn save_then_load_round_trips() {
    let dir = TempDir::new().unwrap();
    let mut cfg = GuiConfig::default();
    cfg.learner.name = "Binti".to_string();
    cfg.learner.age = 9;
    cfg.learner.locale = "de".to_string();
    cfg.backend.kind = "cloud".to_string();
    cfg.backend.model = Some("claude-sonnet-4-6".to_string());
    cfg.backend.api_key_source = ApiKeySource::Inline {
        key: "test-key-not-real".to_string(),
    };
    cfg.embedder.kind = "fastembed".to_string();
    cfg.vocab.max_per_prompt = Some(6);
    cfg.breaks.after_mins = 45;
    cfg.persistence.no_persist = true;

    save(dir.path(), &cfg).unwrap();
    let round_trip = load(dir.path()).unwrap();
    assert_eq!(round_trip, cfg);
}

#[test]
fn save_creates_primer_subdirectory_if_missing() {
    let dir = TempDir::new().unwrap();
    let primer_dir = dir.path().join(primer_engine::PRIMER_HOME_DIR);
    assert!(!primer_dir.exists());

    save(dir.path(), &GuiConfig::default()).unwrap();
    assert!(primer_dir.is_dir());
    assert!(config_path(dir.path()).exists());
}

#[test]
fn save_is_atomic_no_temp_left_on_success() {
    let dir = TempDir::new().unwrap();
    save(dir.path(), &GuiConfig::default()).unwrap();
    let tmp = config_path(dir.path()).with_extension("json.tmp");
    assert!(!tmp.exists(), "temp file must be renamed away on success");
}

#[cfg(unix)]
#[test]
fn save_sets_mode_0600_on_unix() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new().unwrap();
    save(dir.path(), &GuiConfig::default()).unwrap();
    let metadata = fs::metadata(config_path(dir.path())).unwrap();
    let mode = metadata.permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "config file must be user-read-write only");
}

#[test]
fn forward_compatibility_unknown_field_is_ignored() {
    // Adding a future field shouldn't poison existing configs; serde
    // skips unknown fields by default. This test pins that contract.
    let dir = TempDir::new().unwrap();
    let path = config_path(dir.path());
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let json = r#"{
            "learner": {"name": "Binti", "age": 9, "locale": "de"},
            "future_field_we_dont_know_about": {"x": 1}
        }"#;
    fs::write(&path, json).unwrap();

    let cfg = load(dir.path()).unwrap();
    assert_eq!(cfg.learner.name, "Binti");
    assert_eq!(cfg.learner.age, 9);
    assert_eq!(cfg.learner.locale, "de");
}

#[test]
fn partial_json_fills_unspecified_fields_with_defaults() {
    // serde(default) on every field/section means an older config
    // missing newer sections still loads cleanly.
    let dir = TempDir::new().unwrap();
    let path = config_path(dir.path());
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let json = r#"{"learner": {"name": "Ada", "age": 7, "locale": "en"}}"#;
    fs::write(&path, json).unwrap();

    let cfg = load(dir.path()).unwrap();
    assert_eq!(cfg.learner.name, "Ada");
    // All other sections come from defaults.
    assert_eq!(cfg.backend, BackendConfig::default());
    assert_eq!(cfg.embedder, EmbedderConfig::default());
    assert_eq!(cfg.ui, UiConfig::default());
}

#[cfg(feature = "embedding")]
#[test]
fn embedder_default_is_fastembed_with_feature() {
    assert_eq!(EmbedderConfig::default().kind, "fastembed");
}

#[cfg(not(feature = "embedding"))]
#[test]
fn embedder_default_is_none_without_feature() {
    assert_eq!(EmbedderConfig::default().kind, "none");
}

// ─── View / Update DTO tests ─────────────────────────────────────

#[test]
fn view_redacts_inline_api_key() {
    // The single most important security test: the inline key must
    // never appear in the JSON the frontend receives.
    let mut cfg = GuiConfig::default();
    cfg.backend.api_key_source = ApiKeySource::Inline {
        key: "sk-secret-token-aaa".to_string(),
    };
    let view: GuiConfigView = (&cfg).into();
    let json = serde_json::to_string(&view).unwrap();
    assert!(
        !json.contains("sk-secret-token-aaa"),
        "redacted view must not contain the key: {json}"
    );
    assert!(
        json.contains("\"has_key\":true"),
        "view must signal a key is set: {json}"
    );
}

#[test]
fn view_redacts_empty_inline_key_as_has_key_false() {
    let mut cfg = GuiConfig::default();
    cfg.backend.api_key_source = ApiKeySource::Inline { key: String::new() };
    let view: GuiConfigView = (&cfg).into();
    assert_eq!(
        view.backend.api_key_source,
        ApiKeySourceView::Inline { has_key: false }
    );
}

#[test]
fn view_passes_env_source_through() {
    let cfg = GuiConfig::default();
    let view: GuiConfigView = (&cfg).into();
    assert_eq!(view.backend.api_key_source, ApiKeySourceView::Env);
}

#[test]
fn view_redacts_inline_openai_compat_key() {
    // The openai-compat key gets the SAME redaction discipline as the
    // cloud key — it must never appear in the JSON the frontend sees.
    let mut cfg = GuiConfig::default();
    cfg.backend.openai_compat_api_key_source = ApiKeySource::Inline {
        key: "sk-oai-secret-bbb".to_string(),
    };
    let view: GuiConfigView = (&cfg).into();
    let json = serde_json::to_string(&view).unwrap();
    assert!(
        !json.contains("sk-oai-secret-bbb"),
        "redacted view must not contain the openai-compat key: {json}"
    );
    assert_eq!(
        view.backend.openai_compat_api_key_source,
        ApiKeySourceView::Inline { has_key: true },
    );
}

#[test]
fn default_openai_compat_url_matches_cli() {
    let cfg = GuiConfig::default();
    assert_eq!(cfg.backend.openai_compat_url, "http://localhost:8000");
    assert_eq!(
        cfg.backend.openai_compat_api_key_source,
        ApiKeySource::Env,
        "openai-compat key defaults to env (OPENAI_COMPAT_API_KEY)"
    );
    assert_eq!(cfg.embedder.openai_compat_url, None);
}

#[test]
fn default_qnn_paths_are_none() {
    let cfg = GuiConfig::default();
    assert_eq!(cfg.backend.qnn_bundle_dir, None);
    assert_eq!(cfg.backend.qnn_qairt_lib_dir, None);
}

#[test]
fn older_config_without_qnn_fields_loads_with_defaults() {
    // An on-disk config from before the QNN GUI picker has no
    // `qnn_bundle_dir` / `qnn_qairt_lib_dir` keys. serde defaults must
    // inject `None` for both without a migration step.
    let dir = TempDir::new().unwrap();
    let path = config_path(dir.path());
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        r#"{
                "learner": {"name": "Ada", "age": 7, "locale": "en"},
                "backend": {
                    "kind": "cloud", "model": null,
                    "ollama_url": "http://localhost:11434",
                    "openai_compat_url": "http://localhost:8000",
                    "api_key_source": {"kind": "env"},
                    "openai_compat_api_key_source": {"kind": "env"}
                }
            }"#,
    )
    .unwrap();

    let cfg = load(dir.path()).unwrap();
    assert_eq!(cfg.backend.kind, "cloud");
    assert_eq!(cfg.backend.qnn_bundle_dir, None);
    assert_eq!(cfg.backend.qnn_qairt_lib_dir, None);
}

#[test]
fn qnn_paths_round_trip_through_disk() {
    let dir = TempDir::new().unwrap();
    let mut cfg = GuiConfig::default();
    cfg.backend.kind = "qnn".to_string();
    cfg.backend.qnn_bundle_dir = Some("/bundles/qwen3-4b".into());
    cfg.backend.qnn_qairt_lib_dir = Some("/qairt/lib/aarch64-android".into());

    save(dir.path(), &cfg).unwrap();
    let round_trip = load(dir.path()).unwrap();
    assert_eq!(round_trip, cfg);
}

#[test]
fn qnn_paths_pass_through_view_verbatim() {
    // Unlike API keys, the QNN paths are not secrets — the view must
    // carry them through unredacted so the settings form can show the
    // currently-configured paths.
    let mut cfg = GuiConfig::default();
    cfg.backend.qnn_bundle_dir = Some("/bundles/qwen3-4b".into());
    cfg.backend.qnn_qairt_lib_dir = Some("/qairt/lib/aarch64-android".into());
    let view: GuiConfigView = (&cfg).into();
    assert_eq!(
        view.backend.qnn_bundle_dir,
        Some("/bundles/qwen3-4b".into())
    );
    assert_eq!(
        view.backend.qnn_qairt_lib_dir,
        Some("/qairt/lib/aarch64-android".into())
    );
}

#[test]
fn qnn_paths_pass_through_update_verbatim() {
    // The write path carries the QNN paths straight to the resolved
    // config (no `Keep` semantics — they're not secrets).
    let current = GuiConfig::default();
    let update_json = r#"{
            "learner": {"name": "Ada", "age": 7, "locale": "en"},
            "backend": {
                "kind": "qnn",
                "model": null,
                "ollama_url": "http://localhost:11434",
                "openai_compat_url": "http://localhost:8000",
                "api_key_source": {"kind": "keep"},
                "openai_compat_api_key_source": {"kind": "keep"},
                "reasoning_markers": "",
                "qnn_bundle_dir": "/bundles/qwen3-4b",
                "qnn_qairt_lib_dir": "/qairt/lib/aarch64-android",
                "gguf_path": null,
                "llamacpp_gpu_layers": null,
                "llamacpp_n_ctx": null,
                "fallback_backend": null,
                "fallback_model": null,
                "primary_ttft_budget_ms": null,
                "router_mode": "local-only"
            },
            "classifier": {"match_main": true, "kind": null, "model": null, "timeout_ms": 3000},
            "extractor": {"match_main": true, "kind": null, "model": null, "timeout_ms": 5000},
            "comprehension": {"match_main": true, "kind": null, "model": null, "timeout_ms": 5000},
            "embedder": {"kind": "none", "model": null, "ollama_url": null, "openai_compat_url": null},
            "vocab": {"max_per_prompt": null},
            "breaks": {"after_mins": 30},
            "persistence": {"session_db": null, "knowledge_db": null, "no_persist": false},
            "ui": {"sidebar_open": true, "last_section": "current_turn"},
            "speech": {"voice_mode_enabled": false, "disable_auto_download": false, "mic_silence_ms": 600, "overrides": {}}
        }"#;
    let update: GuiConfigUpdate = serde_json::from_str(update_json).unwrap();
    let resolved = update.into_config(&current);
    assert_eq!(resolved.backend.kind, "qnn");
    assert_eq!(
        resolved.backend.qnn_bundle_dir,
        Some("/bundles/qwen3-4b".into())
    );
    assert_eq!(
        resolved.backend.qnn_qairt_lib_dir,
        Some("/qairt/lib/aarch64-android".into())
    );
}

#[test]
fn default_reasoning_markers_is_empty() {
    let cfg = GuiConfig::default();
    assert_eq!(cfg.backend.reasoning_markers, "");
}

#[test]
fn reasoning_markers_round_trip_through_disk() {
    let dir = TempDir::new().unwrap();
    let mut cfg = GuiConfig::default();
    cfg.backend.kind = "ollama".to_string();
    cfg.backend.reasoning_markers = "[[r]] [[/r]]\n<x> </x>".to_string();

    save(dir.path(), &cfg).unwrap();
    let round_trip = load(dir.path()).unwrap();
    assert_eq!(round_trip, cfg);
}

#[test]
fn reasoning_markers_pass_through_view_verbatim() {
    // Not a secret — the view must carry the raw textarea text through
    // unredacted so the settings form can re-show what the user typed.
    let mut cfg = GuiConfig::default();
    cfg.backend.reasoning_markers = "[[r]] [[/r]]".to_string();
    let view: GuiConfigView = (&cfg).into();
    assert_eq!(view.backend.reasoning_markers, "[[r]] [[/r]]");
}

#[test]
fn reasoning_markers_pass_through_update_verbatim() {
    let current = GuiConfig::default();
    let update_json = r#"{
            "learner": {"name": "Ada", "age": 7, "locale": "en"},
            "backend": {
                "kind": "ollama",
                "model": null,
                "ollama_url": "http://localhost:11434",
                "openai_compat_url": "http://localhost:8000",
                "api_key_source": {"kind": "keep"},
                "openai_compat_api_key_source": {"kind": "keep"},
                "reasoning_markers": "[[r]] [[/r]]",
                "qnn_bundle_dir": null,
                "qnn_qairt_lib_dir": null,
                "gguf_path": null,
                "llamacpp_gpu_layers": null,
                "llamacpp_n_ctx": null,
                "fallback_backend": null,
                "fallback_model": null,
                "primary_ttft_budget_ms": null,
                "router_mode": "local-only"
            },
            "classifier": {"match_main": true, "kind": null, "model": null, "timeout_ms": 3000},
            "extractor": {"match_main": true, "kind": null, "model": null, "timeout_ms": 5000},
            "comprehension": {"match_main": true, "kind": null, "model": null, "timeout_ms": 5000},
            "embedder": {"kind": "none", "model": null, "ollama_url": null, "openai_compat_url": null},
            "vocab": {"max_per_prompt": null},
            "breaks": {"after_mins": 30},
            "persistence": {"session_db": null, "knowledge_db": null, "no_persist": false},
            "ui": {"sidebar_open": true, "last_section": "current_turn"},
            "speech": {"voice_mode_enabled": false, "disable_auto_download": false, "mic_silence_ms": 600, "overrides": {}}
        }"#;
    let update: GuiConfigUpdate = serde_json::from_str(update_json).unwrap();
    let resolved = update.into_config(&current);
    assert_eq!(resolved.backend.reasoning_markers, "[[r]] [[/r]]");
}

#[test]
fn default_fallback_is_none() {
    let cfg = GuiConfig::default();
    assert_eq!(cfg.backend.fallback_backend, None);
    assert_eq!(cfg.backend.fallback_model, None);
}

#[test]
fn fallback_round_trips_through_disk() {
    let dir = TempDir::new().unwrap();
    let mut cfg = GuiConfig::default();
    cfg.backend.kind = "llamacpp".to_string();
    cfg.backend.fallback_backend = Some("cloud".to_string());
    cfg.backend.fallback_model = Some("claude-opus-4-7".to_string());

    save(dir.path(), &cfg).unwrap();
    let round_trip = load(dir.path()).unwrap();
    assert_eq!(round_trip, cfg);
}

#[test]
fn diagnostics_defaults_off() {
    // The privacy posture: a fresh install records nothing (issue #228).
    assert!(!GuiConfig::default().diagnostics.qnn_metrics_enabled);
}

#[test]
fn diagnostics_round_trips_through_disk() {
    let dir = TempDir::new().unwrap();
    let mut cfg = GuiConfig::default();
    cfg.diagnostics.qnn_metrics_enabled = true;
    save(dir.path(), &cfg).unwrap();
    let round_trip = load(dir.path()).unwrap();
    assert!(round_trip.diagnostics.qnn_metrics_enabled);
    assert_eq!(round_trip, cfg);
}

#[test]
fn older_config_without_diagnostics_loads_off() {
    // A `gui-config.json` written before #228 has no `diagnostics` key. The
    // struct-level `#[serde(default)]` on GuiConfig must fill it with the
    // OFF default rather than erroring — the on-disk mirror of
    // `update_without_diagnostics_keeps_off_default` (which covers the
    // IPC Update path). Mirrors `older_config_without_fallback_fields_*`.
    let dir = TempDir::new().unwrap();
    let path = config_path(dir.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let legacy = r#"{
            "learner": {"name": "Ada", "age": 7, "locale": "en"},
            "backend": {"kind": "stub", "model": null, "ollama_url": "http://localhost:11434"}
        }"#;
    std::fs::write(&path, legacy).unwrap();
    let cfg = load(dir.path()).unwrap();
    assert!(!cfg.diagnostics.qnn_metrics_enabled);
}

#[test]
fn diagnostics_passes_through_view_verbatim() {
    let mut cfg = GuiConfig::default();
    cfg.diagnostics.qnn_metrics_enabled = true;
    let view: GuiConfigView = (&cfg).into();
    assert!(view.diagnostics.qnn_metrics_enabled);
}

#[test]
fn update_without_diagnostics_keeps_off_default() {
    // An older settings.js (pre-#228) sends no `diagnostics` block. The
    // field-level `#[serde(default)]` must accept that and resolve to OFF
    // — never erroring, never silently enabling recording.
    let current = GuiConfig::default();
    let update_json = r#"{
            "learner": {"name": "Ada", "age": 7, "locale": "en"},
            "backend": {
                "kind": "stub", "model": null,
                "ollama_url": "http://localhost:11434",
                "openai_compat_url": "http://localhost:8000",
                "api_key_source": {"kind": "keep"},
                "openai_compat_api_key_source": {"kind": "keep"},
                "reasoning_markers": "",
                "qnn_bundle_dir": null, "qnn_qairt_lib_dir": null,
                "gguf_path": null, "llamacpp_gpu_layers": null, "llamacpp_n_ctx": null,
                "fallback_backend": null, "fallback_model": null,
                "primary_ttft_budget_ms": null, "router_mode": "local-only"
            },
            "classifier": {"match_main": true, "kind": null, "model": null, "timeout_ms": 3000},
            "extractor": {"match_main": true, "kind": null, "model": null, "timeout_ms": 5000},
            "comprehension": {"match_main": true, "kind": null, "model": null, "timeout_ms": 5000},
            "embedder": {"kind": "none", "model": null, "ollama_url": null, "openai_compat_url": null},
            "vocab": {"max_per_prompt": null},
            "breaks": {"after_mins": 30},
            "persistence": {"session_db": null, "knowledge_db": null, "no_persist": false},
            "ui": {"sidebar_open": true, "last_section": "current_turn"},
            "speech": {"voice_mode_enabled": false, "disable_auto_download": false, "mic_silence_ms": 600, "overrides": {}}
        }"#;
    let update: GuiConfigUpdate = serde_json::from_str(update_json).unwrap();
    let resolved = update.into_config(&current);
    assert!(!resolved.diagnostics.qnn_metrics_enabled);
}

#[test]
fn update_with_diagnostics_enables_recording() {
    let current = GuiConfig::default();
    let update_json = r#"{
            "learner": {"name": "Ada", "age": 7, "locale": "en"},
            "backend": {
                "kind": "stub", "model": null,
                "ollama_url": "http://localhost:11434",
                "openai_compat_url": "http://localhost:8000",
                "api_key_source": {"kind": "keep"},
                "openai_compat_api_key_source": {"kind": "keep"},
                "reasoning_markers": "",
                "qnn_bundle_dir": null, "qnn_qairt_lib_dir": null,
                "gguf_path": null, "llamacpp_gpu_layers": null, "llamacpp_n_ctx": null,
                "fallback_backend": null, "fallback_model": null,
                "primary_ttft_budget_ms": null, "router_mode": "local-only"
            },
            "classifier": {"match_main": true, "kind": null, "model": null, "timeout_ms": 3000},
            "extractor": {"match_main": true, "kind": null, "model": null, "timeout_ms": 5000},
            "comprehension": {"match_main": true, "kind": null, "model": null, "timeout_ms": 5000},
            "embedder": {"kind": "none", "model": null, "ollama_url": null, "openai_compat_url": null},
            "vocab": {"max_per_prompt": null},
            "breaks": {"after_mins": 30},
            "persistence": {"session_db": null, "knowledge_db": null, "no_persist": false},
            "ui": {"sidebar_open": true, "last_section": "current_turn"},
            "speech": {"voice_mode_enabled": false, "disable_auto_download": false, "mic_silence_ms": 600, "overrides": {}},
            "diagnostics": {"qnn_metrics_enabled": true}
        }"#;
    let update: GuiConfigUpdate = serde_json::from_str(update_json).unwrap();
    let resolved = update.into_config(&current);
    assert!(resolved.diagnostics.qnn_metrics_enabled);
}

#[test]
fn older_config_without_fallback_fields_loads_with_defaults() {
    // A config written before the fallback fields existed must still load
    // — struct-level `#[serde(default)]` on BackendConfig fills both as
    // None rather than erroring. Mirrors the qnn/llamacpp forward-compat.
    let dir = TempDir::new().unwrap();
    let path = config_path(dir.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let legacy = r#"{
            "learner": {"name": "Ada", "age": 7, "locale": "en"},
            "backend": {"kind": "ollama", "model": null, "ollama_url": "http://localhost:11434"}
        }"#;
    std::fs::write(&path, legacy).unwrap();
    let cfg = load(dir.path()).unwrap();
    assert_eq!(cfg.backend.fallback_backend, None);
    assert_eq!(cfg.backend.fallback_model, None);
}

#[test]
fn fallback_passes_through_view_verbatim() {
    // Not a secret — the view carries the fallback choice through
    // unredacted so the settings form can re-show it.
    let mut cfg = GuiConfig::default();
    cfg.backend.fallback_backend = Some("cloud".to_string());
    cfg.backend.fallback_model = None;
    let view: GuiConfigView = (&cfg).into();
    assert_eq!(view.backend.fallback_backend, Some("cloud".to_string()));
    assert_eq!(view.backend.fallback_model, None);
}

#[test]
fn fallback_passes_through_update_verbatim() {
    let current = GuiConfig::default();
    let update_json = r#"{
            "learner": {"name": "Ada", "age": 7, "locale": "en"},
            "backend": {
                "kind": "llamacpp",
                "model": null,
                "ollama_url": "http://localhost:11434",
                "openai_compat_url": "http://localhost:8000",
                "api_key_source": {"kind": "keep"},
                "openai_compat_api_key_source": {"kind": "keep"},
                "reasoning_markers": "",
                "qnn_bundle_dir": null,
                "qnn_qairt_lib_dir": null,
                "gguf_path": null,
                "llamacpp_gpu_layers": null,
                "llamacpp_n_ctx": null,
                "fallback_backend": "cloud",
                "fallback_model": "claude-opus-4-7",
                "primary_ttft_budget_ms": null,
                "router_mode": "local-only"
            },
            "classifier": {"match_main": true, "kind": null, "model": null, "timeout_ms": 3000},
            "extractor": {"match_main": true, "kind": null, "model": null, "timeout_ms": 5000},
            "comprehension": {"match_main": true, "kind": null, "model": null, "timeout_ms": 5000},
            "embedder": {"kind": "none", "model": null, "ollama_url": null, "openai_compat_url": null},
            "vocab": {"max_per_prompt": null},
            "breaks": {"after_mins": 30},
            "persistence": {"session_db": null, "knowledge_db": null, "no_persist": false},
            "ui": {"sidebar_open": true, "last_section": "current_turn"},
            "speech": {"voice_mode_enabled": false, "disable_auto_download": false, "mic_silence_ms": 600, "overrides": {}}
        }"#;
    let update: GuiConfigUpdate = serde_json::from_str(update_json).unwrap();
    let resolved = update.into_config(&current);
    assert_eq!(resolved.backend.fallback_backend, Some("cloud".to_string()));
    assert_eq!(
        resolved.backend.fallback_model,
        Some("claude-opus-4-7".to_string())
    );
}

#[test]
fn update_keep_preserves_existing_openai_compat_key() {
    // Independent of the cloud key: a `Keep` on the openai-compat
    // source carries the persisted secret forward untouched.
    let mut current = GuiConfig::default();
    current.backend.openai_compat_api_key_source = ApiKeySource::Inline {
        key: "sk-oai-original".to_string(),
    };
    let resolved = ApiKeyUpdate::Keep.resolve(&current.backend.openai_compat_api_key_source);
    assert_eq!(
        resolved,
        ApiKeySource::Inline {
            key: "sk-oai-original".to_string()
        }
    );
}

#[test]
fn older_config_without_openai_compat_fields_loads_with_defaults() {
    // An on-disk config from before openai-compat GUI parity has no
    // `openai_compat_url` / `openai_compat_api_key_source` keys. serde
    // defaults must inject them without a migration step.
    let dir = TempDir::new().unwrap();
    let path = config_path(dir.path());
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
            &path,
            r#"{
                "learner": {"name": "Ada", "age": 7, "locale": "en"},
                "backend": {"kind": "cloud", "model": null, "ollama_url": "http://localhost:11434", "api_key_source": {"kind": "env"}}
            }"#,
        )
        .unwrap();

    let cfg = load(dir.path()).unwrap();
    assert_eq!(cfg.backend.kind, "cloud");
    assert_eq!(cfg.backend.openai_compat_url, "http://localhost:8000");
    assert_eq!(cfg.backend.openai_compat_api_key_source, ApiKeySource::Env);
}

#[test]
fn update_keep_preserves_existing_inline_key() {
    let mut current = GuiConfig::default();
    current.backend.api_key_source = ApiKeySource::Inline {
        key: "sk-original".to_string(),
    };
    let update_json = r#"{
            "learner": {"name": "Ada", "age": 7, "locale": "en"},
            "backend": {
                "kind": "cloud",
                "model": null,
                "ollama_url": "http://localhost:11434",
                "openai_compat_url": "http://localhost:8000",
                "api_key_source": {"kind": "keep"},
                "openai_compat_api_key_source": {"kind": "keep"},
                "reasoning_markers": "",
                "qnn_bundle_dir": null,
                "qnn_qairt_lib_dir": null,
                "gguf_path": null,
                "llamacpp_gpu_layers": null,
                "llamacpp_n_ctx": null,
                "fallback_backend": null,
                "fallback_model": null,
                "primary_ttft_budget_ms": null,
                "router_mode": "local-only"
            },
            "classifier": {"match_main": true, "kind": null, "model": null, "timeout_ms": 3000},
            "extractor": {"match_main": true, "kind": null, "model": null, "timeout_ms": 5000},
            "comprehension": {"match_main": true, "kind": null, "model": null, "timeout_ms": 5000},
            "embedder": {"kind": "none", "model": null, "ollama_url": null, "openai_compat_url": null},
            "vocab": {"max_per_prompt": null},
            "breaks": {"after_mins": 30},
            "persistence": {"session_db": null, "knowledge_db": null, "no_persist": false},
            "ui": {"sidebar_open": true, "last_section": "current_turn"},
            "speech": {"voice_mode_enabled": false, "disable_auto_download": false, "mic_silence_ms": 600, "overrides": {}}
        }"#;
    let update: GuiConfigUpdate = serde_json::from_str(update_json).unwrap();
    let resolved = update.into_config(&current);
    assert_eq!(
        resolved.backend.api_key_source,
        ApiKeySource::Inline {
            key: "sk-original".to_string()
        },
        "Keep variant must carry forward the persisted key"
    );
    // Other fields come from the update, not the current.
    assert_eq!(resolved.learner.name, "Ada");
}

#[test]
fn update_inline_overwrites_existing_key() {
    let mut current = GuiConfig::default();
    current.backend.api_key_source = ApiKeySource::Inline {
        key: "sk-original".to_string(),
    };
    let new = ApiKeyUpdate::Inline {
        key: "sk-rotated".to_string(),
    };
    let resolved = new.resolve(&current.backend.api_key_source);
    assert_eq!(
        resolved,
        ApiKeySource::Inline {
            key: "sk-rotated".to_string()
        }
    );
}

#[test]
fn update_env_clears_existing_inline_key() {
    let mut current = GuiConfig::default();
    current.backend.api_key_source = ApiKeySource::Inline {
        key: "sk-original".to_string(),
    };
    let resolved = ApiKeyUpdate::Env.resolve(&current.backend.api_key_source);
    assert_eq!(resolved, ApiKeySource::Env);
}

#[test]
fn subsystem_defaults_match_consts() {
    let cls = SubsystemConfig::default_classifier();
    assert_eq!(
        cls.timeout_ms,
        primer_classifier::consts::DEFAULT_BLOCKING_TIMEOUT_MS
    );
    assert!(cls.match_main);
    let ext = SubsystemConfig::default_extractor();
    assert_eq!(
        ext.timeout_ms,
        primer_extractor::consts::DEFAULT_BLOCKING_TIMEOUT_MS
    );
    let cmp = SubsystemConfig::default_comprehension();
    assert_eq!(
        cmp.timeout_ms,
        primer_comprehension::consts::DEFAULT_BLOCKING_TIMEOUT_MS
    );
}

#[test]
fn speech_settings_default_has_600ms_silence() {
    let s = SpeechSettings::default();
    assert!(!s.voice_mode_enabled, "voice mode is off by default");
    assert!(
        !s.disable_auto_download,
        "auto-download is offered by default"
    );
    assert_eq!(
        s.mic_silence_ms,
        primer_core::consts::speech::DEFAULT_MIC_SILENCE_MS,
        "mic_silence_ms default reads from primer_core consts",
    );
    assert!(s.overrides.is_empty(), "no per-locale overrides by default");
}

#[test]
fn speech_settings_default_download_timeout_reads_from_consts() {
    let s = SpeechSettings::default();
    assert_eq!(
        s.download_timeout_secs,
        primer_core::consts::speech::DEFAULT_DOWNLOAD_TIMEOUT_SECS,
        "download_timeout_secs default reads from primer_core consts",
    );
}

#[test]
fn older_config_without_download_timeout_loads_with_default() {
    // An on-disk speech block from before issue #92 has no
    // `download_timeout_secs` field. Loading it must succeed and
    // inject the default without requiring a migration step.
    let dir = TempDir::new().unwrap();
    let path = config_path(dir.path());
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        r#"{
                "learner": {"name": "Ada", "age": 7, "locale": "en"},
                "speech": {
                    "voice_mode_enabled": true,
                    "disable_auto_download": false,
                    "mic_silence_ms": 750,
                    "overrides": {}
                }
            }"#,
    )
    .unwrap();

    let cfg = load(dir.path()).unwrap();
    assert!(cfg.speech.voice_mode_enabled);
    assert_eq!(cfg.speech.mic_silence_ms, 750);
    assert_eq!(
        cfg.speech.download_timeout_secs,
        primer_core::consts::speech::DEFAULT_DOWNLOAD_TIMEOUT_SECS,
    );
}

#[test]
fn speech_settings_round_trips_through_disk() {
    let dir = TempDir::new().unwrap();
    let mut cfg = GuiConfig::default();
    cfg.speech.voice_mode_enabled = true;
    cfg.speech.mic_silence_ms = 750;
    cfg.speech.overrides.insert(
        "de".to_string(),
        SpeechLocaleOverride {
            piper_onnx_path: Some("/tmp/de.onnx".into()),
            piper_config_path: Some("/tmp/de.onnx.json".into()),
            whisper_model_path: None,
            voice_id: Some("de_DE-thorsten-medium".to_string()),
            ..SpeechLocaleOverride::default()
        },
    );

    save(dir.path(), &cfg).unwrap();
    let round_trip = load(dir.path()).unwrap();
    assert_eq!(round_trip, cfg);
}

#[test]
fn older_config_without_speech_block_loads_with_defaults() {
    // An on-disk config from before PR 2 has no `speech` field. Loading
    // it must succeed and inject SpeechSettings::default() without
    // requiring a migration step.
    let dir = TempDir::new().unwrap();
    let path = config_path(dir.path());
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        r#"{"learner": {"name": "Ada", "age": 7, "locale": "en"}}"#,
    )
    .unwrap();

    let cfg = load(dir.path()).unwrap();
    assert_eq!(cfg.learner.name, "Ada");
    assert_eq!(cfg.speech, SpeechSettings::default());
}

#[test]
fn speech_settings_round_trip_through_view_and_update() {
    let mut cfg = GuiConfig::default();
    cfg.speech.voice_mode_enabled = true;
    cfg.speech.mic_silence_ms = 800;

    let view: GuiConfigView = (&cfg).into();
    assert_eq!(view.speech, cfg.speech);

    let update_json = serde_json::to_string(&serde_json::json!({
        "learner": {"name": "Binti", "age": 8, "locale": "en"},
        "backend": {
            "kind": "stub", "model": null,
            "ollama_url": "http://localhost:11434",
            "openai_compat_url": "http://localhost:8000",
            "api_key_source": {"kind": "keep"},
            "openai_compat_api_key_source": {"kind": "keep"},
            "reasoning_markers": "",
            "qnn_bundle_dir": null,
            "qnn_qairt_lib_dir": null,
            "gguf_path": null,
            "llamacpp_gpu_layers": null,
            "llamacpp_n_ctx": null,
            "fallback_backend": null,
            "fallback_model": null,
            "primary_ttft_budget_ms": null,
            "router_mode": "local-only",
        },
        "classifier": {"match_main": true, "kind": null, "model": null, "timeout_ms": 3000},
        "extractor": {"match_main": true, "kind": null, "model": null, "timeout_ms": 5000},
        "comprehension": {"match_main": true, "kind": null, "model": null, "timeout_ms": 5000},
        "embedder": {"kind": "none", "model": null, "ollama_url": null},
        "vocab": {"max_per_prompt": null},
        "breaks": {"after_mins": 30},
        "persistence": {"session_db": null, "knowledge_db": null, "no_persist": false},
        "ui": {"sidebar_open": true, "last_section": "current_turn"},
        "speech": {
            "voice_mode_enabled": true,
            "disable_auto_download": false,
            "mic_silence_ms": 800,
            "overrides": {}
        }
    }))
    .unwrap();
    let update: GuiConfigUpdate = serde_json::from_str(&update_json).unwrap();
    let resolved = update.into_config(&cfg);
    assert!(resolved.speech.voice_mode_enabled);
    assert_eq!(resolved.speech.mic_silence_ms, 800);
}

/// A settings update carrying `speech.backend = macos-native` must
/// survive `into_config` unchanged. This is the invariant the
/// frontend relies on once gather() round-trips the field — if a
/// future refactor drops `speech.backend` from `GuiConfigUpdate` or
/// `into_config`, the GUI toggle silently reverts to whisper-piper.
#[test]
fn update_preserves_macos_native_speech_backend() {
    let current = GuiConfig::default();
    let update_json = r#"{
            "learner": {"name": "Bo", "age": 7, "locale": "en"},
            "backend": {
                "kind": "stub",
                "model": null,
                "ollama_url": "http://localhost:11434",
                "openai_compat_url": "http://localhost:8000",
                "api_key_source": {"kind": "keep"},
                "openai_compat_api_key_source": {"kind": "keep"},
                "qnn_bundle_dir": null,
                "qnn_qairt_lib_dir": null,
                "gguf_path": null,
                "llamacpp_gpu_layers": null,
                "llamacpp_n_ctx": null,
                "reasoning_markers": "",
                "fallback_backend": null,
                "fallback_model": null,
                "primary_ttft_budget_ms": null,
                "router_mode": "local-only"
            },
            "classifier": {"match_main": true, "kind": null, "model": null, "timeout_ms": 3000},
            "extractor": {"match_main": true, "kind": null, "model": null, "timeout_ms": 5000},
            "comprehension": {"match_main": true, "kind": null, "model": null, "timeout_ms": 5000},
            "embedder": {"kind": "none", "model": null, "ollama_url": null, "openai_compat_url": null},
            "vocab": {"max_per_prompt": null},
            "breaks": {"after_mins": 30},
            "persistence": {"session_db": null, "knowledge_db": null, "no_persist": false},
            "ui": {"sidebar_open": true, "last_section": "current_turn"},
            "speech": {
                "voice_mode_enabled": false,
                "disable_auto_download": false,
                "backend": "macos-native",
                "mic_silence_ms": 600,
                "download_timeout_secs": 3600,
                "overrides": {}
            }
        }"#;
    let update: GuiConfigUpdate = serde_json::from_str(update_json).unwrap();
    let resolved = update.into_config(&current);
    assert_eq!(resolved.speech.backend, Some(SpeechBackend::MacosNative));
    assert_eq!(resolved.speech.download_timeout_secs, 3600);
}

/// `settings.js::gather()` emits `download_timeout_secs: undefined` on
/// the defensive never-populated path, which drops the key from the
/// `update_settings` payload entirely. This pins the IPC-layer
/// invariant that path relies on: a `GuiConfigUpdate.speech` block
/// missing `download_timeout_secs` must still deserialize and resolve
/// to the consts-backed default (not a deserialize error), so saving
/// never wedges on the defensive path. Mirrors the on-disk
/// `older_config_without_download_timeout_loads_with_default` test but
/// for the IPC write path rather than the `load()` read path.
#[test]
fn update_with_missing_download_timeout_falls_back_to_default() {
    let current = GuiConfig::default();
    let update_json = r#"{
            "learner": {"name": "Bo", "age": 7, "locale": "en"},
            "backend": {
                "kind": "stub",
                "model": null,
                "ollama_url": "http://localhost:11434",
                "openai_compat_url": "http://localhost:8000",
                "api_key_source": {"kind": "keep"},
                "openai_compat_api_key_source": {"kind": "keep"},
                "qnn_bundle_dir": null,
                "qnn_qairt_lib_dir": null,
                "gguf_path": null,
                "llamacpp_gpu_layers": null,
                "llamacpp_n_ctx": null,
                "reasoning_markers": "",
                "fallback_backend": null,
                "fallback_model": null,
                "primary_ttft_budget_ms": null,
                "router_mode": "local-only"
            },
            "classifier": {"match_main": true, "kind": null, "model": null, "timeout_ms": 3000},
            "extractor": {"match_main": true, "kind": null, "model": null, "timeout_ms": 5000},
            "comprehension": {"match_main": true, "kind": null, "model": null, "timeout_ms": 5000},
            "embedder": {"kind": "none", "model": null, "ollama_url": null, "openai_compat_url": null},
            "vocab": {"max_per_prompt": null},
            "breaks": {"after_mins": 30},
            "persistence": {"session_db": null, "knowledge_db": null, "no_persist": false},
            "ui": {"sidebar_open": true, "last_section": "current_turn"},
            "speech": {
                "voice_mode_enabled": false,
                "disable_auto_download": false,
                "backend": "macos-native",
                "mic_silence_ms": 600,
                "overrides": {}
            }
        }"#;
    let update: GuiConfigUpdate = serde_json::from_str(update_json).unwrap();
    let resolved = update.into_config(&current);
    // backend still round-trips even with the sibling key dropped
    assert_eq!(resolved.speech.backend, Some(SpeechBackend::MacosNative));
    assert_eq!(
        resolved.speech.download_timeout_secs,
        primer_core::consts::speech::DEFAULT_DOWNLOAD_TIMEOUT_SECS,
        "missing download_timeout_secs resolves to the consts default",
    );
}

#[test]
fn legacy_backend_macos_native_migrates_to_both_native_halves() {
    let json = r#"{ "backend": "macos-native" }"#;
    let speech: SpeechSettings = serde_json::from_str(json).unwrap();
    assert_eq!(
        speech.resolve_backends(),
        (SttBackend::MacosNative, TtsBackend::MacosNative)
    );
}

#[test]
fn legacy_backend_whisper_piper_migrates_to_whisper_piper() {
    let json = r#"{ "backend": "whisper-piper" }"#;
    let speech: SpeechSettings = serde_json::from_str(json).unwrap();
    assert_eq!(
        speech.resolve_backends(),
        (SttBackend::Whisper, TtsBackend::Piper)
    );
}

#[test]
fn new_fields_take_precedence_over_legacy_backend() {
    let json =
        r#"{ "backend": "whisper-piper", "stt_backend": "whisper", "tts_backend": "supertonic" }"#;
    let speech: SpeechSettings = serde_json::from_str(json).unwrap();
    assert_eq!(
        speech.resolve_backends(),
        (SttBackend::Whisper, TtsBackend::Supertonic)
    );
}

#[test]
fn no_legacy_no_new_resolves_to_defaults() {
    let speech = SpeechSettings::default();
    assert_eq!(
        speech.resolve_backends(),
        (SttBackend::Whisper, TtsBackend::Piper)
    );
}

#[test]
fn view_resolves_legacy_backend_into_stt_tts() {
    // A config carrying only the legacy macos-native backend must surface
    // as macos-native on BOTH halves through the View (so the settings
    // modal shows the user's real choice, not the default).
    let mut cfg = GuiConfig::default();
    cfg.speech.backend = Some(SpeechBackend::MacosNative);
    // new fields still at defaults → migration applies
    let view: GuiConfigView = (&cfg).into();
    assert_eq!(view.speech.stt_backend, SttBackend::MacosNative);
    assert_eq!(view.speech.tts_backend, TtsBackend::MacosNative);
    assert_eq!(
        view.speech.backend, None,
        "legacy field cleared in the view"
    );
}

#[test]
fn legacy_backend_is_not_reserialized() {
    // skip_serializing means a migrated config doesn't keep writing `backend`.
    let speech = SpeechSettings {
        backend: Some(SpeechBackend::MacosNative),
        ..SpeechSettings::default()
    };
    let json = serde_json::to_string(&speech).unwrap();
    assert!(
        !json.contains("\"backend\""),
        "legacy backend must not be serialized: {json}"
    );
}

#[test]
fn backend_config_carries_ttft_budget() {
    let mut cfg = BackendConfig::default();
    assert_eq!(cfg.primary_ttft_budget_ms, None, "OFF by default");
    cfg.primary_ttft_budget_ms = Some(750);
    let json = serde_json::to_string(&cfg).unwrap();
    let back: BackendConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(back.primary_ttft_budget_ms, Some(750));
    // Old configs without the field still deserialize (serde default).
    // Note: `router_mode` uses Rust-enum serde form ("LocalOnly") not the
    // kebab-case name ("local-only") used by `FromStr`/`name()`.
    let old = r#"{"kind":"stub","model":null,"ollama_url":"u","openai_compat_url":"u","api_key_source":{"kind":"env"},"openai_compat_api_key_source":{"kind":"env"},"qnn_bundle_dir":null,"qnn_qairt_lib_dir":null,"gguf_path":null,"llamacpp_gpu_layers":null,"llamacpp_n_ctx":null,"reasoning_markers":"","fallback_backend":null,"fallback_model":null,"router_mode":"LocalOnly"}"#;
    let parsed: BackendConfig = serde_json::from_str(old).unwrap();
    assert_eq!(parsed.primary_ttft_budget_ms, None);
}

#[test]
fn supertonic_override_paths_round_trip() {
    let ov = SpeechLocaleOverride {
        supertonic_onnx_dir: Some("/sup/onnx".into()),
        supertonic_voice_style_path: Some("/sup/F1.json".into()),
        ..SpeechLocaleOverride::default()
    };
    let json = serde_json::to_string(&ov).unwrap();
    let back: SpeechLocaleOverride = serde_json::from_str(&json).unwrap();
    assert_eq!(back.supertonic_onnx_dir, ov.supertonic_onnx_dir);
    assert_eq!(
        back.supertonic_voice_style_path,
        ov.supertonic_voice_style_path
    );
}
