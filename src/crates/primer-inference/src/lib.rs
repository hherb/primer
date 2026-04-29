//! # primer-inference
//!
//! LLM inference backend implementations.
//!
//! Available backends:
//! - `StubBackend`: Returns canned responses for testing without a model.
//! - `CloudBackend`: Calls the Anthropic Claude API (or compatible endpoint).
//! - `OllamaBackend`: Calls a local Ollama server — handy for prototype
//!   testing against real models without integrating llama.cpp directly.
//! - `LlamaCppBackend`: (TODO) Binds to llama.cpp for local inference.
//! - `QnnBackend`: (TODO) Qualcomm QNN SDK for Snapdragon NPU.
//! - `RknnBackend`: (TODO) Rockchip RKNN-LLM for RK1828 NPU.

pub mod stub;
pub mod cloud;
pub mod ollama;

pub use stub::StubBackend;
pub use cloud::CloudBackend;
pub use ollama::OllamaBackend;
