//! Low-level FFI bindings + runtime dlopen for the Qualcomm Genie SDK.
//!
//! This crate is the bottom layer of the Phase 1.2 Qualcomm NPU backend
//! (see `docs/superpowers/specs/2026-05-28-qnn-backend-design.md`). It
//! provides:
//!
//! - Raw `extern "C"` type aliases and symbol names matching the Genie
//!   C API (module [`bindings`]).
//! - [`GenieLibrary`], a thin `libloading::Library` wrapper that
//!   lazy-resolves the six Genie functions the Primer needs.
//!
//! ## Why dlopen instead of `#[link]`?
//!
//! QAIRT (and therefore `libGenie.so`) is closed-source Qualcomm
//! proprietary, distributed via the Qualcomm developer portal. Linking it
//! at compile time would either force every Primer build host to stage the
//! `.so` files in `$LIBRARY_PATH` (a brittle pre-build step) or force us
//! to redistribute the closed-source libraries from this repo (a licence
//! decision that is explicitly out of scope for v1 — see Phase 1.2 design
//! §5). Runtime dlopen via `libloading::Library::new` keeps the user
//! install step honest: a user who has never installed QAIRT never loads
//! the library.
//!
//! ## Cross-platform behaviour
//!
//! On `target_os = "android"`, [`GenieLibrary::open`] attempts the real
//! dlopen and returns the populated wrapper on success. On every other
//! target (macOS, Linux desktop, Windows) it returns
//! [`GenieLibraryError::PlatformUnsupported`] without touching the
//! filesystem — this is what keeps `cargo check` green from desktop
//! workstations and CI runners. The decision is deliberate: even if a
//! user manually copied `libGenie.so` to a Linux desktop, the Hexagon
//! DSP backend it needs would not be present, so a "successful" desktop
//! load would only fail later in a confusing way during model
//! construction.
//!
//! ## What this crate is NOT
//!
//! - It is not the safe wrapper. Construction of `GenieDialog` instances,
//!   the C-ABI token callback that bridges into a `futures` channel, the
//!   `tokio::sync::Mutex` that serialises concurrent calls, and the
//!   `Drop` impl that frees the dialog + config handles all live in
//!   `primer-inference::qnn` (Phase 1.2 design §3). This crate exposes
//!   the minimum surface the safe wrapper needs.
//! - It is not a Tauri-Android GUI shim. Phase 1.2 is CLI-only on
//!   Termux; GUI on Android is a Phase 3 enclosure concern.

pub mod bindings;

use std::path::{Path, PathBuf};

use libloading::Library;
use thiserror::Error;

pub use bindings::*;

/// Errors that [`GenieLibrary::open`] can return.
///
/// The variants are kept structured (not collapsed into a single
/// `String`-bearing `Other`) because the safe wrapper in
/// `primer-inference` will i18n the user-facing message at the
/// `primer_core::error::PrimerError` boundary, and routing on a typed
/// variant is much more robust than substring-matching a message.
#[derive(Debug, Error)]
pub enum GenieLibraryError {
    /// The current build target has no QAIRT runtime support.
    ///
    /// Returned on every non-Android target. The string is intentionally
    /// dev-facing only — the safe wrapper will substitute a generic
    /// "local model not available, falling back to cloud" message for the
    /// child.
    #[error(
        "QNN/Genie is only supported on Android (target_os=\"android\"); current target is {0}"
    )]
    PlatformUnsupported(&'static str),

    /// `libGenie.so` could not be dlopen'd from the supplied directory.
    ///
    /// The wrapped `libloading::Error` carries the OS-level reason
    /// (missing file, missing transitive dep, ABI mismatch, etc.).
    /// `dir` is the directory the caller asked us to look in; the actual
    /// filename probed is [`bindings::LIBGENIE_BASENAME`].
    #[error("failed to dlopen libGenie.so from {dir}: {source}")]
    DlopenFailed {
        dir: PathBuf,
        #[source]
        source: libloading::Error,
    },

    /// A required Genie symbol was missing from the loaded library.
    ///
    /// This means the dlopen succeeded but the resolved library is the
    /// wrong version or a stub — `libGenie.so` is present but
    /// `GenieDialog_create` (or one of the other five entry points) is
    /// not exported. Usually a QAIRT version skew between what the user
    /// installed and what the Primer expects.
    #[error("Genie symbol {symbol} could not be resolved from libGenie.so: {source}")]
    SymbolMissing {
        symbol: &'static str,
        #[source]
        source: libloading::Error,
    },
}

/// Per-process handle to the Qualcomm Genie shared library.
///
/// Constructed via [`GenieLibrary::open`]; cheap to clone because the
/// underlying `Library` is move-only and consumers are expected to wrap
/// this struct in an `Arc` (see Phase 1.2 design §3, `QnnBackend.lib:
/// Arc<GenieLibrary>`). The six function-pointer fields are resolved
/// eagerly at construction so the safe wrapper does not have to handle
/// "missing symbol" errors per-call.
///
/// Lifetime: when the last `Arc<GenieLibrary>` drops, `libloading`
/// unloads the library. Every `GenieDialog` and `GenieDialogConfig`
/// handle that this library produced MUST be freed before the library
/// is unloaded — that is the safe wrapper's responsibility, not this
/// crate's.
pub struct GenieLibrary {
    /// The underlying `libloading::Library`. Kept private so consumers
    /// cannot accidentally resolve unaudited symbols from it. The
    /// `_library` prefix marks it as field-owned-for-lifetime only —
    /// access goes through the typed function pointers below.
    _library: Library,

    /// `GenieDialogConfig_createFromJson` entry point.
    pub config_create_from_json: GenieDialogConfig_createFromJson_fn,
    /// `GenieDialogConfig_free` entry point.
    pub config_free: GenieDialogConfig_free_fn,
    /// `GenieDialog_create` entry point.
    pub dialog_create: GenieDialog_create_fn,
    /// `GenieDialog_query` entry point. In QAIRT 2.45 this call also
    /// registers the streaming token callback (see [`GenieDialog_query_fn`]);
    /// there is no separate `setTokenCallback` symbol.
    pub dialog_query: GenieDialog_query_fn,
    /// `GenieDialog_free` entry point.
    pub dialog_free: GenieDialog_free_fn,

    /// `GenieLog_create` entry point — **optional**. `None` when the loaded
    /// `libGenie.so` does not export the logging API. The Primer uses this
    /// (with [`Self::config_bind_logger`]) to route DSP-init diagnostics to
    /// a file on Android, where some ROMs have a dead `logcat`. Logging is
    /// best-effort: an absent symbol leaves the streaming path untouched.
    pub log_create: Option<GenieLog_create_fn>,
    /// `GenieLog_free` entry point — **optional** (paired with
    /// [`Self::log_create`]).
    pub log_free: Option<GenieLog_free_fn>,
    /// `GenieDialogConfig_bindLogger` entry point — **optional**. Binds a
    /// logger to a dialog config so the logger is inherited by dialogs
    /// created from it. `None` when the loaded library lacks the symbol.
    pub config_bind_logger: Option<GenieDialogConfig_bindLogger_fn>,
}

impl GenieLibrary {
    /// Open `libGenie.so` from `qairt_lib_dir` and resolve the five
    /// entry points the Primer needs (QAIRT 2.45 — `setTokenCallback` was
    /// removed; the token callback is now a parameter of
    /// `GenieDialog_query`).
    ///
    /// On `target_os = "android"`:
    /// - Probes `qairt_lib_dir.join(LIBGENIE_BASENAME)`.
    /// - On dlopen failure, returns [`GenieLibraryError::DlopenFailed`].
    /// - On symbol-resolution failure, returns
    ///   [`GenieLibraryError::SymbolMissing`].
    ///
    /// On every other target: returns
    /// [`GenieLibraryError::PlatformUnsupported`] without touching the
    /// filesystem.
    ///
    /// # Safety
    ///
    /// The unsafe blocks below are bounded to:
    /// 1. `Library::new` — `libloading` is sound when the path is valid
    ///    UTF-8 and the OS dynamic-loader contract is honoured. The
    ///    caller-supplied directory is treated as untrusted input but
    ///    the only attack surface is "user points the Primer at the
    ///    wrong dir and gets a DlopenFailed error".
    /// 2. `Library::get::<T>(name)` — transmutes the resolved symbol to
    ///    the typed function pointer. Sound as long as the typedef in
    ///    [`bindings`] matches the upstream Genie ABI. The
    ///    [`bindings::tests::handle_aliases_are_pointer_sized`] test
    ///    catches ABI-shape drift; full type-fidelity verification
    ///    happens at the first real call on the device (Phase 1.2
    ///    step 1.2.6 benchmark).
    pub fn open(qairt_lib_dir: &Path) -> Result<Self, GenieLibraryError> {
        Self::open_impl(qairt_lib_dir)
    }

    #[cfg(target_os = "android")]
    fn open_impl(qairt_lib_dir: &Path) -> Result<Self, GenieLibraryError> {
        let lib_path = genie_load_target(qairt_lib_dir);

        // SAFETY: `Library::new` is sound when the path identifies a
        // platform-native shared library and the underlying dynamic loader
        // honours its contract. We do NOT validate the .so beyond what
        // dlopen + symbol resolution catches; deeper integrity checks
        // (e.g. SHA pinning) are out of scope for v1 and would shift
        // burden onto every QAIRT release.
        let library = unsafe { Library::new(&lib_path) }.map_err(|source| {
            GenieLibraryError::DlopenFailed {
                dir: qairt_lib_dir.to_path_buf(),
                source,
            }
        })?;

        let config_create_from_json = resolve_symbol::<GenieDialogConfig_createFromJson_fn>(
            &library,
            SYM_GENIE_DIALOG_CONFIG_CREATE_FROM_JSON,
            "GenieDialogConfig_createFromJson",
        )?;
        let config_free = resolve_symbol::<GenieDialogConfig_free_fn>(
            &library,
            SYM_GENIE_DIALOG_CONFIG_FREE,
            "GenieDialogConfig_free",
        )?;
        let dialog_create = resolve_symbol::<GenieDialog_create_fn>(
            &library,
            SYM_GENIE_DIALOG_CREATE,
            "GenieDialog_create",
        )?;
        let dialog_query = resolve_symbol::<GenieDialog_query_fn>(
            &library,
            SYM_GENIE_DIALOG_QUERY,
            "GenieDialog_query",
        )?;
        let dialog_free = resolve_symbol::<GenieDialog_free_fn>(
            &library,
            SYM_GENIE_DIALOG_FREE,
            "GenieDialog_free",
        )?;

        // Logging symbols are optional: a `libGenie.so` that doesn't export
        // them must not fail the whole backend (which already reaches
        // `GenieDialog_create` without them). Resolve best-effort; `None`
        // simply disables the file-logging diagnostic.
        let log_create =
            resolve_optional_symbol::<GenieLog_create_fn>(&library, SYM_GENIE_LOG_CREATE);
        let log_free = resolve_optional_symbol::<GenieLog_free_fn>(&library, SYM_GENIE_LOG_FREE);
        let config_bind_logger = resolve_optional_symbol::<GenieDialogConfig_bindLogger_fn>(
            &library,
            SYM_GENIE_DIALOG_CONFIG_BIND_LOGGER,
        );

        Ok(Self {
            _library: library,
            config_create_from_json,
            config_free,
            dialog_create,
            dialog_query,
            dialog_free,
            log_create,
            log_free,
            config_bind_logger,
        })
    }

    #[cfg(not(target_os = "android"))]
    fn open_impl(_qairt_lib_dir: &Path) -> Result<Self, GenieLibraryError> {
        Err(GenieLibraryError::PlatformUnsupported(std::env::consts::OS))
    }
}

/// Resolve the dlopen target for `libGenie.so` from the QAIRT lib dir.
///
/// An **empty** `qairt_lib_dir` yields the bare basename `libGenie.so`.
/// `libloading::Library::new` passes that to `dlopen` verbatim, and a
/// name with no path separator is resolved through the dynamic linker's
/// namespace search rather than as a filesystem path. On Android that
/// namespace includes the app's `nativeLibraryDir`, where an APK's
/// bundled `lib/arm64-v8a/*.so` are extracted at install time — so the
/// QAIRT runtime (and its DT_NEEDED dependencies) load without any
/// caller-supplied path. A non-empty dir is joined with the basename for
/// an explicit-path load (desktop / staged-SDK layouts).
///
/// Pure helper — no filesystem access. Always compiled (and host-tested)
/// even though it is only *called* on the Android `open_impl` path.
pub fn genie_load_target(qairt_lib_dir: &Path) -> PathBuf {
    if qairt_lib_dir.as_os_str().is_empty() {
        PathBuf::from(LIBGENIE_BASENAME)
    } else {
        qairt_lib_dir.join(LIBGENIE_BASENAME)
    }
}

/// Resolve a function-shaped symbol from a `libloading::Library` and
/// flatten the borrow/lifetime layer into a bare `fn` pointer.
///
/// `libloading::Library::get::<T>(name)` returns
/// `Symbol<'lib, T>` whose lifetime is bounded by the `Library`. We
/// cannot return that directly from [`GenieLibrary::open`] because the
/// `Library` is owned by the same struct (self-referential lifetime).
/// `libloading` provides `Symbol::into_raw` (and `Symbol::deref` to the
/// underlying `T`); the standard pattern for function-pointer symbols
/// is to dereference and copy out the `unsafe extern "C" fn` value,
/// which is `Copy`. The `Library` field then keeps the underlying
/// `.so` mapped for the lifetime of the wrapper.
#[cfg(target_os = "android")]
fn resolve_symbol<T: Copy>(
    library: &Library,
    name: &[u8],
    debug_name: &'static str,
) -> Result<T, GenieLibraryError> {
    // SAFETY: `name` is NUL-terminated (pinned by
    // `bindings::tests::symbol_strings_are_nul_terminated`) and `T` is the
    // typedef matching the upstream signature in `bindings.rs`. ABI-shape
    // sanity is pinned by `handle_aliases_are_pointer_sized`. Full
    // type-fidelity sanity happens at the first real call on the device.
    let symbol = unsafe {
        library
            .get::<T>(name)
            .map_err(|source| GenieLibraryError::SymbolMissing {
                symbol: debug_name,
                source,
            })?
    };
    Ok(*symbol)
}

/// Resolve an **optional** function-shaped symbol, returning `None` when
/// the loaded library does not export it.
///
/// Unlike [`resolve_symbol`], a missing symbol is not an error: the Genie
/// logging entry points (`GenieLog_create`, `GenieLog_free`,
/// `GenieDialogConfig_bindLogger`) are a best-effort diagnostic that must
/// never regress the core streaming path, which a `libGenie.so` without
/// these symbols still supports. Same `T: Copy` function-pointer
/// flattening as [`resolve_symbol`].
#[cfg(target_os = "android")]
fn resolve_optional_symbol<T: Copy>(library: &Library, name: &[u8]) -> Option<T> {
    // SAFETY: identical contract to `resolve_symbol` — `name` is
    // NUL-terminated and `T` matches the upstream typedef. The only
    // difference is that a resolution failure maps to `None` rather than
    // an error, so an older library degrades gracefully.
    let symbol = unsafe { library.get::<T>(name) }.ok()?;
    Some(*symbol)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn genie_load_target_empty_dir_uses_bare_basename_for_linker_search() {
        // An empty QAIRT lib dir means "let the dynamic linker find it":
        // a bare basename (no path separator) is what `dlopen` resolves
        // through its namespace search. On Android that namespace includes
        // the app's nativeLibraryDir, where the APK's bundled
        // `lib/arm64-v8a/libGenie.so` is extracted at install time.
        let target = genie_load_target(Path::new(""));
        assert_eq!(target, PathBuf::from(LIBGENIE_BASENAME));
        // It must NOT contain a path separator, or dlopen would treat it
        // as a path and skip the namespace search.
        assert!(
            target.parent().is_none_or(|p| p.as_os_str().is_empty()),
            "basename load target must have no directory component: {target:?}"
        );
    }

    #[test]
    fn genie_load_target_nonempty_dir_joins_explicit_path() {
        let dir = Path::new("/opt/qairt/lib/aarch64-android");
        let target = genie_load_target(dir);
        assert_eq!(target, dir.join(LIBGENIE_BASENAME));
    }

    /// On the host (macOS/Linux), `GenieLibrary::open` must reject without
    /// any filesystem probe. The exact directory passed does not exist;
    /// if the function were attempting to dlopen on a host, we would see
    /// a `DlopenFailed`. Asserting `PlatformUnsupported` pins both the
    /// short-circuit AND the typed-error contract that the safe wrapper
    /// will pattern-match against.
    #[test]
    fn host_open_returns_platform_unsupported() {
        let result = GenieLibrary::open(Path::new("/non/existent/path"));
        // We cannot `{:?}`-print the full `Result` because `GenieLibrary`
        // wraps a `libloading::Library` and we deliberately do not impl
        // `Debug` on the safe wrapper (the symbol-table pointers carry no
        // useful debug info and a derived impl would just clutter
        // unrelated error sites). Match on the error variant directly and
        // print its `Display` rendering when the test fails.
        let err = match result {
            Ok(_) => panic!(
                "GenieLibrary::open unexpectedly succeeded on {} \
                 (no QAIRT runtime is installed in CI)",
                std::env::consts::OS
            ),
            Err(e) => e,
        };
        match err {
            #[cfg(not(target_os = "android"))]
            GenieLibraryError::PlatformUnsupported(os) => {
                assert_eq!(os, std::env::consts::OS);
            }
            #[cfg(target_os = "android")]
            GenieLibraryError::DlopenFailed { .. } => {
                // Android host running the test suite with no QAIRT
                // installed: dlopen of a non-existent path fails. This
                // branch is taken on Termux + RedMagic when libGenie.so
                // is not staged; still the right shape, just a
                // different variant. Acceptable for the host-only
                // sanity test.
            }
            other => {
                panic!("expected PlatformUnsupported (or DlopenFailed on Android), got: {other}")
            }
        }
    }

    /// The error rendering is dev-facing only (children never see it via
    /// the i18n boundary), but the message must clearly identify the
    /// failure mode so a developer staring at a bug report can diagnose
    /// without re-reading the source. Pinning the wording keeps a future
    /// refactor from silently breaking diagnostic continuity.
    #[test]
    fn platform_unsupported_message_mentions_android_and_current_os() {
        let err = GenieLibraryError::PlatformUnsupported("macos");
        let msg = err.to_string();
        assert!(
            msg.contains("Android"),
            "error message should mention Android; got {msg:?}"
        );
        assert!(
            msg.contains("macos"),
            "error message should mention the current OS; got {msg:?}"
        );
    }
}
