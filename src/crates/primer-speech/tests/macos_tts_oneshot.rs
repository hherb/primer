// Custom test harness for MacosTextToSpeech one-shot synthesis.
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

use primer_core::speech::{TextToSpeech, VoiceProfile};
use primer_speech::macos::MacosTextToSpeech;

fn main() {
    println!("running 1 test");
    println!("test synthesize_hello_returns_non_empty_audio ...");

    let result = std::panic::catch_unwind(|| {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");
        rt.block_on(async { synthesize_hello_returns_non_empty_audio().await })
    });

    match result {
        Ok(()) => {
            println!("test synthesize_hello_returns_non_empty_audio ... ok");
            println!("\ntest result: ok. 1 passed; 0 failed; 0 ignored;");
            std::process::exit(0);
        }
        Err(e) => {
            let msg = if let Some(s) = e.downcast_ref::<&str>() {
                s.to_string()
            } else if let Some(s) = e.downcast_ref::<String>() {
                s.clone()
            } else {
                "unknown panic".into()
            };
            eprintln!("test synthesize_hello_returns_non_empty_audio ... FAILED");
            eprintln!("  thread 'main' panicked at: {msg}");
            println!("\ntest result: FAILED. 0 passed; 1 failed; 0 ignored;");
            std::process::exit(101);
        }
    }
}

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
