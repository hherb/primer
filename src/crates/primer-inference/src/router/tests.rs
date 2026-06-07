use super::*;
use futures::{StreamExt, stream};
use primer_core::conversation::PedagogicalIntent;
use primer_core::error::PrimerError;
use primer_core::inference::TokenChunk;
use primer_core::router::RoutingSignals;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone)]
enum Behavior {
    Ok(String),
    PreStreamErr,
    MidStreamErr,
}

struct MockBackend {
    name: String,
    calls: Arc<AtomicUsize>,
    behavior: Behavior,
}

impl MockBackend {
    fn new(name: &str, behavior: Behavior) -> (Arc<Self>, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let b = Arc::new(Self {
            name: name.to_string(),
            calls: calls.clone(),
            behavior,
        });
        (b, calls)
    }
}

#[async_trait]
impl InferenceBackend for MockBackend {
    fn name(&self) -> &str {
        &self.name
    }
    async fn is_available(&self) -> bool {
        !matches!(self.behavior, Behavior::PreStreamErr)
    }
    async fn generate_stream(
        &self,
        _prompt: &Prompt,
        _params: &GenerationParams,
    ) -> Result<TokenStream> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match &self.behavior {
            Behavior::Ok(text) => {
                let chunk = TokenChunk {
                    text: text.clone(),
                    done: true,
                };
                Ok(Box::pin(stream::once(async move { Ok(chunk) })))
            }
            Behavior::PreStreamErr => Err(PrimerError::Inference("primary down".into())),
            Behavior::MidStreamErr => Ok(Box::pin(stream::once(async {
                Err(PrimerError::Inference("mid-stream boom".into()))
            }))),
        }
    }
}

fn prompt() -> Prompt {
    Prompt {
        system: String::new(),
        messages: vec![],
    }
}

fn params_with(intent: PedagogicalIntent, passages: usize) -> GenerationParams {
    GenerationParams {
        routing: Some(RoutingSignals {
            intent,
            retrieved_passages: passages,
        }),
        ..GenerationParams::default()
    }
}

async fn drive(mut s: TokenStream) -> Result<String> {
    let mut out = String::new();
    while let Some(item) = s.next().await {
        let chunk = item?;
        out.push_str(&chunk.text);
        if chunk.done {
            break;
        }
    }
    Ok(out)
}

#[tokio::test]
async fn hybrid_high_score_routes_to_secondary() {
    let (primary, pcalls) = MockBackend::new("llamacpp:m", Behavior::Ok("LOCAL".into()));
    let (secondary, scalls) = MockBackend::new("cloud", Behavior::Ok("CLOUD".into()));
    let r = RouterBackend::new(primary, secondary, RouterMode::Hybrid);
    let out = drive(
        r.generate_stream(&prompt(), &params_with(PedagogicalIntent::Scaffolding, 3))
            .await
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(out, "CLOUD");
    assert_eq!(scalls.load(Ordering::SeqCst), 1);
    assert_eq!(pcalls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn hybrid_low_score_routes_to_primary() {
    let (primary, pcalls) = MockBackend::new("llamacpp:m", Behavior::Ok("LOCAL".into()));
    let (secondary, scalls) = MockBackend::new("cloud", Behavior::Ok("CLOUD".into()));
    let r = RouterBackend::new(primary, secondary, RouterMode::Hybrid);
    let out = drive(
        r.generate_stream(&prompt(), &params_with(PedagogicalIntent::Encouragement, 0))
            .await
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(out, "LOCAL");
    assert_eq!(pcalls.load(Ordering::SeqCst), 1);
    assert_eq!(scalls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn cloud_preferred_always_tries_secondary_first() {
    let (primary, pcalls) = MockBackend::new("llamacpp:m", Behavior::Ok("LOCAL".into()));
    let (secondary, scalls) = MockBackend::new("cloud", Behavior::Ok("CLOUD".into()));
    let r = RouterBackend::new(primary, secondary, RouterMode::CloudPreferred);
    let out = drive(
        r.generate_stream(&prompt(), &params_with(PedagogicalIntent::Encouragement, 0))
            .await
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(out, "CLOUD");
    assert_eq!(scalls.load(Ordering::SeqCst), 1);
    assert_eq!(pcalls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn pre_stream_failure_falls_over_to_other_leg() {
    let (primary, pcalls) = MockBackend::new("llamacpp:m", Behavior::Ok("LOCAL".into()));
    let (secondary, scalls) = MockBackend::new("cloud", Behavior::PreStreamErr);
    let r = RouterBackend::new(primary, secondary, RouterMode::Hybrid);
    let out = drive(
        r.generate_stream(&prompt(), &params_with(PedagogicalIntent::Scaffolding, 3))
            .await
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(out, "LOCAL");
    assert_eq!(
        scalls.load(Ordering::SeqCst),
        1,
        "secondary attempted first"
    );
    assert_eq!(
        pcalls.load(Ordering::SeqCst),
        1,
        "primary served the fallover"
    );
    assert!(
        r.ttft_ema_for_test().is_some(),
        "primary serving the fallover is still timed"
    );
}

#[tokio::test]
async fn mid_stream_error_propagates_without_reroute() {
    let (primary, pcalls) = MockBackend::new("llamacpp:m", Behavior::MidStreamErr);
    let (secondary, scalls) = MockBackend::new("cloud", Behavior::Ok("CLOUD".into()));
    let r = RouterBackend::new(primary, secondary, RouterMode::Hybrid);
    let stream = r
        .generate_stream(&prompt(), &params_with(PedagogicalIntent::Encouragement, 0))
        .await
        .unwrap();
    assert!(drive(stream).await.is_err());
    assert_eq!(pcalls.load(Ordering::SeqCst), 1);
    assert_eq!(scalls.load(Ordering::SeqCst), 0, "no mid-stream reroute");
}

#[tokio::test]
async fn name_returns_primary_name() {
    let (primary, _) = MockBackend::new("qnn:Qwen3-4B", Behavior::Ok("x".into()));
    let (secondary, _) = MockBackend::new("cloud", Behavior::Ok("y".into()));
    let r = RouterBackend::new(primary, secondary, RouterMode::Hybrid);
    assert_eq!(r.name(), "qnn:Qwen3-4B");
}

#[tokio::test]
async fn missing_routing_signals_scores_zero_and_uses_primary_in_hybrid() {
    let (primary, pcalls) = MockBackend::new("llamacpp:m", Behavior::Ok("LOCAL".into()));
    let (secondary, scalls) = MockBackend::new("cloud", Behavior::Ok("CLOUD".into()));
    let r = RouterBackend::new(primary, secondary, RouterMode::Hybrid);
    let out = drive(
        r.generate_stream(&prompt(), &GenerationParams::default())
            .await
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(out, "LOCAL");
    assert_eq!(pcalls.load(Ordering::SeqCst), 1);
    assert_eq!(scalls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn budget_none_is_byte_identical_to_today() {
    // A routine turn (low base score) with NO budget stays on primary even
    // if we seed a huge EMA — latency routing is inert without a budget.
    let (primary, pcalls) = MockBackend::new("llamacpp:m", Behavior::Ok("LOCAL".into()));
    let (secondary, scalls) = MockBackend::new("cloud", Behavior::Ok("CLOUD".into()));
    let r = RouterBackend::with_ttft_ema_for_test(
        primary,
        secondary,
        RouterMode::Hybrid,
        None,           // no budget
        Some(99_999.0), // pretend local is extremely slow
    );
    let out = drive(
        r.generate_stream(&prompt(), &params_with(PedagogicalIntent::Encouragement, 0))
            .await
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(out, "LOCAL");
    assert_eq!(pcalls.load(Ordering::SeqCst), 1);
    assert_eq!(scalls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn latency_nudge_escalates_borderline_turn() {
    // Borderline base score (ComprehensionCheck = 0.25, 0 passages, no
    // message) is BELOW threshold on its own, but a slow local leg over
    // budget adds W_LATENCY (0.30) → 0.55 ≥ 0.5 → routes to the secondary.
    let (primary, pcalls) = MockBackend::new("llamacpp:m", Behavior::Ok("LOCAL".into()));
    let (secondary, scalls) = MockBackend::new("cloud", Behavior::Ok("CLOUD".into()));
    let r = RouterBackend::with_ttft_ema_for_test(
        primary,
        secondary,
        RouterMode::Hybrid,
        Some(500),     // 500 ms budget
        Some(2_000.0), // local has been averaging 2 s TTFT (over budget)
    );
    let out = drive(
        r.generate_stream(
            &prompt(),
            &params_with(PedagogicalIntent::ComprehensionCheck, 0),
        )
        .await
        .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(out, "CLOUD");
    assert_eq!(scalls.load(Ordering::SeqCst), 1);
    assert_eq!(pcalls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn borderline_turn_stays_local_without_budget() {
    // Same borderline turn, but NO budget → latency inert → 0.25 < 0.5 →
    // stays local. Proves the nudge needs a configured budget.
    let (primary, pcalls) = MockBackend::new("llamacpp:m", Behavior::Ok("LOCAL".into()));
    let (secondary, scalls) = MockBackend::new("cloud", Behavior::Ok("CLOUD".into()));
    let r = RouterBackend::with_ttft_ema_for_test(
        primary,
        secondary,
        RouterMode::Hybrid,
        None,          // no budget
        Some(2_000.0), // slow local, but irrelevant without a budget
    );
    let out = drive(
        r.generate_stream(
            &prompt(),
            &params_with(PedagogicalIntent::ComprehensionCheck, 0),
        )
        .await
        .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(out, "LOCAL");
    assert_eq!(pcalls.load(Ordering::SeqCst), 1);
    assert_eq!(scalls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn slow_local_keeps_trivial_turn_local() {
    // Self-healing property: a TRIVIAL turn (Encouragement = 0.0 base) over
    // budget gets 0.0 + 0.30 = 0.30 < 0.5 → STAYS LOCAL. Latency is a
    // nudge, not a circuit-breaker, so routine turns keep exercising the
    // local leg and its TTFT EMA can recover.
    let (primary, pcalls) = MockBackend::new("llamacpp:m", Behavior::Ok("LOCAL".into()));
    let (secondary, scalls) = MockBackend::new("cloud", Behavior::Ok("CLOUD".into()));
    let r = RouterBackend::with_ttft_ema_for_test(
        primary,
        secondary,
        RouterMode::Hybrid,
        Some(500),
        Some(2_000.0),
    );
    let out = drive(
        r.generate_stream(&prompt(), &params_with(PedagogicalIntent::Encouragement, 0))
            .await
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(out, "LOCAL");
    assert_eq!(pcalls.load(Ordering::SeqCst), 1);
    assert_eq!(scalls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn primary_leg_records_ttft_into_ema() {
    // After a primary-served turn, the EMA transitions from None to Some.
    let (primary, _) = MockBackend::new("llamacpp:m", Behavior::Ok("LOCAL".into()));
    let (secondary, _) = MockBackend::new("cloud", Behavior::Ok("CLOUD".into()));
    let r = RouterBackend::new(primary, secondary, RouterMode::Hybrid);
    assert!(r.ttft_ema_for_test().is_none(), "EMA starts empty");
    // Routine turn → primary → stream wrapped → first chunk records TTFT.
    let stream = r
        .generate_stream(&prompt(), &params_with(PedagogicalIntent::Encouragement, 0))
        .await
        .unwrap();
    let _ = drive(stream).await.unwrap();
    assert!(
        r.ttft_ema_for_test().is_some(),
        "primary leg's first chunk recorded a TTFT sample"
    );
}

#[tokio::test]
async fn secondary_leg_does_not_record_ttft() {
    // A cloud-preferred turn runs the secondary; its TTFT must NOT pollute
    // the primary EMA.
    let (primary, _) = MockBackend::new("llamacpp:m", Behavior::Ok("LOCAL".into()));
    let (secondary, _) = MockBackend::new("cloud", Behavior::Ok("CLOUD".into()));
    let r = RouterBackend::new(primary, secondary, RouterMode::CloudPreferred);
    let stream = r
        .generate_stream(&prompt(), &params_with(PedagogicalIntent::Encouragement, 0))
        .await
        .unwrap();
    let _ = drive(stream).await.unwrap();
    assert!(
        r.ttft_ema_for_test().is_none(),
        "secondary leg must not record into the primary EMA"
    );
}
