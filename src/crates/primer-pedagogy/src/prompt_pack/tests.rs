//! Unit tests for [`super`] (the prompt-pack loader).
//!
//! Extracted from the original flat `prompt_pack.rs` to keep each module
//! under the ~500-line guideline. `use super::*` carries the public
//! surface re-exported from `prompt_pack::mod`; the internal helpers now
//! live in sibling submodules, so they are imported explicitly below.

use std::sync::Arc;

use super::intents::{ALL_INTENTS, intent_key};
use super::loader::{emit_preview_warning_if_first, reset_preview_warn_once_for_test};
use super::render::{render_template, unescape_braces, validate_placeholders};
use super::*;

fn english_pack() -> Arc<dyn PromptPack> {
    load(Locale::English).expect("english pack loads")
}

fn german_pack() -> Arc<dyn PromptPack> {
    load(Locale::German).expect("german pack loads")
}

#[test]
fn german_pack_loads_from_embedded_toml() {
    let pack = german_pack();
    assert_eq!(pack.locale(), Locale::German);
    assert_eq!(pack.child_label(), "Kind");
    assert_eq!(pack.primer_label(), "Primer");
}

#[test]
fn german_pack_renders_base_with_name_and_age() {
    let pack = german_pack();
    let s = pack.render_base("Lieschen", 8);
    assert!(s.contains("namens Lieschen"), "got: {s}");
    assert!(s.contains("8 Jahre alt"), "got: {s}");
    assert!(
        !s.contains("{name}") && !s.contains("{age}") && !s.contains("{language_guidance}"),
        "all placeholders substituted: {s}"
    );
    // Sanity: a few key German phrases that should appear in the
    // rendered base — guards against accidental English fragments.
    assert!(s.contains("sokratischer"), "expected German ‘sokratischer’");
    assert!(s.contains("geduldiger"), "expected German ‘geduldiger’");
    assert!(!s.contains("Socratic learning"), "no English fragments");
}

#[test]
fn german_pack_instructs_informal_register() {
    // The base prompt MUST explicitly tell the model to address the
    // child with the informal "du", never the formal "Sie". German
    // children are universally duzed outside formal institutions;
    // small local models default to Sie for assistant↔user without
    // an explicit instruction. Regression guard for the bug where
    // granite4.1:8b-q8_0 addressed the child with "Sie".
    let pack = german_pack();
    let s = pack.render_base("Lieschen", 8);
    assert!(
        s.contains("ANREDE"),
        "base prompt should carry an explicit ANREDE block: {s}"
    );
    assert!(
        s.contains("informellen „du\""),
        "base prompt should name informal „du\" by exact word: {s}"
    );
    assert!(
        s.contains("NIEMALS"),
        "base prompt should forbid „Sie\" emphatically (NIEMALS): {s}"
    );
}

#[test]
fn german_pack_age_band_selection() {
    let pack = german_pack();
    // Pick a unique German marker per band.
    assert!(pack.render_base("X", 5).contains("Kindergarten"));
    assert!(pack.render_base("X", 8).contains("Grundschule"));
    assert!(
        pack.render_base("X", 11)
            .contains("Mittlere Satzlängen sind in Ordnung")
            || pack.render_base("X", 11).contains("mittlere Satzlängen")
    );
    assert!(pack.render_base("X", 15).contains("Erwachsenenwortschatz"));
}

#[test]
fn all_intents_slice_covers_every_pedagogical_intent() {
    // `ALL_INTENTS` is hand-maintained alongside `PedagogicalIntent::ALL`.
    // The pack-load and pack-lookup tests iterate `ALL_INTENTS`, so a new
    // enum variant forgotten here would silently skip pack-instruction
    // validation (the `name()`/`intent_key` matches are compiler-forced,
    // but this slice is not). This guard makes that drift a test failure.
    assert_eq!(
        ALL_INTENTS.len(),
        primer_core::conversation::PedagogicalIntent::ALL.len(),
        "ALL_INTENTS is out of sync with PedagogicalIntent::ALL"
    );
    for &intent in primer_core::conversation::PedagogicalIntent::ALL {
        assert!(
            ALL_INTENTS.contains(&intent),
            "ALL_INTENTS is missing {intent:?}"
        );
    }
}

#[test]
fn german_pack_intent_lookups_all_populated() {
    let pack = german_pack();
    for &intent in ALL_INTENTS {
        let s = pack.intent_instruction(intent);
        assert!(!s.is_empty(), "missing instruction for {intent:?}");
        // Every intent message should be in German — assert the
        // absence of the English-pack signature phrase as a smoke
        // test against accidental copy-paste from en.toml.
        assert!(
            !s.contains("Your next response"),
            "english fragment leaked into intent {intent:?}: {s}"
        );
    }
}

#[test]
fn german_pack_engagement_notes_in_german() {
    let pack = german_pack();
    let frustrated = pack.engagement_note(EngagementState::FrustratedStuck);
    assert!(frustrated.contains("WICHTIG"), "got: {frustrated}");
    assert!(frustrated.contains("frustriert"), "got: {frustrated}");
    let disengaging = pack.engagement_note(EngagementState::Disengaging);
    assert!(disengaging.contains("HINWEIS"), "got: {disengaging}");
    assert!(disengaging.contains("Interesse"), "got: {disengaging}");
}

#[test]
fn german_pack_factual_prefixes_are_german() {
    let pack = german_pack();
    let prefixes: Vec<&str> = pack.factual_prefixes().iter().map(String::as_str).collect();
    assert!(
        !prefixes.is_empty(),
        "german factual_prefixes must not be empty"
    );
    assert!(
        prefixes.contains(&"was ist "),
        "expected ‘was ist ’: {prefixes:?}"
    );
    assert!(
        prefixes.contains(&"wie funktioniert "),
        "expected ‘wie funktioniert ’: {prefixes:?}"
    );
    // Negative: no English prefixes should leak in.
    assert!(
        !prefixes.contains(&"what is "),
        "english prefix leaked into german pack: {prefixes:?}"
    );
}

#[test]
fn shipping_packs_populate_assertion_detection_openers() {
    // The `[assertion_detection]` section is `#[serde(default)]`, so a pack
    // that omits it loads with empty lists and silently keeps the broad
    // ProbeReasoning routing. The stable shipping packs (en, de) must
    // populate both lists, or the "how do you know?" route would fire on
    // requests/confusion in those locales. This guards that gap.
    for (name, pack) in [("en", english_pack()), ("de", german_pack())] {
        assert!(
            !pack.confusion_openers().is_empty(),
            "{name} pack confusion_openers must not be empty"
        );
        assert!(
            !pack.request_openers().is_empty(),
            "{name} pack request_openers must not be empty"
        );
    }
}

#[test]
fn german_pack_knowledge_intro_substitutes_age() {
    let pack = german_pack();
    let s = pack.knowledge_intro(8);
    assert!(s.contains("8-jähriges"), "got: {s}");
    assert!(!s.contains("{age}"));
}

#[test]
fn corrupted_german_pack_with_unknown_placeholder_returns_err() {
    // Deliberately corrupt the language_guidance field of an
    // otherwise-valid German pack with a `{nme}` typo. Validates
    // that the per-field placeholder allowlist fires for German
    // exactly as it does for English (same code path; this is a
    // sanity test that the locale-dispatch wiring doesn't somehow
    // bypass validation).
    let body = format!(
        r#"
[meta]
language = "de"
language_name = "German"
bcp47 = "de-DE"

[system_prompt]
base = "Hallo {{name}}, {{age}} Jahre alt.\n{{language_guidance}}"

[language_guidance]
ages_0_6 = "Hallo {{nme}}"
ages_7_9 = ""
ages_10_12 = ""
ages_13_plus = ""

[intent]
{INTENT_KEYS}

[engagement]
frustrated = ""
disengaging = ""

[sections]
knowledge_intro = ""
summary_intro = ""
retrieved_intro = ""
vocab_review_intro = ""
break_suggestion_intro = ""
memory_limit_retry = "x"
memory_limit_soft_stop = "x"

[labels]
child = "Kind"
primer = "Primer"

[question_detection]
factual_prefixes = []

[voice_state]
listen_label = "x"
listen_hint = "x"
thinking_label = "x"
thinking_hint = "x"
speak_label = "x"
speak_hint = "x"
"#,
        INTENT_KEYS = all_intents_zeroed_toml(),
    );
    let result = TomlPromptPack::from_toml_str(Locale::German, &body);
    let err = result.err().expect("expected unknown-placeholder error");
    let s = format!("{err}");
    assert!(s.contains("unknown placeholder"), "got: {s}");
    assert!(s.contains("nme"), "got: {s}");
}

#[test]
fn english_pack_loads_from_embedded_toml() {
    let pack = english_pack();
    assert_eq!(pack.locale(), Locale::English);
    assert_eq!(pack.child_label(), "Child");
    assert_eq!(pack.primer_label(), "Primer");
}

#[test]
fn english_pack_renders_base_with_name_and_age() {
    let pack = english_pack();
    let s = pack.render_base("Tester", 8);
    assert!(s.contains("named Tester"), "got: {s}");
    assert!(s.contains("age 8"), "got: {s}");
    assert!(
        !s.contains("{name}") && !s.contains("{age}") && !s.contains("{language_guidance}"),
        "all placeholders substituted: {s}"
    );
}

#[test]
fn english_pack_age_band_selection() {
    let pack = english_pack();
    assert!(pack.render_base("X", 5).contains("kindergarten"));
    assert!(pack.render_base("X", 8).contains("primary school"));
    assert!(
        pack.render_base("X", 11)
            .contains("moderate sentence length")
    );
    assert!(pack.render_base("X", 15).contains("Adult-level vocabulary"));
}

#[test]
fn english_pack_intent_lookups() {
    let pack = english_pack();
    for &intent in ALL_INTENTS {
        assert!(
            !pack.intent_instruction(intent).is_empty(),
            "missing instruction for {intent:?}"
        );
    }
}

#[test]
fn english_pack_engagement_notes() {
    let pack = english_pack();
    assert!(
        pack.engagement_note(EngagementState::FrustratedStuck)
            .contains("frustrated")
    );
    assert!(
        pack.engagement_note(EngagementState::FrustratedTrying)
            .contains("frustrated")
    );
    assert!(
        pack.engagement_note(EngagementState::Disengaging)
            .contains("losing interest")
    );
    assert!(pack.engagement_note(EngagementState::Engaged).is_empty());
    assert!(pack.engagement_note(EngagementState::Reflecting).is_empty());
    assert!(pack.engagement_note(EngagementState::Unknown).is_empty());
}

#[test]
fn english_pack_knowledge_intro_substitutes_age() {
    let pack = english_pack();
    let s = pack.knowledge_intro(8);
    assert!(s.contains("8-year-old"), "got: {s}");
    assert!(!s.contains("{age}"));
}

#[test]
fn english_pack_factual_prefixes_match_legacy_list() {
    let pack = english_pack();
    let want: &[&str] = &[
        "what is ",
        "what are ",
        "what's ",
        "how does ",
        "how do ",
        "how is ",
        "how are ",
    ];
    let got: Vec<&str> = pack.factual_prefixes().iter().map(String::as_str).collect();
    assert_eq!(got, want);
}

#[test]
fn english_pack_excludes_what_does_to_preserve_vocab_discipline() {
    let pack = english_pack();
    assert!(
        !pack.factual_prefixes().iter().any(|p| p == "what does "),
        "\"what does \" must NOT be in en.toml factual_prefixes — \
             it would short-circuit the vocabulary-discipline pedagogy \
             (\"what does X mean?\" should reach the LLM, not DirectAnswer)"
    );
}

#[test]
fn unknown_placeholder_in_base_returns_err() {
    let body = format!(
        r#"
[meta]
language = "en"
language_name = "English"
bcp47 = "en-US"

[system_prompt]
base = "Hello {{nme}}, age {{age}}.\n{{language_guidance}}"

[language_guidance]
ages_0_6 = ""
ages_7_9 = ""
ages_10_12 = ""
ages_13_plus = ""

[intent]
{INTENT_KEYS}

[engagement]
frustrated = ""
disengaging = ""

[sections]
knowledge_intro = ""
summary_intro = ""
retrieved_intro = ""
vocab_review_intro = ""
break_suggestion_intro = ""
memory_limit_retry = "x"
memory_limit_soft_stop = "x"

[labels]
child = "Child"
primer = "Primer"

[question_detection]
factual_prefixes = []

[voice_state]
listen_label = "x"
listen_hint = "x"
thinking_label = "x"
thinking_hint = "x"
speak_label = "x"
speak_hint = "x"
"#,
        INTENT_KEYS = all_intents_zeroed_toml(),
    );
    let result = TomlPromptPack::from_toml_str(Locale::English, &body);
    let err = result.err().expect("expected unknown-placeholder error");
    let s = format!("{err}");
    assert!(s.contains("unknown placeholder"), "got: {s}");
    assert!(s.contains("system_prompt.base"), "got: {s}");
    assert!(s.contains("nme"), "got: {s}");
}

// ─── Brace-escape: render_template (single-pass) ────────────────────────
// `{{` / `}}` render as literal `{` / `}`; `{key}` substitutes a known
// var; an unknown `{...}` is emitted verbatim. See issue #20.

#[test]
fn render_template_substitutes_known_key() {
    assert_eq!(
        render_template("Hi {name}!", &[("name", "Binti")]),
        "Hi Binti!"
    );
}

#[test]
fn render_template_leaves_unknown_token_verbatim() {
    // No matching var (the verbatim-field case): emit the token as-is.
    assert_eq!(render_template("a {x} b", &[]), "a {x} b");
}

#[test]
fn render_template_unescapes_doubled_braces() {
    assert_eq!(render_template("a {{b}} c", &[]), "a {b} c");
}

#[test]
fn render_template_does_not_substitute_escaped_placeholder_name() {
    // `{{name}}` is a literal `{name}`, NOT an interpolation — even when
    // `name` is a known var. This is the bug the naive replace-chain had.
    assert_eq!(render_template("{{name}}", &[("name", "Binti")]), "{name}");
}

#[test]
fn render_template_handles_escape_around_real_placeholder() {
    // `{{{name}}}` == literal `{` + `{name}` + literal `}` == `{Binti}`.
    assert_eq!(
        render_template("{{{name}}}", &[("name", "Binti")]),
        "{Binti}"
    );
}

#[test]
fn render_template_leaves_non_ident_braces_verbatim() {
    // `{Hello, world}` is not an identifier; it survives validation and
    // must render verbatim.
    assert_eq!(
        render_template("see {Hello, world}", &[]),
        "see {Hello, world}"
    );
}

#[test]
fn render_template_emits_unterminated_open_brace_verbatim() {
    assert_eq!(render_template("a {b", &[]), "a {b");
}

#[test]
fn render_template_emits_lone_close_brace_verbatim() {
    assert_eq!(render_template("a } b", &[]), "a } b");
}

#[test]
fn render_template_preserves_utf8_runs() {
    assert_eq!(
        render_template("Tschüß {name} 世界", &[("name", "Lieschen")]),
        "Tschüß Lieschen 世界"
    );
}

#[test]
fn render_template_substitutes_multiple_keys() {
    assert_eq!(
        render_template("{a}-{b}-{a}", &[("a", "X"), ("b", "Y")]),
        "X-Y-X"
    );
}

#[test]
fn render_template_substitutes_then_emits_trailing_lone_close_brace() {
    // `{name}}` is lenient-parsed as `{name}` (a substituted placeholder)
    // followed by a lone `}` emitted verbatim — NOT as `{name` + `}}`.
    // No shipping pack relies on this, but pin it so a future renderer
    // refactor can't silently change the edge behaviour.
    assert_eq!(render_template("{name}}", &[("name", "Binti")]), "Binti}");
}

#[test]
fn unescape_braces_is_render_template_with_no_vars() {
    assert_eq!(unescape_braces("use {{braces}} here"), "use {braces} here");
    assert_eq!(unescape_braces("no braces"), "no braces");
}

// ─── Brace-escape: validate_placeholders ────────────────────────────────

#[test]
fn validate_accepts_escaped_braces_as_literal() {
    // `{{Beispiel}}` is a translator-authored literal, not a placeholder.
    assert!(validate_placeholders("f", "see {{Beispiel}} here", &[]).is_ok());
}

#[test]
fn validate_still_rejects_single_brace_unknown_placeholder() {
    // Single-brace narrative still errors — translators must double-up.
    assert!(validate_placeholders("f", "see {Beispiel} here", &[]).is_err());
}

#[test]
fn validate_accepts_bare_doubled_open_brace() {
    assert!(validate_placeholders("f", "a {{ b", &[]).is_ok());
}

#[test]
fn validate_rejects_real_placeholder_adjacent_to_escape() {
    // The escape is skipped, but the genuine `{nme}` typo is still caught.
    let err = validate_placeholders("base", "{{x}} {nme}", &["name"]).unwrap_err();
    assert!(format!("{err}").contains("nme"), "got: {err}");
}

#[test]
fn missing_intent_variant_returns_err() {
    // Build a pack body that omits one intent key.
    let body = r#"
[meta]
language = "en"
language_name = "English"
bcp47 = "en-US"

[system_prompt]
base = "x"

[language_guidance]
ages_0_6 = ""
ages_7_9 = ""
ages_10_12 = ""
ages_13_plus = ""

[intent]
socratic_question = "x"

[engagement]
frustrated = ""
disengaging = ""

[sections]
knowledge_intro = ""
summary_intro = ""
retrieved_intro = ""
vocab_review_intro = ""
break_suggestion_intro = ""
memory_limit_retry = "x"
memory_limit_soft_stop = "x"

[labels]
child = "Child"
primer = "Primer"

[question_detection]
factual_prefixes = []

[voice_state]
listen_label = "x"
listen_hint = "x"
thinking_label = "x"
thinking_hint = "x"
speak_label = "x"
speak_hint = "x"
"#;
    let result = TomlPromptPack::from_toml_str(Locale::English, body);
    let err = result.err().expect("expected missing-intent error");
    assert!(format!("{err}").contains("missing intent"), "got: {err}");
}

fn all_intents_zeroed_toml() -> String {
    let mut out = String::new();
    for &i in ALL_INTENTS {
        out.push_str(&format!("{} = \"x\"\n", intent_key(i)));
    }
    out
}

/// Build a structurally-valid English pack body with caller-supplied
/// `system_prompt.base` and `sections.summary_intro` values. Uses
/// `.replace()` (not `format!`) so brace characters in `base` /
/// `summary_intro` pass through literally — `format!` would treat `{{`
/// as its own escape and corrupt the very syntax under test.
fn pack_body_with_base_and_summary(base: &str, summary_intro: &str) -> String {
    let template = r#"
[meta]
language = "en"
language_name = "English"
bcp47 = "en-US"

[system_prompt]
base = "__BASE__"

[language_guidance]
ages_0_6 = ""
ages_7_9 = ""
ages_10_12 = ""
ages_13_plus = ""

[intent]
__INTENTS__

[engagement]
frustrated = ""
disengaging = ""

[sections]
knowledge_intro = ""
summary_intro = "__SUMMARY__"
retrieved_intro = ""
vocab_review_intro = ""
break_suggestion_intro = ""
memory_limit_retry = "x"
memory_limit_soft_stop = "x"

[labels]
child = "Child"
primer = "Primer"

[question_detection]
factual_prefixes = []

[voice_state]
listen_label = "x"
listen_hint = "x"
thinking_label = "x"
thinking_hint = "x"
speak_label = "x"
speak_hint = "x"
"#;
    template
        .replace("__INTENTS__", &all_intents_zeroed_toml())
        .replace("__BASE__", base)
        .replace("__SUMMARY__", summary_intro)
}

#[test]
fn escaped_braces_in_verbatim_field_render_as_literal() {
    // A translator writes `{{focus}}` in narrative; the loaded pack
    // exposes a literal `{focus}`, not the doubled form.
    let body = pack_body_with_base_and_summary("x", "Type {{focus}} to begin");
    let pack = TomlPromptPack::from_toml_str(Locale::English, &body)
        .expect("escaped braces in summary_intro should load");
    assert_eq!(pack.summary_intro(), "Type {focus} to begin");
}

#[test]
fn escaped_braces_in_base_template_render_as_literal() {
    // Escapes in the (templated) base survive alongside real placeholders.
    let body = pack_body_with_base_and_summary("Hi {name}, e.g. {{ratio}} here", "y");
    let pack = TomlPromptPack::from_toml_str(Locale::English, &body)
        .expect("escaped braces in base should load");
    let rendered = pack.render_base("Binti", 8);
    assert!(rendered.contains("Hi Binti"), "got: {rendered}");
    assert!(rendered.contains("{ratio}"), "got: {rendered}");
    assert!(!rendered.contains("{{ratio}}"), "got: {rendered}");
}

#[test]
fn single_brace_narrative_in_verbatim_field_still_errors() {
    // Single braces remain a placeholder attempt and fail loudly.
    let body = pack_body_with_base_and_summary("x", "Type {focus} to begin");
    let err = TomlPromptPack::from_toml_str(Locale::English, &body)
        .err()
        .expect("single-brace narrative should error");
    let s = format!("{err}");
    assert!(s.contains("summary_intro"), "got: {s}");
    assert!(s.contains("focus"), "got: {s}");
}

/// Build a minimal but structurally-valid pack body with overridable
/// `[meta]` and `[question_detection]` blocks. Used by the meta-
/// consistency and factual-prefix tests.
fn synthetic_pack_body(
    meta_language: &str,
    meta_language_name: &str,
    meta_bcp47: &str,
    factual_prefixes_array: &str,
) -> String {
    synthetic_pack_body_with_status(
        meta_language,
        meta_language_name,
        meta_bcp47,
        factual_prefixes_array,
        None,
    )
}

#[test]
fn pack_status_defaults_to_stable_when_field_absent_for_english() {
    let pack = english_pack();
    assert_eq!(pack.status(), PackStatus::Stable);
}

#[test]
fn pack_status_defaults_to_stable_when_field_absent_for_german() {
    let pack = german_pack();
    assert_eq!(pack.status(), PackStatus::Stable);
}

#[test]
fn pack_status_explicit_stable_loads_as_stable() {
    let body = synthetic_pack_body_with_status("en", "English", "en-US", "[]", Some("stable"));
    let pack = TomlPromptPack::from_toml_str(Locale::English, &body)
        .expect("explicit status=stable should load");
    assert_eq!(pack.status(), PackStatus::Stable);
}

#[test]
fn pack_status_explicit_preview_loads_as_preview() {
    let body = synthetic_pack_body_with_status("en", "English", "en-US", "[]", Some("preview"));
    let pack = TomlPromptPack::from_toml_str(Locale::English, &body)
        .expect("explicit status=preview should load");
    assert_eq!(pack.status(), PackStatus::Preview);
}

#[test]
fn pack_status_rejects_unknown_value() {
    let body = synthetic_pack_body_with_status("en", "English", "en-US", "[]", Some("wip"));
    let err = TomlPromptPack::from_toml_str(Locale::English, &body)
        .err()
        .expect("expected unknown-status error");
    let s = format!("{err}");
    assert!(s.contains("status"), "got: {s}");
    assert!(s.contains("wip"), "got: {s}");
    assert!(
        s.contains("allowed"),
        "error should name the allow-list: {s}"
    );
    assert!(s.contains("stable"), "error should name valid values: {s}");
    assert!(s.contains("preview"), "error should name valid values: {s}");
}

#[test]
fn preview_warning_emits_once_per_locale_on_load_cached() {
    // Use a captured-tracing subscriber to count events. Reset the
    // per-locale warn-once gate via the test-only helper so this test
    // is order-independent.
    use std::sync::{Arc, Mutex};
    use tracing::{Level, subscriber::with_default};

    struct Counter(Arc<Mutex<usize>>);

    impl<S> tracing_subscriber::Layer<S> for Counter
    where
        S: tracing::Subscriber,
    {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            let m = event.metadata();
            if m.level() == &Level::WARN && m.target() == "primer::prompt_pack" {
                *self.0.lock().unwrap() += 1;
            }
        }
    }

    // Skip the strict assertion under PRIMER_PROMPTS_DIR — load_cached
    // bypasses the cache there, so the warn-once gate has nothing to
    // gate.
    if std::env::var_os("PRIMER_PROMPTS_DIR").is_some() {
        return;
    }

    // Reset the warn-once gate for the test locale so we measure
    // *this* test's emissions, not a previous test's residue.
    reset_preview_warn_once_for_test(Locale::English);

    let count = Arc::new(Mutex::new(0usize));
    let layer = Counter(Arc::clone(&count));
    use tracing_subscriber::prelude::*;
    let subscriber = tracing_subscriber::registry().with(layer);

    with_default(subscriber, || {
        // We need a Preview English pack to exercise the warn path,
        // but the embedded EN pack is Stable. Easiest path: bypass
        // the cache via from_toml_str on a synthetic preview body
        // and emit the warning manually — the test is a child module
        // so it can call the module-private function directly.
        // -- This test asserts the function's idempotence.
        emit_preview_warning_if_first(Locale::English);
        emit_preview_warning_if_first(Locale::English);
        emit_preview_warning_if_first(Locale::English);
    });

    assert_eq!(
        *count.lock().unwrap(),
        1,
        "expected exactly one warn event for repeated preview emits"
    );
}

/// The English pack's `[meta]` block must agree with `Locale::English`
/// across all three projections — language id, display name, and
/// BCP-47 tag. This is what the meta-consistency check inside
/// `from_toml_str` enforces; the test guards the en.toml file in
/// the tree against drift.
#[test]
fn english_pack_meta_matches_locale_projections() {
    // The successful load is itself the strongest assertion the
    // file's meta block matches the enum (a mismatch would Err).
    let pack = english_pack();
    assert_eq!(pack.locale(), Locale::English);
    // Spot-check the projections against the canonical values so a
    // future refactor that drops the load-time check still trips this
    // test.
    assert_eq!(Locale::English.pack_id(), "en");
    assert_eq!(Locale::English.name(), "English");
    assert_eq!(Locale::English.bcp47(), "en-US");
}

#[test]
fn meta_language_mismatch_returns_err() {
    let body = synthetic_pack_body("zz", "English", "en-US", "[]");
    let err = TomlPromptPack::from_toml_str(Locale::English, &body)
        .err()
        .expect("expected meta.language mismatch error");
    let s = format!("{err}");
    assert!(s.contains("meta.language"), "got: {s}");
}

#[test]
fn meta_language_name_mismatch_returns_err() {
    let body = synthetic_pack_body("en", "Englsih", "en-US", "[]");
    let err = TomlPromptPack::from_toml_str(Locale::English, &body)
        .err()
        .expect("expected meta.language_name mismatch error");
    let s = format!("{err}");
    assert!(s.contains("meta.language_name"), "got: {s}");
}

#[test]
fn meta_bcp47_mismatch_returns_err() {
    let body = synthetic_pack_body("en", "English", "en-GB", "[]");
    let err = TomlPromptPack::from_toml_str(Locale::English, &body)
        .err()
        .expect("expected meta.bcp47 mismatch error");
    let s = format!("{err}");
    assert!(s.contains("meta.bcp47"), "got: {s}");
}

/// Empty `factual_prefixes` round-trips through the loader — locales
/// where prefix matching doesn't apply (Japanese particles, Mandarin
/// tone-disambiguation) are expected to ship with `factual_prefixes = []`.
#[test]
fn empty_factual_prefixes_loads_and_disables_prefix_matching() {
    let body = synthetic_pack_body("en", "English", "en-US", "[]");
    let pack = TomlPromptPack::from_toml_str(Locale::English, &body).expect("synthetic pack loads");
    assert!(pack.factual_prefixes().is_empty());
}

/// `load_cached` returns the same `Arc` on repeated calls. PRIMER_
/// PROMPTS_DIR-driven test isolation is achieved by NOT setting that
/// env var here, so the cache path is exercised.
#[test]
fn load_cached_returns_same_arc_on_repeated_calls() {
    // SAFETY: the cache only short-circuits when PRIMER_PROMPTS_DIR
    // is unset; this test inherits the parent process env and must
    // not have it set. cargo test inherits a clean env in CI; locally
    // a developer who has set it is exercising the bypass path on
    // purpose, so we skip the strict-equality check there.
    if std::env::var_os("PRIMER_PROMPTS_DIR").is_some() {
        return;
    }
    let a = load_cached(Locale::English).expect("first load_cached");
    let b = load_cached(Locale::English).expect("second load_cached");
    assert!(
        Arc::ptr_eq(&a, &b),
        "load_cached should return the same Arc on repeat calls"
    );
}

#[test]
fn english_pack_exposes_vocab_review_intro() {
    let pack = english_pack();
    let intro = pack.vocab_review_intro();
    assert!(!intro.is_empty(), "vocab_review_intro must not be empty");
    assert!(
        intro.contains("topically relevant"),
        "expected English intro to contain 'topically relevant', got: {intro}"
    );
}

#[test]
fn german_pack_exposes_vocab_review_intro() {
    let pack = german_pack();
    let intro = pack.vocab_review_intro();
    assert!(
        !intro.is_empty(),
        "German vocab_review_intro must not be empty"
    );
    assert!(
        intro.contains("thematisch passen"),
        "expected German intro to contain 'thematisch passen', got: {intro}"
    );
}

#[test]
fn english_pack_exposes_break_suggestion_intro() {
    let pack = load(Locale::English).unwrap();
    let rendered = pack.break_suggestion_intro(30);
    assert!(
        !rendered.is_empty(),
        "break_suggestion_intro must not be empty"
    );
    assert!(
        rendered.contains("30"),
        "rendered intro should contain the minutes value: {rendered:?}"
    );
    assert!(
        !rendered.contains("{minutes}"),
        "{{minutes}} placeholder must be substituted: {rendered:?}"
    );
}

#[test]
fn german_pack_exposes_break_suggestion_intro() {
    let pack = load(Locale::German).unwrap();
    let rendered = pack.break_suggestion_intro(30);
    assert!(
        !rendered.is_empty(),
        "German break_suggestion_intro must not be empty"
    );
    assert!(
        rendered.contains("30"),
        "rendered intro should contain the minutes value: {rendered:?}"
    );
    assert!(
        rendered.contains("Minuten"),
        "German rendered intro should contain 'Minuten': {rendered:?}"
    );
    assert!(
        !rendered.contains("{minutes}"),
        "{{minutes}} placeholder must be substituted: {rendered:?}"
    );
}

#[test]
fn break_suggestion_intro_substitutes_arbitrary_minutes() {
    let pack = load(Locale::English).unwrap();
    let rendered = pack.break_suggestion_intro(45);
    assert!(rendered.contains("45"), "{rendered:?}");
}

#[test]
fn english_pack_exposes_memory_limit_strings() {
    let pack = english_pack();
    assert!(!pack.memory_limit_retry().is_empty());
    assert!(!pack.memory_limit_soft_stop().is_empty());
}

#[test]
fn german_pack_exposes_memory_limit_strings() {
    let pack = german_pack();
    assert!(!pack.memory_limit_retry().is_empty());
    assert!(!pack.memory_limit_soft_stop().is_empty());
}

#[test]
fn empty_memory_limit_retry_returns_err() {
    // The apology is streamed unconditionally on the truncation path, so
    // an empty value must fail loudly at load time (like voice_state),
    // not silently stream nothing.
    let body = synthetic_pack_body("en", "English", "en-US", "[]");
    let bad = body.replace(r#"memory_limit_retry = "x""#, r#"memory_limit_retry = """#);
    let err = TomlPromptPack::from_toml_str(Locale::English, &bad)
        .err()
        .expect("expected empty memory_limit_retry to fail");
    let s = format!("{err}");
    assert!(s.contains("sections.memory_limit_retry"), "got: {s}");
    assert!(s.contains("must not be empty"), "got: {s}");
}

/// The English pack's [voice_state] table holds the same byte-identical
/// strings the GUI used to hardcode in `VoiceStateCopy::for_locale`
/// before the i18n move. Any drift here will be flagged in the GUI's
/// `voice_state_copy_english_strings_pinned` regression witness too,
/// but pinning at the pack layer first localises a future failure to
/// the en.toml file rather than the GUI bridge.
#[test]
fn english_pack_exposes_voice_state_labels() {
    let pack = english_pack();
    let labels = pack.voice_state_labels();
    assert_eq!(labels.listen_label, "Listening…");
    assert_eq!(labels.listen_hint, "take your time");
    assert_eq!(labels.thinking_label, "Thinking…");
    assert_eq!(labels.thinking_hint, "the Primer is working on a reply");
    assert_eq!(labels.speak_label, "Speaking…");
    assert_eq!(labels.speak_hint, "let the Primer finish");
}

/// Sibling of [`english_pack_exposes_voice_state_labels`] — pins the
/// byte-identical German strings the GUI used to hardcode before the
/// i18n move.
#[test]
fn german_pack_exposes_voice_state_labels() {
    let pack = german_pack();
    let labels = pack.voice_state_labels();
    assert_eq!(labels.listen_label, "Höre zu…");
    assert_eq!(labels.listen_hint, "lass dir Zeit");
    assert_eq!(labels.thinking_label, "Denke nach…");
    assert_eq!(labels.thinking_hint, "der Primer überlegt eine Antwort");
    assert_eq!(labels.speak_label, "Spreche…");
    assert_eq!(labels.speak_hint, "lass den Primer ausreden");
}

/// Empty values in any [voice_state] field are a pack-shape error.
/// Consumers render the strings without checking for emptiness, so a
/// silent empty would produce a blank UI label rather than failing
/// loudly at startup.
#[test]
fn empty_voice_state_field_returns_err() {
    let body = synthetic_pack_body("en", "English", "en-US", "[]");
    let bad = body.replace(r#"listen_label = "x""#, r#"listen_label = """#);
    let err = TomlPromptPack::from_toml_str(Locale::English, &bad)
        .err()
        .expect("expected empty voice_state field to fail");
    let s = format!("{err}");
    assert!(s.contains("voice_state.listen_label"), "got: {s}");
    assert!(s.contains("must not be empty"), "got: {s}");
}

#[test]
fn break_suggestion_intro_with_zero_minutes_renders_zero() {
    // Even though zero is the "disabled" sentinel at the gate level,
    // the trait method itself should faithfully substitute whatever
    // it's given. The dialogue manager's gate prevents zero from
    // ever reaching the trait method in production.
    let pack = load(Locale::English).unwrap();
    let rendered = pack.break_suggestion_intro(0);
    assert!(rendered.contains('0'), "{rendered:?}");
}

fn hindi_pack() -> Arc<dyn PromptPack> {
    load(Locale::Hindi).expect("hindi pack loads")
}

#[test]
fn hindi_pack_loads_in_preview_status() {
    let pack = hindi_pack();
    assert_eq!(pack.locale(), Locale::Hindi);
    assert_eq!(pack.status(), PackStatus::Preview);
}

#[test]
fn hindi_pack_intent_lookups_all_populated() {
    let pack = hindi_pack();
    for &intent in ALL_INTENTS {
        let s = pack.intent_instruction(intent);
        assert!(!s.is_empty(), "missing instruction for {intent:?}");
        assert!(
            s.chars().any(|c| ('\u{0900}'..='\u{097F}').contains(&c)),
            "no Devanagari in intent {intent:?}: {s}"
        );
    }
}

#[test]
fn hindi_pack_voice_state_section_complete() {
    let pack = hindi_pack();
    let labels = pack.voice_state_labels();
    for (name, value) in [
        ("listen_label", &labels.listen_label),
        ("listen_hint", &labels.listen_hint),
        ("thinking_label", &labels.thinking_label),
        ("thinking_hint", &labels.thinking_hint),
        ("speak_label", &labels.speak_label),
        ("speak_hint", &labels.speak_hint),
    ] {
        assert!(!value.is_empty(), "voice_state.{name} is empty");
        assert!(
            value
                .chars()
                .any(|c| ('\u{0900}'..='\u{097F}').contains(&c)),
            "no Devanagari in voice_state.{name}: {value}"
        );
    }
}

#[test]
fn hindi_pack_renders_base_with_name_and_age() {
    let pack = hindi_pack();
    let s = pack.render_base("Aarav", 8);
    assert!(s.contains("Aarav"), "got: {s}");
    assert!(s.contains("8"), "got: {s}");
    assert!(
        !s.contains("{name}") && !s.contains("{age}") && !s.contains("{language_guidance}"),
        "all placeholders substituted: {s}"
    );
}

#[test]
fn hindi_pack_knowledge_intro_substitutes_age() {
    let pack = hindi_pack();
    let s = pack.knowledge_intro(8);
    assert!(s.contains("8"), "got: {s}");
    assert!(!s.contains("{age}"));
}

#[test]
fn hindi_pack_break_suggestion_intro_substitutes_minutes() {
    let pack = hindi_pack();
    let s = pack.break_suggestion_intro(30);
    assert!(s.contains("30"), "got: {s}");
    assert!(!s.contains("{minutes}"));
}

/// Calling load_cached(Hindi) twice in the same process must emit
/// exactly one warn event (target = "primer::prompt_pack"). The Hindi
/// pack is the only Preview locale at the time of this test, so any
/// warn at that target during repeated loads is the gated event.
#[test]
fn hindi_load_cached_warns_exactly_once_per_locale() {
    use std::sync::{Arc, Mutex};
    use tracing::{Level, subscriber::with_default};

    struct Counter(Arc<Mutex<usize>>);

    impl<S> tracing_subscriber::Layer<S> for Counter
    where
        S: tracing::Subscriber,
    {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            let m = event.metadata();
            if m.level() == &Level::WARN && m.target() == "primer::prompt_pack" {
                *self.0.lock().unwrap() += 1;
            }
        }
    }

    if std::env::var_os("PRIMER_PROMPTS_DIR").is_some() {
        return;
    }

    reset_preview_warn_once_for_test(Locale::Hindi);

    let count = Arc::new(Mutex::new(0usize));
    let layer = Counter(Arc::clone(&count));
    use tracing_subscriber::prelude::*;
    let subscriber = tracing_subscriber::registry().with(layer);

    with_default(subscriber, || {
        let _ = load_cached(Locale::Hindi).expect("first load_cached");
        let _ = load_cached(Locale::Hindi).expect("second load_cached");
        let _ = load_cached(Locale::Hindi).expect("third load_cached");
    });

    assert_eq!(
        *count.lock().unwrap(),
        1,
        "expected exactly one warn for repeated load_cached(Hindi)"
    );
}

/// Variant of `synthetic_pack_body` that lets callers inject a
/// `[meta] status = "..."` line. `None` omits the line entirely
/// (verifies the "absent => Stable" default).
fn synthetic_pack_body_with_status(
    meta_language: &str,
    meta_language_name: &str,
    meta_bcp47: &str,
    factual_prefixes_array: &str,
    status: Option<&str>,
) -> String {
    let status_line = match status {
        Some(s) => format!("status = \"{s}\"\n"),
        None => String::new(),
    };
    format!(
        r#"
[meta]
language = "{meta_language}"
language_name = "{meta_language_name}"
bcp47 = "{meta_bcp47}"
{status_line}
[system_prompt]
base = "x"

[language_guidance]
ages_0_6 = ""
ages_7_9 = ""
ages_10_12 = ""
ages_13_plus = ""

[intent]
{INTENT_KEYS}

[engagement]
frustrated = ""
disengaging = ""

[sections]
knowledge_intro = ""
summary_intro = ""
retrieved_intro = ""
vocab_review_intro = ""
break_suggestion_intro = ""
memory_limit_retry = "x"
memory_limit_soft_stop = "x"

[labels]
child = "Child"
primer = "Primer"

[question_detection]
factual_prefixes = {factual_prefixes_array}

[voice_state]
listen_label = "x"
listen_hint = "x"
thinking_label = "x"
thinking_hint = "x"
speak_label = "x"
speak_hint = "x"
"#,
        INTENT_KEYS = all_intents_zeroed_toml(),
    )
}
