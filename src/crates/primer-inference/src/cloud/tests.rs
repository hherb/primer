use super::*;

#[test]
fn sse_buffer_yields_complete_event() {
    let mut buf = SseBuffer::new();
    buf.extend(b"event: content_block_delta\ndata: {\"x\":1}\n\n");
    let ev = buf.next_event().unwrap();
    assert_eq!(ev.event, "content_block_delta");
    assert_eq!(ev.data, "{\"x\":1}");
    assert!(buf.next_event().is_none());
}

#[test]
fn sse_buffer_handles_event_split_across_chunks() {
    let mut buf = SseBuffer::new();
    buf.extend(b"event: message_stop\n");
    assert!(buf.next_event().is_none());
    buf.extend(b"data: {}\n\n");
    let ev = buf.next_event().unwrap();
    assert_eq!(ev.event, "message_stop");
    assert_eq!(ev.data, "{}");
}

#[test]
fn sse_buffer_handles_two_events_in_one_chunk() {
    let mut buf = SseBuffer::new();
    buf.extend(b"event: content_block_delta\ndata: {\"a\":1}\n\nevent: message_stop\ndata: {}\n\n");
    let e1 = buf.next_event().unwrap();
    assert_eq!(e1.event, "content_block_delta");
    assert_eq!(e1.data, "{\"a\":1}");
    let e2 = buf.next_event().unwrap();
    assert_eq!(e2.event, "message_stop");
    assert_eq!(e2.data, "{}");
    assert!(buf.next_event().is_none());
}

#[test]
fn sse_buffer_ignores_comment_and_blank_lines_outside_event() {
    let mut buf = SseBuffer::new();
    buf.extend(b":keepalive\n\nevent: ping\ndata: {}\n\n");
    // The ":keepalive\n\n" should not produce a phantom event.
    let ev = buf.next_event().unwrap();
    assert_eq!(ev.event, "ping");
    assert!(buf.next_event().is_none());
}

#[test]
fn parse_anthropic_event_yields_text_delta() {
    let ev = SseEvent {
        event: "content_block_delta".to_string(),
        data:
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hi"}}"#
                .to_string(),
    };
    let chunk = parse_anthropic_event(&ev).unwrap().unwrap();
    assert_eq!(chunk.text, "Hi");
    assert!(!chunk.done);
}

#[test]
fn parse_anthropic_event_yields_done_on_message_stop() {
    let ev = SseEvent {
        event: "message_stop".to_string(),
        data: "{}".to_string(),
    };
    let chunk = parse_anthropic_event(&ev).unwrap().unwrap();
    assert_eq!(chunk.text, "");
    assert!(chunk.done);
}

#[test]
fn map_anthropic_stop_reason_max_tokens_is_length() {
    assert_eq!(
        map_anthropic_stop_reason("max_tokens"),
        FinishReason::Length
    );
}

#[test]
fn map_anthropic_stop_reason_end_turn_is_stop() {
    assert_eq!(map_anthropic_stop_reason("end_turn"), FinishReason::Stop);
}

#[test]
fn finish_reason_from_message_delta_max_tokens_is_length() {
    let ev = SseEvent {
        event: "message_delta".to_string(),
        data:
            r#"{"type":"message_delta","delta":{"stop_reason":"max_tokens","stop_sequence":null}}"#
                .to_string(),
    };
    assert_eq!(
        anthropic_finish_reason_from_event(&ev),
        Some(FinishReason::Length)
    );
}

#[test]
fn finish_reason_from_message_delta_without_stop_reason_is_none() {
    let ev = SseEvent {
        event: "message_delta".to_string(),
        data: r#"{"type":"message_delta","delta":{}}"#.to_string(),
    };
    assert_eq!(anthropic_finish_reason_from_event(&ev), None);
}

#[test]
fn finish_reason_from_non_delta_event_is_none() {
    let ev = SseEvent {
        event: "content_block_delta".to_string(),
        data: r#"{"delta":{"text":"hi"}}"#.to_string(),
    };
    assert_eq!(anthropic_finish_reason_from_event(&ev), None);
}

#[test]
fn translator_stamps_length_from_delta_onto_message_stop() {
    let mut translator = AnthropicEventTranslator::new();
    // A content delta forwards a non-terminal chunk.
    let content = SseEvent {
        event: "content_block_delta".to_string(),
        data:
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hi"}}"#
                .to_string(),
    };
    let c = translator.step(&content).unwrap().unwrap();
    assert!(!c.done);
    // The message_delta carries the stop reason but yields no chunk.
    let delta = SseEvent {
        event: "message_delta".to_string(),
        data: r#"{"type":"message_delta","delta":{"stop_reason":"max_tokens"}}"#.to_string(),
    };
    assert!(translator.step(&delta).unwrap().is_none());
    // The terminal message_stop chunk inherits the tracked reason.
    let stop = SseEvent {
        event: "message_stop".to_string(),
        data: "{}".to_string(),
    };
    let terminal = translator.step(&stop).unwrap().unwrap();
    assert!(terminal.done);
    assert_eq!(terminal.finish_reason, FinishReason::Length);
}

#[test]
fn translator_defaults_terminal_chunk_to_stop() {
    let mut translator = AnthropicEventTranslator::new();
    let stop = SseEvent {
        event: "message_stop".to_string(),
        data: "{}".to_string(),
    };
    let terminal = translator.step(&stop).unwrap().unwrap();
    assert!(terminal.done);
    assert_eq!(terminal.finish_reason, FinishReason::Stop);
}

/// Drive the exact `SseBuffer` + `AnthropicEventTranslator` pair the
/// `generate_stream` spawn loop uses over a raw byte stream, split
/// across chunk boundaries (the `message_delta` lands in a separate
/// network chunk from the terminal `message_stop`). Asserts the
/// terminal chunk carries `Length` — covering the buffer→event→
/// translator integration, not just the translator in isolation.
fn drive_stream(raw_chunks: &[&[u8]]) -> Vec<TokenChunk> {
    let mut buf = SseBuffer::new();
    let mut translator = AnthropicEventTranslator::new();
    let mut chunks = Vec::new();
    for raw in raw_chunks {
        buf.extend(raw);
        while let Some(ev) = buf.next_event() {
            if let Some(chunk) = translator.step(&ev).unwrap() {
                chunks.push(chunk);
            }
        }
    }
    chunks
}

#[test]
fn stream_loop_threads_max_tokens_onto_terminal_chunk() {
    let chunks = drive_stream(&[
            b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hi\"}}\n\n",
            b"event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"max_tokens\"}}\n\n",
            b"event: message_stop\ndata: {}\n\n",
        ]);
    let text: String = chunks.iter().map(|c| c.text.as_str()).collect();
    assert_eq!(text, "Hi");
    let terminal = chunks.last().expect("a terminal chunk");
    assert!(terminal.done);
    assert_eq!(terminal.finish_reason, FinishReason::Length);
}

#[test]
fn stream_loop_defaults_terminal_chunk_to_stop() {
    let chunks = drive_stream(&[
            b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Done\"}}\n\n",
            b"event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n\n",
            b"event: message_stop\ndata: {}\n\n",
        ]);
    let terminal = chunks.last().expect("a terminal chunk");
    assert!(terminal.done);
    assert_eq!(terminal.finish_reason, FinishReason::Stop);
}

#[test]
fn parse_anthropic_event_skips_ignorable_events() {
    for name in [
        "ping",
        "message_start",
        "content_block_start",
        "content_block_stop",
        "message_delta",
    ] {
        let ev = SseEvent {
            event: name.to_string(),
            data: "{}".to_string(),
        };
        assert!(
            parse_anthropic_event(&ev).unwrap().is_none(),
            "expected {name} to be ignored"
        );
    }
}

#[test]
fn api_request_omits_top_p() {
    // Anthropic's newer models (e.g. claude-sonnet-4-6) reject requests
    // that set both `temperature` and `top_p`. We pick temperature as the
    // canonical knob and skip `top_p` from the wire format entirely.
    let req = ApiRequest {
        model: "claude-sonnet-4-6".to_string(),
        max_tokens: 100,
        system: "s".to_string(),
        messages: vec![],
        temperature: 0.7,
        stop_sequences: vec![],
        stream: true,
    };
    let json = serde_json::to_string(&req).unwrap();
    assert!(
        !json.contains("top_p"),
        "top_p must not be in the request body, got: {json}"
    );
    assert!(
        json.contains("\"temperature\":0.7"),
        "temperature should still be sent"
    );
}

#[test]
fn sse_buffer_strips_only_one_space_after_data_prefix() {
    // Per the SSE spec, ONE optional leading space is stripped after
    // `data:`. Additional whitespace must be preserved as part of the
    // payload. Anthropic's JSON never relies on this, but if we ever
    // reuse this parser for another source it must not corrupt payloads.
    let mut buf = SseBuffer::new();
    buf.extend(b"event: x\ndata:   spaced\n\n");
    let ev = buf.next_event().unwrap();
    assert_eq!(ev.data, "  spaced");
}

#[test]
fn parse_anthropic_event_overloaded_error_is_typed_service_unavailable() {
    // Mid-stream `overloaded_error` events must surface as the typed
    // ServiceUnavailable variant so the i18n layer renders the right
    // friendly message (otherwise users see the generic Other fallback
    // even though Anthropic told us exactly what went wrong).
    let ev = SseEvent {
        event: "error".to_string(),
        data: r#"{"type":"error","error":{"type":"overloaded_error","message":"slow down"}}"#
            .to_string(),
    };
    let err = parse_anthropic_event(&ev).unwrap_err();
    match err {
        primer_core::error::PrimerError::Inference(
            primer_core::error::InferenceError::ServiceUnavailable,
        ) => {}
        other => panic!("expected typed ServiceUnavailable, got {other:?}"),
    }
}

#[test]
fn parse_anthropic_event_rate_limit_error_is_typed_rate_limited() {
    let ev = SseEvent {
        event: "error".to_string(),
        data: r#"{"type":"error","error":{"type":"rate_limit_error","message":"too many"}}"#
            .to_string(),
    };
    let err = parse_anthropic_event(&ev).unwrap_err();
    match err {
        primer_core::error::PrimerError::Inference(
            primer_core::error::InferenceError::RateLimited { retry_after: None },
        ) => {}
        other => panic!("expected typed RateLimited{{None}}, got {other:?}"),
    }
}

#[test]
fn parse_anthropic_event_authentication_error_is_typed_auth() {
    let ev = SseEvent {
        event: "error".to_string(),
        data: r#"{"type":"error","error":{"type":"authentication_error","message":"bad key"}}"#
            .to_string(),
    };
    let err = parse_anthropic_event(&ev).unwrap_err();
    match err {
        primer_core::error::PrimerError::Inference(primer_core::error::InferenceError::Auth) => {}
        other => panic!("expected typed Auth, got {other:?}"),
    }
}
