//! # primer-pedagogy
//!
//! The Socratic dialogue engine — the Primer's soul.
//!
//! This crate is responsible for:
//! - Constructing system prompts that make the LLM behave Socratically
//! - Deciding what kind of response to generate (question, scaffold, extension)
//! - Assessing the child's comprehension from their responses
//! - Updating the learner model based on conversation evidence
//! - Managing conversation flow (when to probe, when to scaffold, when to stop)
//!
//! The pedagogical engine does NOT call the LLM directly. It produces
//! a `Prompt` that the orchestrator sends to whatever `InferenceBackend`
//! is active. This separation means the pedagogy can be tested with the
//! stub backend, without any model loaded.

pub mod consts;
pub mod dialogue_manager;
pub mod prompt_builder;

pub use dialogue_manager::DialogueManager;
