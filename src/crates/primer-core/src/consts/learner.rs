//! Defaults shared across the CLI, GUI, and frontend for the learner
//! profile. Kept here so a brand-new install behaves identically across
//! every entry point — and so the JS picker has a documented Rust
//! source of truth to mirror.

/// Placeholder name a fresh learner profile carries until the user
/// supplies their own. Used as both:
/// - the CLI's `--name` default
/// - the GUI's `LearnerConfig::default().name`
/// - the JS picker's "is the name still the default?" check (it
///   suppresses the personalised "Welcome back, {name}" greeting
///   for unconfigured installs). The JS side mirrors this literal;
///   keep them in sync when changing.
pub const DEFAULT_NAME: &str = "Explorer";
