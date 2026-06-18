//! Real `AndroidSpeechBridge` over `jni`. Device-only (compiled only for
//! target_os = "android"). Runtime behaviour is validated on a physical
//! device; this module only needs to cross-compile here.

use crate::android::bridge::AndroidSpeechBridge;
use crate::android::capabilities::SpeechCapabilities;
use crate::android::events::SpeechEvent;
use jni::JNIEnv;
use jni::JavaVM;
use jni::objects::{JClass, JString};
use primer_core::error::{PrimerError, Result};

pub struct JniSpeechBridge {
    vm: &'static JavaVM,
}

fn jerr(e: impl std::fmt::Display) -> PrimerError {
    PrimerError::Speech(format!("android speech JNI: {e}"))
}

/// Resolve the `org.theprimer.gui.PrimerSpeech` class from the cached
/// `GlobalRef` (populated by `nativeInit`), as a fresh local `JClass` valid
/// on the current attached thread.
///
/// This deliberately does NOT call `JNIEnv::find_class`: on a thread
/// attached from native code `find_class` uses the system classloader,
/// which cannot see app classes and throws
/// `ClassNotFoundException: "org.theprimer.gui.PrimerSpeech"` (Plan 1
/// risk #2, confirmed on-device via the 2026-06-19 dropbox tombstone). The
/// class was resolved once on a real Java thread in `nativeInit` and
/// cached; here we just materialise a local reference to it.
fn primer_speech_class<'local>(env: &mut JNIEnv<'local>) -> Result<JClass<'local>> {
    let global = crate::android::vm::primer_speech_class()?;
    let local = env.new_local_ref(global.as_obj()).map_err(jerr)?;
    Ok(JClass::from(local))
}

impl JniSpeechBridge {
    pub fn new() -> Result<Self> {
        // The JavaVM is cached by the `nativeInit` JNI export
        // (MainActivity.onCreate → PrimerSpeech.nativeInit). We no longer
        // touch ndk_context — it is not populated for our call path under
        // the Tauri-mobile runtime (Plan 1 gate finding, 2026-06-18). The
        // cache hands back a `&'static JavaVM` (it lives for the process
        // lifetime), so the bridge borrows it directly — no raw-pointer
        // re-wrap, no unsafe. `attach_current_thread` only needs `&self`.
        let vm = crate::android::vm::java_vm()?;
        Ok(Self { vm })
    }
}

impl AndroidSpeechBridge for JniSpeechBridge {
    fn query_capabilities(&self) -> Result<SpeechCapabilities> {
        let mut env = self.vm.attach_current_thread().map_err(jerr)?;
        let class = primer_speech_class(&mut env)?;
        // call_static_method returns JValueOwned; .l() extracts the JObject.
        let obj = env
            .call_static_method(&class, "queryCapabilities", "()Ljava/lang/String;", &[])
            .map_err(jerr)?
            .l()
            .map_err(jerr)?;
        // Safe: the Java method declares String as its return type.
        let jstr = JString::from(obj);
        let java_str = env.get_string(&jstr).map_err(jerr)?;
        let s: String = java_str.into();
        serde_json::from_str(&s).map_err(jerr)
    }

    // ── Voice-loop methods — real JNI impls land in Plan 2 Task 7
    // (device-only). Stubbed here so the trait is satisfied and the
    // aarch64-linux-android cross-compile stays green between Task 2 and
    // Task 7. Each returns a clear "not yet implemented" Speech error.
    fn start_listening(&self, _bcp47: &str) -> Result<()> {
        Err(jerr("start_listening not yet implemented (Plan 2 Task 7)"))
    }

    fn stop_listening(&self) -> Result<()> {
        Err(jerr("stop_listening not yet implemented (Plan 2 Task 7)"))
    }

    fn poll_event(&self, _timeout_ms: u32) -> Result<Option<SpeechEvent>> {
        Err(jerr("poll_event not yet implemented (Plan 2 Task 7)"))
    }

    fn speak(&self, _text: &str) -> Result<()> {
        Err(jerr("speak not yet implemented (Plan 2 Task 7)"))
    }

    fn cancel_speech(&self) -> Result<()> {
        Err(jerr("cancel_speech not yet implemented (Plan 2 Task 7)"))
    }
}
