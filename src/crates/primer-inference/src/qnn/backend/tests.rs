//! Unit tests for [`super::QnnBackend`]. Lives in a sibling file
//! (declared via `#[cfg(test)] mod tests;` in `super::mod`) so the impl
//! module stays under the 500-line file guideline. Tests use the
//! `MockGenieLibrary` from `crate::qnn::genie::mock` to exercise the
//! construction → query → drop happy path entirely on host.

use super::*;
use crate::qnn::genie::GenieCallError;
use crate::qnn::genie::mock::{MockEvent, MockGenieLibrary};
use crate::qnn::meta::PRIMER_META_FILENAME;
use futures::StreamExt;
use primer_core::error::Result as PrimerResult;
use primer_core::inference::{Message, Role};
use tempfile::tempdir;

fn write_genie_config(bundle_dir: &Path) {
    std::fs::write(
        bundle_dir.join(GENIE_CONFIG_FILENAME),
        r#"{ "test_only": true }"#,
    )
    .unwrap();
}

fn write_primer_meta(bundle_dir: &Path) {
    std::fs::write(
        bundle_dir.join(PRIMER_META_FILENAME),
        r#"{
            "model_id": "qwen3-4b",
            "context_length": 4096,
            "chat_template": "{% for m in messages %}<|im_start|>{{ m.role }}\n{{ m.content }}<|im_end|>\n{% endfor %}{% if add_generation_prompt %}<|im_start|>assistant\n{% endif %}",
            "vocab_size": 151936,
            "stop_sequences": ["<|im_end|>", "<|endoftext|>"]
        }"#,
    )
    .unwrap();
}

fn user_prompt(content: &str) -> Prompt {
    Prompt {
        system: "You are a helpful Socratic tutor.".to_string(),
        messages: vec![Message {
            role: Role::User,
            content: content.to_string(),
        }],
    }
}

#[tokio::test]
async fn construct_with_mock_library_succeeds_with_smoke_check_enabled() {
    let dir = tempdir().unwrap();
    write_genie_config(dir.path());
    write_primer_meta(dir.path());
    let lib: Arc<dyn GenieLibrary> = Arc::new(MockGenieLibrary::new_with_response("ok"));
    let backend = QnnBackend::new_with_library(dir.path().to_path_buf(), Arc::clone(&lib), true)
        .await
        .unwrap();
    // Name format pins step 1.2.5's prefix-matching contract.
    assert_eq!(backend.name(), "qnn:qwen3-4b");
    assert!(backend.is_available().await);
}

#[tokio::test]
async fn smoke_check_runs_one_query_at_construction() {
    let dir = tempdir().unwrap();
    write_genie_config(dir.path());
    write_primer_meta(dir.path());
    let raw_lib = MockGenieLibrary::new_with_response("smoke-check-response");
    // Snapshot the mock so we can read events even after the backend
    // takes ownership of an `Arc<dyn GenieLibrary>` clone.
    let arc_lib: Arc<MockGenieLibrary> = Arc::new(raw_lib);
    let lib: Arc<dyn GenieLibrary> = Arc::clone(&arc_lib) as Arc<dyn GenieLibrary>;
    let _backend = QnnBackend::new_with_library(dir.path().to_path_buf(), lib, true)
        .await
        .unwrap();
    let events = arc_lib.events();
    // OpenDialog (1) + Query with SMOKE_CHECK_PROMPT (2) — no drop yet
    // since _backend is still alive.
    assert!(matches!(events[0], MockEvent::OpenDialog { .. }));
    match &events[1] {
        MockEvent::Query { prompt } => assert_eq!(prompt, SMOKE_CHECK_PROMPT),
        other => panic!("expected smoke-check Query, got {other:?}"),
    }
}

#[tokio::test]
async fn smoke_check_disabled_skips_pre_query() {
    let dir = tempdir().unwrap();
    write_genie_config(dir.path());
    write_primer_meta(dir.path());
    let arc_lib: Arc<MockGenieLibrary> = Arc::new(MockGenieLibrary::new_with_response("x"));
    let lib: Arc<dyn GenieLibrary> = Arc::clone(&arc_lib) as Arc<dyn GenieLibrary>;
    let _backend = QnnBackend::new_with_library(dir.path().to_path_buf(), lib, false)
        .await
        .unwrap();
    let events = arc_lib.events();
    // Just the OpenDialog event; no smoke-check Query.
    assert_eq!(events.len(), 1);
    assert!(matches!(events[0], MockEvent::OpenDialog { .. }));
}

#[tokio::test]
async fn construct_falls_back_when_primer_meta_absent() {
    let dir = tempdir().unwrap();
    // Bundle dir basename becomes the derived model id.
    let bundle = dir.path().join("my-cool-model");
    std::fs::create_dir(&bundle).unwrap();
    write_genie_config(&bundle);
    // No primer-meta.json — fallback path.
    let lib: Arc<dyn GenieLibrary> = Arc::new(MockGenieLibrary::new_with_response("ok"));
    let backend = QnnBackend::new_with_library(bundle, lib, false)
        .await
        .unwrap();
    assert_eq!(backend.name(), "qnn:my-cool-model");
}

#[tokio::test]
async fn construct_errors_when_genie_config_missing() {
    let dir = tempdir().unwrap();
    write_primer_meta(dir.path());
    // No genie_config.json.
    let lib: Arc<dyn GenieLibrary> = Arc::new(MockGenieLibrary::new_with_response("ok"));
    let result = QnnBackend::new_with_library(dir.path().to_path_buf(), lib, false).await;
    match result {
        Err(PrimerError::Inference(inner)) => {
            let msg = inner.to_string();
            assert!(msg.contains(GENIE_CONFIG_FILENAME), "{msg}");
        }
        other => panic!("expected Inference error, got {other:?}"),
    }
}

#[tokio::test]
async fn construct_errors_when_bundle_dir_missing() {
    let lib: Arc<dyn GenieLibrary> = Arc::new(MockGenieLibrary::new_with_response("ok"));
    let result =
        QnnBackend::new_with_library(PathBuf::from("/definitely/not/a/real/bundle"), lib, false)
            .await;
    match result {
        Err(PrimerError::Inference(inner)) => {
            let msg = inner.to_string();
            assert!(msg.contains("bundle directory"), "{msg}");
        }
        other => panic!("expected Inference error, got {other:?}"),
    }
}

#[tokio::test]
async fn construct_propagates_open_dialog_error() {
    let dir = tempdir().unwrap();
    write_genie_config(dir.path());
    write_primer_meta(dir.path());
    let lib: Arc<dyn GenieLibrary> = Arc::new(MockGenieLibrary::new_failing_open(
        GenieCallError::NonSuccess {
            operation: "GenieDialog_create",
            status: -5,
        },
    ));
    let result = QnnBackend::new_with_library(dir.path().to_path_buf(), lib, false).await;
    match result {
        Err(PrimerError::Inference(inner)) => {
            let msg = inner.to_string();
            assert!(msg.contains("GenieDialog_create"), "{msg}");
            assert!(msg.contains("-5"), "{msg}");
        }
        other => panic!("expected Inference error, got {other:?}"),
    }
}

#[tokio::test]
async fn construct_propagates_smoke_check_error() {
    // The smoke check (`run_smoke_check = true`) is THE differentiator
    // vs. "construction silently succeeds even when the FFI ABI is
    // broken". If the very first query against a freshly-opened
    // dialog fails, `QnnBackend::new_with_library` must surface that
    // error rather than handing back a half-broken backend the
    // dialogue manager will only hit mid-conversation.
    let dir = tempdir().unwrap();
    write_genie_config(dir.path());
    write_primer_meta(dir.path());
    let lib: Arc<dyn GenieLibrary> = Arc::new(MockGenieLibrary::new_failing_query(
        GenieCallError::NonSuccess {
            operation: "GenieDialog_query",
            status: -7,
        },
    ));
    let result = QnnBackend::new_with_library(dir.path().to_path_buf(), lib, true).await;
    match result {
        Err(PrimerError::Inference(inner)) => {
            let msg = inner.to_string();
            assert!(msg.contains("GenieDialog_query"), "{msg}");
            assert!(msg.contains("-7"), "{msg}");
        }
        other => panic!("expected Inference error from smoke check, got {other:?}"),
    }
}

#[tokio::test]
async fn smoke_check_disabled_does_not_propagate_query_error() {
    // Twin of the above: with `run_smoke_check = false`, the failing
    // query is never issued at construction time, so construction
    // succeeds. Pins that the smoke-check fail-fast behaviour is
    // gated by the flag rather than by some other code path
    // accidentally querying during `new_with_library`.
    let dir = tempdir().unwrap();
    write_genie_config(dir.path());
    write_primer_meta(dir.path());
    let lib: Arc<dyn GenieLibrary> = Arc::new(MockGenieLibrary::new_failing_query(
        GenieCallError::NonSuccess {
            operation: "GenieDialog_query",
            status: -7,
        },
    ));
    let backend = QnnBackend::new_with_library(dir.path().to_path_buf(), lib, false)
        .await
        .expect("construction with smoke check disabled should succeed");
    assert_eq!(backend.name(), "qnn:qwen3-4b");
}

#[tokio::test]
async fn dialog_is_dropped_when_backend_drops() {
    let dir = tempdir().unwrap();
    write_genie_config(dir.path());
    write_primer_meta(dir.path());
    let arc_lib: Arc<MockGenieLibrary> = Arc::new(MockGenieLibrary::new_with_response("ok"));
    let lib: Arc<dyn GenieLibrary> = Arc::clone(&arc_lib) as Arc<dyn GenieLibrary>;
    {
        let _backend = QnnBackend::new_with_library(dir.path().to_path_buf(), lib, false)
            .await
            .unwrap();
        let mid_events = arc_lib.events();
        assert!(
            !mid_events
                .iter()
                .any(|e| matches!(e, MockEvent::DropDialog)),
            "dialog should still be alive while backend held",
        );
    }
    let final_events = arc_lib.events();
    assert!(
        final_events
            .iter()
            .any(|e| matches!(e, MockEvent::DropDialog)),
        "dialog should be dropped after backend dropped: {final_events:?}",
    );
}

#[tokio::test]
async fn generate_stream_emits_canned_response_as_body_chunk_then_done() {
    // Step 1.2.3 contract: a single canned response (the back-compat
    // shape from `new_with_response`) reaches the consumer as exactly
    // two chunks — one body chunk carrying the text, then one done
    // chunk with empty text. Pins that the streaming path doesn't
    // accidentally coalesce or drop the body chunk.
    let dir = tempdir().unwrap();
    write_genie_config(dir.path());
    write_primer_meta(dir.path());
    let arc_lib: Arc<MockGenieLibrary> =
        Arc::new(MockGenieLibrary::new_with_response("Yes, gravity is real."));
    let lib: Arc<dyn GenieLibrary> = Arc::clone(&arc_lib) as Arc<dyn GenieLibrary>;
    let backend = QnnBackend::new_with_library(dir.path().to_path_buf(), lib, false)
        .await
        .unwrap();
    let prompt = user_prompt("Is gravity real?");
    let mut stream = backend
        .generate_stream(&prompt, &GenerationParams::default())
        .await
        .unwrap();
    let body = stream
        .next()
        .await
        .expect("stream yields a body chunk first")
        .unwrap();
    assert_eq!(body.text, "Yes, gravity is real.");
    assert!(!body.done, "body chunk: done=false");
    let done = stream
        .next()
        .await
        .expect("stream yields a final done chunk")
        .unwrap();
    assert_eq!(done.text, "");
    assert!(done.done, "final chunk: done=true sentinel");
    // No further chunks.
    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn generate_stream_renders_prompt_through_chat_template() {
    let dir = tempdir().unwrap();
    write_genie_config(dir.path());
    write_primer_meta(dir.path());
    let arc_lib: Arc<MockGenieLibrary> = Arc::new(MockGenieLibrary::new_with_response("ack"));
    let lib: Arc<dyn GenieLibrary> = Arc::clone(&arc_lib) as Arc<dyn GenieLibrary>;
    let backend = QnnBackend::new_with_library(dir.path().to_path_buf(), lib, false)
        .await
        .unwrap();
    let prompt = user_prompt("hello primer");
    let _ = backend
        .generate_stream(&prompt, &GenerationParams::default())
        .await
        .unwrap()
        .next()
        .await;
    let events = arc_lib.events();
    // The most recent Query event should carry the rendered ChatML
    // form of the prompt — pin that the backend doesn't accidentally
    // bypass the template.
    let last_query = events
        .iter()
        .rev()
        .find_map(|e| match e {
            MockEvent::Query { prompt } => Some(prompt.as_str()),
            _ => None,
        })
        .expect("at least one Query event");
    assert!(
        last_query.contains("<|im_start|>user\nhello primer<|im_end|>"),
        "rendered prompt should include the user message in ChatML form: {last_query}",
    );
    assert!(
        last_query.contains("<|im_start|>system\nYou are a helpful Socratic tutor.<|im_end|>"),
        "rendered prompt should include the system message: {last_query}",
    );
}

#[tokio::test]
async fn render_prompt_pins_chatml_shape() {
    // Direct pin of the renderer through the public backend
    // accessor, independent of any `generate_stream` plumbing.
    let dir = tempdir().unwrap();
    write_genie_config(dir.path());
    write_primer_meta(dir.path());
    let lib: Arc<dyn GenieLibrary> = Arc::new(MockGenieLibrary::new_with_response("x"));
    let backend = QnnBackend::new_with_library(dir.path().to_path_buf(), lib, false)
        .await
        .unwrap();
    let rendered = backend
        .render_prompt(&user_prompt("why is the sky blue?"))
        .unwrap();
    assert!(rendered.contains("<|im_start|>user\nwhy is the sky blue?<|im_end|>"));
    assert!(rendered.trim_end().ends_with("<|im_start|>assistant"));
}

// ===== Phase 1.2 step 1.2.3 — streaming token bridge =====
//
// The next three tests pin the per-token streaming contract introduced
// by step 1.2.3:
//
// - The mock backend emits N body chunks (`done = false`) followed by
//   one final `done = true` chunk. Total = N + 1.
// - A mid-stream Genie error replaces the final done chunk with an
//   `Err(PrimerError::Inference(...))` and closes the channel.
// - Two concurrent `generate_stream` calls against the same backend
//   serialise through the dialog mutex without panicking or dropping
//   chunks.

#[tokio::test]
async fn generate_stream_yields_n_body_chunks_then_done() {
    let dir = tempdir().unwrap();
    write_genie_config(dir.path());
    write_primer_meta(dir.path());
    let arc_lib: Arc<MockGenieLibrary> = Arc::new(MockGenieLibrary::new_with_tokens([
        "Yes,", " gravity", " is", " real.",
    ]));
    let lib: Arc<dyn GenieLibrary> = Arc::clone(&arc_lib) as Arc<dyn GenieLibrary>;
    let backend = QnnBackend::new_with_library(dir.path().to_path_buf(), lib, false)
        .await
        .unwrap();
    let prompt = user_prompt("Is gravity real?");
    let stream = backend
        .generate_stream(&prompt, &GenerationParams::default())
        .await
        .unwrap();
    let results: Vec<PrimerResult<TokenChunk>> = stream.collect().await;
    let chunks: Vec<TokenChunk> = results
        .into_iter()
        .map(|r| r.expect("no error chunks expected on happy path"))
        .collect();
    assert_eq!(chunks.len(), 5, "4 body + 1 done = 5: {chunks:?}");
    assert_eq!(chunks[0].text, "Yes,");
    assert!(!chunks[0].done);
    assert_eq!(chunks[1].text, " gravity");
    assert!(!chunks[1].done);
    assert_eq!(chunks[2].text, " is");
    assert!(!chunks[2].done);
    assert_eq!(chunks[3].text, " real.");
    assert!(!chunks[3].done);
    // Final chunk: done marker, text is empty.
    assert_eq!(chunks[4].text, "");
    assert!(chunks[4].done);
}

#[tokio::test]
async fn generate_stream_mid_stream_error_yields_partial_chunks_then_err_and_closes() {
    let dir = tempdir().unwrap();
    write_genie_config(dir.path());
    write_primer_meta(dir.path());
    let arc_lib: Arc<MockGenieLibrary> = Arc::new(MockGenieLibrary::new_with_tokens_then_error(
        ["partial1", "partial2"],
        GenieCallError::NonSuccess {
            operation: "GenieDialog_query",
            status: -9,
        },
    ));
    let lib: Arc<dyn GenieLibrary> = Arc::clone(&arc_lib) as Arc<dyn GenieLibrary>;
    let backend = QnnBackend::new_with_library(dir.path().to_path_buf(), lib, false)
        .await
        .unwrap();
    let prompt = user_prompt("doesn't matter");
    let stream = backend
        .generate_stream(&prompt, &GenerationParams::default())
        .await
        .unwrap();
    let results: Vec<PrimerResult<TokenChunk>> = stream.collect().await;
    assert_eq!(results.len(), 3, "2 body + 1 err = 3: {results:?}");
    assert_eq!(results[0].as_ref().unwrap().text, "partial1");
    assert!(!results[0].as_ref().unwrap().done);
    assert_eq!(results[1].as_ref().unwrap().text, "partial2");
    assert!(!results[1].as_ref().unwrap().done);
    let err = results[2]
        .as_ref()
        .expect_err("third item is the error chunk");
    let msg = err.to_string();
    assert!(msg.contains("GenieDialog_query"), "{msg}");
    assert!(msg.contains("-9"), "{msg}");
}

#[tokio::test]
async fn generate_stream_serialises_concurrent_callers() {
    // Two concurrent `generate_stream` calls against the same backend
    // share one dialog handle, which is single-session-per-Genie-contract.
    // The dialog mutex serialises them. This test pins that both calls
    // complete cleanly, with the full chunk shape (N body + 1 done) on
    // each receiver — i.e. neither call drops chunks or panics when
    // contending on the mutex.
    let dir = tempdir().unwrap();
    write_genie_config(dir.path());
    write_primer_meta(dir.path());
    let arc_lib: Arc<MockGenieLibrary> =
        Arc::new(MockGenieLibrary::new_with_tokens(["a", "b", "c"]));
    let lib: Arc<dyn GenieLibrary> = Arc::clone(&arc_lib) as Arc<dyn GenieLibrary>;
    let backend = Arc::new(
        QnnBackend::new_with_library(dir.path().to_path_buf(), lib, false)
            .await
            .unwrap(),
    );
    let p1 = user_prompt("first prompt");
    let p2 = user_prompt("second prompt");
    let s1 = backend
        .generate_stream(&p1, &GenerationParams::default())
        .await
        .unwrap();
    let s2 = backend
        .generate_stream(&p2, &GenerationParams::default())
        .await
        .unwrap();
    let (r1, r2): (Vec<_>, Vec<_>) = tokio::join!(s1.collect(), s2.collect());
    assert_eq!(r1.len(), 4, "first receiver: 3 body + 1 done");
    assert_eq!(r2.len(), 4, "second receiver: 3 body + 1 done");
    assert!(r1.iter().all(|r: &PrimerResult<TokenChunk>| r.is_ok()));
    assert!(r2.iter().all(|r: &PrimerResult<TokenChunk>| r.is_ok()));
    // Both Query events should be present; the mutex makes them
    // sequential rather than interleaved, but with a fast mock we can't
    // distinguish strict ordering without inserting a barrier — what we
    // CAN check is that both queries were actually issued.
    let events = arc_lib.events();
    let query_count = events
        .iter()
        .filter(|e| matches!(e, MockEvent::Query { .. }))
        .count();
    assert_eq!(
        query_count, 2,
        "both prompts must hit the dialog: {events:?}"
    );
}
