//! Real `libGenie.so`-backed implementation of [`super::GenieLibrary`].
//!
//! Built on every target because [`primer_qnn_sys::GenieLibrary::open`]
//! already handles the `target_os = "android"` gate by returning
//! [`primer_qnn_sys::GenieLibraryError::PlatformUnsupported`] on
//! non-Android. The mapping to [`super::GenieCallError`] happens here so
//! the parent module's trait surface stays platform-neutral.

use std::ffi::{CString, c_void};
use std::path::Path;
use std::ptr;
use std::sync::Arc;

use futures::channel::mpsc::UnboundedSender;
use primer_core::error::Result as PrimerResult;
use primer_core::inference::TokenChunk;
use primer_qnn_sys::{
    GENIE_DIALOG_SENTENCE_COMPLETE, GENIE_STATUS_SUCCESS, Genie_Dialog_SentenceCode_t,
    GenieDialog_Handle_t, GenieDialog_TokenCallback_t, GenieDialogConfig_Handle_t,
    GenieLibrary as RawGenieLibrary, GenieLibraryError,
};

use super::{GenieCallError, GenieDialog, GenieLibrary};

/// Real `libGenie.so` wrapper.
///
/// Wraps the raw dlopen handle from [`primer_qnn_sys`] and implements
/// the [`GenieLibrary`] trait by routing every call through the
/// resolved function pointers. The struct holds an `Arc` so dialogs
/// can outlive the borrow when needed by the safe wrapper.
pub struct RealGenieLibrary {
    raw: Arc<RawGenieLibrary>,
}

impl RealGenieLibrary {
    /// Open `libGenie.so` from the given QAIRT lib directory.
    ///
    /// Returns [`GenieCallError::PlatformUnsupported`] on non-Android
    /// (delegated to [`primer_qnn_sys::GenieLibrary::open`]), so this
    /// constructor can be called on every host without `#[cfg]`
    /// gating at the call site.
    pub fn open(qairt_lib_dir: &Path) -> Result<Arc<Self>, GenieCallError> {
        let raw = RawGenieLibrary::open(qairt_lib_dir).map_err(map_library_error)?;
        Ok(Arc::new(Self { raw: Arc::new(raw) }))
    }
}

fn map_library_error(err: GenieLibraryError) -> GenieCallError {
    match err {
        GenieLibraryError::PlatformUnsupported(os) => GenieCallError::PlatformUnsupported(os),
        GenieLibraryError::DlopenFailed { dir, source } => GenieCallError::LibraryLoad(format!(
            "failed to dlopen libGenie.so from {}: {source}",
            dir.display()
        )),
        GenieLibraryError::SymbolMissing { symbol, source } => GenieCallError::SymbolMissing {
            symbol: symbol.to_string(),
            detail: source.to_string(),
        },
    }
}

impl GenieLibrary for RealGenieLibrary {
    fn open_dialog(&self, config_path: &Path) -> Result<Box<dyn GenieDialog>, GenieCallError> {
        // Genie consumes a NUL-terminated C string for the path. We
        // do not validate file existence here — Genie will surface a
        // non-success status if the file is missing — but a path
        // carrying an embedded NUL is caught at this layer because
        // `CString::new` would refuse it.
        let path_bytes = config_path
            .to_str()
            .ok_or_else(|| GenieCallError::BadConfigPath {
                detail: format!("{} is not valid UTF-8", config_path.display()),
            })?;
        let c_path = CString::new(path_bytes).map_err(|e| GenieCallError::BadConfigPath {
            detail: format!("embedded NUL byte at position {}", e.nul_position()),
        })?;

        // SAFETY: We pass a valid C-string pointer and an out-pointer
        // to a stack local. The Genie API contract is that on
        // `GENIE_STATUS_SUCCESS`, `out_handle` is written with a
        // non-null `GenieDialogConfig` pointer; on failure, it is
        // left untouched (we initialise to `null_mut()` so a buggy
        // backend that ignores the status code still produces a
        // benign null we can detect). The `*c_path.as_ptr()`
        // lifetime is bounded by `c_path` which lives until the end
        // of this block — well after `config_create_from_json`
        // returns.
        let mut cfg_handle: GenieDialogConfig_Handle_t = ptr::null_mut();
        let status =
            unsafe { (self.raw.config_create_from_json)(c_path.as_ptr(), &mut cfg_handle) };
        if status != GENIE_STATUS_SUCCESS || cfg_handle.is_null() {
            return Err(GenieCallError::NonSuccess {
                operation: "GenieDialogConfig_createFromJson",
                status,
            });
        }

        // Even if `dialog_create` fails, we need to free the config
        // handle before propagating — otherwise we leak it. Track
        // the config handle in a guard that frees it on drop unless
        // explicitly defused.
        let cfg_guard = ConfigHandleGuard::new(cfg_handle, &self.raw);

        let mut dialog_handle: GenieDialog_Handle_t = ptr::null_mut();
        // SAFETY: `cfg_handle` is non-null and valid (just produced
        // by a successful `config_create_from_json`). The out-pointer
        // is a stack local initialised to null.
        let status = unsafe { (self.raw.dialog_create)(cfg_handle, &mut dialog_handle) };
        if status != GENIE_STATUS_SUCCESS || dialog_handle.is_null() {
            // `cfg_guard` is dropped here, freeing the config handle.
            return Err(GenieCallError::NonSuccess {
                operation: "GenieDialog_create",
                status,
            });
        }

        // Construction succeeded — transfer both handles into the
        // owning `RealGenieDialog`. Defuse the guard so it doesn't
        // double-free the config.
        let cfg = cfg_guard.defuse();
        Ok(Box::new(RealGenieDialog {
            lib: Arc::clone(&self.raw),
            dialog: dialog_handle,
            config: cfg,
        }))
    }
}

/// Owning handle to a live dialog. Drop calls `GenieDialog_free` +
/// `GenieDialogConfig_free` in that order.
struct RealGenieDialog {
    lib: Arc<RawGenieLibrary>,
    dialog: GenieDialog_Handle_t,
    config: GenieDialogConfig_Handle_t,
}

// SAFETY: The raw handles are opaque pointers into Genie's internal
// state. Genie's documented contract is that a dialog handle is owned
// by exactly one thread at a time but ownership can be transferred
// between threads — the safe wrapper enforces single-owner via
// `Box<dyn GenieDialog>` and serialises calls via the
// `tokio::sync::Mutex` in [`super::super::backend::QnnBackend`]. The
// `Arc<RawGenieLibrary>` is `Send + Sync` (the underlying
// `libloading::Library` is `Send + Sync` and the resolved function
// pointers are `Copy`).
//
// `Sync` is deliberately NOT impl'd: Genie forbids concurrent queries
// against the same dialog handle, and the safe wrapper relies on
// `tokio::sync::Mutex` to serialise access. `tokio::sync::Mutex<T>` is
// already `Send + Sync` when `T: Send` — so the backend struct stays
// `Send + Sync` without us claiming a `Sync` here we can't actually
// honour.
unsafe impl Send for RealGenieDialog {}

impl GenieDialog for RealGenieDialog {
    fn query_streaming(&self, prompt: &str, sender: UnboundedSender<PrimerResult<TokenChunk>>) {
        // Genie's query takes a NUL-terminated UTF-8 prompt. An
        // embedded NUL in the prompt is operator error (the Primer's
        // prompt builder never emits NULs); the streaming contract
        // requires us to surface this through the channel, not return
        // it.
        let c_prompt = match CString::new(prompt) {
            Ok(c) => c,
            Err(e) => {
                let err = GenieCallError::BadConfigPath {
                    detail: format!("embedded NUL in prompt at position {}", e.nul_position()),
                };
                let _ = sender.unbounded_send(Err(err.to_primer_error()));
                return;
            }
        };

        // Box the sender so we have a stable address for the
        // `user_data` pointer. Plan §1.2.3 task 1: `Box::into_raw` the
        // sender, recover via `Box::from_raw` after `dialog_query`
        // returns so the box is dropped exactly once (closing the
        // channel).
        let boxed_sender: Box<UnboundedSender<PrimerResult<TokenChunk>>> = Box::new(sender);
        let raw_sender_ptr = Box::into_raw(boxed_sender);
        let user_data = raw_sender_ptr as *mut c_void;

        // SAFETY: `streaming_token_callback` is `unsafe extern "C"`
        // and we register it on every query, so the most recently
        // installed `user_data` is always the live
        // `Box<UnboundedSender<...>>` for the in-flight query. The C
        // ABI signature matches `GenieDialog_TokenCallback_t`. The
        // box address is stable across the call because we hold the
        // raw pointer and don't touch the box until after
        // `dialog_query` returns.
        let callback: GenieDialog_TokenCallback_t = streaming_token_callback;
        let set_status =
            unsafe { (self.lib.dialog_set_token_callback)(self.dialog, callback, user_data) };
        if set_status != GENIE_STATUS_SUCCESS {
            // No callback ever fires — reclaim the box, send a single
            // error chunk through the now-reclaimed sender, drop.
            // SAFETY: `raw_sender_ptr` came from `Box::into_raw` above
            // and no callback has fired, so nothing else holds the
            // pointer.
            let sender = unsafe { reclaim_sender_box(raw_sender_ptr) };
            let err = GenieCallError::NonSuccess {
                operation: "GenieDialog_setTokenCallback",
                status: set_status,
            };
            let _ = sender.unbounded_send(Err(err.to_primer_error()));
            return;
        }

        // Run the (blocking) query. The C-ABI callback fires N times
        // during this call, each time forwarding a body chunk into
        // the sender. Genie's documented contract is that callbacks
        // are dispatched synchronously inside `dialog_query` and not
        // retained past its return — that's the load-bearing
        // assumption that lets us reclaim the box below.
        //
        // SAFETY: `self.dialog` is a non-null pointer produced by a
        // successful `GenieDialog_create` and not yet freed. The
        // prompt pointer is bounded by `c_prompt`. We pass
        // `null_mut()` for `response_out` because all output flows
        // through the callback.
        let query_status = unsafe {
            (self.lib.dialog_query)(
                self.dialog,
                c_prompt.as_ptr(),
                GENIE_DIALOG_SENTENCE_COMPLETE,
                ptr::null_mut(),
            )
        };

        // Reclaim the sender so we can emit the final chunk and close
        // the channel by dropping the box.
        // SAFETY: `dialog_query` has returned, so per the contract
        // above no further callbacks will fire and we are the sole
        // owner of `raw_sender_ptr`.
        let sender = unsafe { reclaim_sender_box(raw_sender_ptr) };

        if query_status != GENIE_STATUS_SUCCESS {
            let err = GenieCallError::NonSuccess {
                operation: "GenieDialog_query",
                status: query_status,
            };
            let _ = sender.unbounded_send(Err(err.to_primer_error()));
            // No done chunk after an error — see the trait
            // documentation for the rationale.
        } else {
            let _ = sender.unbounded_send(Ok(TokenChunk {
                text: String::new(),
                done: true,
            }));
        }
        // sender drops here, closing the channel.
    }
}

/// Reclaim ownership of the boxed sender from a raw pointer.
///
/// # Safety
///
/// `ptr` must have been produced by `Box::into_raw` and not yet
/// reclaimed. After this call the caller owns the box; the raw pointer
/// must not be used again.
unsafe fn reclaim_sender_box(
    ptr: *mut UnboundedSender<PrimerResult<TokenChunk>>,
) -> Box<UnboundedSender<PrimerResult<TokenChunk>>> {
    // SAFETY: caller upholds the invariant documented above.
    unsafe { Box::from_raw(ptr) }
}

impl Drop for RealGenieDialog {
    fn drop(&mut self) {
        // Drop order: dialog before config (config is the parent
        // resource; Genie's lifetime contract is config_create →
        // dialog_create → dialog_free → config_free). Status codes
        // from these frees are observed via `tracing::warn!` only —
        // there's no recovery path in `Drop`.
        // SAFETY: `self.dialog` and `self.config` were produced by
        // successful Genie create calls and have not been freed yet
        // (Drop is reached exactly once per instance).
        unsafe {
            let dialog_status = (self.lib.dialog_free)(self.dialog);
            if dialog_status != GENIE_STATUS_SUCCESS {
                tracing::warn!(
                    target: "primer::qnn",
                    status = dialog_status,
                    "GenieDialog_free returned non-success status during Drop",
                );
            }
            let config_status = (self.lib.config_free)(self.config);
            if config_status != GENIE_STATUS_SUCCESS {
                tracing::warn!(
                    target: "primer::qnn",
                    status = config_status,
                    "GenieDialogConfig_free returned non-success status during Drop",
                );
            }
        }
    }
}

/// Stack-scoped guard that frees a `GenieDialogConfig` handle on drop,
/// unless explicitly defused. Keeps [`RealGenieLibrary::open_dialog`]
/// panic-safe in the gap between `config_create_from_json` succeeding
/// and `dialog_create` completing.
struct ConfigHandleGuard<'lib> {
    handle: GenieDialogConfig_Handle_t,
    lib: &'lib RawGenieLibrary,
    defused: bool,
}

impl<'lib> ConfigHandleGuard<'lib> {
    fn new(handle: GenieDialogConfig_Handle_t, lib: &'lib RawGenieLibrary) -> Self {
        Self {
            handle,
            lib,
            defused: false,
        }
    }

    fn defuse(mut self) -> GenieDialogConfig_Handle_t {
        self.defused = true;
        self.handle
    }
}

impl<'lib> Drop for ConfigHandleGuard<'lib> {
    fn drop(&mut self) {
        if !self.defused {
            // SAFETY: `self.handle` was produced by a successful
            // `config_create_from_json` call and has not been freed
            // yet. The defused branch transfers ownership instead.
            unsafe {
                let _ = (self.lib.config_free)(self.handle);
            }
        }
    }
}

/// C-ABI callback invoked once per token (or token chunk) during a
/// `GenieDialog_query`. Forwards the bytes pointed to by `response_str`
/// into the boxed [`UnboundedSender`] whose address is in `user_data`.
///
/// # Safety
///
/// - `response_str` must point to a NUL-terminated byte sequence the
///   callee reads only between callback entry and return.
/// - `user_data` must be a non-null pointer to a
///   `Box<UnboundedSender<PrimerResult<TokenChunk>>>` that outlives
///   every callback firing for this query (i.e. spans the
///   `dialog_query` invocation).
///
/// Both invariants are upheld by the call-site in
/// [`RealGenieDialog::query_streaming`]. The callback only borrows the
/// sender — the box is not reconstructed here. That keeps the lifetime
/// bookkeeping linear: exactly one `Box::into_raw` at query start,
/// exactly one `Box::from_raw` at query end, an unbounded number of
/// non-consuming borrows in between.
///
/// Send-failure on the channel is ignored: per the trait contract,
/// consumers may drop the receiver mid-stream (cancellation) and
/// Genie has no in-flight-cancellation API, so we let `dialog_query`
/// run to completion regardless and discard the unsent tokens.
unsafe extern "C" fn streaming_token_callback(
    response_str: *const std::ffi::c_char,
    _sentence_code: Genie_Dialog_SentenceCode_t,
    user_data: *mut c_void,
) {
    if response_str.is_null() || user_data.is_null() {
        return;
    }
    // SAFETY: `response_str` is non-null and the caller guarantees it
    // points at a NUL-terminated UTF-8 token. `to_string_lossy`
    // tolerates non-UTF-8 by substituting `U+FFFD` — never panic in
    // an `extern "C"` callback, which would unwind across an FFI
    // boundary (UB on most platforms).
    let token = unsafe { std::ffi::CStr::from_ptr(response_str) }.to_string_lossy();
    // SAFETY: `user_data` was constructed by `Box::into_raw` on a
    // `Box<UnboundedSender<PrimerResult<TokenChunk>>>` and the box is
    // still alive (the call-site reclaims it only after
    // `dialog_query` returns). Borrowing the sender via `&*` does
    // not consume the box.
    let sender = unsafe { &*(user_data as *const UnboundedSender<PrimerResult<TokenChunk>>) };
    let _ = sender.unbounded_send(Ok(TokenChunk {
        text: token.into_owned(),
        done: false,
    }));
}
