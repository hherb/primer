use super::*;

/// Sanity: 48 kHz → 16 kHz resampling produces ~1/3 the sample count.
/// FFT buffering may cause the first call to return 0 samples; feed two
/// chunks to verify the rate is correct.
#[test]
fn resampler_48k_to_16k_reduces_sample_count_by_about_three() {
    let chunk = 1024;
    let mut r = Resampler::new(48_000, 16_000, chunk).expect("construct");
    let input = vec![0.0f32; chunk];
    // Prime the buffer with a dummy chunk.
    let _primed = r.process(&input).expect("process (prime)");
    // Second chunk; should contain real output.
    let out = r.process(&input).expect("process (main)");
    // Allow ±20% slack for FFT edge effects.
    let expected = chunk / 3;
    let lower = expected * 4 / 5;
    let upper = expected * 6 / 5;
    assert!(
        out.len() >= lower && out.len() <= upper,
        "expected ~{expected} samples, got {}",
        out.len()
    );
}

#[test]
fn resampler_rejects_wrong_input_length() {
    let mut r = Resampler::new(48_000, 16_000, 1024).expect("construct");
    let too_short = vec![0.0f32; 512];
    let err = r.process(&too_short).expect_err("should reject");
    assert!(
        matches!(err, PrimerError::Speech(_)),
        "expected PrimerError::Speech, got {err:?}"
    );
}

#[test]
fn push_mono_f32_sums_stereo_to_mono() {
    let rb = HeapRb::<f32>::new(64);
    let (mut prod, mut cons) = rb.split();
    // Stereo frames: (1.0, 0.5), (-0.5, 0.5)
    let stereo: Vec<f32> = vec![1.0, 0.5, -0.5, 0.5];
    push_mono_f32(&mut prod, &stereo, 2);
    let frame0 = cons.try_pop().expect("frame 0");
    let frame1 = cons.try_pop().expect("frame 1");
    assert!((frame0 - 0.75).abs() < 1e-6);
    assert!((frame1 - 0.0).abs() < 1e-6);
}

#[test]
fn push_mono_f32_passes_through_mono() {
    let rb = HeapRb::<f32>::new(64);
    let (mut prod, mut cons) = rb.split();
    let mono: Vec<f32> = vec![0.1, 0.2, 0.3];
    push_mono_f32(&mut prod, &mono, 1);
    assert!((cons.try_pop().unwrap() - 0.1).abs() < 1e-6);
    assert!((cons.try_pop().unwrap() - 0.2).abs() < 1e-6);
    assert!((cons.try_pop().unwrap() - 0.3).abs() < 1e-6);
}

#[test]
fn push_all_with_bail_writes_everything_when_capacity_is_ample() {
    let rb = HeapRb::<f32>::new(64);
    let (mut prod, mut cons) = rb.split();
    let errored = AtomicBool::new(false);
    let samples: Vec<f32> = (0..32).map(|i| i as f32).collect();

    let written = push_all_with_bail(&mut prod, &samples, &errored, Duration::ZERO);

    assert_eq!(written, samples.len());
    let mut drained: Vec<f32> = Vec::new();
    while let Some(s) = cons.try_pop() {
        drained.push(s);
    }
    assert_eq!(drained, samples);
}

#[test]
fn push_all_with_bail_returns_zero_when_already_errored() {
    let rb = HeapRb::<f32>::new(64);
    let (mut prod, _cons) = rb.split();
    let errored = AtomicBool::new(true);
    let samples = vec![0.5f32; 16];

    let written = push_all_with_bail(&mut prod, &samples, &errored, Duration::ZERO);

    assert_eq!(written, 0);
}

#[test]
fn push_all_with_bail_terminates_when_consumer_stalls_and_errored_flips() {
    // Capacity 4: first push fills the buffer, then push_slice
    // returns 0 forever (no consumer popping). Without the errored
    // bail this loop would spin indefinitely; with it, flipping the
    // flag mid-spin causes the function to return.
    let rb = HeapRb::<f32>::new(4);
    let (mut prod, _cons) = rb.split();
    let errored = Arc::new(AtomicBool::new(false));
    let samples = vec![0.25f32; 16];

    let flag = Arc::clone(&errored);
    let flipper = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(20));
        flag.store(true, Ordering::Release);
    });

    // Use a small non-zero retry sleep so the produce loop yields
    // rather than spinning hot while we wait for the flipper.
    let written = push_all_with_bail(&mut prod, &samples, &errored, Duration::from_millis(2));
    flipper.join().unwrap();

    // Wrote at least one sample, then bailed once the flag flipped —
    // could not have written everything because the consumer never
    // drained. Range-bounded rather than `== 4` so we don't pin to
    // ringbuf bulk-push semantics.
    assert!(
        (1..samples.len()).contains(&written),
        "expected partial write (1..{}); got {written}",
        samples.len()
    );
}

#[test]
fn wait_for_drain_returns_true_immediately_when_buffer_empty() {
    let rb = HeapRb::<f32>::new(64);
    let (prod, _cons) = rb.split();
    let errored = AtomicBool::new(false);

    let drained = wait_for_drain(
        &prod,
        &errored,
        Duration::from_millis(1),
        3,
        Duration::ZERO,
        Duration::from_secs(1),
    );

    assert!(drained, "empty buffer should report drained");
}

#[test]
fn wait_for_drain_waits_until_consumer_empties_buffer() {
    use std::sync::Arc;
    let rb = HeapRb::<f32>::new(64);
    let (mut prod, mut cons) = rb.split();
    let errored = Arc::new(AtomicBool::new(false));

    // Pre-fill the buffer.
    let samples = vec![0.5f32; 32];
    let _written = prod.push_slice(&samples);
    assert!(prod.occupied_len() > 0);

    // Spawn a consumer that drains the buffer after a short delay.
    let drainer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(15));
        while cons.try_pop().is_some() {}
        cons // hand back so it isn't dropped (which would close the channel)
    });

    let drained = wait_for_drain(
        &prod,
        &errored,
        Duration::from_millis(2),
        3,
        Duration::ZERO,
        Duration::from_secs(1),
    );
    let _cons = drainer.join().unwrap();

    assert!(drained, "should observe drain after consumer empties");
}

#[test]
fn wait_for_drain_returns_false_when_errored() {
    use std::sync::Arc;
    let rb = HeapRb::<f32>::new(64);
    let (mut prod, _cons) = rb.split();
    let errored = Arc::new(AtomicBool::new(false));

    // Fill the buffer so it never drains naturally (no consumer).
    let samples = vec![0.5f32; 32];
    let _written = prod.push_slice(&samples);

    let flag = Arc::clone(&errored);
    let flipper = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(20));
        flag.store(true, Ordering::Release);
    });

    let drained = wait_for_drain(
        &prod,
        &errored,
        Duration::from_millis(2),
        3,
        Duration::ZERO,
        Duration::from_secs(2),
    );
    flipper.join().unwrap();

    assert!(!drained, "should report not-drained when errored mid-wait");
}

#[test]
fn wait_for_drain_returns_false_on_max_wait_timeout() {
    let rb = HeapRb::<f32>::new(64);
    let (mut prod, _cons) = rb.split();
    let errored = AtomicBool::new(false);

    // Fill the buffer; never drained.
    let samples = vec![0.5f32; 32];
    let _written = prod.push_slice(&samples);

    let drained = wait_for_drain(
        &prod,
        &errored,
        Duration::from_millis(2),
        3,
        Duration::ZERO,
        Duration::from_millis(20),
    );

    assert!(!drained, "should time out when nothing drains the buffer");
}

#[test]
fn pull_to_device_f32_underruns_to_silence() {
    let rb = HeapRb::<f32>::new(8);
    let (mut prod, mut cons) = rb.split();
    let _ = prod.try_push(0.5);
    let mut out = vec![99.0f32; 6]; // 3 stereo frames
    pull_to_device_f32(&mut cons, &mut out, 2);
    // First frame: 0.5 / 0.5 (stereo dup); rest: silence (0.0).
    assert!((out[0] - 0.5).abs() < 1e-6);
    assert!((out[1] - 0.5).abs() < 1e-6);
    assert!((out[2] - 0.0).abs() < 1e-6);
    assert!((out[5] - 0.0).abs() < 1e-6);
}

/// Hardware smoke: capture 1 second of mic, immediately play it back to
/// the speaker. Manual sanity for cpal device defaults; never in CI.
///
/// Run with: `cargo test -p primer-speech --features cpal --release -- --ignored hardware_loopback`
#[test]
#[ignore = "needs mic + speaker; mac asks for mic permission on first run"]
fn hardware_loopback_one_second() {
    // 1 s of capture
    let (mic, mut mic_cons) = MicCapture::start().expect("mic");
    let mic_rate = mic.sample_rate;
    let mut buf: Vec<f32> = Vec::with_capacity(mic_rate as usize);
    let started = std::time::Instant::now();
    while started.elapsed() < std::time::Duration::from_secs(1) {
        while let Some(s) = mic_cons.try_pop() {
            buf.push(s);
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    drop(mic);

    // Open speaker; resample mic audio if needed; push.
    let (spk, mut spk_prod) = SpeakerSink::start().expect("spk");
    let spk_rate = spk.sample_rate;

    let to_play: Vec<f32> = if mic_rate == spk_rate {
        buf
    } else {
        let chunk = 1024;
        let mut r = Resampler::new(mic_rate, spk_rate, chunk).expect("resampler");
        let mut out = Vec::new();
        for window in buf.chunks(chunk) {
            if window.len() != chunk {
                break;
            }
            out.extend(r.process(window).expect("process"));
        }
        out
    };

    use ringbuf::traits::Producer;
    let mut written = 0;
    while written < to_play.len() {
        written += spk_prod.push_slice(&to_play[written..]);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    // Let the device drain.
    std::thread::sleep(std::time::Duration::from_millis(500));
    drop(spk);
}
