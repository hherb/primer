//! Backend / classifier / extractor / comprehension / embedder
//! construction matrix shared between binaries.
//!
//! The dispatch logic for "main backend + per-subsystem override"
//! lives here so `primer-cli` and `primer-gui` produce identical
//! wiring from identical inputs.
//!
//! This module is split by responsibility across sibling files, all
//! re-exported below so `primer_engine::wiring::<name>` paths are
//! unchanged:
//! - [`backend`] — the `--backend` dispatch, opt-in fallback, router,
//!   and the feature-gated QNN / llama.cpp constructors.
//! - [`subsystems`] — classifier / extractor / comprehension builders.
//! - [`embedders`] — the three `Embedder` backends.
//!
//! The shared [`BackendParams`] input struct and the QAIRT path helpers
//! stay in this file: they are the common vocabulary the submodules (and
//! the `qnn_dispatch_tests`) borrow via `super::`.

use std::path::{Path, PathBuf};

mod backend;
mod embedders;
mod subsystems;

pub use backend::{
    MainBackendPlan, build_backend, build_main_backend, plan_main_backend, resolve_fallback_model,
};
pub use embedders::{
    build_fastembed_embedder, build_ollama_embedder, build_openai_compat_embedder,
};
pub use subsystems::{build_classifier, build_comprehension, build_extractor};

/// Parameters needed by `build_backend` and the per-subsystem builders
/// that would otherwise require borrowing from the binary's `Cli` /
/// settings struct. Extracted so the helpers can be called after
/// partial moves of the source-of-truth fields.
///
/// **Invariant:** every backend-affecting CLI flag in `primer-cli` (and
/// the eventual `primer-gui` equivalent) must round-trip through this
/// struct. Adding a new flag means adding a field here AND threading it
/// in at the construction site — silent omission would let the binary
/// see a flag the wiring helpers ignore.
pub struct BackendParams {
    pub api_key: Option<String>,
    pub ollama_url: String,
    pub openai_compat_url: String,
    pub openai_compat_api_key: Option<String>,
    pub classifier_backend: Option<String>,
    pub classifier_model: Option<String>,
    pub extractor_backend: Option<String>,
    pub extractor_model: Option<String>,
    pub comprehension_backend: Option<String>,
    pub comprehension_model: Option<String>,
    /// QNN bundle directory (Phase 1.2 step 1.2.4). Contains
    /// `genie_config.json`, `primer-meta.json`, and the per-shard
    /// context binaries. Consumed by the `"qnn"` arm of
    /// [`build_backend`]; ignored by every other arm. Always present
    /// in the struct (not `#[cfg(feature = "qnn")]`-gated) so the
    /// struct shape stays identical across feature combinations —
    /// the qnn arm itself is the only feature-gated piece.
    pub qnn_bundle_dir: Option<PathBuf>,
    /// QNN QAIRT library directory (containing `libGenie.so` +
    /// dependencies). Consumed by the `"qnn"` arm only. Callers can
    /// resolve a sensible default with [`default_qairt_lib_dir`] from
    /// a known bundle dir; the CLI surfaces this as the optional
    /// `--qnn-qairt-lib-dir` flag with the same default applied when
    /// unset.
    pub qnn_qairt_lib_dir: Option<PathBuf>,
    /// Path to the GGUF model file. Consumed by the `"llamacpp"` arm of
    /// [`build_backend`] only; ignored by every other arm. Always present
    /// in the struct (qnn-style) so the shape is identical across feature
    /// combinations. The CLI surfaces this by reusing `--model`.
    pub gguf_path: Option<PathBuf>,
    /// Explicit `n_gpu_layers` override for the llama.cpp backend
    /// (`--llamacpp-gpu-layers`). `None` ⇒ resolved by feature
    /// (`primer_inference::llamacpp::params::resolve_gpu_layers`).
    pub llamacpp_gpu_layers: Option<i32>,
    /// Explicit `n_ctx` override (`--llamacpp-n-ctx`). `None` ⇒ the model's
    /// trained default.
    pub llamacpp_n_ctx: Option<u32>,
    /// Extra `(open, close)` reasoning-marker pairs appended to the built-in
    /// defaults for the Ollama / openai-compat backends. Empty ⇒ defaults
    /// only. Ignored by every other backend arm.
    ///
    /// These markers propagate to any subsystem backend (classifier,
    /// extractor, comprehension) constructed through the same `params` via
    /// [`build_backend`]. That is intentional: the built-in defaults SHOULD
    /// strip reasoning from subsystem responses too (it keeps their JSON
    /// output clean for parsing), and a user-supplied custom pair applies
    /// everywhere rather than to the chat backend alone.
    pub reasoning_markers: Vec<(String, String)>,
    /// Opt-in fallback secondary backend name (`stub`/`cloud`/`ollama`/
    /// `openai-compat`). `None` ⇒ no fallback ⇒ local-only (the privacy
    /// default). Consumed only by [`build_main_backend`]; the flag's
    /// presence is the explicit cloud-fallback consent.
    pub fallback_backend: Option<String>,
    /// Model for the fallback secondary. Resolution rules live in
    /// [`resolve_fallback_model`]. `None` is valid (cloud defaults; stub
    /// ignores it; ollama/openai-compat error).
    pub fallback_model: Option<String>,
    /// Phase 1.3 inference-router mode. `LocalOnly` (the default) ⇒ today's
    /// behavior (primary or `FallbackBackend`). `Hybrid`/`CloudPreferred` ⇒
    /// `build_main_backend` produces a `RouterBackend` over the primary +
    /// secondary legs. Consumed only by [`build_main_backend`].
    pub router_mode: primer_core::router::RouterMode,
    /// Phase 1.3 latency-aware routing budget (ms). `None` ⇒ latency routing
    /// OFF (the default). When set AND `router_mode == Hybrid`, the
    /// `RouterBackend` nudges a turn toward the secondary when its rolling
    /// primary-leg TTFT EMA exceeds this budget. Consumed only by
    /// [`build_main_backend`]'s router path.
    pub primary_ttft_budget_ms: Option<u64>,
}

/// Conventional default location of the QAIRT runtime libraries
/// relative to a QNN bundle directory.
///
/// Pure helper — no filesystem access, no library load. Returns the
/// path `<bundle>/../qairt/lib/aarch64-android/` exactly as the QAIRT
/// SDK layout documents under "AI Hub apps" assets. Callers may pass
/// this directly to [`build_backend`] via
/// [`BackendParams::qnn_qairt_lib_dir`] when the user did not provide
/// an explicit override.
pub fn default_qairt_lib_dir(bundle_dir: &Path) -> PathBuf {
    bundle_dir
        .parent()
        .map(|parent| parent.join("qairt/lib/aarch64-android"))
        // Bundle paths are always absolute in practice (CLI clamps
        // to `PathBuf`), but tolerate the missing-parent case by
        // returning a same-directory-relative fallback rather than
        // panicking — the downstream `RealGenieLibrary::open` will
        // produce a clear `LibraryLoad` error.
        .unwrap_or_else(|| PathBuf::from("qairt/lib/aarch64-android"))
}

/// Resolve the QAIRT runtime-library directory to hand the QNN backend,
/// given the user's optional override and the bundle dir.
///
/// Platform-aware (delegates to the pure [`resolve_qairt_lib_dir_for`]):
///
/// - **Android:** the 9 QAIRT `.so`s ship inside the APK's
///   `lib/arm64-v8a/` and are extracted to the app's `nativeLibraryDir`,
///   which the system linker already searches. An absent override
///   therefore resolves to an **empty** path, signalling
///   [`primer_inference::QnnBackend`] to dlopen `libGenie.so` by
///   *basename* (so the linker finds it and its DT_NEEDED deps in
///   nativeLibraryDir). `qnn_qairt_lib_dir` is thus unnecessary on-device.
/// - **Desktop / non-Android:** an absent override falls back to the
///   conventional bundle-relative [`default_qairt_lib_dir`] layout.
///
/// An explicit override always wins on every platform.
pub fn resolve_qairt_lib_dir(explicit: Option<PathBuf>, bundle_dir: &Path) -> PathBuf {
    resolve_qairt_lib_dir_for(explicit, bundle_dir, cfg!(target_os = "android"))
}

/// Pure core of [`resolve_qairt_lib_dir`] with the platform decision
/// lifted to an explicit `is_android` parameter so both branches are
/// host-testable. See [`resolve_qairt_lib_dir`] for the rationale.
fn resolve_qairt_lib_dir_for(
    explicit: Option<PathBuf>,
    bundle_dir: &Path,
    is_android: bool,
) -> PathBuf {
    match explicit {
        Some(dir) => dir,
        None if is_android => PathBuf::new(),
        None => default_qairt_lib_dir(bundle_dir),
    }
}

#[cfg(test)]
mod build_main_backend_tests;
#[cfg(test)]
mod classifier_construction_tests;
#[cfg(test)]
mod comprehension_construction_tests;
#[cfg(test)]
mod extractor_construction_tests;
#[cfg(test)]
mod main_backend_plan_tests;
#[cfg(test)]
mod qnn_dispatch_tests;
#[cfg(test)]
mod resolve_fallback_model_tests;
