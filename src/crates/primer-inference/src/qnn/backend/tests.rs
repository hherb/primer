//! Unit tests for [`super::QnnBackend`]. Lives in a sibling file
//! (declared via `#[cfg(test)] mod tests;` in `super::mod`) so the impl
//! module stays under the 500-line file guideline. Tests use the
//! `MockGenieLibrary` from `crate::qnn::genie::mock` to exercise the
//! construction → query → drop happy path entirely on host.

use super::*;
use crate::qnn::genie::mock::{MockEvent, MockGenieLibrary};
use crate::qnn::meta::PRIMER_META_FILENAME;
use futures::StreamExt;
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
async fn generate_stream_returns_canned_response_as_single_chunk() {
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
    let first = stream
        .next()
        .await
        .expect("stream yields at least one chunk")
        .unwrap();
    assert_eq!(first.text, "Yes, gravity is real.");
    assert!(first.done, "step 1.2.2 returns a single done chunk");
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
