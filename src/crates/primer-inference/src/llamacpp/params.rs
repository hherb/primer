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
