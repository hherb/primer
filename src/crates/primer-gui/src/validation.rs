//! Validation of a [`GuiConfig`] before it lands on disk.
//!
//! The goal is to surface obviously-bad configs at `update_settings`
//! time rather than at the next `start_session`, where the same checks
//! eventually run as part of `build_active_session`. Catching them
//! early means the settings modal can render the error inline and
//! never lets a broken config persist.
//!
//! This is *cheap, structural* validation only — no I/O, no network.
//! `start_session` remains the ultimate authority on whether the
//! config actually produces a working session (model availability,
//! embedder construction, etc.).

use primer_core::i18n::Locale;

use crate::config::GuiConfig;

/// Run validation and return an inline-friendly error message on the
/// first failure, or `Ok(())` if the config is structurally sound.
pub fn validate(cfg: &GuiConfig) -> Result<(), String> {
    validate_learner(&cfg.learner)?;
    validate_locale(&cfg.learner.locale)?;
    validate_backend(&cfg.backend.kind)?;
    // `match_main = true` causes the wiring code to ignore `kind`, so
    // an invalid `kind` in that branch is dead data — don't fail the
    // save on it.
    validate_subsystem_kind("classifier", subsystem_override(&cfg.classifier))?;
    validate_subsystem_kind("extractor", subsystem_override(&cfg.extractor))?;
    validate_subsystem_kind("comprehension", subsystem_override(&cfg.comprehension))?;
    validate_embedder(&cfg.embedder.kind)?;
    validate_breaks(cfg.breaks.after_mins)?;
    validate_router(&cfg.backend)?;
    Ok(())
}

/// A routing mode (`cloud-preferred` / `hybrid`) has nothing to route to
/// without a configured secondary leg (`fallback_backend`). Catch this at
/// save time so the modal renders the error inline, matching the CLI's
/// early `--router-mode` validation rather than waiting for the (otherwise
/// identical) `build_router_backend` error at `start_session`.
fn validate_router(backend: &crate::config::BackendConfig) -> Result<(), String> {
    if backend.router_mode.uses_secondary() && backend.fallback_backend.is_none() {
        return Err(format!(
            "router mode '{}' requires a fallback backend (the secondary leg to route to)",
            backend.router_mode
        ));
    }
    Ok(())
}

/// Learner profile guards. An empty (or whitespace-only) name would
/// otherwise slug to `""` at session-start time, producing a `.db`
/// filename of `.db` — a real filesystem edge case worth blocking at
/// the modal rather than at first send. Age 0 is similarly meaningless
/// for a learner profile and would produce a confusing system prompt.
fn validate_learner(learner: &crate::config::LearnerConfig) -> Result<(), String> {
    if learner.name.trim().is_empty() {
        return Err("Learner name is required.".to_string());
    }
    if learner.age == 0 {
        return Err("Learner age must be at least 1.".to_string());
    }
    Ok(())
}

fn subsystem_override(s: &crate::config::SubsystemConfig) -> Option<&str> {
    if s.match_main {
        None
    } else {
        s.kind.as_deref()
    }
}

fn validate_locale(pack_id: &str) -> Result<(), String> {
    Locale::from_pack_id(pack_id).map(|_| ()).ok_or_else(|| {
        let known: Vec<&str> = Locale::ALL.iter().map(|l| l.pack_id()).collect();
        format!("locale {pack_id:?} is not a supported pack. Known: {known:?}")
    })
}

fn validate_backend(kind: &str) -> Result<(), String> {
    match kind {
        // `qnn` is structurally valid here; whether the backend can
        // actually construct (cargo feature present, bundle dir set,
        // libGenie.so loadable) is a wiring-layer concern that surfaces
        // its own error inline — mirroring how ollama-without-model is a
        // wiring check, not a validation one.
        "stub" | "cloud" | "ollama" | "openai-compat" | "qnn" => Ok(()),
        other => Err(format!(
            "unknown backend kind {other:?}: expected one of stub, cloud, ollama, openai-compat, qnn"
        )),
    }
}

/// Subsystem kind is optional — `None` means "match the main backend"
/// (paired with `match_main = true` in `SubsystemConfig`). Only an
/// explicit override is validated here.
fn validate_subsystem_kind(label: &str, kind: Option<&str>) -> Result<(), String> {
    match kind {
        None => Ok(()),
        Some("stub" | "cloud" | "ollama") => Ok(()),
        Some(other) => Err(format!(
            "unknown {label} backend {other:?}: expected one of stub, cloud, ollama"
        )),
    }
}

fn validate_embedder(kind: &str) -> Result<(), String> {
    match kind {
        "none" | "stub" | "fastembed" | "ollama" | "openai-compat" => Ok(()),
        other => Err(format!(
            "unknown embedder backend {other:?}: expected one of none, stub, fastembed, ollama, openai-compat"
        )),
    }
}

fn validate_breaks(after_mins: u32) -> Result<(), String> {
    if after_mins == 0 {
        Err("break-suggestion interval must be at least 1 minute".to_string())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_valid() {
        validate(&GuiConfig::default()).expect("default config must validate");
    }

    #[test]
    fn unknown_locale_rejected() {
        let mut cfg = GuiConfig::default();
        cfg.learner.locale = "klingon".to_string();
        let err = validate(&cfg).unwrap_err();
        assert!(
            err.contains("klingon"),
            "error must name the offender: {err}"
        );
    }

    #[test]
    fn unknown_backend_rejected() {
        let mut cfg = GuiConfig::default();
        cfg.backend.kind = "magic".to_string();
        let err = validate(&cfg).unwrap_err();
        assert!(err.contains("magic"));
    }

    #[test]
    fn unknown_embedder_rejected() {
        let mut cfg = GuiConfig::default();
        cfg.embedder.kind = "secret-sauce".to_string();
        let err = validate(&cfg).unwrap_err();
        assert!(err.contains("secret-sauce"));
    }

    #[test]
    fn routing_mode_without_fallback_rejected() {
        let mut cfg = GuiConfig::default();
        cfg.backend.router_mode = primer_core::router::RouterMode::Hybrid;
        cfg.backend.fallback_backend = None;
        let err = validate(&cfg).unwrap_err();
        assert!(
            err.contains("hybrid") && err.to_lowercase().contains("fallback"),
            "error must name the mode + the missing secondary: {err}"
        );
    }

    #[test]
    fn routing_mode_with_fallback_accepted() {
        let mut cfg = GuiConfig::default();
        cfg.backend.router_mode = primer_core::router::RouterMode::Hybrid;
        cfg.backend.fallback_backend = Some("cloud".to_string());
        validate(&cfg).expect("hybrid + a configured secondary is valid");
    }

    #[test]
    fn openai_compat_backend_kind_accepted() {
        // Structural validation only checks the kind is known; the
        // model-required check lives in the wiring layer (mirrors how
        // ollama-without-model is a wiring test, not a validation one).
        let mut cfg = GuiConfig::default();
        cfg.backend.kind = "openai-compat".to_string();
        cfg.backend.model = Some("mlx-community/Qwen3-8B-4bit".to_string());
        validate(&cfg).expect("openai-compat is a known backend kind");
    }

    #[test]
    fn openai_compat_embedder_kind_accepted() {
        let mut cfg = GuiConfig::default();
        cfg.embedder.kind = "openai-compat".to_string();
        cfg.embedder.model = Some("nomic-embed-text".to_string());
        validate(&cfg).expect("openai-compat is a known embedder kind");
    }

    #[test]
    fn qnn_backend_kind_accepted() {
        // Structural validation accepts qnn regardless of cargo feature
        // or bundle-dir presence; those are wiring-layer checks that
        // surface their own errors inline.
        let mut cfg = GuiConfig::default();
        cfg.backend.kind = "qnn".to_string();
        cfg.backend.qnn_bundle_dir = Some("/bundles/qwen3-4b".into());
        validate(&cfg).expect("qnn is a known backend kind");
    }

    #[test]
    fn qnn_backend_kind_accepted_without_bundle_dir() {
        // A missing bundle dir is NOT a structural error — it's caught at
        // session-start by build_qnn_backend. Validation only gates the
        // kind string.
        let mut cfg = GuiConfig::default();
        cfg.backend.kind = "qnn".to_string();
        validate(&cfg).expect("qnn validates even without a bundle dir");
    }

    #[test]
    fn subsystem_kind_override_validates() {
        let mut cfg = GuiConfig::default();
        cfg.classifier.match_main = false;
        cfg.classifier.kind = Some("bogus".to_string());
        let err = validate(&cfg).unwrap_err();
        assert!(
            err.contains("classifier"),
            "error must name the subsystem: {err}"
        );
        assert!(err.contains("bogus"));
    }

    #[test]
    fn subsystem_match_main_skips_kind_validation() {
        // `match_main = true` + bogus kind is OK because the wiring
        // code ignores `kind` in that case.
        let mut cfg = GuiConfig::default();
        cfg.classifier.match_main = true;
        cfg.classifier.kind = Some("bogus".to_string());
        validate(&cfg).expect("match_main=true ignores kind");
    }

    #[test]
    fn zero_break_interval_rejected() {
        let mut cfg = GuiConfig::default();
        cfg.breaks.after_mins = 0;
        let err = validate(&cfg).unwrap_err();
        assert!(err.contains("1 minute"));
    }

    #[test]
    fn empty_learner_name_rejected() {
        let mut cfg = GuiConfig::default();
        cfg.learner.name = String::new();
        let err = validate(&cfg).unwrap_err();
        assert!(err.contains("name"), "error must mention the field: {err}");
    }

    #[test]
    fn whitespace_only_learner_name_rejected() {
        let mut cfg = GuiConfig::default();
        cfg.learner.name = "   ".to_string();
        let err = validate(&cfg).unwrap_err();
        assert!(err.contains("name"));
    }

    #[test]
    fn zero_learner_age_rejected() {
        let mut cfg = GuiConfig::default();
        cfg.learner.age = 0;
        let err = validate(&cfg).unwrap_err();
        assert!(err.contains("age"), "error must mention the field: {err}");
    }
}
