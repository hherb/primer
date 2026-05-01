//! # primer-core
//!
//! Core traits, types, and abstractions for the Primer learning companion.
//!
//! This crate defines the API contracts between the major subsystems:
//! - **Inference**: LLM text generation (local or cloud)
//! - **Speech**: STT and TTS
//! - **Knowledge**: RAG retrieval from the offline knowledge base
//! - **Pedagogy**: Socratic dialogue management, learner model, comprehension verification
//!
//! Each subsystem is implemented in its own crate behind these traits,
//! allowing backends to be swapped by configuration without touching
//! the pedagogical engine.

pub mod classifier;
pub mod config;
pub mod conversation;
pub mod error;
pub mod inference;
pub mod knowledge;
pub mod learner;
pub mod speech;
pub mod storage;
