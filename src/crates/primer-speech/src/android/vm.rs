//! Process-wide `JavaVM` handle, populated once by the `nativeInit` JNI
//! export (called from `MainActivity.onCreate`) and read by every JNI
//! bridge. Replaces `ndk_context::android_context()`, which the
//! Tauri-mobile runtime does not populate for our call path (Plan 1's #1
//! risk, confirmed on-device — see the 2026-06-18 capability-gate handoff).
//!
//! The cache is a [`VmCache`] (a `OnceLock` wrapper): set exactly once at
//! startup, read for the process lifetime. Reading before `nativeInit`
//! ran is a recoverable error (a clear "voice not initialised" message),
//! never a panic — the whole point of moving off `ndk_context`.
//!
//! [`VmCache`] is generic purely so its set-once / unset-is-an-error
//! semantics are host-testable without a real `JavaVM`; the only
//! production instantiation is `VmCache<jni::JavaVM>` (android-only).

use std::sync::OnceLock;

use primer_core::error::{PrimerError, Result};

/// Set-once cache for the process `JavaVM` (generic for host-testability).
/// `get` before `set` is a recoverable [`PrimerError::Speech`], never a
/// panic; a second `set` is rejected and the first value is kept.
pub struct VmCache<T> {
    cell: OnceLock<T>,
}

impl<T> Default for VmCache<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> VmCache<T> {
    /// A new, empty cache. `const` so it can back a `static`.
    pub const fn new() -> Self {
        Self {
            cell: OnceLock::new(),
        }
    }

    /// Store the value on the first call. A later call returns `Err(value)`
    /// (the rejected value) and leaves the stored one untouched.
    pub fn set(&self, value: T) -> std::result::Result<(), T> {
        self.cell.set(value)
    }

    /// Borrow the stored value, or a clear "not initialised" error if
    /// `set` has not run yet.
    pub fn get(&self) -> Result<&T> {
        self.cell.get().ok_or_else(|| {
            PrimerError::Speech(
                "android speech not initialised: nativeInit() has not run \
                 (MainActivity.onCreate must call PrimerSpeech.nativeInit)"
                    .into(),
            )
        })
    }
}

// ── Android-only: the real JavaVM cache + accessors ─────────────────
// Device-verified (no host test — `JavaVM` is android-only), mirroring
// the Plan 1 `jni_bridge` precedent. The host-testable semantics live in
// `VmCache` above.
#[cfg(target_os = "android")]
mod imp {
    use super::VmCache;
    use jni::JavaVM;
    use primer_core::error::Result;

    static JAVA_VM: VmCache<JavaVM> = VmCache::new();

    /// Cache the process `JavaVM`. Called once from the `nativeInit` JNI
    /// export. A redundant second call is benign and logged.
    pub fn set_java_vm(vm: JavaVM) {
        if JAVA_VM.set(vm).is_err() {
            tracing::warn!(
                target: "primer::speech::android",
                "nativeInit called more than once; keeping the first JavaVM"
            );
        }
    }

    /// Borrow the cached `JavaVM`, or error if `nativeInit` has not run.
    pub fn java_vm() -> Result<&'static JavaVM> {
        JAVA_VM.get()
    }
}

#[cfg(target_os = "android")]
pub use imp::{java_vm, set_java_vm};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_before_set_is_error_not_panic() {
        let cache: VmCache<u32> = VmCache::new();
        assert!(
            cache.get().is_err(),
            "reading an unset cache must be an error, never a panic"
        );
    }

    #[test]
    fn set_once_then_get_returns_value() {
        let cache: VmCache<u32> = VmCache::new();
        assert!(cache.set(7).is_ok(), "first set succeeds");
        assert_eq!(*cache.get().unwrap(), 7);
    }

    #[test]
    fn second_set_is_rejected_and_first_value_kept() {
        let cache: VmCache<u32> = VmCache::new();
        assert!(cache.set(7).is_ok());
        assert!(cache.set(9).is_err(), "second set is rejected");
        assert_eq!(*cache.get().unwrap(), 7, "the first value is kept");
    }
}
