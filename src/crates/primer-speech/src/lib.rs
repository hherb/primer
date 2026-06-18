//! # primer-speech
//!
//! Speech backend implementations.
//!
//! For Phase 0, speech I/O can be bypassed entirely — the CLI binary
//! accepts text input and produces text output. Speech integration
//! happens in Phase 1 when hardware audio is connected.

#[cfg(all(feature = "macos-native", feature = "macos-native-26"))]
compile_error!(
    "`macos-native` and `macos-native-26` are mutually exclusive — pick one \
     (`macos-native-26` for macOS 26+, `macos-native` for older macOS)"
);

#[cfg(all(
    feature = "android-native",
    any(feature = "macos-native", feature = "macos-native-26")
))]
compile_error!(
    "android-native is mutually exclusive with macos-native / macos-native-26; \
     pick one via --features"
);

pub mod locale_defaults;
pub mod phrase_split;
pub mod stub;
pub mod time_ms;
pub mod vad_debounce;

pub use phrase_split::PhraseSplitter;
pub use stub::{StubStt, StubTts};
pub use time_ms::clamp_signed_ms_to_u64;
pub use vad_debounce::{VadDebouncer, ms_to_chunks};

#[cfg(feature = "silero")]
pub mod silero;
#[cfg(feature = "silero")]
pub use silero::{SileroVad, SileroVadParams};

#[cfg(feature = "whisper")]
pub mod whisper;
#[cfg(feature = "whisper")]
pub use whisper::WhisperStt;

#[cfg(feature = "piper")]
pub mod piper;
#[cfg(feature = "piper")]
pub mod piper_config;
#[cfg(feature = "piper")]
pub use piper::PiperTts;

#[cfg(feature = "supertonic")]
pub mod supertonic;
#[cfg(feature = "supertonic")]
pub use supertonic::SupertonicTts;

#[cfg(feature = "cpal")]
pub mod cpal_io;
#[cfg(feature = "cpal")]
pub use cpal_io::{MicCapture, Resampler, SpeakerSink, push_all_with_bail, wait_for_drain};

#[cfg(feature = "voice-loop")]
pub mod voice_loop;

#[cfg(all(
    target_os = "macos",
    any(feature = "macos-native", feature = "macos-native-26")
))]
pub mod macos;

// `macos26` is gated on `target_os = "macos"` (not the aspirational
// `target_vendor = "apple"`) because today the module is genuinely
// macOS-only: build.rs hardcodes the `apple-macos26.0` Swift target
// triple, and the TTS surface re-exports `crate::macos::MacosTextToSpeech`
// which is itself macOS-gated. The internal files keep
// `target_vendor = "apple"` cfg gates as structural preparation for an
// eventual iOS-26 host (audio_session.rs already cfg-splits between the
// macOS no-op and the iOS placeholder), but the parent gate is the
// load-bearing one and must reflect actual build support.
#[cfg(all(target_os = "macos", feature = "macos-native-26"))]
pub mod macos26;

#[cfg(feature = "android-native")]
pub mod android;

/// JNI entry point cached at app startup. Kotlin declares this as
/// `external fun nativeInit()` on `PrimerSpeech` and calls it from
/// `MainActivity.onCreate`. We capture the `JavaVM` from the provided
/// `JNIEnv` and stash it for every later JNI bridge to reuse — the fix for
/// Plan 1's `ndk_context` blocker (the Tauri-mobile runtime does not
/// populate `ndk_context` for our call path).
///
/// The symbol name MUST be `Java_<pkg>_<Class>_nativeInit` with `.`/`/`
/// replaced by `_`; here `org.theprimer.gui.PrimerSpeech` produces the
/// name below. A mismatch is an `UnsatisfiedLinkError` at the Kotlin call
/// site. Device-verified (Plan 2 Task 10).
#[cfg(all(target_os = "android", feature = "android-native"))]
#[unsafe(no_mangle)]
pub extern "system" fn Java_org_theprimer_gui_PrimerSpeech_nativeInit(
    env: jni::JNIEnv,
    _class: jni::objects::JClass,
) {
    match env.get_java_vm() {
        Ok(vm) => {
            crate::android::vm::set_java_vm(vm);
            tracing::info!(target: "primer::speech::android", "nativeInit: JavaVM cached");
        }
        Err(e) => {
            tracing::error!(
                target: "primer::speech::android",
                "nativeInit: get_java_vm failed: {e}"
            );
        }
    }
}
