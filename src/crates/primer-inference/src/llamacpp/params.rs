//! Pure, feature-independent helpers for the llama.cpp backend.
//!
//! Everything here compiles and is tested WITHOUT the `llamacpp` cargo
//! feature, so the orchestration logic stays in CI's reach. The
//! feature-gated `RealLlamaEngine` consumes these helpers.

#[cfg(any(
    feature = "llamacpp-metal",
    feature = "llamacpp-cuda",
    feature = "llamacpp-vulkan"
))]
use primer_core::consts::inference::LLAMACPP_GPU_LAYERS_ALL;
#[cfg(not(any(
    feature = "llamacpp-metal",
    feature = "llamacpp-cuda",
    feature = "llamacpp-vulkan"
)))]
use primer_core::consts::inference::LLAMACPP_GPU_LAYERS_CPU;
use primer_core::consts::inference::{LLAMACPP_DEFAULT_N_CTX, LLAMACPP_DEFAULT_SAMPLER_SEED};
use primer_core::error::InferenceError;
use primer_core::inference::GenerationParams;
use std::path::Path;

/// A feature-independent description of the sampler chain to build.
///
/// `RealLlamaEngine` (behind the `llamacpp` feature) turns this into a real
/// `llama_cpp_2::sampling::LlamaSampler`. Kept as plain data so it is
/// host-testable without the binding.
#[derive(Debug, Clone, PartialEq)]
pub struct SamplerSpec {
    pub top_p: f32,
    pub temperature: f32,
    pub seed: u32,
}

/// Derive the sampler chain description from generation params.
pub fn sampler_spec(params: &GenerationParams) -> SamplerSpec {
    SamplerSpec {
        top_p: params.top_p,
        temperature: params.temperature,
        seed: LLAMACPP_DEFAULT_SAMPLER_SEED,
    }
}

/// Resolve the `n_gpu_layers` value for model load.
///
/// An explicit CLI/GUI override wins. Otherwise default to "offload all"
/// when a GPU passthrough feature is compiled, else CPU-only.
pub fn resolve_gpu_layers(override_value: Option<i32>) -> i32 {
    if let Some(n) = override_value {
        return n;
    }
    #[cfg(any(
        feature = "llamacpp-metal",
        feature = "llamacpp-cuda",
        feature = "llamacpp-vulkan"
    ))]
    {
        LLAMACPP_GPU_LAYERS_ALL
    }
    #[cfg(not(any(
        feature = "llamacpp-metal",
        feature = "llamacpp-cuda",
        feature = "llamacpp-vulkan"
    )))]
    {
        LLAMACPP_GPU_LAYERS_CPU
    }
}

/// Resolve `n_ctx`. `None` -> the model's trained default (encoded as 0).
pub fn resolve_n_ctx(override_value: Option<u32>) -> u32 {
    override_value.unwrap_or(LLAMACPP_DEFAULT_N_CTX)
}

/// Stop-sequence handling for the decode loop.
///
/// `piece` is the token text just produced; `accumulated` is the full
/// visible text *including* `piece`. If appending `piece` made
/// `accumulated` end with any non-empty stop sequence, return
/// `Some(prefix)` where `prefix` is the leading slice of `piece` that
/// should still reach the consumer — everything before the stop marker
/// starts, trimmed back to a UTF-8 char boundary. The caller emits
/// `prefix` and then stops, so the matched stop marker is never shown to
/// the child. `None` means no stop matched — forward `piece` unchanged.
///
/// Only the portion of the marker that falls inside the final `piece` is
/// trimmed; a marker that straddled an earlier piece boundary has already
/// been partially emitted (an accepted edge case — stop sequences are rare
/// and usually template artifacts that arrive in a single token).
pub fn visible_prefix_before_stop<'a>(
    piece: &'a str,
    accumulated: &str,
    stops: &[String],
) -> Option<&'a str> {
    for s in stops.iter().filter(|s| !s.is_empty()) {
        if accumulated.ends_with(s.as_str()) {
            // Bytes of the stop marker that fall inside this final piece.
            let overlap = s.len().min(piece.len());
            let mut cut = piece.len() - overlap;
            while cut > 0 && !piece.is_char_boundary(cut) {
                cut -= 1;
            }
            return Some(&piece[..cut]);
        }
    }
    None
}

/// Parse the `tokenizer.ggml.add_bos_token` GGUF metadata value to a bool.
///
/// GGUF boolean metadata surfaces through llama.cpp's `meta_val_str` as a
/// string — conventionally `"true"`/`"false"`, but some converters emit
/// `"1"`/`"0"`. Matching is case-insensitive and whitespace-trimmed. Any
/// other value returns `None` so the caller falls back to its own default
/// rather than guessing.
pub fn parse_add_bos_metadata(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "true" | "1" => Some(true),
        "false" | "0" => Some(false),
        _ => None,
    }
}

/// Decide whether to prepend the model's BOS token when tokenizing a
/// chat-template-rendered prompt.
///
/// `str_to_token` parses special tokens in its input (llama.cpp's
/// `parse_special` is on), so a chat template that embeds a *literal* BOS
/// marker — e.g. Gemma's `<bos>` or Llama 3's `<|begin_of_text|>` — already
/// yields a BOS token. Adding another via `AddBos::Always` produces a
/// quality-degrading double-BOS (issue #201).
///
/// Decision, in priority order:
/// 1. If `bos_piece` is a non-empty string and `rendered` already begins with
///    it, the template embeds the literal BOS — never add another (`false`).
/// 2. Otherwise honour the model's `add_bos_token` metadata when present.
/// 3. When metadata is absent (`None`), default to `true` — the historical
///    `AddBos::Always` behaviour, correct for the common chat models
///    (Qwen / Llama 2 / Mistral) whose templates do not embed a literal BOS.
pub fn should_prepend_bos(
    rendered: &str,
    bos_piece: Option<&str>,
    meta_add_bos: Option<bool>,
) -> bool {
    // 1. Literal-BOS-in-template guard (Gemma / Llama 3 style).
    if template_embeds_bos(rendered, bos_piece) {
        return false;
    }
    // 2 + 3. Honour metadata when present; default to the historical
    // "always add" behaviour otherwise.
    meta_add_bos.unwrap_or(true)
}

/// Whether `rendered` already begins with the model's literal BOS piece — the
/// leg that makes the Gemma / Llama-3 family skip the extra BOS. A `None` or
/// empty `bos_piece` never matches.
///
/// Single source of truth for the template-detection predicate: both
/// [`should_prepend_bos`] (the production decision) and [`bos_decision`] (the
/// diagnostic projection) call this, so the recorded `template_embeds_bos`
/// flag can never drift from the leg that actually drives the outcome.
pub fn template_embeds_bos(rendered: &str, bos_piece: Option<&str>) -> bool {
    bos_piece.is_some_and(|b| !b.is_empty() && rendered.starts_with(b))
}

/// Diagnostic snapshot of the BOS-prepend decision for one rendered prompt
/// (issue #201).
///
/// Built by `RealLlamaEngine::bos_decision` so the owner-gated real-model
/// smoke can assert the per-model outcome — Gemma's template embeds a literal
/// `<bos>` so we must NOT add another (`prepend_bos == false`); Qwen3 has no
/// literal BOS so the historical add-once path holds (`prepend_bos == true`) —
/// without reaching into raw token ids. Plain data so the shape is documented
/// and host-tested next to [`should_prepend_bos`], the function whose decision
/// it records.
#[derive(Debug, Clone, PartialEq)]
pub struct BosDecision {
    /// The model's BOS token in text form (e.g. `<bos>`), or `None` when the
    /// model has no/empty BOS piece.
    pub bos_piece: Option<String>,
    /// Parsed `tokenizer.ggml.add_bos_token` metadata, or `None` when the key
    /// is absent or unparseable.
    pub meta_add_bos: Option<bool>,
    /// Whether `rendered` already begins with the literal `bos_piece` — the
    /// leg that makes the Gemma / Llama-3 family skip the extra BOS.
    pub template_embeds_bos: bool,
    /// Final decision: `true` ⇒ tokenize with `AddBos::Always`, `false` ⇒
    /// `AddBos::Never`. Equals [`should_prepend_bos`] over the same inputs.
    pub prepend_bos: bool,
}

/// Build a [`BosDecision`] from the model-constant BOS inputs and a rendered
/// prompt. Pure projection of [`should_prepend_bos`] plus the intermediate
/// `template_embeds_bos` flag, surfaced so the real-model smoke can pinpoint
/// *which* leg drove the outcome.
pub fn bos_decision(
    rendered: &str,
    bos_piece: Option<&str>,
    meta_add_bos: Option<bool>,
) -> BosDecision {
    BosDecision {
        bos_piece: bos_piece.map(str::to_string),
        meta_add_bos,
        template_embeds_bos: template_embeds_bos(rendered, bos_piece),
        prepend_bos: should_prepend_bos(rendered, bos_piece, meta_add_bos),
    }
}

/// Validate that `path` points to an existing GGUF file. Dev-facing error.
pub fn validate_gguf_path(path: &Path) -> Result<(), InferenceError> {
    if !path.is_file() {
        return Err(format!(
            "GGUF model file does not exist or is not a file: {}",
            path.display()
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use primer_core::consts::inference::{LLAMACPP_GPU_LAYERS_ALL, LLAMACPP_GPU_LAYERS_CPU};
    use std::path::PathBuf;

    #[test]
    fn sampler_spec_carries_params_and_default_seed() {
        let p = GenerationParams {
            top_p: 0.8,
            temperature: 0.5,
            ..Default::default()
        };
        let spec = sampler_spec(&p);
        assert_eq!(spec.top_p, 0.8);
        assert_eq!(spec.temperature, 0.5);
        assert_eq!(spec.seed, LLAMACPP_DEFAULT_SAMPLER_SEED);
    }

    #[test]
    fn gpu_layers_explicit_override_wins() {
        assert_eq!(resolve_gpu_layers(Some(20)), 20);
        assert_eq!(resolve_gpu_layers(Some(0)), 0);
    }

    #[test]
    fn gpu_layers_default_matches_compiled_features() {
        // On a plain (no-GPU-feature) test build this is CPU (0). The value
        // is the const for whichever arm the cfg selects, so assert it is
        // exactly one of the two sentinels.
        let v = resolve_gpu_layers(None);
        assert!(v == LLAMACPP_GPU_LAYERS_ALL || v == LLAMACPP_GPU_LAYERS_CPU);
    }

    #[test]
    fn n_ctx_override_and_default() {
        assert_eq!(resolve_n_ctx(Some(4096)), 4096);
        assert_eq!(resolve_n_ctx(None), LLAMACPP_DEFAULT_N_CTX);
    }

    #[test]
    fn stop_sequence_trims_marker_from_visible_piece() {
        let stops = vec!["</s>".to_string(), "\n\nUser:".to_string()];
        // Marker wholly inside the final piece → emit only the prefix.
        assert_eq!(
            visible_prefix_before_stop("bye</s>", "hello bye</s>", &stops),
            Some("bye")
        );
        // Piece is exactly the marker → nothing visible.
        assert_eq!(
            visible_prefix_before_stop("</s>", "hi</s>", &stops),
            Some("")
        );
        // Multi-byte char before the marker stays intact (char-boundary trim).
        assert_eq!(
            visible_prefix_before_stop("ü</s>", "café ü</s>", &stops),
            Some("ü")
        );
        // No stop matched → None (forward the piece unchanged).
        assert_eq!(
            visible_prefix_before_stop(" there", "hello there", &stops),
            None
        );
        // Empty stop strings never match.
        assert_eq!(
            visible_prefix_before_stop("x", "anything", &["".to_string()]),
            None
        );
    }

    #[test]
    fn parse_add_bos_metadata_recognises_true_false_and_numeric() {
        assert_eq!(parse_add_bos_metadata("true"), Some(true));
        assert_eq!(parse_add_bos_metadata("false"), Some(false));
        assert_eq!(parse_add_bos_metadata("1"), Some(true));
        assert_eq!(parse_add_bos_metadata("0"), Some(false));
    }

    #[test]
    fn parse_add_bos_metadata_is_case_insensitive_and_trims() {
        assert_eq!(parse_add_bos_metadata(" TRUE "), Some(true));
        assert_eq!(parse_add_bos_metadata("False"), Some(false));
    }

    #[test]
    fn parse_add_bos_metadata_returns_none_for_unrecognised() {
        assert_eq!(parse_add_bos_metadata("yes"), None);
        assert_eq!(parse_add_bos_metadata(""), None);
        assert_eq!(parse_add_bos_metadata("2"), None);
    }

    #[test]
    fn should_prepend_bos_skips_when_template_embeds_literal_bos() {
        // Gemma-style: rendered already starts with the literal BOS marker.
        // The guard wins even when metadata says add_bos_token = true.
        assert!(!should_prepend_bos(
            "<bos><start_of_turn>user\nhi",
            Some("<bos>"),
            Some(true),
        ));
        // Llama 3-style literal BOS marker, no metadata.
        assert!(!should_prepend_bos(
            "<|begin_of_text|>system\n",
            Some("<|begin_of_text|>"),
            None,
        ));
    }

    #[test]
    fn should_prepend_bos_honours_metadata_when_no_literal_bos() {
        // Template does NOT embed the literal BOS → respect metadata.
        assert!(should_prepend_bos("User: hi", Some("<s>"), Some(true)));
        assert!(!should_prepend_bos("User: hi", Some("<s>"), Some(false)));
    }

    #[test]
    fn should_prepend_bos_defaults_to_true_when_metadata_absent() {
        // Historical AddBos::Always behaviour for common chat models.
        assert!(should_prepend_bos("User: hi", Some("<s>"), None));
        assert!(should_prepend_bos("User: hi", None, None));
    }

    #[test]
    fn should_prepend_bos_ignores_empty_bos_piece() {
        // An empty/invalid BOS piece (e.g. a model with no BOS token) must
        // not match the start of every string; fall through to metadata.
        assert!(should_prepend_bos("anything", Some(""), None));
        assert!(!should_prepend_bos("anything", Some(""), Some(false)));
    }

    #[test]
    fn template_embeds_bos_matches_only_a_nonempty_leading_piece() {
        assert!(template_embeds_bos("<bos>hi", Some("<bos>")));
        assert!(!template_embeds_bos("hi<bos>", Some("<bos>"))); // not leading
        assert!(!template_embeds_bos("anything", Some(""))); // empty piece
        assert!(!template_embeds_bos("anything", None)); // no piece
    }

    #[test]
    fn bos_decision_records_gemma_skip_via_template_leg() {
        // Gemma-shaped: rendered starts with the literal BOS → skip the extra
        // BOS even though metadata says add_bos_token = true.
        let d = bos_decision("<bos><start_of_turn>user\nhi", Some("<bos>"), Some(true));
        assert_eq!(d.bos_piece.as_deref(), Some("<bos>"));
        assert_eq!(d.meta_add_bos, Some(true));
        assert!(d.template_embeds_bos);
        assert!(!d.prepend_bos);
    }

    #[test]
    fn bos_decision_records_qwen_add_once_via_default_leg() {
        // Qwen3-shaped: no literal BOS in the rendered prompt, no metadata →
        // historical add-once path. The template leg did NOT drive it.
        let d = bos_decision("<|im_start|>user\nhi", Some("<|endoftext|>"), None);
        assert!(!d.template_embeds_bos);
        assert!(d.prepend_bos);
    }

    #[test]
    fn bos_decision_prepend_matches_should_prepend_bos() {
        // The recorded decision is exactly should_prepend_bos over the same
        // inputs, across the cross-product of the interesting cases.
        let cases = [
            ("<bos>x", Some("<bos>"), Some(true)),
            ("User: hi", Some("<s>"), Some(false)),
            ("User: hi", Some("<s>"), None),
            ("anything", Some(""), None),
            ("anything", None, Some(true)),
        ];
        for (rendered, bos, meta) in cases {
            assert_eq!(
                bos_decision(rendered, bos, meta).prepend_bos,
                should_prepend_bos(rendered, bos, meta),
                "mismatch for {rendered:?}/{bos:?}/{meta:?}"
            );
        }
    }

    #[test]
    fn validate_gguf_path_rejects_missing() {
        let missing = PathBuf::from("/no/such/model.gguf");
        assert!(validate_gguf_path(&missing).is_err());
    }

    #[test]
    fn validate_gguf_path_accepts_real_file() {
        let f = tempfile::NamedTempFile::new().unwrap();
        assert!(validate_gguf_path(f.path()).is_ok());
    }
}
