//! Pin the contract that modal dialogs use the native `<dialog>` element
//! opened via `showModal()`, so the browser supplies focus-trap, Escape,
//! and inert-background behaviour for free.
//!
//! Why a dedicated module instead of inline tests: the contract spans
//! three static assets (`ui/index.html`, `ui/settings.js`, `ui/voice.js`)
//! plus the stylesheet. Pinning the shape at the crate level keeps the
//! contract discoverable (`grep -rn modal_dialog_contract src/`) and
//! makes the failure mode loud — a future edit that reverts a modal to
//! the old `<div class="modal-backdrop"><div class="modal">` pattern
//! breaks `cargo test --workspace` rather than silently regressing
//! accessibility.
//!
//! See issue #81 for the long-form rationale. The settings modal (and
//! the voice-consent modal that shares its markup pattern) previously
//! used `<div role="dialog">` toggled via the `hidden` attribute. That
//! shape required a manual focus trap to prevent Tab from drifting onto
//! the chat shell behind the dim backdrop. Native `<dialog>` opened with
//! `HTMLDialogElement.showModal()` traps focus inside the dialog,
//! Escape-dismisses via a `cancel` event, and renders an inert
//! `::backdrop` pseudo-element — all UA-provided. The tests below pin
//! the two load-bearing facts:
//!   1. Both modal elements are `<dialog>` (not `<div>`).
//!   2. Both controllers call `.showModal()` (not just `.hidden = false`).

#[cfg(test)]
mod tests {
    /// Static-embed the UI assets that carry the contract. `include_str!`
    /// resolves relative to this source file; the UI lives at the crate
    /// root under `ui/`.
    const INDEX_HTML: &str = include_str!("../ui/index.html");
    const SETTINGS_JS: &str = include_str!("../ui/settings.js");
    const VOICE_JS: &str = include_str!("../ui/voice.js");
    const STYLES_CSS: &str = include_str!("../ui/styles.css");

    /// Modal element ids that must be `<dialog>` elements opened via
    /// `showModal()`. Driven from one list so adding a third modal in
    /// the future is one edit, not three.
    const MODAL_DIALOG_IDS: &[&str] = &["settings-modal", "voice-consent-modal"];

    /// Locate the opening tag in `haystack` that carries `id="<id>"` and
    /// return the tag name (the word after `<`). Returns `None` if no
    /// such id is found, or if the structure is malformed (no `<` before
    /// the id). Whitespace-tolerant: works for both `<dialog id="x"`
    /// and `<dialog\n      id="x"` shapes.
    ///
    /// Walks back from the id attribute to the nearest `<`, then grabs
    /// the next ASCII-alphanumeric run — the HTML tag name. Avoids the
    /// failure mode of substring-matching `<dialog` somewhere unrelated
    /// in the file.
    ///
    /// **Quoting:** only matches double-quoted `id="…"`. The codebase
    /// uses double quotes uniformly in HTML; a single-quoted id would
    /// silently report `None` here. Deliberate YAGNI — adding
    /// single-quote support invites question of which quoting style is
    /// canonical, and we'd rather flag unexpected variance than paper
    /// over it.
    fn tag_for_id<'a>(haystack: &'a str, id: &str) -> Option<&'a str> {
        let needle = format!("id=\"{id}\"");
        let id_pos = haystack.find(&needle)?;
        let prefix = &haystack[..id_pos];
        let lt_pos = prefix.rfind('<')?;
        let after_lt = &haystack[lt_pos + 1..];
        let tag_end = after_lt
            .find(|c: char| !c.is_ascii_alphanumeric())
            .unwrap_or(after_lt.len());
        Some(&after_lt[..tag_end])
    }

    /// Strip `// ... EOL` and `/* ... */` comments from a JS source so
    /// substring assertions don't false-pass on commented-out code.
    /// Naive: doesn't track string or regex literals, so `"//"` inside a
    /// string would still be treated as a comment marker. Adequate for
    /// the contract assertions here — no UI file embeds `//` inside a
    /// string that wraps a load-bearing identifier.
    fn strip_js_comments(src: &str) -> String {
        let mut out = String::with_capacity(src.len());
        let mut chars = src.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '/' {
                match chars.peek() {
                    Some('*') => {
                        chars.next();
                        let mut prev = '\0';
                        for cc in chars.by_ref() {
                            if prev == '*' && cc == '/' {
                                break;
                            }
                            prev = cc;
                        }
                        continue;
                    }
                    // Line comment (handles `//`, `///`, `////…`). Keep
                    // the trailing newline so the test failure message's
                    // line numbers stay aligned with the source.
                    Some('/') => {
                        chars.next();
                        for cc in chars.by_ref() {
                            if cc == '\n' {
                                out.push('\n');
                                break;
                            }
                        }
                        continue;
                    }
                    _ => {}
                }
            }
            out.push(c);
        }
        out
    }

    #[test]
    fn every_modal_element_is_a_native_dialog() {
        for id in MODAL_DIALOG_IDS {
            let tag = tag_for_id(INDEX_HTML, id)
                .unwrap_or_else(|| panic!("no element with id=\"{id}\" found in ui/index.html"));
            assert_eq!(
                tag, "dialog",
                "element id=\"{id}\" must be a <dialog> (got <{tag}>); \
                 see issue #81. Native <dialog> + showModal() supplies \
                 the focus trap the manual <div role=\"dialog\"> shape \
                 did not."
            );
        }
    }

    #[test]
    fn no_legacy_modal_backdrop_wrapper_remains() {
        // The old shape was `<div class="modal-backdrop">` wrapping each
        // modal card. `<dialog>` exposes the backdrop via the
        // `::backdrop` pseudo-element instead, so the wrapper div must
        // be gone from the HTML.
        assert!(
            !INDEX_HTML.contains("modal-backdrop"),
            "ui/index.html still references `modal-backdrop`; the \
             native <dialog> migration (#81) removed the wrapper div."
        );
    }

    #[test]
    fn settings_controller_opens_via_show_modal() {
        // Strip comments before substring-matching so the test can't
        // false-pass on a commented-out `.showModal()` reference — e.g.
        // a stale doc comment left after the call site was reverted.
        let code = strip_js_comments(SETTINGS_JS);
        assert!(
            code.contains(".showModal()"),
            "ui/settings.js no longer calls .showModal() — the focus \
             trap supplied by the browser only kicks in when the dialog \
             is opened via HTMLDialogElement.showModal(). Toggling \
             `.hidden` or `.open` directly skips the focus-trap setup. \
             See issue #81."
        );
    }

    #[test]
    fn voice_controller_opens_via_show_modal() {
        let code = strip_js_comments(VOICE_JS);
        assert!(
            code.contains(".showModal()"),
            "ui/voice.js no longer calls .showModal() — the focus \
             trap supplied by the browser only kicks in when the dialog \
             is opened via HTMLDialogElement.showModal(). Toggling \
             `.hidden` or `.open` directly skips the focus-trap setup. \
             See issue #81."
        );
    }

    #[test]
    fn stylesheet_declares_native_backdrop_rule() {
        // The dim background previously came from `.modal-backdrop`'s
        // `background: color-mix(...)`. With native <dialog>, the same
        // styling must live on the `::backdrop` pseudo-element. Without
        // it, the modal opens over a transparent overlay and the user
        // can't tell the chat shell is inert.
        assert!(
            STYLES_CSS.contains("::backdrop"),
            "ui/styles.css does not declare a `::backdrop` rule. The \
             native <dialog> migration (#81) moved the dim-overlay \
             styling from `.modal-backdrop` onto `dialog::backdrop`."
        );
    }

    #[test]
    fn tag_for_id_walks_back_to_the_nearest_opening_bracket() {
        let html = r#"<section><dialog
              class="modal"
              id="x"
              role="dialog">content</dialog></section>"#;
        assert_eq!(tag_for_id(html, "x"), Some("dialog"));
    }

    #[test]
    fn tag_for_id_distinguishes_dialog_from_div() {
        let html = r#"<div class="legacy" id="legacy-modal"></div>"#;
        assert_eq!(tag_for_id(html, "legacy-modal"), Some("div"));
    }

    #[test]
    fn tag_for_id_returns_none_for_missing_id() {
        let html = r#"<dialog id="present"></dialog>"#;
        assert_eq!(tag_for_id(html, "absent"), None);
    }

    #[test]
    fn strip_js_comments_removes_line_comments_but_leaves_calls() {
        let src = "// dialog.showModal() — stale comment\nreal.showModal();";
        let stripped = strip_js_comments(src);
        assert!(stripped.contains(".showModal()"));
        assert!(!stripped.contains("stale comment"));
    }

    #[test]
    fn strip_js_comments_removes_block_comments() {
        let src = "/* dialog.showModal() lives here */ real.showModal();";
        let stripped = strip_js_comments(src);
        assert!(stripped.contains(".showModal()"));
        assert!(!stripped.contains("lives here"));
    }

    #[test]
    fn strip_js_comments_preserves_utf8() {
        // em-dash is two bytes; ensure char-iteration leaves it intact.
        let src = "const x = 1; // — note\nconst y = 2;";
        let stripped = strip_js_comments(src);
        assert!(stripped.contains("const x = 1;"));
        assert!(stripped.contains("const y = 2;"));
        assert!(!stripped.contains("note"));
    }

    #[test]
    fn strip_js_comments_handles_triple_slash_doc_comments() {
        let src = "/// Show the dialog\nfunction f() { dom.modal.showModal(); }";
        let stripped = strip_js_comments(src);
        assert!(stripped.contains(".showModal()"));
        assert!(!stripped.contains("Show the dialog"));
    }
}
