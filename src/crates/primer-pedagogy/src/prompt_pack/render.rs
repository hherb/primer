//! Placeholder validation and brace-aware template rendering.
//!
//! Pure functions shared by the pack loader (`toml_pack`) and its tests.
//! They implement the issue-#20 brace-escape rule (`{{` / `}}` → literal
//! `{` / `}`, like Rust `format!`) consistently across validation and
//! rendering so a translator's escaped braces never trip the validator
//! and always render as literals.

use primer_core::error::{PrimerError, Result};

/// Scan `content` for `{ident}` placeholders and return Err if any
/// token is not in `allowed`. Identifier rule: ASCII alpha or `_` first
/// char, then ASCII alphanumeric or `_`. Anything else inside `{...}`
/// (e.g. `{Hello, world}`) is left alone — translators can use brace
/// characters in narrative text without false positives.
///
/// `{{` / `}}` are the escape for a literal `{` / `}` (issue #20): a
/// doubled open-brace is skipped here and unescaped at render time by
/// [`render_template`], so narrative like `{{Beispiel}}` is never flagged.
/// A single `{Beispiel}` is still treated as a placeholder attempt and
/// rejected — translators must double-up to get a literal.
pub(crate) fn validate_placeholders(field: &str, content: &str, allowed: &[&str]) -> Result<()> {
    let bytes = content.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            // Escaped literal `{{` — not a placeholder delimiter.
            if bytes.get(i + 1) == Some(&b'{') {
                i += 2;
                continue;
            }
            let start = i + 1;
            let mut end = start;
            while end < bytes.len() && bytes[end] != b'}' {
                end += 1;
            }
            if end < bytes.len() {
                let token = &content[start..end];
                if is_placeholder_ident(token) && !allowed.contains(&token) {
                    return Err(PrimerError::Config(format!(
                        "prompt pack: field {field} contains unknown placeholder {{{token}}}; allowed: {allowed:?}"
                    )));
                }
                i = end + 1;
            } else {
                break;
            }
        } else {
            i += 1;
        }
    }
    Ok(())
}

fn is_placeholder_ident(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Render `template`, substituting `{key}` for the matching `vars` value
/// and treating `{{` / `}}` as literal `{` / `}` (the issue-#20
/// brace-escape, familiar from Rust `format!`).
///
/// A single left-to-right pass — substituted values are emitted as-is and
/// never re-scanned, so `{{name}}` always renders as the literal `{name}`
/// even when `name` is a known var. A naive `str::replace` chain cannot do
/// this: `{{name}}` contains the substring `{name}`, which `replace` would
/// wrongly interpolate.
///
/// A `{...}` whose contents match no var is emitted verbatim (e.g.
/// narrative `{Hello, world}` that survives [`validate_placeholders`]).
/// Unterminated `{` and lone `}` are emitted verbatim rather than erroring
/// — validation already runs at load time, so the renderer is lenient.
pub(crate) fn render_template(template: &str, vars: &[(&str, &str)]) -> String {
    let bytes = template.as_bytes();
    let mut out = String::with_capacity(template.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => {
                if bytes.get(i + 1) == Some(&b'{') {
                    out.push('{');
                    i += 2;
                    continue;
                }
                let start = i + 1;
                let mut end = start;
                while end < bytes.len() && bytes[end] != b'}' {
                    end += 1;
                }
                if end >= bytes.len() {
                    // Unterminated `{` — emit the remainder verbatim.
                    out.push_str(&template[i..]);
                    break;
                }
                let key = &template[start..end];
                match vars.iter().find(|(k, _)| *k == key) {
                    Some((_, val)) => out.push_str(val),
                    None => out.push_str(&template[i..=end]),
                }
                i = end + 1;
            }
            b'}' => {
                // `}}` is a literal `}`; a lone `}` is emitted as-is.
                if bytes.get(i + 1) == Some(&b'}') {
                    i += 1;
                }
                out.push('}');
                i += 1;
            }
            _ => {
                // Copy the run of non-brace bytes in one go. Braces are
                // ASCII, so the run boundaries are always char boundaries.
                let run_start = i;
                while i < bytes.len() && bytes[i] != b'{' && bytes[i] != b'}' {
                    i += 1;
                }
                out.push_str(&template[run_start..i]);
            }
        }
    }
    out
}

/// Unescape `{{` / `}}` to literal braces in a field that carries no
/// interpolated placeholders (validated with an empty allowlist). Defined
/// in terms of [`render_template`] with no vars so the escape rule is
/// identical everywhere.
pub(crate) fn unescape_braces(s: &str) -> String {
    render_template(s, &[])
}
