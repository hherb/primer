//! Backend-name conventions shared across crate boundaries.
//!
//! An inference backend's `name()` is the one piece of backend identity
//! that crosses from `primer-inference` into `primer-pedagogy` *without* a
//! crate dependency: the dialogue manager only ever sees a
//! `&dyn InferenceBackend` (a `primer-core` trait) and so can read
//! `name()` but cannot import anything concrete from `primer-inference`.
//!
//! Naming conventions therefore live here, in the shared layer, so the
//! backend that *produces* the name and the pedagogy that *consumes* it
//! agree on one constant rather than two hand-copied string literals.

/// Prefix that `QnnBackend::name()` prepends to its `model_id`
/// (e.g. `"qnn:Qwen3-4B"`).
///
/// `primer-inference` re-exports this as
/// `primer_inference::qnn::QNN_NAME_PREFIX`, so the producer and the
/// dialogue manager's per-backend context-budget logic (step 1.2.5) share
/// a single source of truth.
pub const QNN_NAME_PREFIX: &str = "qnn:";

/// Prefix that `LlamaCppBackend::name()` prepends to its model id
/// (e.g. `"llamacpp:Qwen3-7B-Q4_K_M"`).
///
/// Re-exported by `primer-inference` as
/// `primer_inference::llamacpp::LLAMACPP_NAME_PREFIX`. Unlike
/// [`QNN_NAME_PREFIX`], a `llamacpp:`-named backend is **not** treated as
/// small-context: local llama models commonly run 8K+ context. A future
/// constrained-device path can revisit [`is_small_context_backend`].
pub const LLAMACPP_NAME_PREFIX: &str = "llamacpp:";

/// True if a backend with this `name()` is a small-context (≈4K-token)
/// backend that should run under the constrained pedagogy budget — a
/// shorter recent-turn window and fewer retrieved passages — instead of
/// the global defaults tuned for large-context cloud models.
///
/// Today only the Qualcomm NPU backend qualifies. A future 4K-bound
/// backend (e.g. an RKNN NPU model) joins by adding its `name()` prefix
/// here — no config-field rename needed, which is exactly why the budget
/// fields on [`crate::config::PedagogyConfig`] are named `*_small_context`
/// rather than `*_qnn`.
pub fn is_small_context_backend(backend_name: &str) -> bool {
    backend_name.starts_with(QNN_NAME_PREFIX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qnn_prefixed_name_is_small_context() {
        assert!(is_small_context_backend("qnn:Qwen3-4B"));
    }

    #[test]
    fn bare_qnn_prefix_is_small_context() {
        assert!(is_small_context_backend(QNN_NAME_PREFIX));
    }

    #[test]
    fn cloud_and_stub_names_are_not_small_context() {
        assert!(!is_small_context_backend("claude-sonnet-4-6"));
        assert!(!is_small_context_backend("stub"));
        assert!(!is_small_context_backend("ollama:llama3.2"));
    }

    #[test]
    fn empty_name_is_not_small_context() {
        assert!(!is_small_context_backend(""));
    }

    #[test]
    fn qnn_substring_not_at_start_is_not_small_context() {
        // The detection is prefix-anchored, not a substring search, so a
        // backend that merely mentions "qnn" later in its name is unaffected.
        assert!(!is_small_context_backend("my-qnn:model"));
    }

    #[test]
    fn llamacpp_prefix_value_and_not_small_context() {
        assert_eq!(LLAMACPP_NAME_PREFIX, "llamacpp:");
        // Local llama models commonly run 8K+ context; the constrained 3B
        // path is deferred (Phase 1.1 bullet c), so llamacpp is NOT small-context.
        assert!(!is_small_context_backend("llamacpp:Qwen3-7B-Q4_K_M"));
    }
}
