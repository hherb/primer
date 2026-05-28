//! Qualcomm NPU inference backend.
//!
//! Behind the `qnn` cargo feature on `primer-inference`. Pulls in
//! [`primer_qnn_sys`] (the FFI scaffold from Phase 1.2 step 1.2.1) and
//! [`minijinja`] for chat-template rendering.
//!
//! ## Layered module shape
//!
//! - [`meta`] — parser for the `primer-meta.json` sidecar (model id,
//!   context length, chat template, stop sequences).
//! - [`template`] — Jinja2 chat-template renderer over [`primer_core::inference::Prompt`].
//! - [`genie`] — trait abstraction over Genie C calls (so a mock library
//!   can stub the backend in tests) plus the real `libGenie.so`-backed
//!   implementation.
//! - [`backend`] — [`QnnBackend`], the `InferenceBackend` impl that
//!   orchestrates meta + template + genie at construction and per call.
//!
//! ## What this step ships (1.2.2)
//!
//! The non-streaming construction → query → free path, exercised entirely
//! against a mock Genie library on host. `generate_stream` returns a
//! single `TokenChunk{ text, done: true }` for now; the per-token C-ABI
//! callback bridge lands in step 1.2.3.

pub mod backend;
pub mod genie;
pub mod meta;
pub mod template;

pub use backend::QnnBackend;
pub use genie::{GenieCallError, GenieDialog, GenieLibrary};
pub use meta::{MetaError, PRIMER_META_FILENAME, PrimerMeta};
pub use template::{ChatTemplate, TemplateError};
