//! Raw FFI declarations for the Qualcomm Genie C API.
//!
//! These are hand-rolled (not produced by `bindgen`) for two reasons:
//!
//! 1. The QAIRT SDK is gated behind a Qualcomm developer-portal login, so we
//!    cannot commit the upstream headers into a public AGPL repo without a
//!    licence-redistribution pass (see Phase 1.2 design §2 and §5). Step
//!    1.2.0 of the plan installs the SDK on a dev box and validates the
//!    chatapp_android consumer; until that lands and the licence question is
//!    resolved, vendored headers + `bindgen` are deferred.
//! 2. The Genie surface the Primer needs is tiny — five functions, three
//!    opaque handle types, one status enum — small enough that hand-rolled
//!    declarations are easier to audit than `bindgen` output. The Phase 1.2
//!    plan's risk register explicitly approves this path:
//!    *"`bindgen` produces unusable bindings for some Genie types →
//!    hand-roll the function declarations (only ~6 functions needed)"*.
//!
//! When QAIRT lands on a dev box and licence-redistribution is signed off,
//! swapping this module for a `build.rs`-bindgen'd `bindings.rs` is a
//! straightforward follow-up: the public surface is identical (raw types
//! and `extern "C"` signatures), only the source of the declarations changes.
//!
//! ## Reference
//!
//! Mirrors the Genie API surface documented in:
//!   - QAIRT 2.45 `include/Genie/GenieDialog.h`
//!   - QAIRT 2.45 `include/Genie/GenieCommon.h`
//!
//! All declarations match the upstream header signatures verbatim; type
//! aliases are chosen for readability on the Rust side, and the opaque
//! handle types follow the standard "extern type via empty enum" pattern.
//!
//! ## 2.45 API note (device-validated 2026-06-10)
//!
//! These declarations were corrected against the QAIRT **Community 2.45**
//! headers after on-device validation of the Primer's [`crate::GenieLibrary`]
//! against the chatapp_android v79 bundle surfaced a symbol-resolution
//! failure: the pre-2.45 scaffold expected a separate
//! `GenieDialog_setTokenCallback` entry point, but 2.45's `libGenie.so`
//! does not export it. In 2.45 the streaming token callback is passed
//! **directly to [`GenieDialog_query`]** as the `callback` + `userData`
//! parameters (see [`GenieDialog_QueryCallback_t`] and
//! [`GenieDialog_query_fn`]). The Primer therefore resolves **five**
//! symbols, not six — `setTokenCallback` is gone.

#![allow(non_camel_case_types)]

use core::ffi::{c_char, c_void};

// ---------------------------------------------------------------------------
// Opaque handle types
// ---------------------------------------------------------------------------

/// Opaque handle to a `GenieDialogConfig` instance (parsed from JSON).
///
/// The "empty enum" pattern is the standard way to express a pointer-only
/// opaque type in Rust FFI: the enum has no variants, so no instance can
/// ever exist on the Rust side, but `*mut GenieDialogConfig` is a valid
/// pointer type. We deliberately omit `#[repr(C)]` because rustc rejects
/// that combination on a zero-variant enum (E0084) — the type is never
/// instantiated, so it has no representation to specify; only the pointer
/// alias below crosses the FFI boundary.
pub enum GenieDialogConfig {}

/// Opaque handle to a `GenieDialog` instance (a model + dialog state).
pub enum GenieDialog {}

/// Convenience aliases matching the upstream header typedefs.
pub type GenieDialogConfig_Handle_t = *mut GenieDialogConfig;
pub type GenieDialog_Handle_t = *mut GenieDialog;

// ---------------------------------------------------------------------------
// Status / sentinel enums (upstream uses `int32_t` underneath)
// ---------------------------------------------------------------------------

/// Status code returned by every Genie call.
///
/// The upstream header defines this as a C enum backed by `int32_t`. We
/// mirror the underlying integer type but use bare `const` items rather
/// than a Rust enum so that unknown future status values (e.g. a new
/// failure code introduced in a later QAIRT) cannot trigger undefined
/// behaviour when crossing the FFI boundary. Only the codes the Primer
/// inspects are named; everything else is observable as the raw `i32`.
pub type Genie_Status_t = i32;

/// Operation completed successfully.
pub const GENIE_STATUS_SUCCESS: Genie_Status_t = 0;

/// `GenieDialog_query` ran out of context window mid-generation: the
/// prompt plus the generated reply exceeded the model's context length
/// (`Context limit exceeded (PROMPT + GEN > CTX)` in `genie.log`).
///
/// This is NOT an ABI/setup failure — the reply already streamed in full
/// via the token callback before the limit was hit, so the Primer treats
/// it as a graceful completion (the turn keeps what was generated) rather
/// than dropping the turn. See the on-device diagnosis (status 4 on QAIRT
/// 2.45, RedMagic 11 Pro, cl2048 bundle): `~/qnn-export-2048/`
/// `genie-status4-diagnosis.txt`. All other non-success codes stay hard
/// errors so a genuine ABI mismatch is never masked.
///
/// The value is reverse-engineered from `genie.log`, not read from a
/// header (the QAIRT SDK is developer-portal-gated and unvendorable).
/// Tracked for header confirmation in <https://github.com/hherb/primer/issues/223>.
pub const GENIE_STATUS_CONTEXT_LIMIT_EXCEEDED: Genie_Status_t = 4;

/// Sentence-completion mode for `GenieDialog_query`.
///
/// The upstream `Genie_Dialog_SentenceCode_t` enum is currently a single-
/// variant signal that the prompt is a complete utterance. Adding more
/// variants is a non-breaking change upstream; we keep the type as a raw
/// alias to absorb future additions without recompiling consumers.
pub type Genie_Dialog_SentenceCode_t = i32;

/// "The prompt passed in is a complete sentence; emit a full response."
pub const GENIE_DIALOG_SENTENCE_COMPLETE: Genie_Dialog_SentenceCode_t = 0;

// ---------------------------------------------------------------------------
// Token callback signature
// ---------------------------------------------------------------------------

/// Streaming token callback passed **directly to [`GenieDialog_query_fn`]**
/// (2.45 API) and invoked for each generated token (or token chunk,
/// depending on the exporter).
///
/// Upstream name: `GenieDialog_QueryCallback_t`. The C ABI signature in
/// QAIRT 2.45 `GenieDialog.h` is:
///
/// ```c
/// typedef void (*GenieDialog_QueryCallback_t)(const char* response,
///                                             const GenieDialog_SentenceCode_t sentenceCode,
///                                             const void* userData);
/// ```
///
/// Arguments:
/// - `response_str`: null-terminated UTF-8 token text. Lifetime is bounded
///   by the callback invocation; the callee must `strdup`-or-copy if it
///   intends to retain the text beyond return.
/// - `sentence_code`: matches the `Genie_Dialog_SentenceCode_t` passed to
///   `GenieDialog_query`. Currently informational.
/// - `user_data`: the opaque `userData` pointer the caller passed to
///   `GenieDialog_query` alongside this callback. The Primer uses it to
///   forward the token into a `futures::channel::mpsc::UnboundedSender`
///   (Phase 1.2 design §3 — *"`generate_stream` flow"*).
///
/// The header types `userData` as `const void*`; we mirror it as
/// `*mut c_void` for symmetry with the call-site (Genie only ever hands
/// back the exact pointer we supplied, so const-ness on the callback side
/// is not ABI-relevant for a pointer-sized argument).
///
/// We treat the callback as non-nullable (bare `unsafe extern "C" fn` rather
/// than `Option<unsafe extern "C" fn ...>`) because the Primer always
/// supplies a real callback — there is no use case for a null one. If a
/// future QAIRT release documents `NULL` as a no-callback sentinel and the
/// safe wrapper needs that behaviour, swap this alias for the `Option`
/// form; nullability is the only ABI-relevant difference.
pub type GenieDialog_QueryCallback_t = unsafe extern "C" fn(
    response_str: *const c_char,
    sentence_code: Genie_Dialog_SentenceCode_t,
    user_data: *mut c_void,
);

// ---------------------------------------------------------------------------
// Function signatures (resolved at runtime via libloading; never linked at
// compile time — see Phase 1.2 design §5 for the rationale).
// ---------------------------------------------------------------------------

/// `GenieDialogConfig_createFromJson(json_path, out_handle) -> status`
pub type GenieDialogConfig_createFromJson_fn = unsafe extern "C" fn(
    json_path: *const c_char,
    out_handle: *mut GenieDialogConfig_Handle_t,
) -> Genie_Status_t;

/// `GenieDialogConfig_free(handle) -> status`
pub type GenieDialogConfig_free_fn =
    unsafe extern "C" fn(handle: GenieDialogConfig_Handle_t) -> Genie_Status_t;

/// `GenieDialog_create(config_handle, out_dialog_handle) -> status`
pub type GenieDialog_create_fn = unsafe extern "C" fn(
    config: GenieDialogConfig_Handle_t,
    out_dialog: *mut GenieDialog_Handle_t,
) -> Genie_Status_t;

/// `GenieDialog_query(dialog_handle, prompt, sentence_code, callback, user_data) -> status`
///
/// Blocking. The `callback` fires once per token (or token chunk) until
/// generation completes; this function returns only after
/// end-of-generation. `user_data` is forwarded verbatim to every callback
/// invocation.
///
/// 2.45 ABI (QAIRT `GenieDialog.h`):
///
/// ```c
/// Genie_Status_t GenieDialog_query(const GenieDialog_Handle_t dialogHandle,
///                                  const char* queryStr,
///                                  const GenieDialog_SentenceCode_t sentenceCode,
///                                  const GenieDialog_QueryCallback_t callback,
///                                  const void* userData);
/// ```
///
/// This replaced the pre-2.45 two-call shape (`setTokenCallback` then a
/// 4-arg `query` with a Genie-internal response sink); 2.45 folds the
/// callback registration into the query call itself.
pub type GenieDialog_query_fn = unsafe extern "C" fn(
    dialog: GenieDialog_Handle_t,
    prompt: *const c_char,
    sentence_code: Genie_Dialog_SentenceCode_t,
    callback: GenieDialog_QueryCallback_t,
    user_data: *mut c_void,
) -> Genie_Status_t;

/// `GenieDialog_free(handle) -> status`
pub type GenieDialog_free_fn = unsafe extern "C" fn(handle: GenieDialog_Handle_t) -> Genie_Status_t;

/// `GenieDialog_reset(handle) -> status`
///
/// Resets the dialog to its initial state — clears the accumulated
/// conversation/KV context so the next `GenieDialog_query` starts from an
/// empty context window. **Load-bearing for the Primer's stateless prompt
/// model:** the engine re-sends the *entire* prompt (system + history) on
/// every query, and the chat turn shares one dialog handle with the three
/// background subsystems (classifier / extractor / comprehension). Without
/// a reset before each query, every query appends to the same KV context
/// and the 2048-token window saturates within a turn or two (the on-device
/// "Context limit exceeded" failure). Resetting before each query keeps
/// every query independent.
///
/// 2.45 ABI (QAIRT `GenieDialog.h`):
///
/// ```c
/// Genie_Status_t GenieDialog_reset(const GenieDialog_Handle_t dialogHandle);
/// ```
pub type GenieDialog_reset_fn =
    unsafe extern "C" fn(handle: GenieDialog_Handle_t) -> Genie_Status_t;

// ---------------------------------------------------------------------------
// Genie logging API (QAIRT 2.45 `GenieLog.h`)
//
// Used by the Primer to surface the FastRPC/HTP error behind a generic
// `GENIE_STATUS_ERROR_GENERAL` (-1) from `GenieDialog_create` on Android,
// where `logcat` is unavailable on some ROMs (e.g. the RedMagic). A
// user-supplied callback is routed to a file the developer reads via
// `run-as cat`. These three symbols are resolved **best-effort** (see
// [`crate::GenieLibrary`]); a `libGenie.so` that does not export them
// leaves logging disabled rather than failing the whole backend.
// ---------------------------------------------------------------------------

/// Opaque handle to a `GenieLogConfig` instance.
///
/// Upstream typedefs this as `const struct _GenieLogConfig_Handle_t*`; the
/// Primer only ever passes `NULL` for it (the 2.45 header documents the
/// config handle as "a placeholder for future logger configurability.
/// Currently, it must be NULL"), so the empty-enum-pointer alias suffices.
pub enum GenieLogConfig {}

/// Opaque handle to a `GenieLog` instance (a live logger).
pub enum GenieLog {}

/// Convenience aliases matching the upstream header typedefs.
pub type GenieLogConfig_Handle_t = *const GenieLogConfig;
pub type GenieLog_Handle_t = *const GenieLog;

/// Log level, mirroring the upstream `GenieLog_Level_t` enum (backed by
/// `int32_t`). Kept as bare `const` items rather than a Rust enum so an
/// unknown future level value crossing the FFI boundary cannot trigger
/// undefined behaviour.
pub type GenieLog_Level_t = i32;

/// Error-level log message.
pub const GENIE_LOG_LEVEL_ERROR: GenieLog_Level_t = 1;
/// Warning-level log message.
pub const GENIE_LOG_LEVEL_WARN: GenieLog_Level_t = 2;
/// Info-level log message.
pub const GENIE_LOG_LEVEL_INFO: GenieLog_Level_t = 3;
/// Verbose-level log message (the most detailed level; what the Primer
/// requests so DSP-init diagnostics are not filtered out).
pub const GENIE_LOG_LEVEL_VERBOSE: GenieLog_Level_t = 4;

/// User-supplied logging callback (upstream `GenieLog_Callback_t`).
///
/// The C ABI signature in QAIRT 2.45 `GenieLog.h` is:
///
/// ```c
/// typedef void (*GenieLog_Callback_t)(const GenieLog_Handle_t handle,
///                                     const char* fmt,
///                                     GenieLog_Level_t level,
///                                     uint64_t timestamp,
///                                     va_list args);
/// ```
///
/// Note there is **no `userData` parameter** — Genie cannot hand back a
/// per-instance pointer, so the safe wrapper routes messages to a
/// process-global sink (see `primer_inference::qnn::genie::log`).
///
/// The final `args` parameter is a C `va_list`. On the AArch64 (Android)
/// ABI a `va_list` is `struct __va_list[1]`, which decays to a pointer
/// when passed as a function argument — so we model it as `*mut c_void`
/// (the pointer to the caller's `va_list` struct) and forward it verbatim
/// to `vsnprintf`, whose own `va_list` parameter decays the same way. This
/// is the standard way to forward a `va_list` callback on AArch64 without
/// the unstable `core::ffi::VaList`.
///
/// The backend warns that this callback "may be called from multiple
/// threads, and expects that it is re-entrant" — the global sink is
/// `Mutex`-guarded and never re-enters Genie, so that contract holds.
///
/// The callback is treated as non-nullable (we always supply a real one).
pub type GenieLog_Callback_t = unsafe extern "C" fn(
    handle: GenieLog_Handle_t,
    fmt: *const c_char,
    level: GenieLog_Level_t,
    timestamp: u64,
    args: *mut c_void,
);

/// `GenieLog_create(config_handle, callback, log_level, out_handle) -> status`
///
/// `config_handle` must be `NULL` in 2.45. A non-null `callback` installs
/// the user logger; `log_level` is the maximum level emitted.
pub type GenieLog_create_fn = unsafe extern "C" fn(
    config: GenieLogConfig_Handle_t,
    callback: GenieLog_Callback_t,
    log_level: GenieLog_Level_t,
    out_handle: *mut GenieLog_Handle_t,
) -> Genie_Status_t;

/// `GenieLog_free(handle) -> status`
pub type GenieLog_free_fn = unsafe extern "C" fn(handle: GenieLog_Handle_t) -> Genie_Status_t;

/// `GenieDialogConfig_bindLogger(config_handle, log_handle) -> status`
///
/// Binds a logger to a dialog config; the logger is inherited by any
/// dialog created from that config, so logs fire during the
/// `GenieDialog_create` model-load/DSP-init path.
pub type GenieDialogConfig_bindLogger_fn = unsafe extern "C" fn(
    config: GenieDialogConfig_Handle_t,
    log_handle: GenieLog_Handle_t,
) -> Genie_Status_t;

// ---------------------------------------------------------------------------
// Symbol names (the strings passed to `Library::get`)
// ---------------------------------------------------------------------------

/// `libGenie.so` symbol for `GenieDialogConfig_createFromJson`.
pub const SYM_GENIE_DIALOG_CONFIG_CREATE_FROM_JSON: &[u8] = b"GenieDialogConfig_createFromJson\0";

/// `libGenie.so` symbol for `GenieDialogConfig_free`.
pub const SYM_GENIE_DIALOG_CONFIG_FREE: &[u8] = b"GenieDialogConfig_free\0";

/// `libGenie.so` symbol for `GenieDialog_create`.
pub const SYM_GENIE_DIALOG_CREATE: &[u8] = b"GenieDialog_create\0";

/// `libGenie.so` symbol for `GenieDialog_query`.
pub const SYM_GENIE_DIALOG_QUERY: &[u8] = b"GenieDialog_query\0";

/// `libGenie.so` symbol for `GenieDialog_free`.
pub const SYM_GENIE_DIALOG_FREE: &[u8] = b"GenieDialog_free\0";

/// `libGenie.so` symbol for `GenieDialog_reset`.
pub const SYM_GENIE_DIALOG_RESET: &[u8] = b"GenieDialog_reset\0";

/// `libGenie.so` symbol for `GenieLog_create` (optional — logging path).
pub const SYM_GENIE_LOG_CREATE: &[u8] = b"GenieLog_create\0";

/// `libGenie.so` symbol for `GenieLog_free` (optional — logging path).
pub const SYM_GENIE_LOG_FREE: &[u8] = b"GenieLog_free\0";

/// `libGenie.so` symbol for `GenieDialogConfig_bindLogger` (optional —
/// logging path).
pub const SYM_GENIE_DIALOG_CONFIG_BIND_LOGGER: &[u8] = b"GenieDialogConfig_bindLogger\0";

/// Default basename of the QAIRT Genie shared library. The actual filesystem
/// path is composed by [`crate::GenieLibrary::open`] from a caller-supplied
/// `qairt_lib_dir`.
pub const LIBGENIE_BASENAME: &str = "libGenie.so";

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::size_of;

    #[test]
    fn handle_aliases_are_pointer_sized() {
        // Sanity: the typedef aliases match the upstream header's pointer-
        // typedef shape. If a future refactor accidentally aliases these to
        // a struct (or to a non-pointer type), this catches it before the
        // mismatch corrupts the FFI ABI.
        assert_eq!(
            size_of::<GenieDialogConfig_Handle_t>(),
            size_of::<*mut core::ffi::c_void>()
        );
        assert_eq!(
            size_of::<GenieDialog_Handle_t>(),
            size_of::<*mut core::ffi::c_void>()
        );
        assert_eq!(
            size_of::<GenieLogConfig_Handle_t>(),
            size_of::<*mut core::ffi::c_void>()
        );
        assert_eq!(
            size_of::<GenieLog_Handle_t>(),
            size_of::<*mut core::ffi::c_void>()
        );
    }

    #[test]
    fn symbol_strings_are_nul_terminated() {
        // `Library::get::<T>(name)` requires a NUL-terminated byte slice;
        // mis-terminated symbol strings would silently fail to resolve at
        // runtime on the device with a misleading "symbol not found"
        // error. Pinning the invariant here so a typo can't slip past
        // review.
        for name in [
            SYM_GENIE_DIALOG_CONFIG_CREATE_FROM_JSON,
            SYM_GENIE_DIALOG_CONFIG_FREE,
            SYM_GENIE_DIALOG_CREATE,
            SYM_GENIE_DIALOG_QUERY,
            SYM_GENIE_DIALOG_FREE,
            SYM_GENIE_LOG_CREATE,
            SYM_GENIE_LOG_FREE,
            SYM_GENIE_DIALOG_CONFIG_BIND_LOGGER,
        ] {
            assert_eq!(
                name.last().copied(),
                Some(0u8),
                "symbol {:?} is not NUL-terminated",
                core::str::from_utf8(&name[..name.len() - 1]).unwrap_or("<non-utf8>")
            );
        }
    }
}
