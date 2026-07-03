//! Default values for invariant numerics shared across primer-core
//! modules. Per the no-magic-numbers convention, every numeric used
//! by primer-core helpers is defined here (or in a sibling settings
//! struct field for tunables).
//!
//! Split by responsibility into one submodule per concern. Each
//! submodule declared below owns the constants for one feature area;
//! external `primer_core::consts::<area>::<NAME>` paths are unchanged
//! by the split.

pub mod break_suggest;
pub mod inference;
pub mod learner;
pub mod pedagogy;
pub mod prompt_budget;
pub mod qnn;
pub mod reasoning;
pub mod retrieval;
pub mod retry;
pub mod router;
pub mod speech;
pub mod vocab;

#[cfg(test)]
mod tests;
