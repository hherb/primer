use super::resolve_fallback_model;
use primer_core::consts::inference::DEFAULT_CLOUD_MODEL;

#[test]
fn stub_ignores_model() {
    assert_eq!(resolve_fallback_model("stub", None).unwrap(), None);
    assert_eq!(
        resolve_fallback_model("stub", Some("x".into())).unwrap(),
        None
    );
}

#[test]
fn cloud_defaults_when_unset() {
    assert_eq!(
        resolve_fallback_model("cloud", None).unwrap(),
        Some(DEFAULT_CLOUD_MODEL.to_string())
    );
}

#[test]
fn cloud_uses_explicit_model() {
    assert_eq!(
        resolve_fallback_model("cloud", Some("claude-opus-4-7".into())).unwrap(),
        Some("claude-opus-4-7".to_string())
    );
}

#[test]
fn ollama_requires_model() {
    assert!(resolve_fallback_model("ollama", None).is_err());
    assert_eq!(
        resolve_fallback_model("ollama", Some("llama3.2".into())).unwrap(),
        Some("llama3.2".to_string())
    );
}

#[test]
fn openai_compat_requires_model() {
    assert!(resolve_fallback_model("openai-compat", None).is_err());
    assert_eq!(
        resolve_fallback_model("openai-compat", Some("Qwen3-8B".into())).unwrap(),
        Some("Qwen3-8B".to_string())
    );
}
