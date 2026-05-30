//! Parse the GUI Settings "reasoning markers" textarea into the
//! `(open, close)` pairs the inference backends consume.
//!
//! The textarea holds free text: one `open<whitespace>close` pair per
//! line. This is the GUI counterpart to the CLI's `--reasoning-marker`
//! flag (which receives pre-tokenised clap pairs). Keeping the parse in
//! pure Rust — rather than in `settings.js` — means it is exhaustively
//! unit-tested and the frontend stays a verbatim pass-through.

/// Parse free textarea text into `(open, close)` reasoning-marker pairs.
///
/// Rules:
/// - Each line is trimmed, then split on its **first** whitespace run:
///   `open` = the text before it, `close` = the remainder, trimmed.
/// - A line is dropped if it has no whitespace (open only, no close) or
///   if `close` is empty after trimming. This mirrors the CLI dropping
///   an incomplete pair — no error, no warning.
/// - Blank / whitespace-only lines are ignored.
/// - `close` is "the rest of the line", so a close marker may contain
///   internal spaces (e.g. `<a> </a> tail` → `("<a>", "</a> tail")`).
///
/// Empty input yields an empty `Vec`, which means "built-in defaults
/// only" downstream.
pub fn parse_reasoning_markers(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            // `split_once` on the first whitespace char; `None` means the
            // line has no whitespace at all (open only) → drop it.
            let (open, close) = line.split_once(char::is_whitespace)?;
            let close = close.trim();
            if open.is_empty() || close.is_empty() {
                return None;
            }
            Some((open.to_string(), close.to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::parse_reasoning_markers;

    fn pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
        v.iter()
            .map(|(o, c)| (o.to_string(), c.to_string()))
            .collect()
    }

    #[test]
    fn empty_input_is_empty() {
        assert_eq!(parse_reasoning_markers(""), Vec::<(String, String)>::new());
    }

    #[test]
    fn single_pair() {
        assert_eq!(
            parse_reasoning_markers("<think> </think>"),
            pairs(&[("<think>", "</think>")])
        );
    }

    #[test]
    fn multiple_lines_in_order() {
        assert_eq!(
            parse_reasoning_markers("<a> </a>\n<b> </b>"),
            pairs(&[("<a>", "</a>"), ("<b>", "</b>")])
        );
    }

    #[test]
    fn leading_and_trailing_whitespace_trimmed() {
        assert_eq!(
            parse_reasoning_markers("   <a>   </a>   "),
            pairs(&[("<a>", "</a>")])
        );
    }

    #[test]
    fn blank_lines_ignored() {
        assert_eq!(
            parse_reasoning_markers("<a> </a>\n\n   \n<b> </b>"),
            pairs(&[("<a>", "</a>"), ("<b>", "</b>")])
        );
    }

    #[test]
    fn crlf_line_endings_handled() {
        assert_eq!(
            parse_reasoning_markers("<a> </a>\r\n<b> </b>"),
            pairs(&[("<a>", "</a>"), ("<b>", "</b>")])
        );
    }

    #[test]
    fn open_only_line_is_dropped() {
        assert_eq!(
            parse_reasoning_markers("<a>"),
            Vec::<(String, String)>::new()
        );
    }

    #[test]
    fn open_with_trailing_whitespace_only_is_dropped() {
        // After trimming, the line is just "<a>" with no whitespace → no close.
        assert_eq!(
            parse_reasoning_markers("<a>    "),
            Vec::<(String, String)>::new()
        );
    }

    #[test]
    fn tab_separator_works() {
        assert_eq!(
            parse_reasoning_markers("<a>\t</a>"),
            pairs(&[("<a>", "</a>")])
        );
    }

    #[test]
    fn close_with_internal_spaces_preserved() {
        assert_eq!(
            parse_reasoning_markers("<a> </a> tail"),
            pairs(&[("<a>", "</a> tail")])
        );
    }
}
