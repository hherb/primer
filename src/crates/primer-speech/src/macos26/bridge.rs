//! swift-bridge declaration of the Swift sidecar `Macos26Pipeline`.
//! Compiled by build.rs alongside the Swift sources.

#![cfg(all(target_vendor = "apple", feature = "macos-native-26"))]

#[swift_bridge::bridge]
pub(crate) mod ffi {
    // Swift-bridge 0.1.x does not support Option<SharedStruct> as an async
    // return type. We add a `stream_done` sentinel field so Rust callers
    // can detect end-of-stream without needing Option wrapping.
    //
    // IMPORTANT: This shared struct intentionally contains NO `String`
    // field. swift-bridge 0.1.x has a heap-allocator-mismatch bug where
    // returning an async shared struct that contains a `String` field
    // crashes with libmalloc reporting "POINTER_BEING_FREED_WAS_NOT_ALLOCATED"
    // when Rust drops the intermediate Box. The text is fetched via the
    // separate sync `last_result_text()` accessor after each await.
    #[swift_bridge(swift_repr = "struct")]
    struct ResultEvent {
        is_final: bool,
        range_start_ms: u64,
        range_end_ms: u64,
        stream_done: bool,
    }

    extern "Swift" {
        type Macos26Pipeline;

        // Async factory. Returns the pipeline or panics on failure.
        // The Rust wrapper (`new_checked`) wraps the ffi call with error handling.
        //
        // Design note: we avoid Option/Result as return types here because
        // swift-bridge 0.1.59 has unimplemented codegen for
        // Option<OpaqueSwiftType> and Result<OpaqueSwiftType, ...> in
        // extern "Swift" function positions. The Swift side fatalErrors on
        // failure since pipeline creation errors (locale unavailable, etc.)
        // are surfaced before voice mode is activated.
        #[swift_bridge(swift_name = "macos26PipelineCreate")]
        async fn create(locale_bcp47: String) -> Macos26Pipeline;

        #[swift_bridge(swift_name = "analyzerSampleRate")]
        fn analyzer_sample_rate(&self) -> f64;

        #[swift_bridge(swift_name = "feedAudio")]
        fn feed_audio(&mut self, samples: Vec<f32>);

        // Maps to `nextResultBridge()` on the Swift side. Returns a ResultEvent
        // with stream_done=true as a sentinel for end-of-stream or error.
        // The accompanying transcript text is cached on the Swift side and
        // must be fetched via `last_result_text()` after this returns.
        #[swift_bridge(swift_name = "nextResultBridge")]
        async fn next_result(&mut self) -> ResultEvent;

        // Returns the text of the most-recently-yielded result. Sync to
        // avoid the swift-bridge 0.1.x async-shared-struct-String bug.
        // Callers must invoke this between successive next_result awaits
        // (each next_result overwrites the cached text).
        #[swift_bridge(swift_name = "lastResultText")]
        fn last_result_text(&self) -> String;

        async fn stop(&mut self);
    }
}

pub(crate) use ffi::Macos26Pipeline;
// ResultEvent is re-exported for use by the pipeline consumer module (not yet written).
#[allow(unused_imports)]
pub(crate) use ffi::ResultEvent;

// The generated Macos26Pipeline wrapper is a newtype over *mut c_void.
// Raw pointers are not Send by default, but the Swift object behind the
// pointer is an ARC-managed object — safe to move across threads as long
// as we serialise access. The voice loop drives this from a single tokio
// task, so the Send impl is sound.
unsafe impl Send for Macos26Pipeline {}
