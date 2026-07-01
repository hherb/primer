//! FTS5 query sanitization.

/// Coerce free-form input into an FTS5-safe phrase. Strips non-alphanumerics
/// (so any FTS5-special character is inert), drops reserved keywords
/// (`AND`, `OR`, `NOT`, `NEAR`), quotes each surviving token, and joins
/// them with explicit `OR`. An empty result means "no useful tokens";
/// the caller short-circuits to empty results rather than issuing
/// `MATCH ''` which FTS5 rejects.
///
/// `OR` is chosen over implicit-AND so a natural-language query like
/// "how does the sun shine" surfaces passages that mention any of
/// `sun`/`shine` rather than requiring all five words. BM25 ranking +
/// the caller's `LIMIT k` keep the list focused on the most relevant.
///
/// Mirrors the same-named function in `primer-storage`. A future cleanup
/// should hoist both into `primer-core::fts_util`.
pub(crate) fn sanitize_fts_phrase(query: &str) -> String {
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
