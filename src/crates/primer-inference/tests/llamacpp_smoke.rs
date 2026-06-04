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
use primer_inference::llamacpp::engine::RealLlamaEngine;

#[tokio::test]
#[ignore = "requires PRIMER_LLAMACPP_TEST_GGUF + a C++ toolchain + GPU"]
async fn generates_nonempty_response() {
    let gguf = std::env::var("PRIMER_LLAMACPP_TEST_GGUF")
        .expect("set PRIMER_LLAMACPP_TEST_GGUF to a .gguf path");
    let engine = RealLlamaEngine::new(std::path::Path::new(&gguf), -1, Some(2048))
        .expect("load model");
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
