//! Public bridge surface for the Android speech module.
//!
//! Re-exports the JNI bridge type on Android targets and exposes a host stub
//! on all other targets so downstream crates can name `bridge::JniSpeechBridge`
//! without `cfg` noise at every call site.

#[cfg(target_os = "android")]
pub use super::jni_bridge::JniSpeechBridge;

/// Host stub — `JniSpeechBridge` is not available outside of Android targets.
/// This type exists solely to allow code that names `bridge::JniSpeechBridge`
/// to compile on non-Android hosts (e.g. CI, developer laptops).
#[cfg(not(target_os = "android"))]
pub struct JniSpeechBridge {
    _private: (),
}
