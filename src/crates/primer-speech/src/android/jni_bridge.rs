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

    fn has_record_audio_permission(&self) -> Result<bool> {
        let mut env = self.vm.attach_current_thread().map_err(jerr)?;
        let class = primer_speech_class(&mut env)?;
        // hasRecordAudioPermission() -> boolean; .z() extracts the jboolean.
        let granted = env
            .call_static_method(&class, "hasRecordAudioPermission", "()Z", &[])
            .map_err(jerr)?
            .z()
            .map_err(jerr)?;
        Ok(granted)
    }

    fn open_app_settings(&self) -> Result<()> {
        let mut env = self.vm.attach_current_thread().map_err(jerr)?;
        let class = primer_speech_class(&mut env)?;
        env.call_static_method(&class, "openAppSettings", "()V", &[])
            .map_err(jerr)?;
        Ok(())
    }

    // ── Voice-loop methods (Plan 2 Task 7) ─────────────────────────────
    // Each mirrors `query_capabilities`'s pattern: attach the current thread,
    // materialise the cached `PrimerSpeech` class (NOT `find_class` — see
    // `primer_speech_class`), then `call_static_method`. Device-verified
    // (Task 10); only the cross-compile is checked here.
    fn start_listening(&self, bcp47: &str) -> Result<()> {
        let mut env = self.vm.attach_current_thread().map_err(jerr)?;
        let class = primer_speech_class(&mut env)?;
        let arg = env.new_string(bcp47).map_err(jerr)?;
        env.call_static_method(
            &class,
            "startListening",
            "(Ljava/lang/String;)V",
            &[(&arg).into()],
        )
        .map_err(jerr)?;
        Ok(())
    }

    fn stop_listening(&self) -> Result<()> {
        let mut env = self.vm.attach_current_thread().map_err(jerr)?;
        let class = primer_speech_class(&mut env)?;
        env.call_static_method(&class, "stopListening", "()V", &[])
            .map_err(jerr)?;
        Ok(())
    }

    fn poll_event(&self, timeout_ms: u32) -> Result<Option<SpeechEvent>> {
        let mut env = self.vm.attach_current_thread().map_err(jerr)?;
        let class = primer_speech_class(&mut env)?;
        // pollSpeechEvent(int) -> String; "" means "no event this poll".
        let obj = env
            .call_static_method(
                &class,
                "pollSpeechEvent",
                "(I)Ljava/lang/String;",
                &[(timeout_ms as i32).into()],
            )
            .map_err(jerr)?
            .l()
            .map_err(jerr)?;
        let jstr = JString::from(obj);
        let s: String = env.get_string(&jstr).map_err(jerr)?.into();
        if s.is_empty() {
            return Ok(None);
        }
        serde_json::from_str(&s).map(Some).map_err(jerr)
    }

    fn speak(&self, text: &str) -> Result<()> {
        let mut env = self.vm.attach_current_thread().map_err(jerr)?;
        let class = primer_speech_class(&mut env)?;
        let arg = env.new_string(text).map_err(jerr)?;
        // Blocks inside Kotlin until the engine reports done (D3).
        env.call_static_method(&class, "speak", "(Ljava/lang/String;)V", &[(&arg).into()])
            .map_err(jerr)?;
        Ok(())
    }

    fn cancel_speech(&self) -> Result<()> {
        let mut env = self.vm.attach_current_thread().map_err(jerr)?;
        let class = primer_speech_class(&mut env)?;
        env.call_static_method(&class, "cancelSpeech", "()V", &[])
            .map_err(jerr)?;
        Ok(())
    }
}
