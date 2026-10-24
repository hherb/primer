//! Pin the GUI's post-"End voice mode" UI reset (issue #352 item 3).
//!
//! `stop_voice_mode` drops the active session server-side (see
//! `commands::voice::stop_voice_mode_inner`), so after a voice stop the
//! header pills, `#backend-notice-banner` and the text composer would keep
//! describing a session that no longer exists — typing then fails with "no
//! active session". The fix sends the user back to the session picker via
//! the same `onSwitchSession` path the header's "Sessions" button uses.
//!
//! There is no JS test runner in this crate, so — like
//! [`crate::diagnostics_toggle_contract`] — the guard is a `cfg(test)`
//! module that `include_str!`s the UI assets and asserts the wiring.

#[cfg(test)]
mod tests {
    const APP_JS: &str = include_str!("../ui/app.js");
    const VOICE_JS: &str = include_str!("../ui/voice.js");

    /// The window-level hook voice.js calls; app.js must export it.
    const RETURN_TO_PICKER: &str = "primerReturnToPicker";

    /// The user's explicit stop call in voice.js `onToggleClick`.
    const TOGGLE_STOP_CALL: &str = r#"invoke(vcmd("stop_voice_mode"))"#;

    #[test]
    fn app_js_exports_return_to_picker_as_on_switch_session() {
        let line = APP_JS
            .lines()
            .find(|l| l.contains(&format!("window.{RETURN_TO_PICKER}")))
            .unwrap_or_else(|| panic!("app.js must export window.{RETURN_TO_PICKER}"));
        assert!(
            line.trim_end().ends_with("= onSwitchSession;"),
            "window.{RETURN_TO_PICKER} must be onSwitchSession, got: {line}"
        );
    }

    #[test]
    fn on_switch_session_resets_header_and_shows_picker() {
        let start = APP_JS
            .find("async function onSwitchSession()")
            .expect("app.js must define onSwitchSession");
        let body = &APP_JS[start..];
        let body = &body[..body.find("\n}\n").expect("onSwitchSession body must close")];
        for needle in ["resetSessionHeader()", "showPicker()"] {
            assert!(body.contains(needle), "onSwitchSession must call {needle}");
        }
    }

    #[test]
    fn voice_toggle_stop_returns_to_picker() {
        let start = VOICE_JS
            .find(TOGGLE_STOP_CALL)
            .expect("voice.js onToggleClick must call stop_voice_mode");
        let after = &VOICE_JS[start..];
        let branch = &after[..after.find("return;").expect("stop branch must return")];
        assert!(
            branch.contains(&format!("window.{RETURN_TO_PICKER}()")),
            "voice.js must call window.{RETURN_TO_PICKER}() after stop_voice_mode, \
             before returning (#352 item 3)"
        );
    }
}
