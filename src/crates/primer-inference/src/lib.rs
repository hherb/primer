//! # primer-inference
//!
//! LLM inference backend implementations.
//!
//! Available backends:
//! - `StubBackend`: Returns canned responses for testing without a model.
//! - `CloudBackend`: Calls the Anthropic Claude API (or compatible endpoint).
//! - `LlamaCppBackend`: (TODO) Binds to llama.cpp for local inference.
//! - `QnnBackend`: (TODO) Qualcomm QNN SDK for Snapdragon NPU.
//! - `RknnBackend`: (TODO) Rockchip RKNN-LLM for RK1828 NPU.

pub mod stub;
pub mod cloud;

pub use stub::StubBackend;
pub use cloud::CloudBackend;
