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
mod tests;
