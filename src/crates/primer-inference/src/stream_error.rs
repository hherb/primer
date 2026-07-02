//! Shared mid-stream error-payload detection for streaming backends.
//!
//! After a 2xx handshake, servers report generation failure inside the
//! stream itself: Ollama as an NDJSON line `{"error":"..."}` (runner
//! crash, OOM, model unloaded), OpenAI-compatible servers as a `data:`
//! event `{"error": {"message": "...", ...}}` — and some (OpenRouter,
//! proxies) attach the `error` object to an otherwise well-formed chunk
//! that still carries a `choices` array. Because that last shape
//! deserializes cleanly as a normal chunk, callers must probe EVERY
//! payload with this helper before (not only after) chunk parsing —
//! coupling detection to chunk-parse failure silently swallows it, and
//! the truncated reply then finishes as a clean turn, bypassing the
//! partial-turn-drops-on-error invariant.

use serde::Deserialize;

/// Extract the error message from a mid-stream error payload, if the
/// payload carries one. Handles both the object shape
/// `{"error": {"message": "..."}}` (OpenAI-compat servers) and the
/// bare-string shape `{"error": "..."}` (Ollama). Returns `None` for
/// normal chunks, `[DONE]`, and garbage — anything without a top-level
/// `error` field of a known shape.
pub(crate) fn parse_stream_error_payload(payload: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct ErrorPayload {
        error: ErrorBody,
    }
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum ErrorBody {
        Object { message: String },
        Text(String),
    }
    serde_json::from_str::<ErrorPayload>(payload)
        .ok()
        .map(|p| match p.error {
            ErrorBody::Object { message } => message,
            ErrorBody::Text(t) => t,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_object_shape() {
        assert_eq!(
            parse_stream_error_payload(
                r#"{"error": {"message": "upstream unavailable", "code": 502}}"#
            )
            .as_deref(),
            Some("upstream unavailable")
        );
    }

    #[test]
    fn detects_string_shape() {
        assert_eq!(
            parse_stream_error_payload(r#"{"error": "model crashed"}"#).as_deref(),
            Some("model crashed")
        );
    }

    #[test]
    fn detects_error_alongside_valid_chunk_fields() {
        // OpenRouter-style: the error object rides on an otherwise
        // well-formed chunk. This is exactly why callers probe every
        // payload rather than only unparseable ones.
        assert_eq!(
            parse_stream_error_payload(
                r#"{"id":"x","choices":[{"delta":{},"finish_reason":"error"}],"error":{"message":"provider failed"}}"#
            )
            .as_deref(),
            Some("provider failed")
        );
    }

    #[test]
    fn ignores_normal_chunks_done_marker_and_garbage() {
        assert!(
            parse_stream_error_payload(r#"{"choices":[{"delta":{"content":"hi"}}]}"#).is_none()
        );
        assert!(
            parse_stream_error_payload(r#"{"message":{"content":"Hello"},"done":false}"#).is_none()
        );
        assert!(parse_stream_error_payload("[DONE]").is_none());
        assert!(parse_stream_error_payload("not json").is_none());
        // A null error field is not a reportable error.
        assert!(parse_stream_error_payload(r#"{"error":null,"choices":[]}"#).is_none());
    }
}
