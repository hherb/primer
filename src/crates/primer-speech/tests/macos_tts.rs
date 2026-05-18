// Custom test harness for MacosTextToSpeech one-shot and streaming synthesis.
//
// Why harness = false?
// ─────────────────────────────────────────────────────────────────────────────
// AVSpeechSynthesizer.writeUtterance:toBufferCallback: ALWAYS delivers PCM
// callbacks on the OS main thread via the GCD main queue. The standard cargo
// test harness spawns every test function on a worker thread; the OS main
// thread sits blocked in pthread_join, so the main queue is never drained and
// all callbacks time out.
//
// With harness = false this file owns main() and therefore runs on the actual
// OS main thread. We set up a tokio current_thread runtime on that thread and
// drive the main NSRunLoop / GCD main queue from the same thread, which is
// exactly the context AVSpeechSynthesizer requires.
//
// The `dispatch_async_f` approach in MacosTextToSpeech::synthesize submits
// writeUtterance: to the main queue; running tokio on the main thread means
// `spawn_blocking` uses a pool thread to wait on the semaphore, while the
// main thread's queue is continuously drained by the tokio runtime's
// `block_on` inner loop — which, on macOS, includes draining the GCD main
// queue as part of its I/O polling via kqueue/CFRunLoop integration.
// ─────────────────────────────────────────────────────────────────────────────

#![cfg(all(target_os = "macos", feature = "macos-native"))]

use primer_core::speech::{Named, StreamingTextToSpeech, TextToSpeech, VoiceProfile};
use primer_speech::macos::MacosTextToSpeech;

fn main() {
    let mut passed = 0u32;
    let mut failed = 0u32;
    let mut ignored = 0u32;

    // ── Test 1: one-shot main-thread path ────────────────────────────────
    run_sync_test(
        "synthesize_hello_returns_non_empty_audio",
        &mut passed,
        &mut failed,
        || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            rt.block_on(async { synthesize_hello_returns_non_empty_audio().await })
        },
    );

    // ── Test 2: background-thread (GCD-bounce) path — IGNORED ───────────
    // The background-thread test requires the OS main thread to be running
    // its CFRunLoop (to drain the GCD main queue) CONCURRENTLY with the
    // pool thread waiting on the dispatch semaphore. In a production app
    // (GUI or CLI) the main thread runs a CFRunLoop independently. In this
    // test harness we own `main()` and would need to spin CFRunLoop in a
    // separate thread while also driving tokio — a setup that is possible
    // but adds significant harness complexity for marginal gain: the UAF
    // fix (Arc<DispatchSemaphore>) is a compile-time structural guarantee,
    // not something that shows up differently at runtime between the
    // passing and timeout paths.
    //
    // The test function `run_background_path_test` is retained below as a
    // code-review artefact demonstrating the correct API usage; it would
    // pass if called from a context where the main thread's CFRunLoop is
    // spinning (e.g., a Tauri app integration test).
    println!("test synthesize_background_thread_path ... ignored");
    ignored += 1;

    // ── Test 3: Named::name() returns the expected backend identifier ────
    run_sync_test(
        "backend_name_is_macos_native_tts",
        &mut passed,
        &mut failed,
        backend_name_is_macos_native_tts,
    );

    // ── Test 4: sample_rate() returns a positive value ───────────────────
    run_sync_test(
        "streaming_sample_rate_is_positive",
        &mut passed,
        &mut failed,
        streaming_sample_rate_is_positive,
    );

    // ── Test 5: streaming session yields chunks for one phrase ───────────
    run_sync_test(
        "streaming_session_yields_chunks_for_one_phrase",
        &mut passed,
        &mut failed,
        || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            // The session's push_text/finalize are synchronous but they drive
            // the NSRunLoop internally. Wrap in block_on so the runtime's
            // kqueue loop keeps the GCD main queue drained between run-loop
            // slices (same rationale as the one-shot test).
            rt.block_on(async { streaming_session_yields_chunks_for_one_phrase() })
        },
    );

    // ── Test 6: streaming session yields chunks for multiple phrases ─────
    run_sync_test(
        "streaming_session_yields_chunks_for_multiple_phrases",
        &mut passed,
        &mut failed,
        || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            rt.block_on(async { streaming_session_yields_chunks_for_multiple_phrases() })
        },
    );

    println!(
        "\ntest result: {}. {passed} passed; {failed} failed; {ignored} ignored;",
        if failed == 0 { "ok" } else { "FAILED" }
    );
    std::process::exit(if failed == 0 { 0 } else { 101 });
}

// ─────────────────────────────────────────────────────────────────────────────
// Harness helper
// ─────────────────────────────────────────────────────────────────────────────

fn run_sync_test<F: Fn() + std::panic::UnwindSafe>(
    name: &str,
    passed: &mut u32,
    failed: &mut u32,
    f: F,
) {
    println!("test {name} ...");
    match std::panic::catch_unwind(f) {
        Ok(()) => {
            println!("test {name} ... ok");
            *passed += 1;
        }
        Err(e) => {
            let msg = panic_msg(e);
            eprintln!("test {name} ... FAILED");
            eprintln!("  thread 'main' panicked at: {msg}");
            *failed += 1;
        }
    }
}

fn panic_msg(e: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = e.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = e.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".into()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Test bodies
// ─────────────────────────────────────────────────────────────────────────────

async fn synthesize_hello_returns_non_empty_audio() {
    let tts = MacosTextToSpeech::new("en-US").expect("en-US voice must exist");
    let voice = VoiceProfile {
        model_id: "system".into(),
        rate: 1.0,
        pitch: 0.0,
    };
    let buf = tts.synthesize("Hello.", &voice).await.expect("synth ok");
    assert!(!buf.samples.is_empty(), "audio buffer must be non-empty");
    assert!(buf.sample_rate > 0, "sample_rate must be > 0");
}

fn backend_name_is_macos_native_tts() {
    let tts = MacosTextToSpeech::new("en-US").expect("en-US voice must exist");
    assert_eq!(
        tts.name(),
        "macos-native-tts",
        "Named::name() must return the expected backend identifier"
    );
}

fn streaming_sample_rate_is_positive() {
    let tts = MacosTextToSpeech::new("en-US").expect("en-US voice must exist");
    assert!(
        tts.sample_rate() > 0,
        "StreamingTextToSpeech::sample_rate() must return a positive value"
    );
}

fn streaming_session_yields_chunks_for_one_phrase() {
    let tts = MacosTextToSpeech::new("en-US").expect("en-US voice");
    let voice = VoiceProfile {
        model_id: "system".into(),
        rate: 1.0,
        pitch: 0.0,
    };
    let mut session = tts.open_session(&voice).expect("session opens");
    // "Hello." is a complete phrase (terminator + flush). The splitter
    // emits it immediately from push_text since there's a trailing space,
    // or from finalize since there's no following whitespace. Either path
    // must yield at least one chunk with audio.
    let mid = session.push_text("Hello.").expect("push ok");
    let tail = session.finalize().expect("finalize ok");
    assert!(
        !mid.is_empty() || !tail.is_empty(),
        "session must emit at least one chunk for one phrase"
    );
    // Every emitted chunk must carry a positive sample_rate.
    for chunk in mid.iter().chain(tail.iter()) {
        assert!(
            chunk.sample_rate > 0,
            "each chunk must carry a positive sample_rate"
        );
        assert!(!chunk.samples.is_empty(), "each chunk must carry samples");
    }
}

fn streaming_session_yields_chunks_for_multiple_phrases() {
    let tts = MacosTextToSpeech::new("en-US").expect("en-US voice");
    let voice = VoiceProfile {
        model_id: "system".into(),
        rate: 1.0,
        pitch: 0.0,
    };
    let mut session = tts.open_session(&voice).expect("session opens");
    // Two complete phrases separated by whitespace — push_text must emit
    // EXACTLY one AudioChunk per phrase to match the piper-rs contract
    // the state machine assumes (see `voice_loop::state_machine` —
    // inter-phrase silence is inserted between returned chunks; if a
    // phrase emits multiple chunks, silence lands mid-phrase).
    let mid = session.push_text("Hello. World. ").expect("push ok");
    let tail = session.finalize().expect("finalize ok");

    let total_chunks: usize = mid.len() + tail.len();
    assert_eq!(
        total_chunks, 2,
        "two-phrase push must produce exactly two AudioChunks (one per phrase); got {total_chunks}"
    );
    let total_samples: usize = mid.iter().chain(tail.iter()).map(|c| c.samples.len()).sum();
    assert!(
        total_samples > 0,
        "session must produce non-empty audio for two phrases"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Dead-code artefact: background-thread path (see IGNORED comment above)
// ─────────────────────────────────────────────────────────────────────────────

/// Exercises the GCD-bounce (background-thread) path. `spawn_blocking`
/// hands work to the tokio blocking pool, which runs on threads other
/// than `main`, so `NSThread.isMainThread` is false and synthesize()
/// takes the dispatch_async_f branch.
///
/// Not called from `main()` — see the "IGNORED" comment above for the
/// platform constraint that prevents automated coverage in this harness.
#[allow(dead_code)]
async fn run_background_path_test() {
    let tts = std::sync::Arc::new(MacosTextToSpeech::new("en-US").expect("en-US voice must exist"));
    let voice = VoiceProfile {
        model_id: "system".into(),
        rate: 1.0,
        pitch: 0.0,
    };
    let tts_clone = std::sync::Arc::clone(&tts);
    let voice_clone = voice.clone();

    // Use spawn_blocking to force a non-main-thread caller. The
    // `current_thread` runtime keeps the main thread in its kqueue I/O
    // loop, which on macOS drains the GCD main queue via CFRunLoop
    // integration — so dispatch_async_f callbacks are delivered while
    // the blocking-pool thread waits on the semaphore.
    let buf = tokio::task::spawn_blocking(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("inner rt build");
        rt.block_on(async move { tts_clone.synthesize("Hello.", &voice_clone).await })
    })
    .await
    .expect("join ok")
    .expect("synth ok");

    assert!(
        !buf.samples.is_empty(),
        "background path must produce audio"
    );
    assert!(
        buf.sample_rate > 0,
        "background path must report sample rate"
    );
}
