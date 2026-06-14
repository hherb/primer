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
    GenieDialog_Handle_t, GenieDialog_QueryCallback_t, GenieDialogConfig_Handle_t,
    GenieLibrary as RawGenieLibrary, GenieLibraryError, GenieLog_Handle_t, GenieLog_free_fn,
};

use super::config::absolutize_genie_config;
use super::{GenieCallError, GenieDialog, GenieLibrary, QueryOutcome, classify_query_status};

/// Environment variable naming a file to which Genie's logging callback is
/// routed. When set (typically by `primer-gui` on Android to
/// `<app_data>/.primer/genie.log`), [`RealGenieLibrary::open_dialog`]
/// creates a Genie logger, binds it to the dialog config, and routes the
/// DSP-init diagnostics to that file — the only way to read the cause
/// behind a generic `GenieDialog_create` -1 on a ROM with a dead `logcat`.
/// Unset ⇒ logging disabled (the desktop/byte-identical default).
const GENIE_LOG_PATH_ENV: &str = "PRIMER_GENIE_LOG_PATH";

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
        // QAIRT 2.45: `GenieDialogConfig_createFromJson` consumes the JSON
        // *content* string, NOT a path (confirmed against the 2.45 header
        // and chatapp_android's `LoadModelConfig`). Read the bundle's
        // `genie_config.json` and rewrite its relative ctx-bins /
        // tokenizer / htp-extensions paths to absolute (resolved against
        // the bundle directory) so Genie locates them regardless of the
        // process working directory. Passing the bare path here — as the
        // pre-2.45 scaffold did — makes Genie try to parse the *path
        // string* as JSON and fail with INVALID_CONFIG.
        let bundle_dir = config_path.parent().unwrap_or_else(|| Path::new("."));
        let raw_json =
            std::fs::read_to_string(config_path).map_err(|e| GenieCallError::BadConfigPath {
                detail: format!("failed to read {}: {e}", config_path.display()),
            })?;
        let config_json = absolutize_genie_config(&raw_json, bundle_dir)
            .map_err(|detail| GenieCallError::BadConfigPath { detail })?;
        let c_config = CString::new(config_json).map_err(|e| GenieCallError::BadConfigPath {
            detail: format!(
                "embedded NUL byte in genie config at position {}",
                e.nul_position()
            ),
        })?;

        // SAFETY: We pass a valid C-string pointer (the rewritten JSON
        // content) and an out-pointer to a stack local. The Genie API
        // contract is that on `GENIE_STATUS_SUCCESS`, `out_handle` is
        // written with a non-null `GenieDialogConfig` pointer; on
        // failure, it is left untouched (we initialise to `null_mut()`
        // so a buggy backend that ignores the status code still produces
        // a benign null we can detect). The `c_config.as_ptr()` lifetime
        // is bounded by `c_config`, which lives until the end of this
        // block — well after `config_create_from_json` returns.
        let mut cfg_handle: GenieDialogConfig_Handle_t = ptr::null_mut();
        let status =
            unsafe { (self.raw.config_create_from_json)(c_config.as_ptr(), &mut cfg_handle) };
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

        // Best-effort: if `PRIMER_GENIE_LOG_PATH` is set and the loaded
        // libGenie exports the logging API, create a logger and bind it to
        // the config so the DSP-init diagnostics behind a `GenieDialog_create`
        // -1 are written to a file. Never fails dialog creation — a logging
        // setup error just leaves diagnostics off.
        let log_guard = setup_logger(&self.raw, cfg_handle);

        let mut dialog_handle: GenieDialog_Handle_t = ptr::null_mut();
        // SAFETY: `cfg_handle` is non-null and valid (just produced
        // by a successful `config_create_from_json`). The out-pointer
        // is a stack local initialised to null.
        let status = unsafe { (self.raw.dialog_create)(cfg_handle, &mut dialog_handle) };
        if status != GENIE_STATUS_SUCCESS || dialog_handle.is_null() {
            // `cfg_guard` and `log_guard` are dropped here, freeing the
            // config and (if any) the log handle.
            return Err(GenieCallError::NonSuccess {
                operation: "GenieDialog_create",
                status,
            });
        }

        // Construction succeeded — transfer the handles into the owning
        // `RealGenieDialog`. Defuse the guards so they don't double-free.
        let cfg = cfg_guard.defuse();
        let logger = log_guard.map(LogHandleGuard::defuse);
        Ok(Box::new(RealGenieDialog {
            lib: Arc::clone(&self.raw),
            dialog: dialog_handle,
            config: cfg,
            logger,
        }))
    }
}

/// Create a Genie logger and bind it to `cfg_handle`, returning a guard
/// that frees the log handle on drop unless defused into the owning
/// dialog. Returns `None` (logging disabled) when any of these hold:
///
/// - `PRIMER_GENIE_LOG_PATH` is unset (the default — no diagnostics file);
/// - the loaded `libGenie.so` does not export the optional logging symbols;
/// - opening the log file, creating the logger, or binding it fails.
///
/// All failure modes are non-fatal (a `tracing::warn!` and `None`): the
/// dialog is still created without logging, exactly as before this path
/// existed. Failures that happen *after* the log file is open (logger
/// create / bind) are *also* written into the log file via
/// [`super::log::write_diagnostic`] — on the target ROM `logcat` is dead,
/// so the file is the only channel where the developer can read why
/// logging didn't engage (otherwise they'd see a marker-only `genie.log`
/// with no explanation).
fn setup_logger(
    lib: &Arc<RawGenieLibrary>,
    cfg_handle: GenieDialogConfig_Handle_t,
) -> Option<LogHandleGuard> {
    let log_path = std::env::var_os(GENIE_LOG_PATH_ENV)?;

    // Optional logging symbols must all be present to do anything useful.
    let (log_create, bind_logger, log_free) =
        match (lib.log_create, lib.config_bind_logger, lib.log_free) {
            (Some(c), Some(b), Some(f)) => (c, b, f),
            _ => {
                tracing::warn!(
                    target: "primer::qnn",
                    "PRIMER_GENIE_LOG_PATH is set but libGenie.so does not export the \
                     logging API (GenieLog_create / GenieDialogConfig_bindLogger / \
                     GenieLog_free); Genie file logging disabled"
                );
                return None;
            }
        };

    if let Err(e) = super::log::set_genie_log_file(std::path::Path::new(&log_path)) {
        tracing::warn!(
            target: "primer::qnn",
            "failed to open Genie log file {:?}: {e}; Genie file logging disabled",
            log_path,
        );
        return None;
    }

    // Resolve verbosity from PRIMER_GENIE_LOG_LEVEL (default WARN); both hand
    // it to Genie *and* install it as the callback-side threshold so the
    // per-token firehose is capped regardless of how Genie honours the arg.
    let log_level = super::log::genie_log_level_from_env();
    super::log::set_genie_log_threshold(log_level);

    let mut log_handle: GenieLog_Handle_t = ptr::null();
    // SAFETY: config handle is NULL (required by the 2.45 API), the
    // callback is a valid `unsafe extern "C"` matching `GenieLog_Callback_t`,
    // and `out` is a stack local initialised to null. On non-success the
    // out-pointer is left null, which we detect below.
    let status = unsafe {
        log_create(
            ptr::null(),
            super::log::genie_log_callback,
            log_level,
            &mut log_handle,
        )
    };
    if status != GENIE_STATUS_SUCCESS || log_handle.is_null() {
        tracing::warn!(
            target: "primer::qnn",
            status,
            "GenieLog_create failed; Genie file logging disabled",
        );
        // The file is open but no logger is bound; record why here so the
        // marker-only file isn't a silent dead end on a dead-logcat ROM.
        super::log::write_diagnostic(&format!(
            "GenieLog_create failed (status {status}); Genie file logging disabled"
        ));
        return None;
    }

    // SAFETY: `cfg_handle` and `log_handle` are both non-null, valid
    // handles just produced by successful create calls.
    let bind_status = unsafe { bind_logger(cfg_handle, log_handle) };
    if bind_status != GENIE_STATUS_SUCCESS {
        tracing::warn!(
            target: "primer::qnn",
            status = bind_status,
            "GenieDialogConfig_bindLogger failed; freeing the log handle",
        );
        // The file is open but the logger never bound, so the Genie
        // callback will never fire; record why here so the developer
        // reading `genie.log` (logcat is dead on the target ROM) knows the
        // diagnostic itself failed rather than the DSP init.
        super::log::write_diagnostic(&format!(
            "GenieDialogConfig_bindLogger failed (status {bind_status}); \
             Genie file logging disabled"
        ));
        // SAFETY: `log_handle` was just created and not bound; free it.
        unsafe {
            let _ = log_free(log_handle);
        }
        return None;
    }

    tracing::info!(
        target: "primer::qnn",
        "Genie logger bound; DSP-init diagnostics routed to {:?}",
        log_path,
    );
    Some(LogHandleGuard {
        handle: log_handle,
        free: log_free,
        defused: false,
    })
}

/// Owning handle to a live dialog. Drop frees the dialog, then the config,
/// then (if logging was enabled) the bound log handle — consumers before
/// the resources they reference.
struct RealGenieDialog {
    lib: Arc<RawGenieLibrary>,
    dialog: GenieDialog_Handle_t,
    config: GenieDialogConfig_Handle_t,
    /// The bound Genie log handle + its free fn, present only when
    /// `PRIMER_GENIE_LOG_PATH` was set and the logger was created and
    /// bound. Freed last in `Drop` (the config and dialog reference it).
    logger: Option<(GenieLog_Handle_t, GenieLog_free_fn)>,
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

        match classify_query_status(query_status) {
            QueryOutcome::Complete => {
                let _ = sender.unbounded_send(Ok(TokenChunk {
                    text: String::new(),
                    done: true,
                }));
            }
            QueryOutcome::ContextLimit => {
                // The context window filled mid-generation. The reply has
                // already streamed in full via the callback above, so we
                // close the turn gracefully (emit the done chunk) with what
                // was generated rather than dropping it. Logged so the
                // prompt budget can be tuned against it (the prompt was too
                // long to leave reply room — see the small-context budget
                // in `primer-pedagogy`).
                tracing::warn!(
                    target: "primer::qnn",
                    "GenieDialog_query hit the context limit (status {query_status}); completing the turn with the streamed reply"
                );
                let _ = sender.unbounded_send(Ok(TokenChunk {
                    text: String::new(),
                    done: true,
                }));
            }
            QueryOutcome::Error => {
                let err = GenieCallError::NonSuccess {
                    operation: "GenieDialog_query",
                    status: query_status,
                };
                let _ = sender.unbounded_send(Err(err.to_primer_error()));
                // No done chunk after a genuine error — see the trait
                // documentation for the rationale.
            }
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

/// Stack-scoped guard that frees a `GenieLog` handle on drop, unless
/// explicitly defused into the owning [`RealGenieDialog`]. Keeps
/// [`setup_logger`] / [`RealGenieLibrary::open_dialog`] leak-free in the
/// gap between binding the logger and `dialog_create` completing.
struct LogHandleGuard {
    handle: GenieLog_Handle_t,
    free: GenieLog_free_fn,
    defused: bool,
}

impl LogHandleGuard {
    /// Transfer ownership of the log handle (+ its free fn) out of the
    /// guard, so it lands in the owning dialog instead of being freed here.
    fn defuse(mut self) -> (GenieLog_Handle_t, GenieLog_free_fn) {
        self.defused = true;
        (self.handle, self.free)
    }
}

impl Drop for LogHandleGuard {
    fn drop(&mut self) {
        if !self.defused {
            // SAFETY: `self.handle` was produced by a successful
            // `GenieLog_create` and has not been freed yet; the defused
            // branch transfers ownership instead.
            unsafe {
                let _ = (self.free)(self.handle);
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
