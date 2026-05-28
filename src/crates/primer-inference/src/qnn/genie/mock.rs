//! Programmable mock implementation of [`super::GenieLibrary`] /
//! [`super::GenieDialog`] for unit tests.
//!
//! Records every method invocation so a test can pin both the call
//! sequence and the per-call arguments. `#[cfg(test)]` only — never
//! reaches release builds.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::{GenieCallError, GenieDialog, GenieLibrary};

/// Recorded events produced by [`MockGenieLibrary`] / [`MockGenieDialog`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MockEvent {
    OpenDialog { config_path: PathBuf },
    Query { prompt: String },
    DropDialog,
}

/// Programmable mock. Single recorded "session" per construction —
/// returns the canned response on every `query_blocking` call. Tests
/// that need different per-call behaviour can construct a fresh mock
/// or extend this with a queue.
pub struct MockGenieLibrary {
    inner: Arc<MockInner>,
}

struct MockInner {
    events: Mutex<Vec<MockEvent>>,
    /// `Some(err)` → the next `open_dialog` returns `Err(err)` and
    /// resets to `None`; `None` → `open_dialog` returns a fresh
    /// `MockGenieDialog`. One-shot to keep behaviour predictable —
    /// `GenieCallError` is not `Clone` (transitively through
    /// `std::io::Error`) so a persistent error would force a
    /// `Box::leak`-style workaround for marginal value.
    open_error: Mutex<Option<GenieCallError>>,
    /// `Some(err)` → the next `query_blocking` against any dialog
    /// returned by this library returns `Err(err)` and resets to
    /// `None`; `None` → returns the canned response. Same one-shot
    /// rationale as `open_error`.
    query_error: Mutex<Option<GenieCallError>>,
    canned_response: String,
}

impl MockGenieLibrary {
    /// Mock that returns `Ok(MockGenieDialog)` from `open_dialog`
    /// and `canned_response` from every `query_blocking`.
    pub fn new_with_response(canned_response: impl Into<String>) -> Self {
        Self {
            inner: Arc::new(MockInner {
                events: Mutex::new(Vec::new()),
                open_error: Mutex::new(None),
                query_error: Mutex::new(None),
                canned_response: canned_response.into(),
            }),
        }
    }

    /// Mock that returns the given error from the next `open_dialog`
    /// call (one-shot), then returns `Ok` for any subsequent calls.
    /// Used to exercise the construction-error path of the safe
    /// wrapper.
    pub fn new_failing_open(err: GenieCallError) -> Self {
        Self {
            inner: Arc::new(MockInner {
                events: Mutex::new(Vec::new()),
                open_error: Mutex::new(Some(err)),
                query_error: Mutex::new(None),
                canned_response: String::new(),
            }),
        }
    }

    /// Mock whose first `query_blocking` returns the given error and
    /// subsequent queries return the empty canned response. Used to
    /// exercise the smoke-check failure path in
    /// [`super::super::backend::QnnBackend::new_with_library`]: the
    /// smoke check is the first query against a freshly-opened
    /// dialog, so a one-shot failing query is exactly the right
    /// shape.
    pub fn new_failing_query(err: GenieCallError) -> Self {
        Self {
            inner: Arc::new(MockInner {
                events: Mutex::new(Vec::new()),
                open_error: Mutex::new(None),
                query_error: Mutex::new(Some(err)),
                canned_response: String::new(),
            }),
        }
    }

    /// Snapshot of the events recorded so far, in order. Cheap clone
    /// of the inner Vec; tests pin call sequences via this.
    pub fn events(&self) -> Vec<MockEvent> {
        self.inner
            .events
            .lock()
            .expect("mock events mutex poisoned")
            .clone()
    }
}

impl GenieLibrary for MockGenieLibrary {
    fn open_dialog(&self, config_path: &Path) -> Result<Box<dyn GenieDialog>, GenieCallError> {
        self.inner
            .events
            .lock()
            .expect("mock events mutex poisoned")
            .push(MockEvent::OpenDialog {
                config_path: config_path.to_path_buf(),
            });
        if let Some(err) = self
            .inner
            .open_error
            .lock()
            .expect("mock open_error mutex poisoned")
            .take()
        {
            return Err(err);
        }
        Ok(Box::new(MockGenieDialog {
            inner: Arc::clone(&self.inner),
        }))
    }
}

pub struct MockGenieDialog {
    inner: Arc<MockInner>,
}

impl GenieDialog for MockGenieDialog {
    fn query_blocking(&self, prompt: &str) -> Result<String, GenieCallError> {
        self.inner
            .events
            .lock()
            .expect("mock events mutex poisoned")
            .push(MockEvent::Query {
                prompt: prompt.to_string(),
            });
        if let Some(err) = self
            .inner
            .query_error
            .lock()
            .expect("mock query_error mutex poisoned")
            .take()
        {
            return Err(err);
        }
        Ok(self.inner.canned_response.clone())
    }
}

impl Drop for MockGenieDialog {
    fn drop(&mut self) {
        // Best-effort: a panicking thread may leave the mutex
        // poisoned; in that case we can't record the drop but we
        // also don't want to panic again during unwind.
        if let Ok(mut events) = self.inner.events.lock() {
            events.push(MockEvent::DropDialog);
        }
    }
}
