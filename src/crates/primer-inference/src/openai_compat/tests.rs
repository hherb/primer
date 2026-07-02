//! Unit tests for the OpenAI-compatible backend. Lives in a sibling file
//! (declared via `#[cfg(test)] mod tests;` in `super::mod`) so the impl
//! module stays under the 500-line file guideline. Mirrors the split
//! used by `qnn::backend`.

use super::*;

// -- Mid-stream error payloads ------------------------------------------

#[test]
fn parse_error_data_detects_object_shape() {
    assert_eq!(
        parse_openai_compat_error_data(
            r#"{"error": {"message": "upstream unavailable", "code": 502}}"#
        )
        .as_deref(),
        Some("upstream unavailable")
    );
}

#[test]
fn parse_error_data_detects_string_shape() {
    assert_eq!(
        parse_openai_compat_error_data(r#"{"error": "model crashed"}"#).as_deref(),
        Some("model crashed")
    );
}

#[test]
fn parse_error_data_ignores_normal_chunks_and_garbage() {
    assert!(
        parse_openai_compat_error_data(r#"{"choices":[{"delta":{"content":"hi"}}]}"#).is_none()
    );
    assert!(parse_openai_compat_error_data("not json").is_none());
}

// -- SSE buffer ---------------------------------------------------------

#[test]
fn sse_buffer_yields_data_payload() {
    let mut buf = OpenAiSseBuffer::new();
    buf.extend(b"data: {\"choices\":[]}\n");
    assert_eq!(buf.pop_data().as_deref(), Some("{\"choices\":[]}"));
    assert_eq!(buf.pop_data(), None);
}

#[test]
fn sse_buffer_holds_partial_line() {
    let mut buf = OpenAiSseBuffer::new();
    buf.extend(b"data: {\"ch");
    assert_eq!(buf.pop_data(), None);
    buf.extend(b"oices\":[]}\n");
    assert_eq!(buf.pop_data().as_deref(), Some("{\"choices\":[]}"));
}

#[test]
fn sse_buffer_skips_comment_lines() {
    let mut buf = OpenAiSseBuffer::new();
    buf.extend(b": keepalive\ndata: hello\n");
    assert_eq!(buf.pop_data().as_deref(), Some("hello"));
}

#[test]
fn sse_buffer_skips_blank_and_event_lines() {
    let mut buf = OpenAiSseBuffer::new();
    buf.extend(b"\nevent: message\nid: 42\ndata: payload\n");
    assert_eq!(buf.pop_data().as_deref(), Some("payload"));
    assert_eq!(buf.pop_data(), None);
}

#[test]
fn sse_buffer_handles_two_data_lines_in_one_chunk() {
    let mut buf = OpenAiSseBuffer::new();
    buf.extend(b"data: first\ndata: second\n");
    assert_eq!(buf.pop_data().as_deref(), Some("first"));
    assert_eq!(buf.pop_data().as_deref(), Some("second"));
    assert_eq!(buf.pop_data(), None);
}

#[test]
fn sse_buffer_strips_only_one_leading_space() {
    let mut buf = OpenAiSseBuffer::new();
    buf.extend(b"data:   spaced\n");
    assert_eq!(buf.pop_data().as_deref(), Some("  spaced"));
}

#[test]
fn sse_buffer_handles_crlf() {
    let mut buf = OpenAiSseBuffer::new();
    buf.extend(b"data: crlf\r\n");
    assert_eq!(buf.pop_data().as_deref(), Some("crlf"));
}

// -- Chunk parser -------------------------------------------------------

#[test]
fn parse_chunk_extracts_content() {
    let data = r#"{"id":"x","choices":[{"delta":{"content":"Hi"},"finish_reason":null}]}"#;
    let chunk = parse_openai_compat_chunk(data).unwrap().unwrap();
    assert_eq!(chunk.text, "Hi");
    assert!(!chunk.done);
}

#[test]
fn parse_chunk_done_on_finish_reason() {
    let data = r#"{"id":"x","choices":[{"delta":{"content":""},"finish_reason":"stop"}]}"#;
    let chunk = parse_openai_compat_chunk(data).unwrap().unwrap();
    assert!(chunk.done);
}

#[test]
fn map_openai_finish_reason_length_is_length() {
    assert_eq!(map_openai_finish_reason("length"), FinishReason::Length);
}

#[test]
fn map_openai_finish_reason_stop_is_stop() {
    assert_eq!(map_openai_finish_reason("stop"), FinishReason::Stop);
}

#[test]
fn parse_chunk_length_finish_reason_flags_length() {
    let data = r#"{"id":"x","choices":[{"delta":{"content":""},"finish_reason":"length"}]}"#;
    let chunk = parse_openai_compat_chunk(data).unwrap().unwrap();
    assert!(chunk.done);
    assert_eq!(chunk.finish_reason, FinishReason::Length);
}

#[test]
fn parse_chunk_stop_finish_reason_flags_stop() {
    let data = r#"{"id":"x","choices":[{"delta":{"content":""},"finish_reason":"stop"}]}"#;
    let chunk = parse_openai_compat_chunk(data).unwrap().unwrap();
    assert!(chunk.done);
    assert_eq!(chunk.finish_reason, FinishReason::Stop);
}

#[test]
fn parse_chunk_done_marker() {
    let chunk = parse_openai_compat_chunk("[DONE]").unwrap().unwrap();
    assert_eq!(chunk.text, "");
    assert!(chunk.done);
}

#[test]
fn parse_chunk_skips_role_only_delta() {
    let data = r#"{"id":"x","choices":[{"delta":{"role":"assistant"},"finish_reason":null}]}"#;
    assert!(parse_openai_compat_chunk(data).unwrap().is_none());
}

#[test]
fn parse_chunk_skips_empty_choices() {
    let data = r#"{"id":"x","choices":[]}"#;
    assert!(parse_openai_compat_chunk(data).unwrap().is_none());
}

#[test]
fn parse_chunk_returns_err_on_garbage() {
    assert!(parse_openai_compat_chunk("not json").is_err());
}

// -- Error classification -----------------------------------------------

mod classify_tests {
    use primer_core::error::InferenceError;
    use reqwest::StatusCode;
    use reqwest::header::{HeaderMap, HeaderValue};
    use std::time::Duration;

    use super::super::classify_openai_compat_status;

    fn empty_headers() -> HeaderMap {
        HeaderMap::new()
    }

    fn headers_with_retry_after(seconds: u64) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("retry-after", HeaderValue::from(seconds));
        h
    }

    #[test]
    fn classifies_401_as_auth() {
        let e = classify_openai_compat_status(StatusCode::UNAUTHORIZED, &empty_headers(), "", "m");
        assert!(matches!(e, InferenceError::Auth));
    }

    #[test]
    fn classifies_403_as_auth() {
        let e = classify_openai_compat_status(StatusCode::FORBIDDEN, &empty_headers(), "", "m");
        assert!(matches!(e, InferenceError::Auth));
    }

    #[test]
    fn classifies_429_with_retry_after() {
        let e = classify_openai_compat_status(
            StatusCode::TOO_MANY_REQUESTS,
            &headers_with_retry_after(5),
            "",
            "m",
        );
        match e {
            InferenceError::RateLimited {
                retry_after: Some(d),
            } => {
                assert_eq!(d, Duration::from_secs(5))
            }
            other => panic!("expected RateLimited(Some(5s)), got {other:?}"),
        }
    }

    #[test]
    fn classifies_429_without_retry_after() {
        let e =
            classify_openai_compat_status(StatusCode::TOO_MANY_REQUESTS, &empty_headers(), "", "m");
        assert!(matches!(
            e,
            InferenceError::RateLimited { retry_after: None }
        ));
    }

    #[test]
    fn classifies_404_model_not_found() {
        let e = classify_openai_compat_status(
            StatusCode::NOT_FOUND,
            &empty_headers(),
            r#"{"error":{"message":"The model `foo` does not exist"}}"#,
            "foo",
        );
        match e {
            InferenceError::ModelNotFound { model } => assert_eq!(model, "foo"),
            other => panic!("expected ModelNotFound, got {other:?}"),
        }
    }

    #[test]
    fn classifies_404_generic_as_other() {
        let e = classify_openai_compat_status(
            StatusCode::NOT_FOUND,
            &empty_headers(),
            "page not found",
            "m",
        );
        assert!(matches!(e, InferenceError::Other(_)));
    }

    /// A 404 whose body merely mentions both "model" and "not" but
    /// not in a model-not-found context must NOT be classified as
    /// `ModelNotFound`. Guards against the old loose `contains("model")
    /// && contains("not")` rule.
    #[test]
    fn classifies_404_with_unrelated_model_and_not_keywords_as_other() {
        let e = classify_openai_compat_status(
            StatusCode::NOT_FOUND,
            &empty_headers(),
            "did you mean to add the model_not_found handler?",
            "m",
        );
        assert!(
            matches!(e, InferenceError::Other(_)),
            "expected Other, got {e:?}"
        );
    }

    #[test]
    fn classifies_404_model_not_found_vllm_phrasing() {
        let e = classify_openai_compat_status(
            StatusCode::NOT_FOUND,
            &empty_headers(),
            r#"{"error":"model not found: foo"}"#,
            "foo",
        );
        match e {
            InferenceError::ModelNotFound { model } => assert_eq!(model, "foo"),
            other => panic!("expected ModelNotFound, got {other:?}"),
        }
    }

    #[test]
    fn classifies_404_model_not_found_groq_phrasing() {
        let e = classify_openai_compat_status(
            StatusCode::NOT_FOUND,
            &empty_headers(),
            r#"{"error":{"message":"unknown model foo"}}"#,
            "foo",
        );
        assert!(matches!(e, InferenceError::ModelNotFound { .. }));
    }

    #[test]
    fn classifies_404_model_not_found_omlx_phrasing() {
        let e = classify_openai_compat_status(
            StatusCode::NOT_FOUND,
            &empty_headers(),
            "no such model: foo",
            "foo",
        );
        assert!(matches!(e, InferenceError::ModelNotFound { .. }));
    }

    #[test]
    fn classifies_500_as_service_unavailable() {
        let e = classify_openai_compat_status(
            StatusCode::INTERNAL_SERVER_ERROR,
            &empty_headers(),
            "",
            "m",
        );
        assert!(matches!(e, InferenceError::ServiceUnavailable));
    }

    #[test]
    fn classifies_503_as_service_unavailable() {
        let e = classify_openai_compat_status(
            StatusCode::SERVICE_UNAVAILABLE,
            &empty_headers(),
            "",
            "m",
        );
        assert!(matches!(e, InferenceError::ServiceUnavailable));
    }
}

// -- Request serialization ----------------------------------------------

#[test]
fn request_serialization_includes_all_fields() {
    let req = ChatCompletionRequest {
        model: "test-model".to_string(),
        messages: vec![
            ChatMessage {
                role: "system".to_string(),
                content: "Be helpful.".to_string(),
            },
            ChatMessage {
                role: "user".to_string(),
                content: "Hello".to_string(),
            },
        ],
        stream: true,
        temperature: 0.7,
        top_p: 0.9,
        max_tokens: 512,
        stop: vec![],
    };
    let json = serde_json::to_string(&req).unwrap();
    assert!(json.contains("\"model\":\"test-model\""));
    assert!(json.contains("\"stream\":true"));
    assert!(json.contains("\"temperature\":0.7"));
    assert!(json.contains("\"top_p\":0.9"));
    assert!(json.contains("\"max_tokens\":512"));
    assert!(
        !json.contains("\"stop\""),
        "empty stop should be omitted, got: {json}"
    );
}

#[test]
fn request_serialization_includes_stop_when_nonempty() {
    let req = ChatCompletionRequest {
        model: "m".to_string(),
        messages: vec![],
        stream: true,
        temperature: 0.7,
        top_p: 0.9,
        max_tokens: 512,
        stop: vec!["END".to_string()],
    };
    let json = serde_json::to_string(&req).unwrap();
    assert!(json.contains("\"stop\":[\"END\"]"));
}

// -- Reasoning-filter wiring (real shared helper) -----------------------

mod reasoning_wiring {
    use super::*;
    use crate::reasoning_stream::{FilterAction, process_filtered_chunk};
    use primer_core::reasoning::ReasoningFilter;

    fn drive(backend: &OpenAiCompatBackend, payloads: &[&str]) -> (String, bool) {
        let mut filter = ReasoningFilter::new(backend.reasoning_markers.clone());
        let mut had_visible = false;
        let mut visible = String::new();
        let mut err = false;
        for p in payloads {
            let Some(chunk) = parse_openai_compat_chunk(p).unwrap() else {
                continue;
            };
            match process_filtered_chunk(&mut filter, chunk, &mut had_visible, "openai-compat") {
                FilterAction::Nothing => {}
                FilterAction::Forward(r) => visible.push_str(&r.unwrap().text),
                FilterAction::Final(r) => {
                    match r {
                        Ok(c) => visible.push_str(&c.text),
                        Err(_) => err = true,
                    }
                    break;
                }
            }
        }
        (visible, err)
    }

    fn delta(content: &str) -> String {
        format!(r#"{{"choices":[{{"delta":{{"content":"{content}"}},"finish_reason":null}}]}}"#)
    }

    fn stop() -> String {
        r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#.to_string()
    }

    #[test]
    fn strips_think_block_end_to_end() {
        let b = OpenAiCompatBackend::new("http://x".into(), "m".into(), None);
        let payloads = [delta("<think>plan</think>"), delta("Hi there"), stop()];
        let refs: Vec<&str> = payloads.iter().map(|s| s.as_str()).collect();
        let (visible, err) = drive(&b, &refs);
        assert_eq!(visible, "Hi there");
        assert!(!err);
    }

    #[test]
    fn only_reasoning_yields_error() {
        let b = OpenAiCompatBackend::new("http://x".into(), "m".into(), None);
        let payloads = [delta("<think>only thinking"), stop()];
        let refs: Vec<&str> = payloads.iter().map(|s| s.as_str()).collect();
        let (visible, err) = drive(&b, &refs);
        assert_eq!(visible, "");
        assert!(err);
    }

    #[test]
    fn custom_marker_extends_defaults() {
        let b = OpenAiCompatBackend::new("http://x".into(), "m".into(), None)
            .with_extra_markers(vec![("[[r]]".into(), "[[/r]]".into())]);
        let payloads = [delta("a[[r]]hidden[[/r]]b"), stop()];
        let refs: Vec<&str> = payloads.iter().map(|s| s.as_str()).collect();
        let (visible, err) = drive(&b, &refs);
        assert_eq!(visible, "ab");
        assert!(!err);
    }
}
