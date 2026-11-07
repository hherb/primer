//! Optional Genie file logging for [`open_dialog`](crate::qnn::genie::GenieLibrary::open_dialog):
//! create a `GenieLog`, bind it to the dialog config, and hold it in a
//! [`LogHandleGuard`] until the owning dialog takes it over.

use std::ptr;
use std::sync::Arc;

use primer_qnn_sys::{
    GENIE_STATUS_SUCCESS, GenieDialogConfig_Handle_t, GenieLibrary as RawGenieLibrary,
    GenieLog_Handle_t, GenieLog_free_fn,
};

/// Environment variable naming a file to which Genie's logging callback is
/// routed. When set (typically by `primer-gui` on Android to
/// `<app_data>/.primer/genie.log`), [`open_dialog`](crate::qnn::genie::GenieLibrary::open_dialog)
/// creates a Genie logger, binds it to the dialog config, and routes the
/// DSP-init diagnostics to that file — the only way to read the cause
/// behind a generic `GenieDialog_create` -1 on a ROM with a dead `logcat`.
/// Unset ⇒ logging disabled (the desktop/byte-identical default).
const GENIE_LOG_PATH_ENV: &str = "PRIMER_GENIE_LOG_PATH";

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
/// [`crate::qnn::genie::log::write_diagnostic`] — on the target ROM `logcat` is dead,
/// so the file is the only channel where the developer can read why
/// logging didn't engage (otherwise they'd see a marker-only `genie.log`
/// with no explanation).
pub(super) fn setup_logger(
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

    if let Err(e) = crate::qnn::genie::log::set_genie_log_file(std::path::Path::new(&log_path)) {
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
    let log_level = crate::qnn::genie::log::genie_log_level_from_env();
    crate::qnn::genie::log::set_genie_log_threshold(log_level);

    let mut log_handle: GenieLog_Handle_t = ptr::null();
    // SAFETY: config handle is NULL (required by the 2.45 API), the
    // callback is a valid `unsafe extern "C"` matching `GenieLog_Callback_t`,
    // and `out` is a stack local initialised to null. On non-success the
    // out-pointer is left null, which we detect below.
    let status = unsafe {
        log_create(
            ptr::null(),
            crate::qnn::genie::log::genie_log_callback,
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
        crate::qnn::genie::log::write_diagnostic(&format!(
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
        crate::qnn::genie::log::write_diagnostic(&format!(
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

/// Stack-scoped guard that frees a `GenieLog` handle on drop, unless
/// explicitly defused into the owning [`super::dialog::RealGenieDialog`]. Keeps
/// [`setup_logger`] / [`open_dialog`](crate::qnn::genie::GenieLibrary::open_dialog) leak-free in the
/// gap between binding the logger and `dialog_create` completing.
pub(super) struct LogHandleGuard {
    handle: GenieLog_Handle_t,
    free: GenieLog_free_fn,
    defused: bool,
}

impl LogHandleGuard {
    /// Transfer ownership of the log handle (+ its free fn) out of the
    /// guard, so it lands in the owning dialog instead of being freed here.
    pub(super) fn defuse(mut self) -> (GenieLog_Handle_t, GenieLog_free_fn) {
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
