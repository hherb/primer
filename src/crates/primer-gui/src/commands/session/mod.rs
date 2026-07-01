//! Session commands, split by responsibility:
//!
//! - [`lifecycle`] — start / close / resume / list / current-info, plus
//!   the `close_session_inner` + `prepare_for_session_change` teardown
//!   helpers other command modules reuse.
//! - [`readers`] — the sidebar reader commands (`get_turn_signals`,
//!   `get_learner_state`, `list_session_turns`, `get_full_session_turns`)
//!   and the pure `read_*` shape mappers they call.
//! - [`turn`] — the per-turn streaming command `send_message`, the
//!   `cancel_response` command, and the `run_turn` / `refresh_snapshot`
//!   helpers.
//!
//! The flat `pub use` re-export façade keeps every external path
//! (`commands::session::<name>`) identical to the pre-split module and
//! lets the shared `tests` module resolve every symbol via `super::*`.

mod lifecycle;
mod readers;
mod turn;

pub use lifecycle::*;
pub use readers::*;
pub use turn::*;

#[cfg(test)]
mod tests;
