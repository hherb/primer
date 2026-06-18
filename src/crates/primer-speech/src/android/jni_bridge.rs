//! Real `AndroidSpeechBridge` over `jni`. Device-only (compiled only for
//! target_os = "android"). Runtime behaviour is validated on a physical
//! device; this module only needs to cross-compile here.

use crate::android::bridge::AndroidSpeechBridge;
use crate::android::capabilities::SpeechCapabilities;
use jni::JavaVM;
use jni::objects::JString;
use primer_core::error::{PrimerError, Result};

pub struct JniSpeechBridge {
    vm: &'static JavaVM,
}

fn jerr(e: impl std::fmt::Display) -> PrimerError {
    PrimerError::Speech(format!("android speech JNI: {e}"))
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
        let class = env
            .find_class("org/theprimer/gui/PrimerSpeech")
            .map_err(jerr)?;
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
}
