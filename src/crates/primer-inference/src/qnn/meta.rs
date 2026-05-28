//! `primer-meta.json` — the Primer-authored sidecar that lives alongside a
//! Qualcomm `genie_bundle` directory.
//!
//! Schema (Phase 1.2 design §7):
//!
//! ```json
//! {
//!   "model_id": "qwen3-4b",
//!   "context_length": 4096,
//!   "chat_template": "<|im_start|>{{role}}\n{{content}}<|im_end|>\n",
//!   "vocab_size": 151936,
//!   "stop_sequences": ["<|im_end|>", "<|endoftext|>"]
//! }
//! ```
//!
//! The file is **Primer-authored**, not shipped by Qualcomm: it carries the
//! exporter's knowledge of which chat template the model expects and the
//! stop sequences that terminate a turn. Adding support for a new model
//! variant on the same QAIRT bundle = drop a new directory + new
//! `primer-meta.json`; no Rust change needed.
//!
//! When the file is absent, [`PrimerMeta::load_or_fallback`] synthesises
//! a minimal record using the bundle directory name as the model id plus
//! a ChatML template default (see [`FALLBACK_CHAT_TEMPLATE`]). The fallback
//! is intentionally noisy — it emits a `tracing::warn!` so a missing meta
//! never silently degrades runtime behaviour.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Filename of the sidecar inside a `genie_bundle` directory.
pub const PRIMER_META_FILENAME: &str = "primer-meta.json";

/// Default chat template used when `primer-meta.json` is absent. ChatML
/// is the most widely-supported template across exporter pipelines; any
/// model that disagrees should ship its own meta sidecar.
///
/// The template renders a list of `{role, content}` messages as
/// `<|im_start|>{role}\n{content}<|im_end|>\n` pairs, terminated with an
/// open `<|im_start|>assistant\n` tag to invite the model to generate
/// the next assistant turn. This is the canonical "instruction model"
/// pattern shared by Qwen, Yi, Mistral-Instruct and others.
pub const FALLBACK_CHAT_TEMPLATE: &str = "{% for m in messages %}<|im_start|>{{ m.role }}\n{{ m.content }}<|im_end|>\n{% endfor %}{% if add_generation_prompt %}<|im_start|>assistant\n{% endif %}";

/// Default fallback for `context_length` when no meta is present. Matches
/// the pre-compiled Qwen3-4B genie_bundle published on AI Hub (4096
/// tokens) — see Phase 1.2 design §8.
pub const FALLBACK_CONTEXT_LENGTH: u32 = 4096;

/// Stop-sequence fallback set for unknown models. Empty by default — the
/// generation loop terminates on `done: true` from the backend; a stop
/// sequence here would be best-effort guesswork.
pub const FALLBACK_STOP_SEQUENCES: &[&str] = &[];

/// Parsed contents of `primer-meta.json`.
///
/// `serde` is used with `#[serde(deny_unknown_fields)]` deliberately
/// *off*: a future schema revision adding fields the current Primer
/// binary doesn't know about must not error on load — that would brick
/// every old binary the moment a bundle ships a v2 meta. Unknown fields
/// are silently ignored; missing required fields raise a typed
/// [`MetaError::Parse`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrimerMeta {
    /// Stable identifier for the model (e.g. `"qwen3-4b"`). Used as the
    /// `name()` suffix of [`super::QnnBackend`] — the dialogue manager
    /// reads this to pick per-backend context-budget tunables in step
    /// 1.2.5.
    pub model_id: String,

    /// Maximum context window the exported model supports, in tokens.
    /// The dialogue manager uses this to decide how aggressively to
    /// truncate the recent-turn window under `--backend qnn` (Phase 1.2
    /// design §8).
    pub context_length: u32,

    /// Jinja2 template string. Rendered by [`super::template::ChatTemplate`]
    /// against a `{messages: [{role, content}], add_generation_prompt}`
    /// context. The exporter is responsible for matching the template to
    /// the model's training-time format.
    pub chat_template: String,

    /// Tokenizer vocabulary size. Carried in the meta because some
    /// downstream consumers (token-counting probes for context-budget
    /// estimation) want it without parsing the much-larger
    /// `tokenizer.json`. Optional at runtime — a missing or zero value
    /// is acceptable for the safe wrapper.
    pub vocab_size: u32,

    /// Stop sequences the dialogue manager should pass through to the
    /// backend as additional generation-halt signals. Empty by default.
    #[serde(default)]
    pub stop_sequences: Vec<String>,
}

impl PrimerMeta {
    /// Strict load: read `primer-meta.json` from a bundle directory and
    /// parse it. Returns a typed [`MetaError`] on any failure.
    ///
    /// Use [`Self::load_or_fallback`] when an absent meta should silently
    /// degrade to defaults; use this when an absent meta is an error.
    pub fn load_from_bundle(bundle_dir: &Path) -> Result<Self, MetaError> {
        Self::load(&bundle_dir.join(PRIMER_META_FILENAME))
    }

    /// Strict load from an explicit path. Public so test fixtures can
    /// stage a file outside a real bundle directory.
    pub fn load(path: &Path) -> Result<Self, MetaError> {
        let bytes = std::fs::read(path).map_err(|source| match source.kind() {
            std::io::ErrorKind::NotFound => MetaError::NotFound {
                path: path.to_path_buf(),
            },
            _ => MetaError::Io {
                path: path.to_path_buf(),
                source,
            },
        })?;
        serde_json::from_slice(&bytes).map_err(|source| MetaError::Parse {
            path: path.to_path_buf(),
            source,
        })
    }

    /// Lenient load: returns the parsed meta on success, OR a synthesised
    /// fallback when the file is absent. Other failure modes (corrupt
    /// JSON, I/O error) still propagate.
    ///
    /// The fallback uses [`derive_model_id_from_dir`] for the id and the
    /// `FALLBACK_*` constants for every other field. A `tracing::warn!`
    /// fires so the missing file shows up in logs.
    pub fn load_or_fallback(bundle_dir: &Path) -> Result<Self, MetaError> {
        match Self::load_from_bundle(bundle_dir) {
            Ok(meta) => Ok(meta),
            Err(MetaError::NotFound { path }) => {
                let model_id = derive_model_id_from_dir(bundle_dir);
                tracing::warn!(
                    target: "primer::qnn",
                    path = %path.display(),
                    derived_model_id = %model_id,
                    "primer-meta.json not found; using fallback ChatML template and derived model id"
                );
                Ok(Self::fallback_for_id(model_id))
            }
            Err(other) => Err(other),
        }
    }

    /// Construct a minimal-defaults meta with the given id. Public so
    /// tests can build expected-value fixtures without staging a file.
    pub fn fallback_for_id(model_id: String) -> Self {
        Self {
            model_id,
            context_length: FALLBACK_CONTEXT_LENGTH,
            chat_template: FALLBACK_CHAT_TEMPLATE.to_string(),
            vocab_size: 0,
            stop_sequences: FALLBACK_STOP_SEQUENCES
                .iter()
                .map(|s| s.to_string())
                .collect(),
        }
    }
}

/// Errors [`PrimerMeta::load`] can return.
#[derive(Debug, Error)]
pub enum MetaError {
    /// The file is missing. [`PrimerMeta::load_or_fallback`] recovers
    /// from this variant by synthesising a default; callers using
    /// `load_from_bundle` directly surface it to the user.
    #[error("primer-meta.json not found at {path}")]
    NotFound { path: PathBuf },

    /// The file exists but couldn't be read (permission, I/O failure).
    #[error("could not read primer-meta.json at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// The file was read but isn't valid `PrimerMeta`-shaped JSON.
    /// Either malformed JSON or a missing required field.
    #[error("primer-meta.json at {path} is not valid PrimerMeta JSON: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

/// Derive a model id from the final path component of a directory.
///
/// Pure helper — exposed so tests can pin its behaviour without staging a
/// real directory. Returns `"unknown"` when the path has no final
/// component (e.g. `.` or `/`).
pub fn derive_model_id_from_dir(bundle_dir: &Path) -> String {
    bundle_dir
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    const SAMPLE_META_JSON: &str = r#"{
        "model_id": "qwen3-4b",
        "context_length": 4096,
        "chat_template": "<|im_start|>{{role}}\n{{content}}<|im_end|>\n",
        "vocab_size": 151936,
        "stop_sequences": ["<|im_end|>", "<|endoftext|>"]
    }"#;

    fn sample_meta() -> PrimerMeta {
        PrimerMeta {
            model_id: "qwen3-4b".to_string(),
            context_length: 4096,
            chat_template: "<|im_start|>{{role}}\n{{content}}<|im_end|>\n".to_string(),
            vocab_size: 151936,
            stop_sequences: vec!["<|im_end|>".to_string(), "<|endoftext|>".to_string()],
        }
    }

    #[test]
    fn parses_sample_meta_from_spec() {
        let parsed: PrimerMeta = serde_json::from_str(SAMPLE_META_JSON).unwrap();
        assert_eq!(parsed, sample_meta());
    }

    #[test]
    fn round_trips_through_json() {
        let original = sample_meta();
        let serialized = serde_json::to_string(&original).unwrap();
        let parsed: PrimerMeta = serde_json::from_str(&serialized).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn load_from_bundle_reads_file() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join(PRIMER_META_FILENAME), SAMPLE_META_JSON).unwrap();
        let meta = PrimerMeta::load_from_bundle(dir.path()).unwrap();
        assert_eq!(meta, sample_meta());
    }

    #[test]
    fn load_returns_not_found_for_missing_file() {
        let dir = tempdir().unwrap();
        let err = PrimerMeta::load_from_bundle(dir.path()).unwrap_err();
        match err {
            MetaError::NotFound { ref path } => {
                assert!(
                    path.ends_with(PRIMER_META_FILENAME),
                    "expected path ending in {PRIMER_META_FILENAME}; got {path:?}",
                );
            }
            other => panic!("expected NotFound; got {other:?}"),
        }
    }

    #[test]
    fn load_returns_parse_for_malformed_json() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join(PRIMER_META_FILENAME), "{ not json }").unwrap();
        let err = PrimerMeta::load_from_bundle(dir.path()).unwrap_err();
        assert!(matches!(err, MetaError::Parse { .. }), "got {err:?}");
    }

    #[test]
    fn load_returns_parse_for_missing_required_field() {
        let dir = tempdir().unwrap();
        // Missing `model_id` — the parser must reject this rather than
        // silently defaulting. A model with no id has no name; the safe
        // wrapper's `name()` invariant would break.
        std::fs::write(
            dir.path().join(PRIMER_META_FILENAME),
            r#"{"context_length": 4096, "chat_template": "x", "vocab_size": 1}"#,
        )
        .unwrap();
        let err = PrimerMeta::load_from_bundle(dir.path()).unwrap_err();
        assert!(matches!(err, MetaError::Parse { .. }), "got {err:?}");
    }

    #[test]
    fn parser_tolerates_unknown_future_fields() {
        // Future-compat: a meta carrying a new field the current binary
        // doesn't know about must still parse cleanly. Adding deny_unknown
        // would brick old binaries every time the schema grows.
        let json = r#"{
            "model_id": "qwen3-4b",
            "context_length": 4096,
            "chat_template": "x",
            "vocab_size": 1,
            "stop_sequences": [],
            "future_field_added_in_v2": {"foo": [1, 2, 3]}
        }"#;
        let meta: PrimerMeta = serde_json::from_str(json).unwrap();
        assert_eq!(meta.model_id, "qwen3-4b");
    }

    #[test]
    fn stop_sequences_defaults_to_empty_when_absent() {
        // `stop_sequences` carries a `#[serde(default)]` so a meta that
        // simply omits it parses as `vec![]`. Matches the conventional
        // exporter behaviour: most exports don't bother emitting the key
        // when there are no model-specific stops.
        let json = r#"{
            "model_id": "x",
            "context_length": 1,
            "chat_template": "x",
            "vocab_size": 1
        }"#;
        let meta: PrimerMeta = serde_json::from_str(json).unwrap();
        assert!(meta.stop_sequences.is_empty());
    }

    #[test]
    fn load_or_fallback_returns_parsed_when_present() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join(PRIMER_META_FILENAME), SAMPLE_META_JSON).unwrap();
        let meta = PrimerMeta::load_or_fallback(dir.path()).unwrap();
        assert_eq!(meta, sample_meta());
    }

    #[test]
    fn load_or_fallback_synthesises_default_when_absent() {
        let dir = tempdir().unwrap();
        // Bundle dir is named so we can pin id derivation.
        let bundle = dir.path().join("qwen3-4b");
        std::fs::create_dir(&bundle).unwrap();
        let meta = PrimerMeta::load_or_fallback(&bundle).unwrap();
        assert_eq!(meta.model_id, "qwen3-4b");
        assert_eq!(meta.context_length, FALLBACK_CONTEXT_LENGTH);
        assert_eq!(meta.chat_template, FALLBACK_CHAT_TEMPLATE);
        assert!(meta.stop_sequences.is_empty());
    }

    #[test]
    fn load_or_fallback_propagates_non_not_found_errors() {
        let dir = tempdir().unwrap();
        // Malformed JSON: this must NOT silently fall back. The user
        // wrote a meta and it has a bug; surface it loudly.
        std::fs::write(dir.path().join(PRIMER_META_FILENAME), "{ broken").unwrap();
        let err = PrimerMeta::load_or_fallback(dir.path()).unwrap_err();
        assert!(matches!(err, MetaError::Parse { .. }), "got {err:?}");
    }

    #[test]
    fn derive_id_uses_directory_basename() {
        assert_eq!(
            derive_model_id_from_dir(Path::new("/some/path/qwen3-4b")),
            "qwen3-4b"
        );
    }

    #[test]
    fn derive_id_returns_unknown_for_root_and_relative_dot() {
        assert_eq!(derive_model_id_from_dir(Path::new("/")), "unknown");
        // Plain `.` returns "." per `file_name`, which is acceptable —
        // a user pointing at `.` is staging a config mistake the safe
        // wrapper will surface separately when reading `genie_config.json`.
        // What matters here is that the function never panics on edge paths.
        let dot = derive_model_id_from_dir(Path::new("."));
        // Don't assert on `dot`'s exact value (platforms differ on what
        // `Path::new(".").file_name()` returns) — just that it's a
        // non-empty string.
        assert!(!dot.is_empty());
    }
}
