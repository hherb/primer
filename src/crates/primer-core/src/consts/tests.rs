#[test]
fn inference_llamacpp_consts_are_sane() {
    use super::inference::*;
    assert_eq!(LLAMACPP_GPU_LAYERS_ALL, -1);
    assert_eq!(LLAMACPP_GPU_LAYERS_CPU, 0);
    // 0 = "use the model's trained context length" (llama.cpp convention).
    assert_eq!(LLAMACPP_DEFAULT_N_CTX, 0);
    // A fixed seed keeps a given prompt reproducible across runs.
    assert_eq!(LLAMACPP_DEFAULT_SAMPLER_SEED, 1234);
}

#[test]
#[allow(clippy::assertions_on_constants)]
fn router_consts_are_sane() {
    use super::router::*;
    // Threshold sits above the heaviest single-intent weight (Scaffolding,
    // 0.45) by design: in `hybrid` mode no intent ALONE routes to the
    // cloud — a heavy intent must combine with at least one more signal (a
    // retrieved passage or a long/multi-question message) to cross it. This
    // is the privacy-preferring default; fewer turns leave the device.
    assert!(ROUTE_SECONDARY_THRESHOLD > 0.0 && ROUTE_SECONDARY_THRESHOLD < 1.0);
    assert_eq!(ROUTE_PASSAGE_CAP, 3);
    assert!(W_PASSAGE > 0.0);
    assert_eq!(MSG_LONG_WORDS, 30);
    assert!(W_MSG_LONG > 0.0);
    assert!(W_MSG_QUESTION > 0.0);
    assert_eq!(MSG_QUESTION_CAP, 2);
    // Latency is a NUDGE, not a circuit-breaker: W_LATENCY alone must not
    // cross the secondary threshold, so a trivial turn (base score 0.0)
    // stays local even when the local leg is slow — keeping routine turns
    // sampling the local TTFT so its EMA self-heals. Tuning W_LATENCY up to
    // or past the threshold would silently break that (every slow turn
    // would escalate regardless of complexity), so pin it here.
    assert!(
        W_LATENCY > 0.0 && W_LATENCY < ROUTE_SECONDARY_THRESHOLD,
        "W_LATENCY must be in (0, ROUTE_SECONDARY_THRESHOLD) — nudge invariant"
    );
    // EMA smoothing factor must be a valid weight in (0, 1].
    assert!(TTFT_EMA_ALPHA > 0.0 && TTFT_EMA_ALPHA <= 1.0);
}
