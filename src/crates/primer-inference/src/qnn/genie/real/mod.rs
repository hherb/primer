//! Real `libGenie.so`-backed implementation of [`super::GenieLibrary`].
//!
//! Built on every target because [`primer_qnn_sys::GenieLibrary::open`]
//! already handles the `target_os = "android"` gate by returning
//! [`primer_qnn_sys::GenieLibraryError::PlatformUnsupported`] on
//! non-Android. The mapping to [`super::GenieCallError`] happens here so
//! the parent module's trait surface stays platform-neutral.
//!
//! Split by responsibility: this file owns the library handle and
//! dialog construction ([`RealGenieLibrary::open_dialog`]) plus the
//! config-handle guard; [`logger`] the optional file-logging setup;
//! [`dialog`] the owning dialog handle and the C-ABI token callback.

use std::ffi::CString;
use std::path::Path;
use std::ptr;
use std::sync::Arc;

use primer_qnn_sys::{
    GENIE_STATUS_SUCCESS, GenieDialog_Handle_t, GenieDialogConfig_Handle_t,
    GenieLibrary as RawGenieLibrary, GenieLibraryError,
};

use super::config::absolutize_genie_config;
use super::{GenieCallError, GenieDialog, GenieLibrary};

mod dialog;
mod logger;

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
        let log_guard = logger::setup_logger(&self.raw, cfg_handle);

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
        let logger = log_guard.map(logger::LogHandleGuard::defuse);
        Ok(Box::new(dialog::RealGenieDialog {
            lib: Arc::clone(&self.raw),
            dialog: dialog_handle,
            config: cfg,
            logger,
        }))
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
