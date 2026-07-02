//! # primer-inference
//!
//! LLM inference backend implementations.
//!
//! Available backends:
//! - `StubBackend`: Returns canned responses for testing without a model.
//! - `CloudBackend`: Calls the Anthropic Claude API (or compatible endpoint).
//! - `OllamaBackend`: Calls a local Ollama server — handy for prototype
//!   testing against real models without integrating llama.cpp directly.
//! - `OpenAiCompatBackend`: Calls any OpenAI-compatible server (oMLX, LM
//!   Studio, vLLM, Together, Groq, OpenRouter, llama.cpp `--server`).
//! - `QnnBackend`: Qualcomm QNN SDK for Snapdragon NPU (behind the
//!   `qnn` cargo feature; Phase 1.2 step 1.2.2 lands the safe wrapper
//!   on top of [`primer-qnn-sys`]).
//! - `LlamaCppBackend`: Embedded llama.cpp inference from a local GGUF
//!   (behind the `llamacpp` cargo feature; CPU + Metal/CUDA/Vulkan
//!   passthrough features). Phase 1.1.
//! - `RknnBackend`: (TODO) Rockchip RKNN-LLM for RK1828 NPU.

pub mod bench;
pub mod cloud;
pub mod fallback;
pub mod llamacpp;
pub mod ollama;
pub mod openai_compat;
mod reasoning_stream;
pub mod router;
mod stream_error;
pub mod stub;

#[cfg(feature = "qnn")]
pub mod qnn;

pub use cloud::CloudBackend;
pub use fallback::FallbackBackend;
pub use llamacpp::LlamaCppBackend;
pub use ollama::OllamaBackend;
pub use openai_compat::OpenAiCompatBackend;
pub use router::RouterBackend;
pub use stub::StubBackend;

#[cfg(feature = "qnn")]
pub use qnn::QnnBackend;
