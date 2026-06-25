//! Tests for the CLI parse/validation helpers and `Cli` parsing.
//!
//! Relocated verbatim from `main.rs`. `Cli` lives in the crate root, so it
//! is reached via `crate::Cli`; the helpers live in the parent module
//! (`super`).

#[cfg(test)]
mod cli_parse_tests {
    use crate::Cli;
    use crate::cli_support::*;
    use clap::Parser;
    #[cfg(feature = "qnn")]
    use std::path::Path;

    #[cfg(feature = "speech")]
    #[test]
    fn parse_mic_silence_ms_accepts_in_range_values() {
        assert_eq!(parse_mic_silence_ms("50"), Ok(50));
        assert_eq!(parse_mic_silence_ms("600"), Ok(600));
        assert_eq!(parse_mic_silence_ms("5000"), Ok(5000));
    }

    #[cfg(feature = "speech")]
    #[test]
    fn parse_mic_silence_ms_rejects_out_of_range() {
        assert!(parse_mic_silence_ms("0").is_err());
        assert!(parse_mic_silence_ms("49").is_err());
        assert!(parse_mic_silence_ms("5001").is_err());
        assert!(parse_mic_silence_ms("100000").is_err());
    }

    #[cfg(feature = "speech")]
    #[test]
    fn parse_mic_silence_ms_rejects_non_numeric() {
        assert!(parse_mic_silence_ms("abc").is_err());
        assert!(parse_mic_silence_ms("").is_err());
        assert!(parse_mic_silence_ms("-100").is_err());
    }

    // ─── --speech requires_all gating (issue #112) ──────────────────────
    //
    // On the macOS-native build (`--features speech,macos-native` on
    // macOS) the whisper/piper asset flags are not declared at all —
    // SFSpeechRecognizer + AVSpeechSynthesizer carry the STT and TTS
    // halves of the loop and the corresponding clap fields disappear.
    // On every other speech build the existing `requires_all` contract
    // still applies, because `build_local_backends` needs all three
    // model paths to open whisper + piper.

    #[cfg(all(
        feature = "speech",
        all(
            target_os = "macos",
            any(feature = "macos-native", feature = "macos-native-26")
        )
    ))]
    #[test]
    fn speech_alone_parses_on_macos_native_without_whisper_piper_flags() {
        let result = Cli::try_parse_from(["primer", "--speech"]);
        assert!(
            result.is_ok(),
            "expected --speech alone to parse on macos-native/macos-native-26; got: {result:?}"
        );
    }

    #[cfg(all(
        feature = "speech",
        not(all(
            target_os = "macos",
            any(feature = "macos-native", feature = "macos-native-26")
        ))
    ))]
    #[test]
    fn speech_alone_still_rejected_off_macos_native() {
        let result = Cli::try_parse_from(["primer", "--speech"]);
        assert!(
            result.is_err(),
            "expected clap to reject --speech without whisper/piper flags on \
             non-macos-native builds; got: {result:?}"
        );
    }

    // ─── --tts piper|supertonic conditional requirements (issue #170) ────

    #[cfg(all(
        feature = "speech",
        not(all(
            target_os = "macos",
            any(feature = "macos-native", feature = "macos-native-26")
        ))
    ))]
    #[test]
    fn plain_repl_without_speech_parses_without_voice_assets() {
        // The default --tts is piper, but with no --speech the voice asset
        // flags must NOT be required (regression guard for the
        // required_if_eq_all gating).
        let res = Cli::try_parse_from(["primer"]);
        assert!(
            res.is_ok(),
            "plain REPL must parse with no speech flags: {res:?}"
        );
    }

    #[cfg(all(
        feature = "speech",
        not(all(
            target_os = "macos",
            any(feature = "macos-native", feature = "macos-native-26")
        ))
    ))]
    #[test]
    fn speech_piper_default_requires_piper_assets() {
        let res = Cli::try_parse_from(["primer", "--speech", "--whisper-model", "/m.bin"]);
        assert!(
            res.is_err(),
            "--speech with default --tts piper still needs --voice-onnx/--voice-config"
        );
    }

    #[cfg(all(
        feature = "speech",
        not(all(
            target_os = "macos",
            any(feature = "macos-native", feature = "macos-native-26")
        ))
    ))]
    #[test]
    fn speech_supertonic_requires_supertonic_assets() {
        let res = Cli::try_parse_from([
            "primer",
            "--speech",
            "--whisper-model",
            "/m.bin",
            "--tts",
            "supertonic",
        ]);
        assert!(
            res.is_err(),
            "--tts supertonic needs --supertonic-dir/--supertonic-voice-style"
        );
    }

    /// Runtime backstop: even if the `tts_assets` ArgGroup is satisfied by the
    /// wrong assets (clap can't express the per-tts split — it only knows
    /// "≥1 asset"), `validate_speech_assets` rejects a Supertonic session with
    /// no supertonic dir, naming the missing flag. Uses the test binary's own
    /// path as the (always-existing) whisper stand-in so validation gets past
    /// the whisper check and reaches the Supertonic arm.
    #[cfg(all(
        feature = "speech",
        not(all(
            target_os = "macos",
            any(feature = "macos-native", feature = "macos-native-26")
        ))
    ))]
    #[test]
    fn validate_rejects_supertonic_without_dir() {
        let existing = std::env::current_exe().expect("test binary path exists");
        let err = validate_speech_assets(
            &existing,
            primer_speech::voice_loop::TtsBackend::Supertonic,
            None,
            None,
            None, // supertonic_dir missing
            None,
            "ignored-voice-id",
        )
        .expect_err("supertonic with no dir must fail validation");
        let msg = format!("{err}");
        assert!(
            msg.contains("supertonic-dir"),
            "error must name the missing flag: {msg}"
        );
    }

    #[cfg(all(
        feature = "speech",
        not(all(
            target_os = "macos",
            any(feature = "macos-native", feature = "macos-native-26")
        ))
    ))]
    #[test]
    fn speech_supertonic_parses_with_assets() {
        let res = Cli::try_parse_from([
            "primer",
            "--speech",
            "--whisper-model",
            "/m.bin",
            "--tts",
            "supertonic",
            "--supertonic-dir",
            "/sup/onnx",
            "--supertonic-voice-style",
            "/sup/voice_styles/F1.json",
        ]);
        assert!(res.is_ok(), "supertonic with assets should parse: {res:?}");
    }

    #[cfg(all(
        feature = "speech",
        not(all(
            target_os = "macos",
            any(feature = "macos-native", feature = "macos-native-26")
        ))
    ))]
    #[test]
    fn speech_piper_parses_with_assets() {
        let res = Cli::try_parse_from([
            "primer",
            "--speech",
            "--whisper-model",
            "/m.bin",
            "--voice-onnx",
            "/v.onnx",
            "--voice-config",
            "/v.onnx.json",
        ]);
        assert!(res.is_ok(), "piper with assets should parse: {res:?}");
    }

    #[test]
    fn no_persist_conflicts_with_resume_at_parse_time() {
        // clap should reject a `--no-persist --resume <uuid>` invocation
        // before we ever try to open anything. In-memory + resume is
        // a contradiction (nothing to resume from).
        let result = Cli::try_parse_from([
            "primer",
            "--no-persist",
            "--resume",
            "00000000-0000-0000-0000-000000000000",
        ]);
        assert!(result.is_err(), "expected clap to reject the combination");
    }

    #[test]
    fn no_persist_conflicts_with_session_db_at_parse_time() {
        // Naming a session DB while asking for in-memory is also a
        // contradiction; clap should reject it up front.
        let result = Cli::try_parse_from(["primer", "--no-persist", "--session-db", "/tmp/x.db"]);
        assert!(result.is_err(), "expected clap to reject the combination");
    }

    // ─── --vocab-max-per-prompt parse tests ─────────────────────────────

    #[test]
    fn parses_vocab_max_per_prompt_explicit_value() {
        let cli = Cli::try_parse_from(["primer", "--vocab-max-per-prompt", "6"]).unwrap();
        assert_eq!(cli.vocab_max_per_prompt, Some(6));
    }

    #[test]
    fn vocab_max_per_prompt_defaults_to_none_when_not_passed() {
        let cli = Cli::try_parse_from(["primer"]).unwrap();
        assert_eq!(cli.vocab_max_per_prompt, None);
    }

    #[test]
    fn vocab_max_per_prompt_zero_is_rejected_at_parse() {
        // 0 is a valid usize value but is meaningless for this flag —
        // clap's range(1..) rejects it with a clear error.
        let result = Cli::try_parse_from(["primer", "--vocab-max-per-prompt", "0"]);
        assert!(result.is_err(), "0 should be rejected; got: {result:?}");
    }

    // ─── --primary-ttft-budget-ms parse tests (Phase 1.3) ───────────────

    #[test]
    fn primary_ttft_budget_defaults_to_none() {
        let cli = Cli::try_parse_from(["primer"]).unwrap();
        assert_eq!(cli.primary_ttft_budget_ms, None);
    }

    #[test]
    fn primary_ttft_budget_parses_explicit_value() {
        let cli = Cli::try_parse_from(["primer", "--primary-ttft-budget-ms", "750"]).unwrap();
        assert_eq!(cli.primary_ttft_budget_ms, Some(750));
    }

    #[test]
    fn primary_ttft_budget_zero_is_rejected_at_parse() {
        // 0 would mean "always nudge"; that's not a budget. Reject it for
        // parity with the GUI's `min="1"` (clap's range(1..)).
        let result = Cli::try_parse_from(["primer", "--primary-ttft-budget-ms", "0"]);
        assert!(result.is_err(), "0 should be rejected; got: {result:?}");
    }

    // ─── NPU serialisation warning (Phase 1.2 step 1.2.4) ────────────────

    #[test]
    fn warn_when_every_subsystem_inherits_main_qnn() {
        // No overrides → each subsystem inherits the main backend.
        // Under --backend qnn this means everything runs on the NPU
        // and serialises through the dialog mutex.
        let w = npu_serialisation_warning(None, None, None);
        assert!(w.is_some(), "expected a warning; got None");
        let msg = w.unwrap();
        assert!(
            msg.contains("serialise") || msg.contains("serialize"),
            "expected serialisation hint; got: {msg}"
        );
    }

    #[test]
    fn warn_when_every_subsystem_is_explicitly_qnn() {
        // Equivalent semantically to "all None under --backend qnn"
        // (both resolve to `"qnn"` per the inherit-the-main-backend
        // rule), but pinned separately because a future refactor
        // that handled the explicit case differently from the
        // inherit case would silently break this contract.
        let w = npu_serialisation_warning(Some("qnn"), Some("qnn"), Some("qnn"));
        assert!(w.is_some(), "expected a warning; got None");
        let msg = w.unwrap();
        assert!(
            msg.contains("serialise") || msg.contains("serialize"),
            "expected serialisation hint; got: {msg}"
        );
    }

    #[test]
    fn warn_when_every_subsystem_is_stub() {
        // All-stub means the conversation runs without classifier-
        // driven features. Deliberate for smoke tests, but worth
        // calling out so a fresh user doesn't think it's broken.
        let w = npu_serialisation_warning(Some("stub"), Some("stub"), Some("stub"));
        assert!(w.is_some(), "expected a warning; got None");
        let msg = w.unwrap();
        assert!(msg.contains("stub"), "expected stub hint; got: {msg}");
    }

    #[test]
    fn no_warning_for_mixed_subsystem_config() {
        // One subsystem stubbed, two inherited → reasonable shape,
        // no warning. Specifically: classifier-on-stub is the
        // canonical "let me focus the NPU on chat" configuration.
        let w = npu_serialisation_warning(Some("stub"), None, None);
        assert!(w.is_none(), "mixed config should not warn; got: {w:?}");
    }

    #[test]
    fn no_warning_when_subsystems_use_external_backend() {
        // Classifier on cloud, others inherit qnn → mixed shape.
        let w = npu_serialisation_warning(Some("cloud"), None, None);
        assert!(
            w.is_none(),
            "cloud-classifier config should not warn; got: {w:?}"
        );
    }

    // ─── --backend qnn clap parse acceptance ─────────────────────────────

    #[cfg(feature = "qnn")]
    #[test]
    fn qnn_backend_requires_qnn_bundle_dir_at_parse() {
        // `--backend qnn` without `--qnn-bundle-dir` (and without the
        // `PRIMER_QNN_BUNDLE_DIR` env var) is rejected by clap before
        // any backend construction is attempted.
        // Use `std::env::remove_var` indirectly by capturing the
        // missing-env scenario: clap's required_if_eq fires when no
        // value source resolved a value. We can't reliably scrub the
        // env from a test (it's process-wide), so this test only
        // asserts the clap-required path. Running this test with
        // PRIMER_QNN_BUNDLE_DIR set in the environment will produce
        // an Ok parse — that's an acceptable degenerate case.
        if std::env::var_os("PRIMER_QNN_BUNDLE_DIR").is_some() {
            // Env var is set externally — the env fallback applies and
            // the required_if_eq check is satisfied. Print the skip so
            // a passing-but-skipped result is visible under `--nocapture`
            // rather than indistinguishable from a real green.
            eprintln!(
                "[skip] qnn_backend_requires_qnn_bundle_dir_at_parse: \
                 PRIMER_QNN_BUNDLE_DIR is set; clap's env fallback satisfies \
                 required_if_eq so we cannot assert the rejection path."
            );
            return;
        }
        let result = Cli::try_parse_from(["primer", "--backend", "qnn"]);
        assert!(
            result.is_err(),
            "expected clap to reject --backend qnn without --qnn-bundle-dir; got: {result:?}"
        );
    }

    #[cfg(feature = "qnn")]
    #[test]
    fn qnn_backend_with_bundle_dir_parses() {
        // Happy path: clap accepts `--backend qnn --qnn-bundle-dir <p>`.
        // Construction itself happens later in async_main and is the
        // engine's responsibility — this test pins the parse contract.
        //
        // Env-var defensive skip: clap's `env = "..."` resolves the
        // optional `--qnn-qairt-lib-dir` from `PRIMER_QNN_QAIRT_LIB_DIR`
        // if set in the test runner's environment, which would make
        // the `cli.qnn_qairt_lib_dir.is_none()` assertion fail. Skip
        // visibly so a developer with QAIRT installed locally doesn't
        // chase a misleading red.
        if std::env::var_os("PRIMER_QNN_QAIRT_LIB_DIR").is_some() {
            eprintln!(
                "[skip] qnn_backend_with_bundle_dir_parses: PRIMER_QNN_QAIRT_LIB_DIR is set; \
                 clap's env fallback would populate qnn_qairt_lib_dir."
            );
            return;
        }
        let cli = Cli::try_parse_from([
            "primer",
            "--backend",
            "qnn",
            "--qnn-bundle-dir",
            "/tmp/bundle",
        ])
        .expect("expected --backend qnn --qnn-bundle-dir to parse");
        assert_eq!(cli.backend, "qnn");
        assert_eq!(
            cli.qnn_bundle_dir.as_deref(),
            Some(Path::new("/tmp/bundle"))
        );
        assert!(
            cli.qnn_qairt_lib_dir.is_none(),
            "--qnn-qairt-lib-dir should default to None (engine resolves it)"
        );
    }

    #[cfg(feature = "qnn")]
    #[test]
    fn qnn_backend_accepts_optional_qairt_lib_dir() {
        let cli = Cli::try_parse_from([
            "primer",
            "--backend",
            "qnn",
            "--qnn-bundle-dir",
            "/tmp/bundle",
            "--qnn-qairt-lib-dir",
            "/opt/qairt/lib/aarch64-android",
        ])
        .expect("expected --qnn-qairt-lib-dir to be accepted alongside --qnn-bundle-dir");
        assert_eq!(
            cli.qnn_qairt_lib_dir.as_deref(),
            Some(Path::new("/opt/qairt/lib/aarch64-android"))
        );
    }

    #[cfg(feature = "qnn")]
    #[test]
    fn qnn_backend_compatible_with_no_persist_at_parse() {
        // --no-persist conflicts with --resume and --session-db, but
        // --backend qnn is orthogonal. Pin the compatibility so a
        // future conflicts-with bug fails this test instead of leaking
        // to runtime.
        let result = Cli::try_parse_from([
            "primer",
            "--backend",
            "qnn",
            "--qnn-bundle-dir",
            "/tmp/bundle",
            "--no-persist",
        ]);
        assert!(
            result.is_ok(),
            "expected --backend qnn + --no-persist to parse; got: {result:?}"
        );
    }
}

#[cfg(test)]
mod break_suggest_flag_tests {
    use crate::Cli;
    use clap::Parser;

    #[test]
    fn explicit_value_overrides_default() {
        let cli = Cli::try_parse_from([
            "primer",
            "--name",
            "Ada",
            "--age",
            "9",
            "--session-break-after-mins",
            "15",
        ])
        .unwrap();
        assert_eq!(cli.session_break_after_mins, 15);
    }

    #[test]
    fn default_is_30() {
        let cli = Cli::try_parse_from(["primer", "--name", "Ada", "--age", "9"]).unwrap();
        assert_eq!(
            cli.session_break_after_mins,
            primer_core::consts::break_suggest::DEFAULT_INTERVAL_MINUTES,
        );
    }

    #[test]
    fn zero_is_rejected() {
        let result = Cli::try_parse_from([
            "primer",
            "--name",
            "Ada",
            "--age",
            "9",
            "--session-break-after-mins",
            "0",
        ]);
        assert!(result.is_err(), "0 should be rejected by the value parser");
    }
}

#[cfg(test)]
mod reasoning_marker_tests {
    use crate::Cli;
    use crate::cli_support::pair_reasoning_markers;
    use clap::Parser;

    #[test]
    fn pairs_flat_args_into_tuples() {
        let flat = vec![
            "<a>".to_string(),
            "</a>".to_string(),
            "<b>".to_string(),
            "</b>".to_string(),
        ];
        assert_eq!(
            pair_reasoning_markers(flat),
            vec![
                ("<a>".to_string(), "</a>".to_string()),
                ("<b>".to_string(), "</b>".to_string()),
            ]
        );
    }

    #[test]
    fn empty_is_empty() {
        assert_eq!(
            pair_reasoning_markers(vec![]),
            Vec::<(String, String)>::new()
        );
    }

    #[test]
    fn odd_trailing_value_is_dropped() {
        // clap's num_args=2 makes odd counts impossible in practice, but the
        // helper must not panic if handed one.
        let flat = vec!["<a>".to_string(), "</a>".to_string(), "<stray>".to_string()];
        assert_eq!(
            pair_reasoning_markers(flat),
            vec![("<a>".to_string(), "</a>".to_string())]
        );
    }

    #[test]
    fn cli_parses_repeated_reasoning_marker_flags_into_pairs() {
        // Two repeated occurrences → a flat Vec of 4, paired into 2 tuples.
        let cli = Cli::parse_from([
            "primer",
            "--reasoning-marker",
            "<a>",
            "</a>",
            "--reasoning-marker",
            "<b>",
            "</b>",
        ]);
        assert_eq!(
            pair_reasoning_markers(cli.reasoning_marker),
            vec![
                ("<a>".to_string(), "</a>".to_string()),
                ("<b>".to_string(), "</b>".to_string()),
            ]
        );
    }

    #[test]
    fn cli_without_reasoning_marker_flag_is_empty() {
        let cli = Cli::parse_from(["primer"]);
        assert!(pair_reasoning_markers(cli.reasoning_marker).is_empty());
    }
}

#[cfg(test)]
mod embedder_backend_default_tests {
    use crate::Cli;
    use clap::Parser;

    /// On a build with the `embedding` feature (the default), a flagless
    /// invocation defaults to hybrid retrieval via fastembed.
    #[cfg(feature = "embedding")]
    #[test]
    fn default_is_fastembed_with_embedding_feature() {
        let cli = Cli::try_parse_from(["primer", "--name", "Ada", "--age", "9"]).unwrap();
        assert_eq!(cli.embedder_backend, "fastembed");
    }

    /// On a `--no-default-features` build (embedding off), the default
    /// stays BM25-only so the binary never hard-fails on a flagless run.
    #[cfg(not(feature = "embedding"))]
    #[test]
    fn default_is_none_without_embedding_feature() {
        let cli = Cli::try_parse_from(["primer", "--name", "Ada", "--age", "9"]).unwrap();
        assert_eq!(cli.embedder_backend, "none");
    }

    /// An explicit value always overrides the default, regardless of which
    /// feature build is active. `stub` is used because it is a valid value
    /// in BOTH build configurations (no cargo feature required) and differs
    /// from either feature-aware default (`fastembed` / `none`), so the
    /// assertion proves a real override on every build.
    #[test]
    fn explicit_value_overrides_default() {
        let cli = Cli::try_parse_from([
            "primer",
            "--name",
            "Ada",
            "--age",
            "9",
            "--embedder-backend",
            "stub",
        ])
        .unwrap();
        assert_eq!(cli.embedder_backend, "stub");
    }
}
