//! Genie file-logging sink for surfacing the error behind a generic
//! `GenieDialog_create` failure on Android.
//!
//! ## Why this exists
//!
//! On the device, `GenieDialog_create` can return
//! `GENIE_STATUS_ERROR_GENERAL` (-1) — a catch-all that hides the real
//! FastRPC/HTP cause (unsigned-PD denial, skel-not-found, signature
//! failure, …). The actionable detail is only available through Genie's
//! own logging callback. On most Android ROMs that detail also reaches
//! `logcat`, but some (e.g. the RedMagic dev device) ship a dead
//! `logcat`, so we route Genie's logger to a **file** the developer reads
//! via `run-as cat`.
//!
//! ## The no-`userData` constraint
//!
//! `GenieLog_Callback_t` carries **no user-data pointer** (unlike the
//! token callback). Genie therefore cannot hand a per-instance sink back
//! to the callback, so the destination must be reachable statically: a
//! process-global, `Mutex`-guarded file. There is exactly one QNN backend
//! per process in practice, so a global sink is sufficient; multiple
//! dialogs simply append to the same file.
//!
//! ## The `va_list` bridge
//!
//! Genie passes a printf-style `fmt` plus a C `va_list` of arguments. To
//! recover the real message we must `vsnprintf` the two together. On the
//! AArch64 (Android) ABI a `va_list` is `struct __va_list[1]`, which
//! decays to a pointer as a function argument — so the callback receives
//! it as `*mut c_void` and forwards it verbatim to `vsnprintf`, whose own
//! `va_list` parameter decays the same way. This is the standard
//! AArch64 `va_list`-forwarding pattern and avoids the unstable
//! `core::ffi::VaList`. The bridge is `target_os = "android"`-gated, which
//! also covers the x86_64 Android emulator: on x86-64 SysV a `va_list` is
//! `__va_list_tag[1]`, an array that likewise decays to a pointer as an
//! argument, so the same pass-through is ABI-correct there too (production
//! hardware is arm64). On the host (where Genie never runs) the callback
//! falls back to recording the bare `fmt` so the module still compiles and
//! its pure helpers stay testable.

#[cfg(not(target_os = "android"))]
use std::ffi::CStr;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

use primer_qnn_sys::{
    GENIE_LOG_LEVEL_ERROR, GENIE_LOG_LEVEL_INFO, GENIE_LOG_LEVEL_VERBOSE, GENIE_LOG_LEVEL_WARN,
    GenieLog_Handle_t, GenieLog_Level_t,
};

use std::ffi::{c_char, c_void};

/// Size of the buffer `vsnprintf` renders each log message into. Messages
/// longer than this are truncated — acceptable for a diagnostic log. Not
/// inlined per the no-magic-numbers convention. Android-only because the
/// `vsnprintf` render path is the only consumer.
#[cfg(target_os = "android")]
const RENDER_BUF_BYTES: usize = 4096;

/// Process-global Genie log sink. `None` until [`set_genie_log_file`]
/// installs a file; the callback is a no-op while it is `None`.
///
/// `Mutex::new` is `const`, so this initialises without a lazy wrapper.
static GENIE_LOG_SINK: Mutex<Option<std::fs::File>> = Mutex::new(None);

/// Open (create/append) `path` and install it as the process-global Genie
/// log sink, returning any I/O error to the caller.
///
/// Append mode means re-running a session accumulates history in one file
/// rather than clobbering the previous run's diagnostics. A session-marker
/// line is written so consecutive runs are visually separable. On lock
/// poisoning the inner value is recovered rather than panicking — this is
/// a diagnostic path and must never take down the backend.
pub(crate) fn set_genie_log_file(path: &Path) -> std::io::Result<()> {
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    // Best-effort session marker; ignore a write error (the open already
    // succeeded, so the sink is usable even if this header didn't land).
    let _ = writeln!(file, "=== genie log session start ===");
    let mut guard = lock_sink();
    *guard = Some(file);
    Ok(())
}

/// Lock the sink, recovering from poisoning instead of panicking.
fn lock_sink() -> std::sync::MutexGuard<'static, Option<std::fs::File>> {
    match GENIE_LOG_SINK.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Human-readable label for a Genie log level. Unknown levels render as
/// `LVL<n>` so a future level value never panics or loses information.
/// Pure — host-tested.
fn level_label(level: GenieLog_Level_t) -> String {
    match level {
        GENIE_LOG_LEVEL_ERROR => "ERROR".to_string(),
        GENIE_LOG_LEVEL_WARN => "WARN".to_string(),
        GENIE_LOG_LEVEL_INFO => "INFO".to_string(),
        GENIE_LOG_LEVEL_VERBOSE => "VERBOSE".to_string(),
        other => format!("LVL{other}"),
    }
}

/// Format one log line: `[LEVEL ts=<timestamp>] <message>`. The message is
/// trimmed of a trailing newline so the writer's own `writeln!` is the
/// single line terminator. Pure — host-tested.
fn format_log_line(level: GenieLog_Level_t, timestamp: u64, message: &str) -> String {
    format!(
        "[{} ts={}] {}",
        level_label(level),
        timestamp,
        message.trim_end_matches(['\n', '\r'])
    )
}

/// Append a fully-rendered message to the sink. No-op when no file is
/// installed. Never panics (poison-recovering lock; write errors ignored).
fn write_to_sink(level: GenieLog_Level_t, timestamp: u64, message: &str) {
    let mut guard = lock_sink();
    if let Some(file) = guard.as_mut() {
        let line = format_log_line(level, timestamp, message);
        let _ = writeln!(file, "{line}");
        let _ = file.flush();
    }
}

/// Append a Primer-originated diagnostic line to the sink, prefixed so it
/// is distinguishable from Genie's own callback output. No-op when no file
/// is installed. Never panics (poison-recovering lock; write errors
/// ignored).
///
/// This is the channel for reporting logger-setup failures (e.g.
/// `GenieLog_create` or `GenieDialogConfig_bindLogger` returning
/// non-success) once the sink file is already open: on the target ROM
/// `logcat` is dead, so a `tracing::warn!` is invisible and the log file
/// is the only place the developer can read *why* logging didn't engage.
pub(crate) fn write_diagnostic(message: &str) {
    let mut guard = lock_sink();
    if let Some(file) = guard.as_mut() {
        let _ = writeln!(file, "[primer] {message}");
        let _ = file.flush();
    }
}

#[cfg(target_os = "android")]
unsafe extern "C" {
    /// libc `vsnprintf`. On AArch64 (and x86-64 SysV) the trailing
    /// `va_list` decays to a pointer, so we model it as `*mut c_void` and
    /// pass the callback's own decayed `va_list` pointer straight through.
    fn vsnprintf(buf: *mut c_char, size: usize, fmt: *const c_char, args: *mut c_void) -> i32;
}

/// Render Genie's `fmt` + `va_list` into an owned `String`.
///
/// On Android, this `vsnprintf`s the format string against the variadic
/// arguments. On any other target (where Genie never calls back) it falls
/// back to the bare `fmt` text, since a host has no real `va_list` to
/// consume — this keeps the module host-compilable and its surrounding
/// pure helpers testable.
///
/// # Safety
///
/// `fmt` must be a non-null, NUL-terminated C string. On Android, `args`
/// must be the live `va_list` pointer Genie passed for this exact `fmt`
/// (the callback contract). The function reads `fmt`/`args` only during
/// the call and never retains them.
unsafe fn render_message(fmt: *const c_char, args: *mut c_void) -> String {
    #[cfg(target_os = "android")]
    {
        let mut buf = vec![0u8; RENDER_BUF_BYTES];
        // SAFETY: `fmt` is a valid C format string and `args` is the
        // matching `va_list` pointer (callback contract). `vsnprintf`
        // writes at most `buf.len()` bytes including the NUL terminator
        // and returns the length that *would* have been written.
        let written = unsafe { vsnprintf(buf.as_mut_ptr() as *mut c_char, buf.len(), fmt, args) };
        if written < 0 {
            return "<vsnprintf error>".to_string();
        }
        // Actual bytes in the buffer = min(would-be length, capacity - 1
        // for the NUL). `from_utf8_lossy` tolerates any non-UTF-8 bytes.
        let len = (written as usize).min(buf.len().saturating_sub(1));
        String::from_utf8_lossy(&buf[..len]).into_owned()
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = args;
        if fmt.is_null() {
            return String::new();
        }
        // SAFETY: caller guarantees `fmt` is a NUL-terminated C string.
        unsafe { CStr::from_ptr(fmt) }
            .to_string_lossy()
            .into_owned()
    }
}

/// C-ABI Genie logging callback (matches `GenieLog_Callback_t`).
///
/// Renders the printf-style message and appends it to the global file
/// sink. A no-op when no sink is installed. Must never panic (it crosses
/// an FFI boundary): all fallible operations swallow their errors.
///
/// # Safety
///
/// `fmt` must be a valid NUL-terminated C string (or null, handled);
/// `args` must be the matching `va_list` pointer Genie supplied. Both are
/// only read during the call. The `handle` is unused.
pub(crate) unsafe extern "C" fn genie_log_callback(
    _handle: GenieLog_Handle_t,
    fmt: *const c_char,
    level: GenieLog_Level_t,
    timestamp: u64,
    args: *mut c_void,
) {
    if fmt.is_null() {
        return;
    }
    // SAFETY: forwarded straight from the callback contract; `render_message`
    // documents its own requirements and never retains the pointers.
    let message = unsafe { render_message(fmt, args) };
    write_to_sink(level, timestamp, &message);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_label_maps_known_levels() {
        assert_eq!(level_label(GENIE_LOG_LEVEL_ERROR), "ERROR");
        assert_eq!(level_label(GENIE_LOG_LEVEL_WARN), "WARN");
        assert_eq!(level_label(GENIE_LOG_LEVEL_INFO), "INFO");
        assert_eq!(level_label(GENIE_LOG_LEVEL_VERBOSE), "VERBOSE");
    }

    #[test]
    fn level_label_renders_unknown_level_without_loss() {
        // A future QAIRT level must not panic and must keep the raw value
        // visible for diagnosis.
        assert_eq!(level_label(99), "LVL99");
    }

    #[test]
    fn format_log_line_includes_level_timestamp_and_message() {
        let line = format_log_line(GENIE_LOG_LEVEL_ERROR, 12345, "FastRPC error 14001");
        assert_eq!(line, "[ERROR ts=12345] FastRPC error 14001");
    }

    #[test]
    fn format_log_line_strips_trailing_newline() {
        // Genie messages often carry a trailing newline; the writer adds
        // its own line terminator, so a doubled blank line would result
        // without this trim.
        let line = format_log_line(GENIE_LOG_LEVEL_INFO, 7, "loaded model\n");
        assert_eq!(line, "[INFO ts=7] loaded model");
        assert!(!line.ends_with('\n'));
    }

    #[test]
    fn set_genie_log_file_writes_callback_output_to_file() {
        // End-to-end through the global sink: install a temp file, drive
        // the host render path via the callback, and confirm the line
        // lands. This is the only test that installs a sink, so the global
        // `GENIE_LOG_SINK` mutation does not race another test today — if a
        // second sink-using test is ever added, both must serialise (e.g.
        // a shared `Mutex` test guard) since the sink is process-global.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("genie.log");
        set_genie_log_file(&path).unwrap();

        let fmt = std::ffi::CString::new("hello %s").unwrap();
        // SAFETY: `fmt` is a valid C string; on the host the args pointer
        // is ignored by `render_message`.
        unsafe {
            genie_log_callback(
                std::ptr::null(),
                fmt.as_ptr(),
                GENIE_LOG_LEVEL_WARN,
                42,
                std::ptr::null_mut(),
            );
        }

        // A Primer-originated diagnostic line lands too, prefixed so it is
        // distinguishable from Genie's own callback output.
        write_diagnostic("GenieLog_create failed (status -1)");

        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(
            contents.contains("=== genie log session start ==="),
            "missing session marker: {contents:?}"
        );
        // Host render path records the bare fmt (no va_list consumption).
        assert!(
            contents.contains("[WARN ts=42] hello %s"),
            "missing rendered line: {contents:?}"
        );
        assert!(
            contents.contains("[primer] GenieLog_create failed (status -1)"),
            "missing diagnostic line: {contents:?}"
        );

        // Reset the global sink so this test doesn't leak a file handle
        // into other tests sharing the process.
        *lock_sink() = None;
    }

    #[test]
    fn null_fmt_is_a_noop() {
        // A null format string must not crash the callback.
        // SAFETY: explicitly passing null to exercise the guard.
        unsafe {
            genie_log_callback(
                std::ptr::null(),
                std::ptr::null(),
                GENIE_LOG_LEVEL_ERROR,
                0,
                std::ptr::null_mut(),
            );
        }
    }
}
