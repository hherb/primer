//! Content-Security-Policy contract pinned against the checked-in
//! `tauri.conf.json`. No runtime code lives here — the module's only
//! purpose is to regression-test that the CSP shipped in the app's
//! security configuration stays tight.
//!
//! Why a dedicated module instead of an inline test next to runtime
//! code: the CSP is a static asset, not a function. Pinning the shape
//! at the file level keeps the contract discoverable (`grep -rn csp
//! src/`) and isolates the JSON-parsing dep on `tests` builds.
//!
//! See issue #71 for the long-form rationale. The codebase ships with
//! no inline `<style>` blocks, no inline `<script>` blocks, no
//! `style="..."` attributes, no inline event handlers, no
//! `eval`/`new Function`, and no `setAttribute('style', ...)` calls,
//! so the two `'unsafe-inline'` allowances that were present in the
//! original Tauri scaffold can be dropped without functional change.
//! JS-side `element.style.foo = bar` writes are NOT covered by CSP's
//! `style-src` directive — only HTML-parsed `style="..."` attributes
//! and `<style>` blocks are — so the existing UI's dynamic styling
//! continues to work after the tightening.

#[cfg(test)]
mod tests {
    use serde_json::Value;
    use std::collections::BTreeSet;

    /// The CSP string baked into the binary at build time. `include_str!`
    /// resolves relative to this source file; `tauri.conf.json` lives at
    /// the crate root one level up.
    const TAURI_CONF: &str = include_str!("../tauri.conf.json");

    /// CSP keywords whose presence in the shipped policy is a regression.
    /// `'unsafe-inline'` is the headline offender that issue #71 set out
    /// to remove; `'unsafe-eval'` is listed defensively so a future
    /// config edit can't sneak it in unobserved.
    const FORBIDDEN_CSP_KEYWORDS: &[&str] = &["'unsafe-inline'", "'unsafe-eval'"];

    /// Directives the GUI relies on. If any of these go missing the
    /// app's network/asset surface breaks (Tauri IPC over the
    /// `ipc.localhost` shim, data: URLs for the inline brand mark,
    /// the default-src self-restriction itself).
    const REQUIRED_CSP_DIRECTIVES: &[&str] = &[
        "default-src",
        "script-src",
        "style-src",
        "img-src",
        "connect-src",
    ];

    fn csp_string() -> String {
        let v: Value = serde_json::from_str(TAURI_CONF).expect("tauri.conf.json parses as JSON");
        v.get("app")
            .and_then(|a| a.get("security"))
            .and_then(|s| s.get("csp"))
            .and_then(Value::as_str)
            .expect("app.security.csp is a string")
            .to_string()
    }

    /// Split the CSP into `(directive_name, sources)` pairs. Whitespace-
    /// tolerant (handles tabs, repeated spaces, trailing `;`, and empty
    /// segments) so the assertions below don't double as JSON-formatter
    /// regression tests — a reformatter that swaps a single space for a
    /// tab must not be able to false-fail the CSP contract.
    ///
    /// Structural tokenisation also prevents the "substring match"
    /// failure mode where e.g. asserting `csp.contains("script-src")`
    /// would silently pass a typo'd `"script-source"`.
    fn parse_csp(csp: &str) -> Vec<(&str, Vec<&str>)> {
        csp.split(';')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(|part| {
                let mut tokens = part.split_whitespace();
                let name = tokens
                    .next()
                    .expect("non-empty after trim => at least one token");
                (name, tokens.collect())
            })
            .collect()
    }

    #[test]
    fn csp_does_not_grant_unsafe_inline_or_eval() {
        let csp = csp_string();
        for (name, sources) in parse_csp(&csp) {
            for src in sources {
                assert!(
                    !FORBIDDEN_CSP_KEYWORDS.contains(&src),
                    "directive `{name}` lists forbidden source `{src}`; \
                     see issue #71 for rationale. Full CSP: {csp}"
                );
            }
        }
    }

    #[test]
    fn csp_declares_required_directives() {
        let csp = csp_string();
        let declared: BTreeSet<&str> = parse_csp(&csp).into_iter().map(|(n, _)| n).collect();
        for required in REQUIRED_CSP_DIRECTIVES {
            assert!(
                declared.contains(required),
                "tauri.conf.json CSP is missing required directive `{required}`. \
                 Declared: {declared:?}. Full CSP: {csp}"
            );
        }
    }

    #[test]
    fn csp_self_origin_is_the_default_source() {
        let csp = csp_string();
        let directives = parse_csp(&csp);
        let default_src: &[&str] = directives
            .iter()
            .find(|(n, _)| *n == "default-src")
            .map(|(_, srcs)| srcs.as_slice())
            .expect("default-src must be declared (see csp_declares_required_directives)");
        assert!(
            default_src.contains(&"'self'"),
            "tauri.conf.json `default-src` must include `'self'` so anything \
             not explicitly allowlisted is rejected. Got: {default_src:?}. \
             Full CSP: {csp}"
        );
    }

    #[test]
    fn parse_csp_tokenises_directives_and_sources() {
        let pairs = parse_csp("a 'self'; b 'unsafe-inline' data:; c");
        assert_eq!(
            pairs,
            vec![
                ("a", vec!["'self'"]),
                ("b", vec!["'unsafe-inline'", "data:"]),
                ("c", vec![]),
            ]
        );
    }

    #[test]
    fn parse_csp_tolerates_extra_whitespace_and_trailing_semicolons() {
        let pairs = parse_csp("  a\t'self'  ;;  b  'self';");
        assert_eq!(pairs, vec![("a", vec!["'self'"]), ("b", vec!["'self'"])]);
    }
}
