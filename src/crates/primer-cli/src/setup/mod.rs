//! The startup steps `async_main` runs before the conversation starts,
//! one file each (split out of `main.rs`, behaviour-preserving).

mod embedder;
mod inference;
mod session;

pub(crate) use embedder::build_embedder;
pub(crate) use inference::{Inference, build_inference};
pub(crate) use session::{SessionSetup, open_session};
