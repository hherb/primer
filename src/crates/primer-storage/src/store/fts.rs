//! FTS5 query sanitisation.
//!
//! Child input is fed through `sanitize_fts_phrase` before being passed
//! to `MATCH` so that user text can never break out of the FTS5 query
//! grammar. BM25 ranking + the caller's `LIMIT k` keep the OR-join from
//! producing noise.

/// Sanitize an arbitrary user query into an FTS5 expression that is
/// safe to pass to `MATCH`. Tokenizes on whitespace, strips every
/// non-alphanumeric character per token (kills `*`, `^`, `:`, `"`, `(`,
/// `)`, slashes, etc.), drops the FTS5 reserved keywords (`AND`, `OR`,
/// `NOT`, `NEAR`), wraps each surviving token in double quotes (so any
/// special character the tokenizer would otherwise see is inert), and
/// joins the tokens with explicit `OR`. An empty result means "no
/// useful tokens"; the caller should skip the query rather than issue
/// `MATCH ''` which FTS5 rejects.
///
/// `OR` is chosen over implicit-AND so that "noise" tokens introduced
/// by sanitization (e.g. fragments from stripped punctuation) do not
/// torpedo the entire query. BM25 ranking + the caller's `LIMIT k` keep
/// the result list focused on the most relevant matches.
pub(super) fn sanitize_fts_phrase(query: &str) -> String {
    const RESERVED: &[&str] = &["AND", "OR", "NOT", "NEAR"];
    let tokens: Vec<String> = query
        .split_whitespace()
        .map(|tok| {
            tok.chars()
                .filter(|c| c.is_alphanumeric())
                .collect::<String>()
        })
        .filter(|tok| !tok.is_empty())
        .filter(|tok| !RESERVED.iter().any(|r| r.eq_ignore_ascii_case(tok)))
        .collect();
    if tokens.is_empty() {
        return String::new();
    }
    tokens
        .iter()
        .map(|t| format!("\"{t}\""))
        .collect::<Vec<_>>()
        .join(" OR ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_returns_empty() {
        assert_eq!(sanitize_fts_phrase(""), "");
        assert_eq!(sanitize_fts_phrase("   "), "");
    }

    #[test]
    fn single_token_is_quoted() {
        assert_eq!(sanitize_fts_phrase("photosynthesis"), "\"photosynthesis\"");
    }

    #[test]
    fn multiple_tokens_are_or_joined() {
        assert_eq!(
            sanitize_fts_phrase("how does gravity work"),
            "\"how\" OR \"does\" OR \"gravity\" OR \"work\""
        );
    }

    #[test]
    fn reserved_keywords_are_dropped() {
        // AND / OR / NOT / NEAR (case-insensitive) are stripped.
        assert_eq!(sanitize_fts_phrase("dogs AND cats"), "\"dogs\" OR \"cats\"");
        assert_eq!(
            sanitize_fts_phrase("apple or pear"),
            "\"apple\" OR \"pear\""
        );
        assert_eq!(sanitize_fts_phrase("near the not"), "\"the\"");
    }

    #[test]
    fn special_chars_are_stripped() {
        // FTS5-active punctuation (* : " ( ) ^) is filtered per-token.
        assert_eq!(
            sanitize_fts_phrase("\"quoted\" (paren) star*"),
            "\"quoted\" OR \"paren\" OR \"star\""
        );
    }

    #[test]
    fn alphanumeric_unicode_is_preserved() {
        // is_alphanumeric is Unicode-aware.
        assert_eq!(sanitize_fts_phrase("café résumé"), "\"café\" OR \"résumé\"");
    }

    #[test]
    fn punctuation_only_input_yields_empty() {
        assert_eq!(sanitize_fts_phrase("!!! ??? ***"), "");
    }
}
