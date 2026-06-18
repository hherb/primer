//! Real `AndroidSpeechBridge` over `jni`. Device-only (compiled only for
//! target_os = "android"). Runtime behaviour is validated on a physical
//! device; this module only needs to cross-compile here.

use crate::android::bridge::AndroidSpeechBridge;
use crate::android::capabilities::SpeechCapabilities;
use jni::objects::JString;
use jni::JavaVM;
use primer_core::error::{PrimerError, Result};

pub struct JniSpeechBridge {
    vm: JavaVM,
}

fn jerr(e: impl std::fmt::Display) -> PrimerError {
    PrimerError::Speech(format!("android speech JNI: {e}"))
}

impl JniSpeechBridge {
    pub fn new() -> Result<Self> {
        // ndk_context is populated by the Tauri-mobile runtime. If the
        // on-device gate shows it is NOT, the documented fallback is a
        // `nativeInit` JNI export caching the JavaVM — see the plan's Risks.
        let ctx = ndk_context::android_context();
        // SAFETY: ndk_context guarantees the pointer is a valid *mut JavaVM
        // for the lifetime of the Android process. We wrap it immediately and
        // never store the raw pointer.
        let vm = unsafe { JavaVM::from_raw(ctx.vm().cast()) }.map_err(jerr)?;
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
            .call_static_method(
                &class,
                "queryCapabilities",
                "()Ljava/lang/String;",
                &[],
            )
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
