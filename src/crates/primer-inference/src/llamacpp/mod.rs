//! Embedded llama.cpp inference backend (Phase 1.1).
//!
//! The `LlamaCppBackend`, the `LlamaEngine` trait, a test-only mock, and the
//! pure helpers in [`params`] are ALWAYS compiled and host-tested on the
//! default `cargo test`. Only [`engine::RealLlamaEngine`]'s `llama-cpp-2`
//! calls are behind the `llamacpp` cargo feature. The engine emits raw
//! tokens; the backend strips reasoning markers (so that integration is
//! CI-covered via the mock).

pub mod params;

pub mod engine;

pub use engine::LlamaEngine;
