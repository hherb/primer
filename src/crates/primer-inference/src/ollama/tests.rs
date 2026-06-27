use super::*;

#[test]
fn ndjson_buffer_yields_complete_line() {
    let mut buf = NdjsonBuffer::new();
    buf.extend(b"{\"a\":1}\n");
    assert_eq!(buf.pop_line().as_deref(), Some("{\"a\":1}"));
    assert_eq!(buf.pop_line(), None);
}

#[test]
fn ndjson_buffer_holds_partial_line_until_newline() {
    let mut buf = NdjsonBuffer::new();
    buf.extend(b"{\"a\":");
    assert_eq!(buf.pop_line(), None);
    buf.extend(b"1}\n");
    assert_eq!(buf.pop_line().as_deref(), Some("{\"a\":1}"));
    assert_eq!(buf.pop_line(), None);
}

#[test]
fn ndjson_buffer_handles_two_lines_in_one_chunk() {
    let mut buf = NdjsonBuffer::new();
    buf.extend(b"line1\nline2\n");
    assert_eq!(buf.pop_line().as_deref(), Some("line1"));
    assert_eq!(buf.pop_line().as_deref(), Some("line2"));
    assert_eq!(buf.pop_line(), None);
}

#[test]
fn ndjson_buffer_handles_line_split_across_three_chunks() {
    let mut buf = NdjsonBuffer::new();
    buf.extend(b"li");
    assert_eq!(buf.pop_line(), None);
    buf.extend(b"ne");
    assert_eq!(buf.pop_line(), None);
    buf.extend(b"1\n");
    assert_eq!(buf.pop_line().as_deref(), Some("line1"));
    assert_eq!(buf.pop_line(), None);
}

#[test]
fn parse_ollama_line_emits_chunk_for_content() {
    let chunk = parse_ollama_line(r#"{"message":{"content":"Hello"},"done":false}"#).unwrap();
    assert_eq!(chunk.text, "Hello");
    assert!(!chunk.done);
}

#[test]
fn parse_ollama_line_emits_done_chunk() {
    let chunk = parse_ollama_line(r#"{"message":{"content":""},"done":true}"#).unwrap();
    assert_eq!(chunk.text, "");
    assert!(chunk.done);
}

#[test]
fn parse_ollama_line_returns_err_on_garbage() {
    assert!(parse_ollama_line("not json").is_err());
}

#[test]
fn map_ollama_finish_reason_length_is_length() {
    assert_eq!(
        map_ollama_finish_reason(Some("length")),
        FinishReason::Length
    );
}

#[test]
fn map_ollama_finish_reason_stop_is_stop() {
    assert_eq!(map_ollama_finish_reason(Some("stop")), FinishReason::Stop);
}

#[test]
fn map_ollama_finish_reason_absent_is_stop() {
    assert_eq!(map_ollama_finish_reason(None), FinishReason::Stop);
}

#[test]
fn parse_ollama_line_done_with_length_reason_flags_length() {
    let chunk =
        parse_ollama_line(r#"{"message":{"content":""},"done":true,"done_reason":"length"}"#)
            .unwrap();
    assert!(chunk.done);
    assert_eq!(chunk.finish_reason, FinishReason::Length);
}

#[test]
fn parse_ollama_line_done_with_stop_reason_flags_stop() {
    let chunk = parse_ollama_line(r#"{"message":{"content":""},"done":true,"done_reason":"stop"}"#)
        .unwrap();
    assert!(chunk.done);
    assert_eq!(chunk.finish_reason, FinishReason::Stop);
}

#[test]
fn ndjson_buffer_surfaces_invalid_utf8_lossily() {
    // A broken UTF-8 byte (0xff) should NOT silently disappear. The line
    // must be surfaced (lossy-decoded with U+FFFD) so the caller can log
    // and skip it via the normal parse-error path.
    let mut buf = NdjsonBuffer::new();
    buf.extend(&[b'a', 0xff, b'b', b'\n']);
    let line = buf
        .pop_line()
        .expect("invalid UTF-8 line should still be surfaced");
    assert!(line.contains('a'));
    assert!(line.contains('b'));
    assert!(line.contains('\u{FFFD}'));
    assert_eq!(buf.pop_line(), None);
}

mod classify_ollama_tests {
    use super::*;
    use primer_core::error::InferenceError;
    use reqwest::StatusCode;

    #[test]
    fn classifies_404_with_model_not_found_body() {
        let body = r#"{"error":"model 'llama3.2' not found, try pulling it first"}"#;
        let e = classify_ollama_status(StatusCode::NOT_FOUND, body, "llama3.2");
        match e {
            InferenceError::ModelNotFound { model } => assert_eq!(model, "llama3.2"),
            other => panic!("expected ModelNotFound, got {other:?}"),
        }
    }

    #[test]
    fn classifies_404_without_model_body_as_other() {
        let e = classify_ollama_status(StatusCode::NOT_FOUND, "page not found", "llama3.2");
        assert!(matches!(e, InferenceError::Other(_)));
    }

    #[test]
    fn classifies_500_as_service_unavailable() {
        let e = classify_ollama_status(StatusCode::INTERNAL_SERVER_ERROR, "", "llama3.2");
        assert!(matches!(e, InferenceError::ServiceUnavailable));
    }

    #[test]
    fn classifies_503_as_service_unavailable() {
        let e = classify_ollama_status(StatusCode::SERVICE_UNAVAILABLE, "", "llama3.2");
        assert!(matches!(e, InferenceError::ServiceUnavailable));
    }

    #[test]
    fn classifies_other_4xx_as_other() {
        let e = classify_ollama_status(StatusCode::BAD_REQUEST, "bad payload", "llama3.2");
        match e {
            InferenceError::Other(s) => assert!(s.contains("400") && s.contains("bad payload")),
            other => panic!("expected Other, got {other:?}"),
        }
    }
}

mod reasoning_wiring {
    use super::*;

    /// Drive a sequence of NDJSON lines through the REAL shared filter
    /// helper, collecting visible text and whether an error chunk was
    /// emitted.
    fn drive(backend: &OllamaBackend, lines: &[&str]) -> (String, bool) {
        use crate::reasoning_stream::{FilterAction, process_filtered_chunk};
        use primer_core::reasoning::ReasoningFilter;
        let mut filter = ReasoningFilter::new(backend.reasoning_markers.clone());
        let mut had_visible = false;
        let mut visible = String::new();
        let mut err = false;
        for line in lines {
            let chunk = parse_ollama_line(line).unwrap();
            match process_filtered_chunk(&mut filter, chunk, &mut had_visible, "ollama") {
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

    #[test]
    fn strips_think_block_end_to_end() {
        let b = OllamaBackend::new("http://x".into(), "m".into());
        let lines = [
            r#"{"message":{"content":"<think>plan</think>"},"done":false}"#,
            r#"{"message":{"content":"Hi there"},"done":false}"#,
            r#"{"message":{"content":""},"done":true}"#,
        ];
        let (visible, err) = drive(&b, &lines);
        assert_eq!(visible, "Hi there");
        assert!(!err);
    }

    #[test]
    fn only_reasoning_yields_error() {
        let b = OllamaBackend::new("http://x".into(), "m".into());
        let lines = [
            r#"{"message":{"content":"<think>only thinking"},"done":false}"#,
            r#"{"message":{"content":""},"done":true}"#,
        ];
        let (visible, err) = drive(&b, &lines);
        assert_eq!(visible, "");
        assert!(err);
    }

    #[test]
    fn custom_marker_extends_defaults() {
        let b = OllamaBackend::new("http://x".into(), "m".into())
            .with_extra_markers(vec![("[[r]]".into(), "[[/r]]".into())]);
        let lines = [
            r#"{"message":{"content":"a[[r]]hidden[[/r]]b"},"done":false}"#,
            r#"{"message":{"content":""},"done":true}"#,
        ];
        let (visible, err) = drive(&b, &lines);
        assert_eq!(visible, "ab");
        assert!(!err);
    }

    /// Live confirmation that the built-in Gemma4 markers match the real
    /// stream. Requires `ollama serve` + `ollama pull gemma4:e4b`. Run on
    /// demand:
    ///   cargo test -p primer-inference gemma4_live -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "requires a running ollama with gemma4:e4b pulled"]
    async fn gemma4_live_reasoning_is_stripped() {
        let b = OllamaBackend::new("http://localhost:11434".into(), "gemma4:e4b".into());
        let prompt = Prompt {
            system: "You are a helpful tutor. Think first, then answer.".into(),
            messages: vec![Message {
                role: Role::User,
                content: "What is 2+2? Explain briefly.".into(),
            }],
        };
        let params = GenerationParams::default();
        let text = b.generate(&prompt, &params).await.expect("generate");
        eprintln!("VISIBLE OUTPUT:\n{text}");
        assert!(
            !text.contains("<|channel>"),
            "open channel marker leaked: {text}"
        );
        assert!(
            !text.contains("<channel|>"),
            "close channel marker leaked: {text}"
        );
        assert!(!text.contains("<think>"), "think marker leaked: {text}");
        assert!(!text.trim().is_empty(), "no visible answer produced");
    }
}
