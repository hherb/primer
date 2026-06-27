use super::*;
use std::path::PathBuf;

/// A request with the same model_id as the loaded model is accepted.
#[test]
fn check_voice_match_accepts_same_id() {
    assert!(check_voice_match("supertonic-F1", "supertonic-F1").is_ok());
}

/// A mismatched model_id is rejected with a `PrimerError::Speech`
/// that names BOTH ids — diagnosing a misconfiguration is the whole
/// point of the early validation, so we lock the wording.
#[test]
fn check_voice_match_rejects_mismatched_id() {
    let err = check_voice_match("supertonic-F1", "supertonic-M3").unwrap_err();
    let s = format!("{err}");
    assert!(s.contains("supertonic-F1"), "missing loaded id in: {s}");
    assert!(s.contains("supertonic-M3"), "missing requested id in: {s}");
}

/// `merge_voice` overrides rate/pitch but PRESERVES the loaded
/// model_id. Same invariant as Piper's `with_voice` — a foreign
/// model_id passed through `with_voice` is dropped, not surfaced
/// as an error, to keep the builder ergonomic while making the
/// footgun (loaded != requested at `open_session` time) impossible.
#[test]
fn merge_voice_preserves_loaded_model_id() {
    let merged = merge_voice(
        "supertonic-F1",
        VoiceProfile {
            model_id: "WRONG-ID".to_string(),
            rate: 1.2,
            pitch: 0.0,
        },
    );
    assert_eq!(merged.model_id, "supertonic-F1");
    assert!((merged.rate - 1.2).abs() < f32::EPSILON);
}

/// `merge_voice` with a default override still yields the loaded id.
/// Sanity check for `SupertonicTts::new(...).with_voice(VoiceProfile::default())`.
#[test]
fn merge_voice_overwrites_model_id_even_when_override_is_default() {
    let merged = merge_voice("supertonic-F1", VoiceProfile::default());
    assert_eq!(merged.model_id, "supertonic-F1");
}

/// `derive_model_id` formats the voice-style stem with a
/// `supertonic-` prefix so it's distinguishable from Piper ids
/// (`en_US-amy-medium`) at a glance in logs / DB rows.
#[test]
fn derive_model_id_prefixes_voice_style_stem() {
    let id = derive_model_id(&PathBuf::from("/some/dir/voice_styles/F1.json"));
    assert_eq!(id, "supertonic-F1");
}

/// A path with no file stem falls back to `"supertonic-voice"`
/// rather than panicking or producing an empty id.
#[test]
fn derive_model_id_falls_back_when_stem_is_missing() {
    let id = derive_model_id(&PathBuf::from("/"));
    assert_eq!(id, "supertonic-voice");
}

/// `speed_for` is the identity for positive finite rates (direct,
/// not inverted — opposite of Piper's `length_scale_for`).
#[test]
fn speed_for_is_identity_for_positive_rate() {
    let v = VoiceProfile {
        rate: 1.5,
        ..VoiceProfile::default()
    };
    assert!((speed_for(&v) - 1.5).abs() < f32::EPSILON);
}

/// `rate = 1.0` round-trips to the default speed.
#[test]
fn speed_for_default_for_unit_rate() {
    let v = VoiceProfile {
        rate: 1.0,
        ..VoiceProfile::default()
    };
    assert!((speed_for(&v) - 1.0).abs() < f32::EPSILON);
}

/// Zero / negative / NaN rates fall back to `1.0` rather than
/// landing a degenerate `speed` value in the helper's denoising
/// path (where division by zero would surface as `inf` durations
/// and a downstream panic in `Array::from_shape_vec`). Forgiving
/// by design, same as Piper's `length_scale_for` fallback.
#[test]
fn speed_for_falls_back_to_default_on_non_positive_rate() {
    for bad in [0.0, -1.0, f32::NAN] {
        let v = VoiceProfile {
            rate: bad,
            ..VoiceProfile::default()
        };
        assert_eq!(speed_for(&v), 1.0);
    }
}

/// `clamp_total_steps` raises the degenerate `0` to the floor and
/// caps an absurd request, but leaves both the recommended range and
/// deliberate above-recommended overrides untouched (it is NOT a snap
/// to 5..=12). Pins the doc claim on [`SupertonicTts::with_total_steps`].
#[test]
fn clamp_total_steps_guards_boundaries_without_snapping_to_recommended_range() {
    assert_eq!(
        clamp_total_steps(0),
        MIN_TOTAL_STEPS,
        "0 must rise to floor"
    );
    assert_eq!(clamp_total_steps(MIN_TOTAL_STEPS), MIN_TOTAL_STEPS);
    assert_eq!(clamp_total_steps(8), 8, "default passes through");
    assert_eq!(
        clamp_total_steps(20),
        20,
        "above-recommended override is honoured, not snapped to 12"
    );
    assert_eq!(clamp_total_steps(MAX_TOTAL_STEPS), MAX_TOTAL_STEPS);
    assert_eq!(
        clamp_total_steps(1_000),
        MAX_TOTAL_STEPS,
        "runaway request capped at ceiling"
    );
}

/// Real-model smoke test. Skipped unless both
/// `$SUPERTONIC_TEST_ONNX_DIR` and `$SUPERTONIC_TEST_VOICE_STYLE` are
/// set, because Supertonic needs a ~400 MB asset bundle on disk and
/// CI doesn't ship them. Only one of the two set is treated as a
/// misconfiguration and panics so the running developer notices.
#[tokio::test]
#[ignore]
async fn supertonic_smoke_synthesise_returns_non_empty_audio() {
    let (onnx_dir, voice_style) = match (
        std::env::var("SUPERTONIC_TEST_ONNX_DIR").ok(),
        std::env::var("SUPERTONIC_TEST_VOICE_STYLE").ok(),
    ) {
        (Some(o), Some(v)) => (o, v),
        (None, None) => return,
        _ => {
            panic!("SUPERTONIC_TEST_ONNX_DIR and SUPERTONIC_TEST_VOICE_STYLE must be set together")
        }
    };
    let tts = SupertonicTts::new(&onnx_dir, &voice_style).expect("load supertonic");
    let voice = tts.voice.clone();
    let audio = tts.synthesize("Hello.", &voice).await.expect("synthesise");
    assert!(!audio.samples.is_empty());
    assert_eq!(audio.sample_rate, tts.sample_rate());
}
