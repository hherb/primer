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

    #[test]
    fn csp_does_not_grant_unsafe_inline_or_eval() {
        let csp = csp_string();
        for forbidden in FORBIDDEN_CSP_KEYWORDS {
            assert!(
                !csp.contains(forbidden),
                "tauri.conf.json CSP contains forbidden token `{forbidden}`; \
                 see issue #71 for rationale. Full CSP: {csp}"
            );
        }
    }

    #[test]
    fn csp_declares_required_directives() {
        let csp = csp_string();
        for directive in REQUIRED_CSP_DIRECTIVES {
            assert!(
                csp.contains(directive),
                "tauri.conf.json CSP is missing required directive `{directive}`. \
                 Full CSP: {csp}"
            );
        }
    }

    #[test]
    fn csp_self_origin_is_the_default_source() {
        let csp = csp_string();
        assert!(
            csp.contains("default-src 'self'"),
            "tauri.conf.json must default to `'self'` so anything not \
             explicitly allowlisted is rejected. Full CSP: {csp}"
        );
    }
}
