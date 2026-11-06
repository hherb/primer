//! The owning [`RealGenieDialog`] handle (query, reset, ordered free on
//! drop) and the C-ABI [`streaming_token_callback`] that forwards each
//! Genie token into the query's channel.

use std::ffi::{CString, c_void};
use std::sync::Arc;

use futures::channel::mpsc::UnboundedSender;
use primer_core::error::Result as PrimerResult;
use primer_core::inference::TokenChunk;
use primer_qnn_sys::{
    GENIE_DIALOG_SENTENCE_COMPLETE, GENIE_STATUS_SUCCESS, Genie_Dialog_SentenceCode_t,
    GenieDialog_Handle_t, GenieDialog_QueryCallback_t, GenieDialogConfig_Handle_t,
    GenieLibrary as RawGenieLibrary, GenieLog_Handle_t, GenieLog_free_fn,
};

use crate::qnn::genie::{GenieCallError, GenieDialog, classify_query_status, emit_query_outcome};

/// Owning handle to a live dialog. Drop frees the dialog, then the config,
/// then (if logging was enabled) the bound log handle — consumers before
/// the resources they reference.
pub(super) struct RealGenieDialog {
    pub(super) lib: Arc<RawGenieLibrary>,
    pub(super) dialog: GenieDialog_Handle_t,
    pub(super) config: GenieDialogConfig_Handle_t,
    /// The bound Genie log handle + its free fn, present only when
    /// `PRIMER_GENIE_LOG_PATH` was set and the logger was created and
    /// bound. Freed last in `Drop` (the config and dialog reference it).
    pub(super) logger: Option<(GenieLog_Handle_t, GenieLog_free_fn)>,
}

// SAFETY: The raw handles are opaque pointers into Genie's internal
// state. Genie's documented contract is that a dialog handle is owned
// by exactly one thread at a time but ownership can be transferred
// between threads — the safe wrapper enforces single-owner via
// `Box<dyn GenieDialog>` and serialises calls via the
// `tokio::sync::Mutex` in [`crate::qnn::backend::QnnBackend`]. The
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
    fn reset(&self) -> PrimerResult<()> {
        // SAFETY: `self.dialog` is a non-null handle produced by a
        // successful `GenieDialog_create` and not yet freed; `dialog_reset`
        // is the resolved `GenieDialog_reset` entry point. The call has no
        // pointer-aliasing concerns — it takes only the opaque handle.
        let status = unsafe { (self.lib.dialog_reset)(self.dialog) };
        if status != GENIE_STATUS_SUCCESS {
            return Err(GenieCallError::NonSuccess {
                operation: "GenieDialog_reset",
                status,
            }
            .to_primer_error());
        }
        Ok(())
    }

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

        // 2.45 API: the streaming token callback + its `user_data` are
        // passed *directly* to `GenieDialog_query` (there is no separate
        // `setTokenCallback` step — that symbol does not exist in QAIRT
        // 2.45's `libGenie.so`; this was confirmed by on-device symbol
        // resolution).
        //
        // SAFETY: `streaming_token_callback` is `unsafe extern "C"` and
        // its C ABI signature matches `GenieDialog_QueryCallback_t`. The
        // `user_data` we hand in is the live `Box<UnboundedSender<...>>`
        // for the in-flight query; its address is stable across the call
        // because we hold the raw pointer and don't touch the box until
        // after `dialog_query` returns. Genie's documented contract is
        // that callbacks are dispatched synchronously inside
        // `dialog_query` and not retained past its return — that's the
        // load-bearing assumption that lets us reclaim the box below.
        // `self.dialog` is a non-null pointer produced by a successful
        // `GenieDialog_create` and not yet freed; the prompt pointer is
        // bounded by `c_prompt`.
        let callback: GenieDialog_QueryCallback_t = streaming_token_callback;
        let query_status = unsafe {
            (self.lib.dialog_query)(
                self.dialog,
                c_prompt.as_ptr(),
                GENIE_DIALOG_SENTENCE_COMPLETE,
                callback,
                user_data,
            )
        };

        // Reclaim the sender so we can emit the final chunk and close
        // the channel by dropping the box.
        // SAFETY: `dialog_query` has returned, so per the contract
        // above no further callbacks will fire and we are the sole
        // owner of `raw_sender_ptr`.
        let sender = unsafe { reclaim_sender_box(raw_sender_ptr) };

        // Classify the return status and emit the terminal chunk. The
        // graceful-vs-error decision and its emission are both host-tested
        // pure helpers in the parent module — see `classify_query_status`
        // and `emit_query_outcome`.
        emit_query_outcome(classify_query_status(query_status), query_status, &sender);
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
            // Free the log handle last — the config and dialog both
            // referenced it via `bindLogger`, so it must outlive them.
            if let Some((handle, free)) = self.logger {
                let log_status = free(handle);
                if log_status != GENIE_STATUS_SUCCESS {
                    tracing::warn!(
                        target: "primer::qnn",
                        status = log_status,
                        "GenieLog_free returned non-success status during Drop",
                    );
                }
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
        ..Default::default()
    }));
}
