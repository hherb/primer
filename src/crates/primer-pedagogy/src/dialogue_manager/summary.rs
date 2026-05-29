//! Rolling-summary refresh helpers for the dialogue manager.
//!
//! Two cadences:
//! - `refresh_summary_if_due` — active-conversation cadence. K-threshold:
//!   re-summarize when `context_window_turns` new pre-window turns have
//!   accumulated (default K=20). Avoids burning an LLM call every turn.
//! - `refresh_summary_if_stale` — resume cadence. Simple "is the summary
//!   out of date?" check. Resume already pays a wall-clock cost; an extra
//!   summary call is acceptable but only if it would actually advance the
//!   summary's coverage.
//!
//! Both flows share `regenerate_summary_through`, which calls the
//! inference backend's `summarize` method and updates the session's
//! summary fields on success. Best-effort: failures emit `tracing::warn!`
//! and leave previous state intact.

use super::DialogueManager;

impl DialogueManager {
    /// Active-conversation cadence. Refresh the rolling summary when at
    /// least `context_window_turns` turns have fallen out of the window
    /// since `summary_through_turn_index` was last set, so per-turn
    /// dialogue doesn't trigger an LLM call every time the boundary
    /// advances. At the default K=20, a summary is built each time 20
    /// new turns have rolled past the boundary.
    pub(super) async fn refresh_summary_if_due(&mut self) {
        let window = self
            .config
            .effective_context_window_turns(self.inference.name());
        let total = self.session.turns.len();
        if total <= window {
            return;
        }
        let pre_window_end = total - window;
        let already_covered = self.session.summary_through_turn_index;
        if pre_window_end < already_covered.saturating_add(window) {
            return;
        }
        self.regenerate_summary_through(pre_window_end).await;
    }

    /// Resume cadence. Refresh the rolling summary when the loaded
    /// session has pre-window content the existing summary doesn't
    /// yet cover. A summary that's already current is preserved
    /// verbatim — there is no value in regenerating identical work.
    pub(super) async fn refresh_summary_if_stale(&mut self) {
        let window = self
            .config
            .effective_context_window_turns(self.inference.name());
        let total = self.session.turns.len();
        if total <= window {
            return;
        }
        let pre_window_end = total - window;
        if self.session.summary_through_turn_index >= pre_window_end {
            return;
        }
        self.regenerate_summary_through(pre_window_end).await;
    }

    /// Common body: re-summarize `turns[..pre_window_end]` from scratch
    /// and stamp the new boundary. Replacing rather than incrementally
    /// extending keeps the summary coherent; the simplicity is fine at
    /// Phase-0 cost. Best-effort: a summary failure is logged and the
    /// previous state stays in place.
    async fn regenerate_summary_through(&mut self, pre_window_end: usize) {
        let to_summarize = &self.session.turns[..pre_window_end];
        match self.inference.summarize(to_summarize, 1500).await {
            Ok(summary) => {
                self.session.summary = summary;
                self.session.summary_through_turn_index = pre_window_end;
            }
            Err(e) => tracing::warn!("summary refresh failed: {e}"),
        }
    }
}
