//! Pin the wiring of the Settings → Diagnostics "Record QNN throughput
//! metrics" opt-in (issue #228) across the three layers that must agree:
//! the HTML checkbox id, the JS that reads it (`dom.fields` registration +
//! `gather()`), and the `populate()` that restores it.
//!
//! There is no JS test runner in this crate, so — like
//! [`crate::modal_dialog_contract`] and [`crate::responsive_layout_contract`]
//! — the guard is a Rust `cfg(test)` module that `include_str!`s the UI
//! assets and asserts the load-bearing identifiers are present in every
//! layer. A typo'd id (HTML `f-diagnostics-qnn-metrics` vs a JS
//! `getElementById` that no longer matches) would otherwise surface only as
//! a runtime `null.checked` throw inside `gather()` when a user saves —
//! exactly the silent-boot-abort class of bug the PR #227 const-TDZ was.
//!
//! The contract: the metrics opt-in is OFF by default and only recordable
//! when this single checkbox is ticked, so all three references to its id
//! must stay in lockstep.

#[cfg(test)]
mod tests {
    /// Static-embed the two UI assets that carry the contract. `include_str!`
    /// resolves relative to this source file; the UI lives at the crate root
    /// under `ui/`.
    const INDEX_HTML: &str = include_str!("../ui/index.html");
    const SETTINGS_JS: &str = include_str!("../ui/settings.js");

    /// The single DOM id that ties the HTML checkbox to its JS reader. If
    /// this id is renamed in one place but not the others, the assertions
    /// below fail loudly.
    const QNN_METRICS_CHECKBOX_ID: &str = "f-diagnostics-qnn-metrics";

    #[test]
    fn html_declares_the_qnn_metrics_checkbox() {
        let needle = format!(r#"id="{QNN_METRICS_CHECKBOX_ID}""#);
        assert!(
            INDEX_HTML.contains(&needle),
            "index.html must declare the diagnostics opt-in checkbox `{QNN_METRICS_CHECKBOX_ID}`"
        );
        // It must be a checkbox — the opt-in is a boolean toggle.
        assert!(
            INDEX_HTML.contains(r#"type="checkbox" id="f-diagnostics-qnn-metrics""#),
            "the diagnostics opt-in must be a <input type=\"checkbox\">"
        );
    }

    #[test]
    fn settings_js_registers_and_reads_the_checkbox() {
        // dom.fields registration anchors the element by id...
        assert!(
            SETTINGS_JS.contains(&format!(r#"getElementById("{QNN_METRICS_CHECKBOX_ID}")"#)),
            "settings.js must register `{QNN_METRICS_CHECKBOX_ID}` in dom.fields"
        );
        // ...and gather() must send the diagnostics block reading that field,
        // or a flipped toggle would never persist.
        assert!(
            SETTINGS_JS.contains("diagnosticsQnnMetrics.checked"),
            "gather()/populate() must read the diagnostics checkbox via dom.fields.diagnosticsQnnMetrics"
        );
        assert!(
            SETTINGS_JS.contains("qnn_metrics_enabled"),
            "gather() must send the `qnn_metrics_enabled` field in the diagnostics block"
        );
    }
}
