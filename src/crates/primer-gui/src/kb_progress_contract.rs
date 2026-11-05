//! Pin the GUI's first-run knowledge-base embed progress (issue #393).
//!
//! `start_session` / `resume_session` emit
//! [`crate::types::KB_EMBEDDING_EVENT`] while a first run embeds the KB
//! (~100 s for BGE-M3). Without a listener the picker or the settings
//! modal just looks hung. There is no JS test runner in this crate, so —
//! like [`crate::voice_stop_contract`] — the guard `include_str!`s the UI
//! assets and asserts the wiring.

#[cfg(test)]
mod tests {
    use crate::types::KB_EMBEDDING_EVENT;

    const PICKER_JS: &str = include_str!("../ui/picker.js");
    const SETTINGS_JS: &str = include_str!("../ui/settings.js");

    /// The body of `function <name>(` in `js`, up to its closing `\n}\n`.
    fn fn_body<'a>(js: &'a str, signature: &str) -> &'a str {
        let start = js
            .find(signature)
            .unwrap_or_else(|| panic!("{signature} not found"));
        let body = &js[start..];
        &body[..body.find("\n}\n").expect("function body must close")]
    }

    #[test]
    fn picker_listens_on_the_event_the_commands_emit() {
        assert!(
            PICKER_JS.contains(&format!(
                r#"const KB_EMBEDDING_EVENT = "{KB_EMBEDDING_EVENT}";"#
            )),
            "picker.js must name the same event as types::KB_EMBEDDING_EVENT"
        );
        assert!(
            fn_body(PICKER_JS, "function listenKbProgress(").contains("listen(KB_EMBEDDING_EVENT")
        );
        assert!(PICKER_JS.contains("window.primerListenKbProgress = listenKbProgress;"));
    }

    /// Each session-building invoke is preceded by the listener and
    /// followed by its unlisten, so progress shows and the handler doesn't
    /// leak into the next build.
    #[test]
    fn both_session_builds_show_progress_and_unlisten() {
        for (js, signature, listen_call, invoke) in [
            (
                PICKER_JS,
                "async function resume(",
                "await listenKbProgress(",
                r#"invoke("resume_session""#,
            ),
            (
                SETTINGS_JS,
                "async function onSave(",
                "await window.primerListenKbProgress(",
                r#"invoke("start_session")"#,
            ),
        ] {
            let body = fn_body(js, signature);
            let listen_at = body
                .find(listen_call)
                .unwrap_or_else(|| panic!("{signature} must call {listen_call}"));
            let invoke_at = body
                .find(invoke)
                .unwrap_or_else(|| panic!("{signature} must call {invoke}"));
            assert!(listen_at < invoke_at, "{signature}: listen before {invoke}");
            assert!(
                body[invoke_at..].contains("unlisten();"),
                "{signature}: unlisten after {invoke}"
            );
        }
    }
}
