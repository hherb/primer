//! Pure helpers for [`build.rs`] failure-path messaging.
//!
//! Loaded two ways:
//!   * `build.rs` pulls this file in via `#[path = "src/macos26/build_hints.rs"]
//!     mod build_hints;` so the script can classify command results and emit
//!     `cargo:warning=` lines without duplicating logic.
//!   * The lib crate declares `#[cfg(test)] mod build_hints;` in
//!     [`crate::macos26`] so `cargo test --features macos-native-26` picks up
//!     the inline unit tests below.
//!
//! Everything here is intentionally pure (no I/O, no `println!`) except where
//! a function returns lines for the caller to print. That separation is what
//! lets the unit tests run without spawning the actual `xcrun` / `swiftc`
//! toolchain.
//!
//! Closes issue #141.
#![allow(dead_code)] // `build.rs` is a separate compilation unit; the lib only sees these via #[cfg(test)].

use std::process::Output;

/// Install hint shown when `xcrun` or `xcode-select` is missing or fails.
///
/// Surfaced via `cargo:warning=` (one warning per line) and embedded in the
/// panic message so it lands in front of the user no matter how cargo's
/// output is being read.
pub(crate) const XCODE_HINT: &str = "\
The `macos-native-26` feature requires Xcode 16+ with the macOS 26 SDK.\n\
Install Xcode from the Mac App Store, then run:\n\
    sudo xcode-select --switch /Applications/Xcode.app/Contents/Developer\n\
Command Line Tools alone do NOT include the macOS SDK — full Xcode is required.";

/// Install hint shown when the `swiftc` invocation itself spawns/compiles
/// unsuccessfully. The swiftc compiler output above the hint usually has the
/// concrete error; this hint guides the user toward the most common cause
/// (out-of-date toolchain).
pub(crate) const SWIFTC_HINT: &str = "\
swiftc failed to compile the macos-native-26 Swift sidecar.\n\
Verify your Xcode toolchain is current (Xcode 16+ with macOS 26 SDK).\n\
The swiftc output above has the specific error.";

/// Classification of one `Command::output()` call.
///
/// `code: None` in [`ProbeOutcome::NonZero`] means the process was terminated
/// by a signal (POSIX) rather than a normal exit; in practice we surface that
/// as a string rather than an integer in the panic message.
#[derive(Debug)]
pub(crate) enum ProbeOutcome {
    /// Command ran and exited 0 — payload is trimmed stdout.
    Ok(String),
    /// Command ran but exited non-zero (or was signalled).
    NonZero { code: Option<i32>, stderr: String },
    /// Command could not be spawned at all (binary missing, PATH wrong, etc.).
    SpawnFailed(String),
}

/// Classify the result of a `Command::output()` call into a [`ProbeOutcome`].
///
/// Pure: no side effects, no panics. The caller decides what to do with each
/// outcome — typically [`format_failure_message`] + [`cargo_warning_lines`] +
/// `panic!`.
pub(crate) fn classify(result: std::io::Result<Output>) -> ProbeOutcome {
    match result {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
            ProbeOutcome::Ok(stdout)
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            ProbeOutcome::NonZero {
                code: out.status.code(),
                stderr,
            }
        }
        Err(err) => ProbeOutcome::SpawnFailed(err.to_string()),
    }
}

/// Format a one-line human description of a probe failure.
///
/// Returns `None` for [`ProbeOutcome::Ok`] so callers can chain through a
/// single `match` arm; returns `Some(msg)` for both failure variants.
pub(crate) fn format_failure_message(
    name: &str,
    args: &[&str],
    outcome: &ProbeOutcome,
) -> Option<String> {
    let label = if args.is_empty() {
        name.to_string()
    } else {
        format!("{name} {}", args.join(" "))
    };
    match outcome {
        ProbeOutcome::Ok(_) => None,
        ProbeOutcome::NonZero { code, stderr } => {
            let code_str = code.map_or_else(|| "<signal>".to_string(), |c| c.to_string());
            let stderr_str = if stderr.trim().is_empty() {
                "(no stderr)".to_string()
            } else {
                stderr.trim().to_string()
            };
            Some(format!(
                "`{label}` exited with code {code_str}: {stderr_str}"
            ))
        }
        ProbeOutcome::SpawnFailed(err) => Some(format!("failed to invoke `{label}`: {err}")),
    }
}

/// Render a hint as a sequence of `cargo:warning=…` lines.
///
/// Pure (returns owned strings); the caller prints them. Each non-empty
/// line of the hint becomes one cargo warning so multi-line hints stay
/// visible no matter how cargo's output is being filtered.
pub(crate) fn cargo_warning_lines(hint: &str) -> Vec<String> {
    hint.lines()
        .map(|line| format!("cargo:warning={line}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn xcode_hint_mentions_xcode_select_switch_command() {
        // The whole point of the hint is to tell users *how* to fix it.
        assert!(
            XCODE_HINT.contains("xcode-select --switch"),
            "hint should include the actual remediation command"
        );
    }

    #[test]
    fn xcode_hint_mentions_app_store_as_install_source() {
        assert!(
            XCODE_HINT.contains("App Store"),
            "hint should tell users where to get Xcode"
        );
    }

    #[test]
    fn xcode_hint_warns_against_clt_only_install() {
        // Most common pre-Xcode state on a dev box is Command Line Tools
        // only. Calling that out explicitly saves a round-trip.
        assert!(
            XCODE_HINT.contains("Command Line Tools"),
            "hint should warn about CLT-only installs"
        );
    }

    #[test]
    fn swiftc_hint_mentions_xcode_toolchain() {
        assert!(SWIFTC_HINT.contains("Xcode"));
    }

    #[test]
    fn classify_spawn_failure_carries_underlying_error_string() {
        let err = std::io::Error::new(std::io::ErrorKind::NotFound, "no such file");
        let outcome = classify(Err(err));
        match outcome {
            ProbeOutcome::SpawnFailed(s) => assert!(
                s.contains("no such file"),
                "underlying io::Error message should survive"
            ),
            other => panic!("expected SpawnFailed, got {other:?}"),
        }
    }

    #[test]
    fn classify_successful_output_into_ok_with_trimmed_stdout() {
        // `echo hello` exits 0 with "hello\n" on stdout. Trim should
        // drop the trailing newline.
        let result = Command::new("echo").arg("hello").output();
        match classify(result) {
            ProbeOutcome::Ok(s) => assert_eq!(s, "hello"),
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    #[test]
    fn classify_nonzero_exit_returns_exit_code() {
        // `sh -c 'exit 7'` is portable across macOS/Linux and lets us pin
        // a specific code rather than relying on `false` (which lives at
        // /bin/false on Linux and /usr/bin/false on macOS).
        let result = Command::new("sh").args(["-c", "exit 7"]).output();
        match classify(result) {
            ProbeOutcome::NonZero { code, .. } => assert_eq!(code, Some(7)),
            other => panic!("expected NonZero, got {other:?}"),
        }
    }

    #[test]
    fn format_failure_message_is_none_on_ok() {
        let outcome = ProbeOutcome::Ok("output".to_string());
        assert!(format_failure_message("xcrun", &[], &outcome).is_none());
    }

    #[test]
    fn format_failure_message_on_spawn_failure_names_the_binary() {
        let outcome = ProbeOutcome::SpawnFailed("not found".to_string());
        let msg = format_failure_message("xcrun", &[], &outcome).unwrap();
        assert!(msg.contains("xcrun"), "should mention command name");
        assert!(msg.contains("not found"), "should include underlying error");
    }

    #[test]
    fn format_failure_message_includes_args_on_nonzero_exit() {
        let outcome = ProbeOutcome::NonZero {
            code: Some(1),
            stderr: "boom".to_string(),
        };
        let msg =
            format_failure_message("xcrun", &["--show-sdk-path", "--sdk", "macosx"], &outcome)
                .unwrap();
        assert!(msg.contains("xcrun --show-sdk-path --sdk macosx"));
        assert!(msg.contains("code 1"));
        assert!(msg.contains("boom"));
    }

    #[test]
    fn format_failure_message_substitutes_placeholder_for_empty_stderr() {
        let outcome = ProbeOutcome::NonZero {
            code: Some(2),
            stderr: String::new(),
        };
        let msg = format_failure_message("xcrun", &[], &outcome).unwrap();
        assert!(
            msg.contains("(no stderr)"),
            "empty stderr should not collapse to an awkward trailing colon"
        );
    }

    #[test]
    fn format_failure_message_uses_signal_placeholder_when_no_code() {
        // Signal-terminated process has no exit code; we surface that
        // distinctively rather than substituting 0 or panicking.
        let outcome = ProbeOutcome::NonZero {
            code: None,
            stderr: "killed".to_string(),
        };
        let msg = format_failure_message("xcrun", &[], &outcome).unwrap();
        assert!(msg.contains("<signal>"));
    }

    #[test]
    fn cargo_warning_lines_prefixes_each_line_of_hint() {
        let hint = "line one\nline two";
        let out = cargo_warning_lines(hint);
        assert_eq!(
            out,
            vec!["cargo:warning=line one", "cargo:warning=line two"]
        );
    }

    #[test]
    fn cargo_warning_lines_emits_one_line_per_hint_line() {
        // Use the real XCODE_HINT to guard against a future refactor
        // collapsing the multi-line install instructions into one line.
        let lines = cargo_warning_lines(XCODE_HINT);
        assert!(
            lines.len() >= 3,
            "XCODE_HINT should occupy several warning lines so cargo's \
             default formatting keeps it readable; got {} lines",
            lines.len()
        );
        assert!(lines.iter().all(|l| l.starts_with("cargo:warning=")));
    }
}
