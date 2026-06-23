//! Real-model smoke for LlamaCppBackend. NEVER runs in default CI.
//!
//! Run on a Metal-capable machine with a small GGUF on disk:
//!   PRIMER_LLAMACPP_TEST_GGUF=/path/to/model.gguf \
//!     ~/.cargo/bin/cargo +1.88 test -p primer-inference \
//!       --features llamacpp-metal --test llamacpp_smoke -- --ignored --nocapture

#![cfg(feature = "llamacpp")]

use std::sync::Arc;

use primer_core::inference::{GenerationParams, InferenceBackend, Message, Prompt, Role};
use primer_inference::LlamaCppBackend;
use primer_inference::llamacpp::engine::{LlamaEngine, RealLlamaEngine};

#[tokio::test]
#[ignore = "requires PRIMER_LLAMACPP_TEST_GGUF + a C++ toolchain + GPU"]
async fn generates_nonempty_response() {
    let gguf = std::env::var("PRIMER_LLAMACPP_TEST_GGUF")
        .expect("set PRIMER_LLAMACPP_TEST_GGUF to a .gguf path");
    let engine =
        RealLlamaEngine::new(std::path::Path::new(&gguf), -1, Some(2048)).expect("load model");
    let backend = LlamaCppBackend::new(Arc::new(engine));

    assert!(backend.name().starts_with("llamacpp:"));

    let prompt = Prompt {
        system: "You are a friendly Socratic tutor. Answer in one short sentence.".into(),
        messages: vec![Message {
            role: Role::User,
            content: "What color is the sky on a clear day?".into(),
        }],
    };
    let params = GenerationParams {
        max_tokens: 64,
        ..Default::default()
    };
    let out = backend.generate(&prompt, &params).await.expect("generate");
    println!("model said: {out}");
    assert!(!out.trim().is_empty(), "expected non-empty response");
}

/// Real-model verification of the issue #201 per-model BOS handling.
///
/// Loads the GGUF at `PRIMER_LLAMACPP_TEST_GGUF`, renders a representative
/// Socratic prompt through the model's own chat template, and reports the
/// resolved [`BosDecision`](primer_inference::llamacpp::params::BosDecision).
/// This surfaces the actual fix — which `AddBos` arm tokenization takes — that
/// `generates_nonempty_response` cannot observe.
///
/// Set `PRIMER_LLAMACPP_EXPECT_PREPEND_BOS` to `true`/`false` to turn the
/// printed diagnostic into a hard assertion:
///   * Gemma (template embeds a literal `<bos>`)  → expect `false`
///   * Qwen3 (no literal BOS in the template)      → expect `true`
/// Absent the env var the test only prints (a pure inspection run).
#[tokio::test]
#[ignore = "requires PRIMER_LLAMACPP_TEST_GGUF + a C++ toolchain + GPU"]
async fn reports_per_model_bos_decision() {
    let gguf = std::env::var("PRIMER_LLAMACPP_TEST_GGUF")
        .expect("set PRIMER_LLAMACPP_TEST_GGUF to a .gguf path");
    let engine =
        RealLlamaEngine::new(std::path::Path::new(&gguf), -1, Some(2048)).expect("load model");

    let prompt = Prompt {
        system: "You are a friendly Socratic tutor. Answer in one short sentence.".into(),
        messages: vec![Message {
            role: Role::User,
            content: "What color is the sky on a clear day?".into(),
        }],
    };
    let rendered = engine.render_prompt(&prompt).expect("render prompt");
    let decision = engine.bos_decision(&rendered);

    // Print enough to diagnose either leg by eye.
    let head: String = rendered.chars().take(48).collect();
    println!("rendered prompt head: {head:?}");
    println!("bos decision: {decision:?}");

    if let Ok(expected) = std::env::var("PRIMER_LLAMACPP_EXPECT_PREPEND_BOS") {
        let expected: bool = expected
            .trim()
            .parse()
            .expect("PRIMER_LLAMACPP_EXPECT_PREPEND_BOS must be `true` or `false`");
        assert_eq!(
            decision.prepend_bos, expected,
            "prepend_bos mismatch: a Gemma-family GGUF must NOT add a second BOS \
             (expect false); a Qwen3 GGUF keeps the historical add-once (expect true)"
        );
    }
}
