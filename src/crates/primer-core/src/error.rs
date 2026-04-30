//! Unified error types for the Primer system.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum PrimerError {
    #[error("Inference error: {0}")]
    Inference(String),

    #[error("Speech error: {0}")]
    Speech(String),

    #[error("Knowledge base error: {0}")]
    Knowledge(String),

    #[error("Learner model error: {0}")]
    LearnerModel(String),

    #[error("Storage error: {0}")]
    Storage(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialisation error: {0}")]
    Serde(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, PrimerError>;
