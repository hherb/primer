use super::MOBILE_BREAKPOINT_PX;

const INDEX_HTML: &str = include_str!("../../ui/index.html");
const STYLES_CSS: &str = include_str!("../../ui/styles.css");
const APP_JS: &str = include_str!("../../ui/app.js");

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

/// The mobile drawer is a functional modal, so it must be *announced*
/// as one: `applyDrawerModality` toggles `role="dialog"` +
/// `aria-modal="true"` on the `#sidebar` element on open and clears them
/// on close. Toggled in JS (not static HTML) and mobile-only, because the
/// same `#sidebar` is the persistent desktop column — a dialog role there
/// would wrongly announce a modal over the two-column layout. The
/// element's existing `aria-label="Evaluation sidebar"` names the dialog.
#[test]
fn app_js_announces_open_drawer_as_modal_dialog() {
    let code = strip_js_comments(APP_JS);
    assert!(
        code.contains("setAttribute(\"role\", \"dialog\")"),
        "ui/app.js must set `role=\"dialog\"` on the `#sidebar` drawer \
         while it is the mobile modal (`setAttribute(\"role\", \
         \"dialog\")`), so assistive tech announces it as a dialog."
    );
    assert!(
        code.contains("setAttribute(\"aria-modal\", \"true\")"),
        "ui/app.js must set `aria-modal=\"true\"` on the open mobile \
         drawer so assistive tech treats the background as inert."
    );
    assert!(
        code.contains("removeAttribute(\"role\")")
            && code.contains("removeAttribute(\"aria-modal\")"),
        "ui/app.js must remove the `role`/`aria-modal` attributes when \
         the drawer closes (and on the breakpoint-change cleanup) so the \
         desktop two-column `#sidebar` column never announces a dialog."
    );
}

/// Strict focus trap: while the mobile drawer is open every interactive
/// header control EXCEPT the close toggle is made `inert`. Combined with
/// the already-inert chat `<main>`, the only tabbable elements are the
/// drawer's contents and `#sidebar-toggle`, so `Tab`/`Shift+Tab` wrap
/// naturally between them — no manual keydown handler. The toggle stays
/// live because it is the close affordance. Pin both the helper and the
/// toggle-exclusion so a refactor can't silently let header chrome leak
/// back into the tab order (re-breaking the `aria-modal` promise).
#[test]
fn app_js_strict_trap_inerts_header_except_toggle() {
    let code = strip_js_comments(APP_JS);
    assert!(
        code.contains("setHeaderChromeInert"),
        "ui/app.js must drive the strict focus trap through a \
         `setHeaderChromeInert(inert)` helper that inerts the non-toggle \
         header controls while the mobile drawer is open."
    );
    assert!(
        code.contains("!== dom.sidebarToggle"),
        "ui/app.js's header-chrome trap must EXCLUDE the close toggle \
         (`el !== dom.sidebarToggle`) — it is the drawer's close \
         affordance and must remain reachable while everything else in \
         the header is inert."
    );
}

/// The breakpoint-change handler must tear down ALL of the drawer's modal
/// DOM state — chat inert, header-chrome inert, and `role`/`aria-modal` —
/// not just the chat inert, since none of those are media-scoped (unlike
/// the CSS scroll-lock + backdrop, which auto-release). Both the close
/// path and this handler funnel teardown through one
/// `releaseDrawerModality()` so they can't drift. Pin that the change
/// handler delegates to it.
#[test]
fn app_js_clears_modal_state_on_breakpoint_change() {
    let code = strip_js_comments(APP_JS);
    let change_pos = code
        .find("mobileQuery.addEventListener(\"change\"")
        .expect("ui/app.js must register a mobileQuery `change` handler");
    let end = code[change_pos..]
        .find("});")
        .map(|e| change_pos + e)
        .unwrap_or(code.len());
    let handler = &code[change_pos..end];
    assert!(
        handler.contains("releaseDrawerModality"),
        "ui/app.js's mobileQuery `change` handler must delegate teardown \
         to `releaseDrawerModality()` so a drawer resized up to desktop \
         clears its inert chat, inert header chrome, AND \
         `role=\"dialog\"`/`aria-modal` — none of which are media-scoped."
    );
}

/// The strict trap inerts the header's non-toggle controls, reached via
/// `dom.header = document.querySelector("header.app-header")`. If that
/// element is renamed, `dom.header` is `null` and the trap throws on
/// drawer open. Pin the selector target so a rename fails `cargo test`
/// here rather than at runtime on a phone (mirrors the chat-main test).
#[test]
fn index_html_has_app_header_for_trap_target() {
    assert!(
        INDEX_HTML.contains("<header class=\"app-header\">"),
        "ui/index.html must contain `<header class=\"app-header\">` — \
         app.js's `dom.header = document.querySelector(\
         \"header.app-header\")` depends on it as the strict-trap target; \
         a rename would make `dom.header` null and throw on drawer open."
    );
}

/// Extract the `#sidebar` `<aside>…</aside>` subtree so assertions about
/// in-dialog content are scoped to the drawer rather than the whole
/// document (where a `drawer-close` button could otherwise match a stray
/// element). Spans the opening `<aside` to its matching `</aside>`.
fn sidebar_subtree(html: &str) -> &str {
    let start = html
        .find("<aside")
        .expect("ui/index.html must contain the `<aside>` sidebar element");
    let rest = &html[start..];
    let end = rest
        .find("</aside>")
        .map(|e| e + "</aside>".len())
        .expect("the sidebar `<aside>` must be closed with `</aside>`");
    &rest[..end]
}

/// `aria-modal="true"` instructs assistive tech to confine the user to the
/// dialog subtree, so the drawer's close affordance must live *inside*
/// `#sidebar`, not only in the out-of-subtree header toggle (issue #233).
/// Pin an in-dialog close button so a screen-reader user is never stranded
/// inside the modal with no reachable dismiss control.
#[test]
fn index_html_has_in_dialog_drawer_close_button() {
    let subtree = sidebar_subtree(INDEX_HTML);
    assert!(
        subtree.contains("id=\"drawer-close\""),
        "ui/index.html's `#sidebar` drawer must contain an in-dialog close \
         button `id=\"drawer-close\"` so the close affordance lives inside \
         the `aria-modal` dialog subtree (issue #233) — the header toggle \
         alone is outside the subtree and unreachable to a confined \
         screen-reader user."
    );
    assert!(
        subtree.contains("aria-label=\"Close evaluation sidebar\""),
        "ui/index.html's `#drawer-close` button must carry \
         `aria-label=\"Close evaluation sidebar\"` so its icon glyph has an \
         accessible name."
    );
}

/// Return the body (between `{` and the next `}`) of the first CSS rule
/// whose selector line matches `selector`, e.g. `css_rule_body(css,
/// ".drawer-header {")`. Scoping the assertion to the rule body avoids both
/// the panic risk of a fixed-width byte slice and a false-positive on a
/// `display: none` belonging to a neighbouring rule.
fn css_rule_body<'a>(css: &'a str, selector: &str) -> Option<&'a str> {
    let open = css.find(selector)? + selector.len();
    let rest = &css[open..];
    let close = rest.find('}')?;
    Some(&rest[..close])
}

/// The in-dialog close button is mobile-only: on desktop the same
/// `#sidebar` is a persistent two-column landmark whose collapse is driven
/// by the header toggle, so a redundant in-panel `×` would confuse. Pin
/// that its `.drawer-header` wrapper is hidden by default and revealed only
/// under the mobile breakpoint, mirroring the drawer-itself mobile scoping.
#[test]
fn stylesheet_hides_drawer_close_on_desktop() {
    assert!(
        STYLES_CSS.contains(".drawer-header"),
        "ui/styles.css must style the `.drawer-header` bar that holds the \
         in-dialog mobile close button (issue #233)."
    );
    // The first `.drawer-header {` rule is the top-level (non-media-query)
    // one, defined before the `@media (max-width: 940px)` reveal; its body
    // must hide the bar so the close button only appears on mobile.
    let default_body = css_rule_body(STYLES_CSS, ".drawer-header {")
        .expect("ui/styles.css must contain a top-level `.drawer-header {{ … }}` rule");
    assert!(
        default_body.contains("display: none"),
        "ui/styles.css must hide `.drawer-header {{ display: none }}` by \
         default so the in-dialog close button only appears on mobile (it \
         is revealed via `display: flex` inside the \
         `@media (max-width: 940px)` block); on the desktop column it would \
         be a redundant control."
    );
}

/// The in-dialog close button must actually close the drawer. Pin that
/// app.js binds a click handler to `#drawer-close` (via the cached
/// `dom.drawerClose`) so the new control isn't inert decoration; closing
/// through `setSidebarOpen(false)` reuses the same focus-restore +
/// modality-teardown path as the toggle and backdrop.
#[test]
fn app_js_wires_in_dialog_drawer_close() {
    let code = strip_js_comments(APP_JS);
    assert!(
        code.contains("drawer-close"),
        "ui/app.js must cache the in-dialog close button \
         (`document.getElementById(\"drawer-close\")`) so its click can \
         dismiss the drawer."
    );
    assert!(
        code.contains("dom.drawerClose"),
        "ui/app.js must reference `dom.drawerClose` and bind its click to \
         `setSidebarOpen(false)`, routing the in-dialog close through the \
         same teardown + focus-restore path as the toggle/backdrop."
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
