//! Shared helpers for LLM-output processing.
//!
//! Used by every structured-output classifier/extractor in the
//! workspace (`primer-classifier`, `primer-extractor`,
//! `primer-comprehension`). Pulled out of the individual crates to a
//! shared home so a fix to either helper applies everywhere; previously
//! both functions were duplicated verbatim across two crates.

/// Return a prefix of `s` that is at most `max_chars` Unicode scalar
/// values long. Unlike a raw byte-index slice, this never panics on
/// multi-byte character boundaries.
pub fn truncate_to_chars(s: &str, max_chars: usize) -> &str {
    if s.chars().count() <= max_chars {
        return s;
    }
    let end = s
        .char_indices()
        .nth(max_chars)
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    &s[..end]
}

/// Extract the first balanced JSON object from `raw`. Tolerates
/// wrapper text before/after, and JSON containing nested braces.
/// Returns the substring including the outer braces, or `None` if no
/// balanced object is found.
///
/// Walks the bytes once, tracking string state and escape state so a
/// brace inside a string literal does not count toward the depth.
pub fn extract_first_json_object(raw: &str) -> Option<&str> {
    let start = raw.find('{')?;
    let bytes = raw.as_bytes();
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape = false;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        if escape {
            escape = false;
            continue;
        }
        if in_string {
            match b {
                b'\\' => escape = true,
                b'"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&raw[start..=i]);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_returns_input_when_within_cap() {
        assert_eq!(truncate_to_chars("hello", 10), "hello");
    }

    #[test]
    fn truncate_cuts_to_char_count() {
        assert_eq!(truncate_to_chars("hello world", 5), "hello");
    }

    #[test]
    fn truncate_is_safe_on_multibyte_boundaries() {
        // "café" is 5 bytes (4 chars). Cutting at char 3 must stop
        // before the multibyte 'é', not slice through it.
        let s = "café";
        let cut = truncate_to_chars(s, 3);
        assert_eq!(cut, "caf");
    }

    #[test]
    fn extract_returns_simple_object() {
        assert_eq!(extract_first_json_object(r#"{"a":1}"#), Some(r#"{"a":1}"#));
    }

    #[test]
    fn extract_handles_leading_and_trailing_text() {
        assert_eq!(
            extract_first_json_object(r#"junk before {"a":1} junk after"#),
            Some(r#"{"a":1}"#)
        );
    }

    #[test]
    fn extract_balances_nested_braces() {
        assert_eq!(
            extract_first_json_object(r#"x {"a":{"b":2}} y"#),
            Some(r#"{"a":{"b":2}}"#)
        );
    }

    #[test]
    fn extract_ignores_braces_inside_strings() {
        assert_eq!(
            extract_first_json_object(r#"{"text": "}{"}"#),
            Some(r#"{"text": "}{"}"#)
        );
    }

    #[test]
    fn extract_ignores_escaped_quotes_inside_strings() {
        // The escaped quote does not close the string; the outer
        // brace at the end closes the object.
        assert_eq!(
            extract_first_json_object(r#"{"x":"a\"b"}"#),
            Some(r#"{"x":"a\"b"}"#)
        );
    }

    #[test]
    fn extract_returns_none_when_unbalanced() {
        assert_eq!(extract_first_json_object("{ unclosed"), None);
    }

    #[test]
    fn extract_returns_none_when_no_brace() {
        assert_eq!(extract_first_json_object("plain text"), None);
    }
}
