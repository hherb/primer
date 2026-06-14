//! Pin the contract that the GUI shell adapts to phone-sized viewports:
//! the evaluation sidebar becomes a slide-in overlay drawer, the header
//! action buttons condense to icons, and the status bar wraps instead of
//! pushing controls off-screen.
//!
//! Why a Rust contract module instead of a JS test runner: the GUI
//! frontend is plain vanilla JS/CSS with no bundler or test runner (no
//! `package.json`). The established pattern for pinning load-bearing
//! frontend facts is a crate-level test that `include_str!`s the static
//! assets and asserts their shape (see [`crate::modal_dialog_contract`]
//! for the same approach applied to the native `<dialog>` migration).
//! These tests make the responsive behaviour a `cargo test --workspace`
//! invariant: a future edit that drops the drawer breakpoint, the
//! backdrop element, or the matchMedia-driven toggle breaks the build
//! rather than silently regressing the phone layout.
//!
//! **The mobile breakpoint is `940px`.** Side-by-side layout needs room
//! for a usable chat (~620px) plus the sidebar (~320px) ≈ 940px; below
//! that the sidebar overlays as a drawer. The value is a single source
//! of truth that must appear identically in BOTH the stylesheet's
//! `@media` query and the JS `matchMedia` string — the
//! [`tests::mobile_breakpoint_is_consistent_between_css_and_js`] test
//! pins that alignment so the two can never drift.

/// The phone breakpoint, in CSS pixels, shared by the stylesheet's
/// `@media (max-width: …)` query and `app.js`'s `matchMedia` string.
/// Kept here so the contract tests reference one constant rather than a
/// scattered literal.
#[cfg(test)]
pub(crate) const MOBILE_BREAKPOINT_PX: u32 = 940;

#[cfg(test)]
mod tests {
    use super::MOBILE_BREAKPOINT_PX;

    const INDEX_HTML: &str = include_str!("../ui/index.html");
    const STYLES_CSS: &str = include_str!("../ui/styles.css");
    const APP_JS: &str = include_str!("../ui/app.js");

    /// Strip `// … EOL` and `/* … */` comments from a JS source so
    /// substring assertions don't false-pass on commented-out code or
    /// doc comments. Naive (doesn't track string/regex literals) but
    /// adequate for the contract assertions here.
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

    /// The stylesheet must declare the phone breakpoint as a `max-width`
    /// media query, otherwise none of the mobile overrides apply.
    #[test]
    fn stylesheet_declares_mobile_breakpoint() {
        let needle = format!("max-width: {MOBILE_BREAKPOINT_PX}px");
        assert!(
            STYLES_CSS.contains(&needle),
            "ui/styles.css must declare a `@media ({needle})` block — the \
             phone breakpoint that turns the sidebar into an overlay \
             drawer and condenses the header. See \
             responsive_layout_contract."
        );
    }

    /// Under the breakpoint the sidebar slides in as a drawer, which
    /// means an off-canvas `translateX` transform. Without it the
    /// sidebar would either overlap statically or push the chat.
    #[test]
    fn stylesheet_declares_drawer_slide_transform() {
        // Match the drawer-specific off-canvas value `translateX(100%)`
        // rather than a bare `translateX`, which already appears in the
        // unrelated `.toast` centering rule and would false-pass.
        assert!(
            STYLES_CSS.contains("translateX(100%)"),
            "ui/styles.css must push the closed mobile sidebar off-canvas \
             with `transform: translateX(100%)` (slid back to 0 when \
             `body.sidebar-open` is set)."
        );
    }

    /// The drawer dims the chat behind it via a dedicated backdrop
    /// element styled in CSS. Its visibility is pure-CSS (shown only
    /// under the breakpoint when the drawer is open) so resizing can't
    /// strand a visible backdrop over the desktop layout.
    #[test]
    fn stylesheet_declares_drawer_backdrop_rule() {
        assert!(
            STYLES_CSS.contains(".drawer-backdrop"),
            "ui/styles.css must style `.drawer-backdrop` — the dim layer \
             behind the mobile sidebar drawer."
        );
    }

    /// The backdrop element must exist in the markup for CSS to show it
    /// and for the click-to-dismiss handler to bind.
    #[test]
    fn index_html_has_drawer_backdrop_element() {
        assert!(
            INDEX_HTML.contains("drawer-backdrop"),
            "ui/index.html must contain the `drawer-backdrop` element \
             that dims the chat behind the open mobile drawer."
        );
    }

    /// Each header action button carries an `aria-hidden` icon span so
    /// the mobile breakpoint can hide the text label and leave a tidy
    /// single row of icons.
    #[test]
    fn header_buttons_carry_icon_spans() {
        assert!(
            INDEX_HTML.contains("btn-icon"),
            "ui/index.html header buttons must carry `btn-icon` spans so \
             the mobile layout can condense them to icons while keeping \
             the text label for desktop (hidden via `.btn-label`)."
        );
        assert!(
            INDEX_HTML.contains("btn-label"),
            "ui/index.html header buttons must wrap their text in a \
             `btn-label` span so the mobile breakpoint can hide just the \
             label, leaving the icon."
        );
    }

    /// The mobile layout hides the button labels under the breakpoint —
    /// pin that the rule exists so a future style refactor can't quietly
    /// restore full-width labels that overflow the phone header.
    #[test]
    fn stylesheet_hides_button_labels_on_mobile() {
        assert!(
            STYLES_CSS.contains("btn-label"),
            "ui/styles.css must reference `.btn-label` (hidden under the \
             mobile breakpoint) so header buttons condense to icons."
        );
    }

    /// The toggle handler is mode-aware: it drives a `matchMedia` query
    /// to decide between the desktop collapse class and the mobile
    /// drawer-open class.
    #[test]
    fn app_js_uses_matchmedia_for_breakpoint() {
        let code = strip_js_comments(APP_JS);
        assert!(
            code.contains("matchMedia"),
            "ui/app.js must use `window.matchMedia(...)` so the sidebar \
             toggle behaves as a desktop collapse vs a mobile drawer \
             depending on viewport width."
        );
    }

    /// The mobile drawer open/closed state is carried by a distinct
    /// `body.sidebar-open` class (default absent = closed on mobile),
    /// separate from the desktop `sidebar-collapsed` class.
    #[test]
    fn app_js_toggles_sidebar_open_class() {
        let code = strip_js_comments(APP_JS);
        assert!(
            code.contains("sidebar-open"),
            "ui/app.js must toggle the `sidebar-open` class to open/close \
             the mobile drawer (distinct from the desktop \
             `sidebar-collapsed` class)."
        );
    }

    /// The Escape-to-dismiss handler must defer to an open native
    /// `<dialog>`: when a modal is showing, the browser fires the
    /// dialog's own cancel event for Escape, and the drawer handler must
    /// not also run or a single keypress would dismiss both. Pin the
    /// `dialog[open]` guard so a future refactor can't quietly drop it.
    #[test]
    fn app_js_escape_handler_defers_to_open_dialog() {
        let code = strip_js_comments(APP_JS);
        assert!(
            code.contains("dialog[open]"),
            "ui/app.js's Escape-to-close-drawer handler must skip while a \
             native `<dialog>` is open (guard on \
             `document.querySelector(\"dialog[open]\")`) so Escape doesn't \
             dismiss the drawer and the modal in the same keypress."
        );
    }

    /// The named breakpoint constant lives in JS (not an inline literal)
    /// so the magic number has one home, per the project's
    /// no-magic-numbers convention.
    #[test]
    fn app_js_names_the_breakpoint_constant() {
        let code = strip_js_comments(APP_JS);
        assert!(
            code.contains("MOBILE_BREAKPOINT_PX"),
            "ui/app.js must define a named `MOBILE_BREAKPOINT_PX` constant \
             and build the matchMedia string from it, rather than inlining \
             the breakpoint literal."
        );
    }

    /// The breakpoint value must be identical in the stylesheet and the
    /// script — they are two halves of one responsive contract and a
    /// drift would make the JS toggle and the CSS layout disagree about
    /// what "mobile" means.
    #[test]
    fn mobile_breakpoint_is_consistent_between_css_and_js() {
        let value = MOBILE_BREAKPOINT_PX.to_string();
        let code = strip_js_comments(APP_JS);
        // CSS: anchor to the media-query form `max-width: 940px` rather than
        // a bare `940`, which could coincidentally match a colour, z-index,
        // or unrelated dimension elsewhere in the stylesheet.
        let css_needle = format!("max-width: {value}px");
        assert!(
            STYLES_CSS.contains(&css_needle),
            "ui/styles.css must declare the breakpoint as `{css_needle}`."
        );
        // JS: anchor to the named-constant assignment `MOBILE_BREAKPOINT_PX
        // = 940` rather than a bare `940` for the same reason. The matchMedia
        // string is built from the constant (`max-width: ${...}px`), so the
        // literal media-query form never appears in the JS source to match.
        let js_needle = format!("MOBILE_BREAKPOINT_PX = {value}");
        assert!(
            code.contains(&js_needle),
            "ui/app.js must define `const {js_needle}` so its matchMedia \
             string matches the stylesheet's media query."
        );
    }

    /// The viewport meta must opt into `viewport-fit=cover`, without which
    /// the Android/iOS WebView never reports non-zero `env(safe-area-inset-*)`
    /// values and the system bars occlude the header + composer. Pin it so a
    /// future meta-tag edit can't silently re-break edge-to-edge handling.
    #[test]
    fn index_html_viewport_opts_into_safe_area() {
        assert!(
            INDEX_HTML.contains("viewport-fit=cover"),
            "ui/index.html's viewport meta must include `viewport-fit=cover` \
             so env(safe-area-inset-*) reports the device insets — otherwise \
             the status/navigation bars cover the header and composer."
        );
    }

    /// The app grid must pad itself by the device safe-area insets (top for
    /// the status bar, bottom for the navigation bar) so the header and
    /// composer are never occluded on a phone. Pin both the top and bottom
    /// insets — the two edges the owner reported as clipped.
    #[test]
    fn stylesheet_insets_app_grid_by_safe_area() {
        assert!(
            STYLES_CSS.contains("env(safe-area-inset-top"),
            "ui/styles.css must inset the app by `env(safe-area-inset-top, …)` \
             so the header clears the status bar/notch."
        );
        assert!(
            STYLES_CSS.contains("env(safe-area-inset-bottom"),
            "ui/styles.css must inset the app by \
             `env(safe-area-inset-bottom, …)` so the composer clears the \
             Android navigation bar."
        );
    }

    /// The mobile drawer is `position: fixed` (viewport-relative) and so
    /// ignores the body safe-area padding; it must fold the top inset into
    /// its own `top` offset, or it would tuck under the status-bar strip.
    #[test]
    fn stylesheet_drawer_accounts_for_top_inset() {
        assert!(
            STYLES_CSS.contains("env(safe-area-inset-top, 0px) + var(--header-h)"),
            "ui/styles.css must offset the fixed mobile drawer below the \
             *visual* header bottom (`calc(env(safe-area-inset-top, 0px) + \
             var(--header-h))`), since position:fixed ignores the body \
             safe-area padding."
        );
    }

    /// Boot ordering: `main()` must be invoked AFTER the module-level
    /// `const mobileQuery` is initialized. `main()` calls
    /// `setupSidebarToggle()`, which references `mobileQuery`; invoking
    /// `main()` above that `const` hits it in its temporal dead zone, and
    /// because `main()` is `async` the throw surfaces as an unhandled
    /// rejection that aborts boot *after* the sidebar toggle is wired but
    /// *before* `showPicker()` runs — the session picker silently never
    /// appears. This pins the call-last ordering so that regression can't
    /// return undetected (it shipped once in PR #225 and only surfaced when
    /// the GUI JS first ran on a device).
    #[test]
    fn app_js_invokes_main_after_mobilequery_is_initialized() {
        let code = strip_js_comments(APP_JS);
        let mobilequery_pos = code
            .find("const mobileQuery")
            .expect("ui/app.js must declare `const mobileQuery`");
        // The bare `main();` invocation (not the `async function main()`
        // declaration, which has no trailing `;`).
        let main_call_pos = code
            .rfind("main();")
            .expect("ui/app.js must invoke `main();`");
        assert!(
            main_call_pos > mobilequery_pos,
            "ui/app.js must call `main();` AFTER `const mobileQuery` is \
             initialized — otherwise setupSidebarToggle() reads mobileQuery in \
             its temporal dead zone and the async main() rejects before \
             showPicker() runs (the boot regression that hid the picker). See \
             responsive_layout_contract."
        );
    }

    /// Extract the opening `<aside …>` tag of the evaluation sidebar so
    /// attribute assertions are scoped to that one element rather than the
    /// whole document (where e.g. `tabindex="-1"` could match an unrelated
    /// element).
    fn sidebar_open_tag(html: &str) -> &str {
        let start = html
            .find("<aside")
            .expect("ui/index.html must contain the `<aside>` sidebar element");
        let rest = &html[start..];
        let end = rest
            .find('>')
            .expect("the sidebar `<aside>` tag must be closed with `>`");
        &rest[..=end]
    }

    /// The drawer receives programmatic focus on open, so the `#sidebar`
    /// `<aside>` must be focusable without joining the tab order — i.e.
    /// carry `tabindex="-1"`. Without it `dom.sidebar.focus()` is a no-op
    /// and the open drawer leaves focus stranded on the (now-inert) chat.
    #[test]
    fn index_html_sidebar_is_programmatically_focusable() {
        let tag = sidebar_open_tag(INDEX_HTML);
        assert!(
            tag.contains("id=\"sidebar\""),
            "the first `<aside>` in ui/index.html must be the `#sidebar` \
             evaluation drawer."
        );
        assert!(
            tag.contains("tabindex=\"-1\""),
            "ui/index.html's `#sidebar` <aside> must carry `tabindex=\"-1\"` \
             so the mobile drawer can receive programmatic focus on open \
             without entering the tab order. See responsive_layout_contract \
             (a11y focus management)."
        );
    }

    /// Opening the drawer moves focus into it and closing restores focus
    /// to the toggle; the mobile-only modality is centralised in a named
    /// `applyDrawerModality` helper so the focus + inert handling can't be
    /// split across the open/close paths and drift.
    #[test]
    fn app_js_centralises_drawer_modality() {
        let code = strip_js_comments(APP_JS);
        assert!(
            code.contains("applyDrawerModality"),
            "ui/app.js must drive the mobile drawer's focus + inert handling \
             through an `applyDrawerModality(open)` helper so opening moves \
             focus into the drawer and closing restores it to \
             `#sidebar-toggle`."
        );
        assert!(
            code.contains("dom.sidebar.focus("),
            "ui/app.js's drawer modality must call `dom.sidebar.focus(...)` to \
             move focus INTO the drawer on open (a bare `.focus(` anywhere is \
             too loose — pin the sidebar as the focus target)."
        );
        assert!(
            code.contains("drawerReturnFocus"),
            "ui/app.js must capture the element to restore focus to on close \
             in `drawerReturnFocus` and call `.focus(...)` on it, so closing \
             the drawer returns focus to `#sidebar-toggle` rather than \
             dropping it to `<body>`."
        );
    }

    /// The drawer-modality helper marks the chat `<main>` `inert`, reached
    /// via `dom.chatMain = document.querySelector("main.chat")`. If that
    /// element is ever renamed, `dom.chatMain` is `null` and the very first
    /// `setAttribute("inert", …)` throws, breaking drawer-open entirely.
    /// Pin the selector target so a rename in `index.html` fails `cargo test`
    /// here rather than silently at runtime on a phone.
    #[test]
    fn index_html_has_chat_main_for_inert_target() {
        assert!(
            INDEX_HTML.contains("<main class=\"chat\">"),
            "ui/index.html must contain `<main class=\"chat\">` — app.js's \
             `dom.chatMain = document.querySelector(\"main.chat\")` depends on \
             it as the `inert` target for the mobile drawer; a rename would \
             make `dom.chatMain` null and throw on drawer open."
        );
    }

    /// While the mobile drawer is open the chat behind it must be `inert`
    /// (unfocusable + hidden from the a11y tree) so the composer textarea
    /// and conversation behind the dim backdrop can't be tabbed to or read
    /// by assistive tech. Pin the `inert` attribute toggle.
    #[test]
    fn app_js_inerts_chat_behind_open_drawer() {
        let code = strip_js_comments(APP_JS);
        assert!(
            code.contains("setAttribute(\"inert\""),
            "ui/app.js must mark the chat `<main>` `inert` while the mobile \
             drawer is open (`setAttribute(\"inert\", …)`), so the content \
             behind the backdrop is neither focusable nor exposed to \
             assistive technology."
        );
        assert!(
            code.contains("removeAttribute(\"inert\""),
            "ui/app.js must clear the `inert` attribute when the drawer \
             closes (and when crossing back to the desktop layout) so the \
             two-column view is never left with an inert chat."
        );
    }

    /// The chat scroll behind the open drawer must be frozen so a
    /// touch-scroll on the dimmed area doesn't move the conversation
    /// underneath. Scoped to `body.sidebar-open` inside the mobile media
    /// query, so it never affects the desktop column and auto-releases
    /// when the window crosses back above the breakpoint.
    #[test]
    fn stylesheet_scroll_locks_chat_behind_open_drawer() {
        assert!(
            STYLES_CSS.contains("body.sidebar-open .chat-scroll"),
            "ui/styles.css must scroll-lock the chat behind the open mobile \
             drawer with a `body.sidebar-open .chat-scroll {{ overflow: \
             hidden }}` rule (inside the `@media (max-width: 940px)` block)."
        );
    }

    #[test]
    fn strip_js_comments_removes_line_and_block_comments() {
        let src = "// sidebar-open in a comment\n/* matchMedia here */ real();";
        let stripped = strip_js_comments(src);
        assert!(stripped.contains("real()"));
        assert!(!stripped.contains("sidebar-open"));
        assert!(!stripped.contains("matchMedia"));
    }
}
