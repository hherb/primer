//! Pure-logic tests for [`super::make_on_audio`] — these don't open a real
//! cpal device, just a vanilla `ringbuf::HeapRb` so the closure body's
//! contract (no-op on flush w/o resampling, pushes everything straight
//! through w/o resampling, bails when `spk_errored` is set) is pinned
//! without an audio-device dep. The resampling branch is exercised by the
//! audio-quality smoke at `examples/tts_hello.rs` since a meaningful unit
//! test would just re-test rubato.

use super::*;
use ringbuf::HeapRb;
use ringbuf::traits::{Consumer as _, Observer as _, Split as _};
use std::sync::atomic::Ordering;

/// Bench-style fixture for the [`super::make_on_audio`] tests. Owning the
/// consumer alongside the producer lets each test verify the closure's
/// push behaviour by inspecting the ringbuf state directly.
struct Fixture {
    spk_prod: Arc<Mutex<HeapProd<f32>>>,
    spk_cons: ringbuf::HeapCons<f32>,
    spk_errored: Arc<AtomicBool>,
    output_resampler: Arc<Mutex<Option<Resampler>>>,
}

fn setup() -> Fixture {
    let rb = HeapRb::<f32>::new(4096);
    let (prod, cons) = rb.split();
    Fixture {
        spk_prod: Arc::new(Mutex::new(prod)),
        spk_cons: cons,
        spk_errored: Arc::new(AtomicBool::new(false)),
        output_resampler: Arc::new(Mutex::new(None)),
    }
}

#[test]
fn make_on_audio_without_resampling_pushes_samples_through() {
    let Fixture {
        spk_prod,
        mut spk_cons,
        spk_errored,
        output_resampler,
    } = setup();
    let mut closure = make_on_audio(
        spk_prod,
        spk_errored,
        output_resampler,
        OUTPUT_RESAMPLER_CHUNK_IN,
        /* need_output_resample */ false,
    );

    let samples: Vec<f32> = (0..16).map(|i| i as f32 * 0.1).collect();
    closure(samples.clone());

    let mut received = vec![0.0_f32; samples.len()];
    let n = spk_cons.pop_slice(&mut received);
    assert_eq!(n, samples.len(), "all input samples reach the speaker");
    assert_eq!(received, samples, "samples flow through verbatim");
}

#[test]
fn make_on_audio_empty_flush_without_resampling_is_a_noop() {
    let Fixture {
        spk_prod,
        spk_cons,
        spk_errored,
        output_resampler,
    } = setup();
    let mut closure = make_on_audio(
        spk_prod,
        spk_errored,
        output_resampler,
        OUTPUT_RESAMPLER_CHUNK_IN,
        /* need_output_resample */ false,
    );

    // End-of-turn flush sentinel: empty Vec, no resampling. With
    // no resampler tail to drain, this must not produce any output.
    closure(Vec::new());

    assert_eq!(
        spk_cons.occupied_len(),
        0,
        "flush w/o resampling must not push samples"
    );
}

#[test]
fn make_on_audio_bails_when_speaker_errored_flag_is_set_before_call() {
    let Fixture {
        spk_prod,
        spk_cons,
        spk_errored,
        output_resampler,
    } = setup();
    spk_errored.store(true, Ordering::SeqCst);

    let mut closure = make_on_audio(
        spk_prod,
        Arc::clone(&spk_errored),
        output_resampler,
        OUTPUT_RESAMPLER_CHUNK_IN,
        /* need_output_resample */ false,
    );

    let samples: Vec<f32> = (0..32).map(|i| i as f32).collect();
    closure(samples);

    // push_all_with_bail bails on the very first iteration when
    // errored is already set — nothing reaches the ringbuf.
    assert_eq!(
        spk_cons.occupied_len(),
        0,
        "bail before pushing when errored is already set"
    );
}
