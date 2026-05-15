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
//! - `LlamaCppBackend`: (TODO) Binds to llama.cpp for local inference.
//! - `QnnBackend`: (TODO) Qualcomm QNN SDK for Snapdragon NPU.
//! - `RknnBackend`: (TODO) Rockchip RKNN-LLM for RK1828 NPU.

pub mod cloud;
pub mod ollama;
pub mod openai_compat;
pub mod stub;

pub use cloud::CloudBackend;
pub use ollama::OllamaBackend;
pub use openai_compat::OpenAiCompatBackend;
pub use stub::StubBackend;
