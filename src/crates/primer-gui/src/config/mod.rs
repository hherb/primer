//! `GuiConfig` — the GUI's persisted settings.
//!
//! Mirrors the [`primer-cli`](../../primer-cli) flag set 1-for-1 so a
//! parent who has used the CLI can switch to the GUI without learning
//! a new vocabulary. Persisted to `~/.primer/gui-config.json` (atomic
//! temp-file write + rename, mode 0600 since the file may carry an
//! inline API key).
//!
//! Defaults are derived from the CLI's clap defaults so a brand-new
//! install behaves identically to `primer` with no flags.
//!
//! **Secret handling:** the inline API key never crosses the IPC
//! boundary in either direction *unless explicitly being set*. The
//! [`view`] / [`update`](view) DTOs are the only types serialised on the
//! frontend round-trip; [`GuiConfig`] itself is reserved for disk and
//! the Rust-side wiring path. See [`ApiKeySource`] / [`ApiKeyUpdate`].
//!
//! The crate is split three ways: [`types`] holds the persisted on-disk
//! shapes, [`persistence`] holds the load/save plumbing, and [`view`]
//! holds the frontend-facing read/write DTOs. Everything is re-exported
//! here so external callers keep using the flat `crate::config::Foo`
//! path regardless of which sub-module a symbol lives in.

mod persistence;
mod types;
mod view;

pub use persistence::*;
pub use types::*;
pub use view::*;

#[cfg(test)]
mod tests;
