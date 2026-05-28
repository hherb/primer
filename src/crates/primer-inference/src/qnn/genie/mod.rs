//! Safe trait abstraction over the Genie C API.
//!
//! The Phase 1.2 plan (step 1.2.2 §Tests) calls for:
//!
//! > introduce `GenieLibraryHandle` as a small trait (`open_dialog_from_config`,
//! > `query`, `set_token_callback`) so a `MockGenieLibrary` can stub out
//! > the C calls.
//!
//! This module implements that abstraction:
//!
//! - [`GenieLibrary`] — open a dialog from a `genie_config.json` path.
//! - [`GenieDialog`] — query the dialog (blocking, single-shot for 1.2.2);
//!   `Drop` releases the underlying handle.
//!
//! The real (`libGenie.so`-backed) implementation lives in [`real`] and is
//! reachable on every target — on non-Android hosts, [`RealGenieLibrary::open`]
//! returns a typed [`GenieCallError::PlatformUnsupported`] without touching
//! the filesystem (delegated to [`primer_qnn_sys::GenieLibrary::open`]).
//!
//! A [`mock`] module ships under `#[cfg(test)]` with a programmable
//! [`mock::MockGenieLibrary`] that records calls and returns canned
//! responses. The [`super::backend::QnnBackend`] is generic over
//! `dyn GenieLibrary` so tests can substitute the mock without touching
//! any FFI symbol.

use std::path::Path;

use thiserror::Error;

mod real;
pub use real::RealGenieLibrary;

#[cfg(test)]
pub(crate) mod mock;

/// Per-process handle to the loaded Genie library. Constructed once
/// (lazy dlopen) and shared across [`super::backend::QnnBackend`] instances
/// via `Arc<dyn GenieLibrary>`. Cheap to clone.
pub trait GenieLibrary: Send + Sync {
    /// Open a dialog session from a `genie_config.json` path. The returned
    /// handle owns the underlying dialog and config handles; dropping it
    /// invokes `GenieDialog_free` + `GenieDialogConfig_free`.
    fn open_dialog(&self, config_path: &Path) -> Result<Box<dyn GenieDialog>, GenieCallError>;
}

/// A live Genie dialog session. Single-threaded by C-API contract:
/// concurrent queries against the same dialog are undefined behaviour.
/// [`super::backend::QnnBackend`] wraps this in a `tokio::sync::Mutex` to
/// serialise callers.
pub trait GenieDialog: Send + Sync {
    /// Single-shot blocking query.
    ///
    /// Sends `prompt` to the dialog and collects the full response into a
    /// `String`. This is the step-1.2.2 contract — step 1.2.3 will add a
    /// streaming variant that returns a token receiver and supersedes this
    /// one.
    ///
    /// The real implementation runs the underlying C call inside
    /// [`tokio::task::spawn_blocking`] at the call site (in
    /// [`super::backend`]); the trait itself is synchronous so mocks
    /// don't have to fake an async boundary.
    fn query_blocking(&self, prompt: &str) -> Result<String, GenieCallError>;
}

/// Errors produced by [`GenieLibrary`] or [`GenieDialog`] calls.
#[derive(Debug, Error)]
pub enum GenieCallError {
    /// The current build target has no QAIRT runtime support. Returned by
    /// [`RealGenieLibrary::open`] on non-Android hosts so the safe wrapper
    /// can route to the i18n boundary as
    /// `InferenceError::Other("local model not available")`.
    #[error("Genie runtime is only supported on Android; current platform is {0}")]
    PlatformUnsupported(&'static str),

    /// `libGenie.so` failed to load. Carries the underlying loader error
    /// for developer-facing logs.
    #[error("Genie library dlopen failed: {0}")]
    LibraryLoad(String),

    /// A required symbol was missing from the loaded library. Different
    /// from `LibraryLoad` so the safe wrapper can hint at "QAIRT version
    /// skew between what's installed and what the Primer expects".
    #[error("Genie symbol {symbol} missing from libGenie.so: {detail}")]
    SymbolMissing { symbol: String, detail: String },

    /// `genie_config.json` is missing from the bundle directory.
    #[error("genie_config.json not found at {path}")]
    ConfigNotFound { path: std::path::PathBuf },

    /// The config path could not be encoded as a NUL-terminated C string
    /// (typically because it contains an embedded NUL byte; non-UTF-8
    /// paths on macOS/Linux are fine — they become a `CString` via
    /// `OsStr`).
    #[error("genie_config.json path is not C-string-clean: {detail}")]
    BadConfigPath { detail: String },

    /// A Genie API call returned a non-success status code.
    #[error("Genie call {operation} returned non-success status {status}")]
    NonSuccess {
        /// Name of the C entry point that failed
        /// (e.g. `"GenieDialog_create"`). Static lifetime because we
        /// always pass a string literal — saves a per-error allocation.
        operation: &'static str,
        /// Raw `Genie_Status_t` value.
        status: i32,
    },
}

impl GenieCallError {
    /// Render this error into a `PrimerError::Inference(...)`. The string
    /// is dev-facing per [[project_multilingual_intent]] — user-visible
    /// translation happens via `primer_core::i18n::render_inference_error`
    /// downstream. This helper keeps the dev string consistent across the
    /// three or four call sites that wrap a Genie error.
    pub fn into_inference_error(self) -> primer_core::error::InferenceError {
        // PlatformUnsupported is the only variant the i18n layer might
        // want to discriminate ("local model not on this device"); the
        // others are all genuine inference failures. For step 1.2.2 we
        // collapse all of them into `Other(dev_string)` — step 1.2.4
        // (CLI wiring) is where we'd plumb the distinction through if
        // a translated message demands it.
        primer_core::error::InferenceError::Other(self.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_library_open_returns_platform_unsupported_on_host() {
        // The real library defers to `primer_qnn_sys::GenieLibrary::open`,
        // which short-circuits to `PlatformUnsupported` on every
        // non-Android target. Pin that mapping so a future refactor that
        // accidentally returned a `LibraryLoad` error on the host (e.g.
        // by trying to dlopen first) would fail this test.
        let result = RealGenieLibrary::open(Path::new("/non/existent/qairt/lib"));
        match result {
            Ok(_) => {
                // On a hypothetical Android host with QAIRT installed,
                // this could succeed; the test isn't checking the
                // negative case there.
                #[cfg(not(target_os = "android"))]
                panic!("RealGenieLibrary::open unexpectedly succeeded on non-Android host");
            }
            Err(e) => {
                #[cfg(not(target_os = "android"))]
                assert!(
                    matches!(e, GenieCallError::PlatformUnsupported(_)),
                    "expected PlatformUnsupported on host, got {e:?}",
                );
                // On Android with no QAIRT installed, the error variant
                // is LibraryLoad — also acceptable shape.
                #[cfg(target_os = "android")]
                assert!(matches!(e, GenieCallError::LibraryLoad(_)), "{e:?}");
            }
        }
    }

    #[test]
    fn genie_call_error_into_inference_error_lands_in_other() {
        // Step 1.2.2: all variants collapse into `Other(dev_string)`.
        // Pinning the contract so a future i18n change is a deliberate
        // edit, not an accidental drop of the developer message.
        let err = GenieCallError::NonSuccess {
            operation: "GenieDialog_query",
            status: 7,
        };
        let inf = err.into_inference_error();
        match inf {
            primer_core::error::InferenceError::Other(msg) => {
                assert!(msg.contains("GenieDialog_query"), "{msg}");
                assert!(msg.contains("7"), "{msg}");
            }
            other => panic!("expected Other, got {other:?}"),
        }
    }

    #[test]
    fn mock_library_records_open_and_query_in_order() {
        use mock::{MockEvent, MockGenieLibrary};
        let lib = MockGenieLibrary::new_with_response("hello from the mock model");
        let dialog = lib
            .open_dialog(Path::new("/some/genie_config.json"))
            .unwrap();
        let response = dialog.query_blocking("Why is the sky blue?").unwrap();
        drop(dialog);
        assert_eq!(response, "hello from the mock model");
        let events = lib.events();
        assert_eq!(events.len(), 3);
        assert!(matches!(
            &events[0],
            MockEvent::OpenDialog { config_path } if config_path.to_str() == Some("/some/genie_config.json")
        ));
        assert!(
            matches!(&events[1], MockEvent::Query { prompt } if prompt == "Why is the sky blue?")
        );
        assert!(matches!(&events[2], MockEvent::DropDialog));
    }

    #[test]
    fn mock_library_propagates_open_error() {
        use mock::MockGenieLibrary;
        let lib = MockGenieLibrary::new_failing_open(GenieCallError::ConfigNotFound {
            path: "/missing".into(),
        });
        let result = lib.open_dialog(Path::new("/missing"));
        assert!(matches!(result, Err(GenieCallError::ConfigNotFound { .. })));
    }
}
