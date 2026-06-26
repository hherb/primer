use super::*;
use primer_core::error::InferenceError;
use reqwest::StatusCode;
use reqwest::header::{HeaderMap, HeaderValue};
use std::time::Duration;

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
    let e = classify_anthropic_status(StatusCode::UNAUTHORIZED, &empty_headers(), "");
    assert!(matches!(e, InferenceError::Auth));
}

#[test]
fn classifies_403_as_auth() {
    let e = classify_anthropic_status(StatusCode::FORBIDDEN, &empty_headers(), "");
    assert!(matches!(e, InferenceError::Auth));
}

#[test]
fn classifies_429_with_retry_after_header() {
    let e = classify_anthropic_status(
        StatusCode::TOO_MANY_REQUESTS,
        &headers_with_retry_after(7),
        "rate limited",
    );
    match e {
        InferenceError::RateLimited {
            retry_after: Some(d),
        } => assert_eq!(d, Duration::from_secs(7)),
        other => panic!("expected RateLimited(Some(7s)), got {other:?}"),
    }
}

#[test]
fn classifies_429_without_retry_after_header() {
    let e = classify_anthropic_status(StatusCode::TOO_MANY_REQUESTS, &empty_headers(), "");
    assert!(matches!(
        e,
        InferenceError::RateLimited { retry_after: None }
    ));
}

#[test]
fn classifies_500_as_service_unavailable() {
    let e = classify_anthropic_status(StatusCode::INTERNAL_SERVER_ERROR, &empty_headers(), "");
    assert!(matches!(e, InferenceError::ServiceUnavailable));
}

#[test]
fn classifies_503_as_service_unavailable() {
    let e = classify_anthropic_status(StatusCode::SERVICE_UNAVAILABLE, &empty_headers(), "");
    assert!(matches!(e, InferenceError::ServiceUnavailable));
}

#[test]
fn classifies_unknown_4xx_as_other_with_status_in_body() {
    let e = classify_anthropic_status(StatusCode::IM_A_TEAPOT, &empty_headers(), "tea");
    match e {
        InferenceError::Other(s) => {
            assert!(s.contains("418") && s.contains("tea"), "got: {s}")
        }
        other => panic!("expected Other, got {other:?}"),
    }
}

#[test]
fn parse_retry_after_handles_integer_seconds() {
    let h = headers_with_retry_after(42);
    assert_eq!(parse_retry_after(&h), Some(Duration::from_secs(42)));
}

#[test]
fn parse_retry_after_handles_missing_header() {
    assert_eq!(parse_retry_after(&empty_headers()), None);
}

#[test]
fn parse_retry_after_drops_http_date_form() {
    // HTTP-date form is silently dropped (returns None) rather than
    // attempting to parse the date — see spec §Retry helper.
    let mut h = HeaderMap::new();
    h.insert(
        "retry-after",
        HeaderValue::from_static("Wed, 21 Oct 2026 07:28:00 GMT"),
    );
    assert_eq!(parse_retry_after(&h), None);
}

#[test]
fn classify_error_event_overloaded_is_service_unavailable() {
    let e = classify_anthropic_error_event("overloaded_error", "slow down");
    assert!(matches!(e, InferenceError::ServiceUnavailable));
}

#[test]
fn classify_error_event_api_error_is_service_unavailable() {
    let e = classify_anthropic_error_event("api_error", "internal blip");
    assert!(matches!(e, InferenceError::ServiceUnavailable));
}

#[test]
fn classify_error_event_rate_limit_is_rate_limited_no_retry_after() {
    // Mid-stream JSON error events have no Retry-After header
    // available; surface RateLimited with retry_after=None.
    let e = classify_anthropic_error_event("rate_limit_error", "too many");
    assert!(matches!(
        e,
        InferenceError::RateLimited { retry_after: None }
    ));
}

#[test]
fn classify_error_event_authentication_is_auth() {
    let e = classify_anthropic_error_event("authentication_error", "bad key");
    assert!(matches!(e, InferenceError::Auth));
}

#[test]
fn classify_error_event_permission_is_auth() {
    let e = classify_anthropic_error_event("permission_error", "no access");
    assert!(matches!(e, InferenceError::Auth));
}

#[test]
fn classify_error_event_unknown_type_falls_through_to_other() {
    let e = classify_anthropic_error_event("invalid_request_error", "bad json");
    match e {
        InferenceError::Other(s) => {
            assert!(
                s.contains("invalid_request_error") && s.contains("bad json"),
                "Other should surface both type and message for diagnostics, got: {s}"
            );
        }
        other => panic!("expected Other for unmapped type, got {other:?}"),
    }
}
