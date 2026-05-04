//! Prompt pack loader.
//!
//! A `PromptPack` is the per-locale source of truth for every piece of
//! pedagogical text the Primer sends to the LLM: the base system prompt,
//! the per-intent instructions, age-banded language guidance, engagement
//! notes, knowledge / memory section intros, speaker labels, and the
//! factual-prefix list used by `decide_intent` to route direct-lookup
//! questions.
//!
//! The English pack (`prompts/en.toml`) is the reference. Adding a new
//! locale means: add the variant to `primer_core::i18n::Locale`, add a
//! `prompts/<pack_id>.toml` file, extend the `embedded_pack` dispatch,
//! and translate the prompts natively (not mechanically — the prompts
//! encode pedagogy, not just words).
//!
//! Loading: by default packs are embedded at compile time via
//! `include_str!`. Setting `PRIMER_PROMPTS_DIR=<dir>` makes `load()`
//! read `<dir>/<pack_id>.toml` from disk instead — useful for translator
//! iteration without recompilation.
//!
//! Validation: every field is scanned at load time for unknown
//! `{placeholder}` tokens. A typo (`{nme}` instead of `{name}`) is a
//! loud panic at startup, never a silent malformed prompt at runtime.

use std::collections::BTreeMap;
use std::sync::Arc;

use primer_core::conversation::PedagogicalIntent;
use primer_core::error::{PrimerError, Result};
use primer_core::i18n::Locale;
use primer_core::learner::EngagementState;
use serde::Deserialize;

/// The trait the prompt builder consumes. All methods are infallible
/// reads against an already-validated pack.
pub trait PromptPack: Send + Sync {
    fn locale(&self) -> Locale;
    /// Render the base system prompt with `{name}`, `{age}`, and
    /// `{language_guidance}` substituted. The `language_guidance` is
    /// itself selected from the pack via `language_guidance(age)`.
    fn render_base(&self, name: &str, age: u8) -> String;
    /// Per-intent next-step instruction. Empty key is a hard error at
    /// load time, so this never returns the empty string.
    fn intent_instruction(&self, intent: PedagogicalIntent) -> &str;
    /// Note appended for frustrated / disengaging states. Returns `""`
    /// (with no leading separator) for states that have no note —
    /// the caller decides whether to prepend `"\n\n"`.
    fn engagement_note(&self, state: EngagementState) -> &str;
    /// Single-line intro for the RAG passages section, with `{age}`
    /// substituted. The body is appended by the caller.
    fn knowledge_intro(&self, age: u8) -> String;
    fn summary_intro(&self) -> &str;
    fn retrieved_intro(&self) -> &str;
    fn child_label(&self) -> &str;
    fn primer_label(&self) -> &str;
    /// Lowercased prefixes that mark a child's input as a direct
    /// factual lookup. Empty for locales (e.g. Japanese) where prefix
    /// matching doesn't apply — `decide_intent` falls back to the
    /// LLM-based classifier in that case.
    fn factual_prefixes(&self) -> &[String];
}

/// Per-locale packs embedded at compile time so a binary can ship
/// without any data files alongside it. Override at runtime via
/// `PRIMER_PROMPTS_DIR`.
const EN_TOML: &str = include_str!("../prompts/en.toml");
const DE_TOML: &str = include_str!("../prompts/de.toml");

fn embedded_pack(locale: Locale) -> &'static str {
    match locale {
        Locale::English => EN_TOML,
        Locale::German => DE_TOML,
    }
}

/// Load the prompt pack for `locale`.
///
/// Lookup order:
/// 1. If `PRIMER_PROMPTS_DIR` is set, read `<dir>/<pack_id>.toml`.
/// 2. Otherwise, parse the compile-time-embedded pack.
///
/// Panics on placeholder validation failure (loud-at-startup); returns
/// `Err` on I/O or TOML-parse failures.
pub fn load(locale: Locale) -> Result<Arc<dyn PromptPack>> {
    let raw = match std::env::var("PRIMER_PROMPTS_DIR") {
        Ok(dir) => {
            let path = std::path::Path::new(&dir).join(format!("{}.toml", locale.pack_id()));
            std::fs::read_to_string(&path).map_err(|e| {
                PrimerError::Config(format!(
                    "PRIMER_PROMPTS_DIR set but {} could not be read: {e}",
                    path.display()
                ))
            })?
        }
        Err(_) => embedded_pack(locale).to_string(),
    };
    let pack = TomlPromptPack::from_toml_str(locale, &raw)?;
    Ok(Arc::new(pack))
}

/// `TomlPromptPack` is the only `PromptPack` impl shipped today; the
/// trait exists so a future test or experiment can substitute a
/// hand-built pack without touching the loader.
pub struct TomlPromptPack {
    locale: Locale,
    base_template: String,
    language_guidance: LanguageGuidanceBands,
    /// Indexed by `intent_index(intent)` — fixed size = `ALL_INTENTS.len()`.
    /// Built once at load time so per-call lookup is O(1) without
    /// requiring `Hash` on `PedagogicalIntent`.
    intents: [String; ALL_INTENTS.len()],
    engagement_frustrated: String,
    engagement_disengaging: String,
    knowledge_intro_template: String,
    summary_intro: String,
    retrieved_intro: String,
    child_label: String,
    primer_label: String,
    factual_prefixes: Vec<String>,
}

impl TomlPromptPack {
    /// Parse a TOML pack body and validate placeholders. Used by the
    /// loader and by tests that want to inject a synthetic pack.
    pub fn from_toml_str(locale: Locale, body: &str) -> Result<Self> {
        let raw: PackFile = toml::from_str(body)
            .map_err(|e| PrimerError::Config(format!("prompt pack: parse failed: {e}")))?;

        // Per-field placeholder allowlists. A typo here panics with the
        // field name and offending token so a broken pack fails loudly
        // at startup rather than producing malformed prompts at runtime.
        validate_placeholders(
            "system_prompt.base",
            &raw.system_prompt.base,
            &["name", "age", "language_guidance"],
        );
        validate_placeholders(
            "language_guidance.ages_0_6",
            &raw.language_guidance.ages_0_6,
            &[],
        );
        validate_placeholders(
            "language_guidance.ages_7_9",
            &raw.language_guidance.ages_7_9,
            &[],
        );
        validate_placeholders(
            "language_guidance.ages_10_12",
            &raw.language_guidance.ages_10_12,
            &[],
        );
        validate_placeholders(
            "language_guidance.ages_13_plus",
            &raw.language_guidance.ages_13_plus,
            &[],
        );
        for (key, value) in &raw.intent {
            validate_placeholders(&format!("intent.{key}"), value, &[]);
        }
        validate_placeholders("engagement.frustrated", &raw.engagement.frustrated, &[]);
        validate_placeholders("engagement.disengaging", &raw.engagement.disengaging, &[]);
        validate_placeholders(
            "sections.knowledge_intro",
            &raw.sections.knowledge_intro,
            &["age"],
        );
        validate_placeholders("sections.summary_intro", &raw.sections.summary_intro, &[]);
        validate_placeholders(
            "sections.retrieved_intro",
            &raw.sections.retrieved_intro,
            &[],
        );
        validate_placeholders("labels.child", &raw.labels.child, &[]);
        validate_placeholders("labels.primer", &raw.labels.primer, &[]);

        // Stage the parsed intent strings keyed by canonical name so we
        // can validate completeness before materialising the indexed
        // array. A `BTreeMap<&str, String>` keeps ordering deterministic
        // for the missing-key error message (helpful for translators).
        let mut staged: BTreeMap<String, String> = BTreeMap::new();
        for (key, value) in raw.intent {
            if parse_intent_key(&key).is_none() {
                return Err(PrimerError::Config(format!(
                    "prompt pack: unknown intent key '{key}'"
                )));
            }
            staged.insert(key, value);
        }
        let intents: [String; ALL_INTENTS.len()] = {
            let mut arr: [String; ALL_INTENTS.len()] = Default::default();
            for (i, variant) in ALL_INTENTS.iter().enumerate() {
                let key = intent_key(*variant);
                match staged.get(key) {
                    Some(v) => arr[i] = v.clone(),
                    None => {
                        return Err(PrimerError::Config(format!(
                            "prompt pack: missing intent '{key}'"
                        )));
                    }
                }
            }
            arr
        };

        Ok(Self {
            locale,
            base_template: raw.system_prompt.base,
            language_guidance: raw.language_guidance,
            intents,
            engagement_frustrated: raw.engagement.frustrated,
            engagement_disengaging: raw.engagement.disengaging,
            knowledge_intro_template: raw.sections.knowledge_intro,
            summary_intro: raw.sections.summary_intro,
            retrieved_intro: raw.sections.retrieved_intro,
            child_label: raw.labels.child,
            primer_label: raw.labels.primer,
            factual_prefixes: raw.question_detection.factual_prefixes,
        })
    }

    fn language_guidance(&self, age: u8) -> &str {
        match age {
            0..=6 => &self.language_guidance.ages_0_6,
            7..=9 => &self.language_guidance.ages_7_9,
            10..=12 => &self.language_guidance.ages_10_12,
            _ => &self.language_guidance.ages_13_plus,
        }
    }
}

impl PromptPack for TomlPromptPack {
    fn locale(&self) -> Locale {
        self.locale
    }

    fn render_base(&self, name: &str, age: u8) -> String {
        // Order matters: substitute `{language_guidance}` first because
        // the band text might (in principle) contain `{age}` (none of
        // the English bands do today, but this keeps semantics stable
        // if a future pack adds one).
        let lg = self.language_guidance(age);
        let age_str = age.to_string();
        self.base_template
            .replace("{language_guidance}", lg)
            .replace("{name}", name)
            .replace("{age}", &age_str)
    }

    fn intent_instruction(&self, intent: PedagogicalIntent) -> &str {
        &self.intents[intent_index(intent)]
    }

    fn engagement_note(&self, state: EngagementState) -> &str {
        match state {
            EngagementState::FrustratedStuck | EngagementState::FrustratedTrying => {
                &self.engagement_frustrated
            }
            EngagementState::Disengaging => &self.engagement_disengaging,
            _ => "",
        }
    }

    fn knowledge_intro(&self, age: u8) -> String {
        self.knowledge_intro_template
            .replace("{age}", &age.to_string())
    }

    fn summary_intro(&self) -> &str {
        &self.summary_intro
    }
    fn retrieved_intro(&self) -> &str {
        &self.retrieved_intro
    }
    fn child_label(&self) -> &str {
        &self.child_label
    }
    fn primer_label(&self) -> &str {
        &self.primer_label
    }
    fn factual_prefixes(&self) -> &[String] {
        &self.factual_prefixes
    }
}

// ─── Raw TOML deserialisation types ─────────────────────────────────────────

#[derive(Deserialize)]
struct PackFile {
    #[allow(dead_code)]
    meta: MetaSection,
    system_prompt: SystemPromptSection,
    language_guidance: LanguageGuidanceBands,
    intent: BTreeMap<String, String>,
    engagement: EngagementSection,
    sections: SectionsSection,
    labels: LabelsSection,
    question_detection: QuestionDetectionSection,
}

#[derive(Deserialize)]
struct MetaSection {
    #[allow(dead_code)]
    language: String,
    #[allow(dead_code)]
    language_name: String,
    #[allow(dead_code)]
    bcp47: String,
}

#[derive(Deserialize)]
struct SystemPromptSection {
    base: String,
}

#[derive(Deserialize)]
struct LanguageGuidanceBands {
    ages_0_6: String,
    ages_7_9: String,
    ages_10_12: String,
    ages_13_plus: String,
}

#[derive(Deserialize)]
struct EngagementSection {
    frustrated: String,
    disengaging: String,
}

#[derive(Deserialize)]
struct SectionsSection {
    knowledge_intro: String,
    summary_intro: String,
    retrieved_intro: String,
}

#[derive(Deserialize)]
struct LabelsSection {
    child: String,
    primer: String,
}

#[derive(Deserialize)]
struct QuestionDetectionSection {
    factual_prefixes: Vec<String>,
}

// ─── Intent key mapping ─────────────────────────────────────────────────────

const ALL_INTENTS: &[PedagogicalIntent] = &[
    PedagogicalIntent::SocraticQuestion,
    PedagogicalIntent::ComprehensionCheck,
    PedagogicalIntent::Scaffolding,
    PedagogicalIntent::Encouragement,
    PedagogicalIntent::Extension,
    PedagogicalIntent::DirectAnswer,
    PedagogicalIntent::AnswerThenPivot,
    PedagogicalIntent::SessionClose,
];

fn intent_key(intent: PedagogicalIntent) -> &'static str {
    match intent {
        PedagogicalIntent::SocraticQuestion => "socratic_question",
        PedagogicalIntent::ComprehensionCheck => "comprehension_check",
        PedagogicalIntent::Scaffolding => "scaffolding",
        PedagogicalIntent::Encouragement => "encouragement",
        PedagogicalIntent::Extension => "extension",
        PedagogicalIntent::DirectAnswer => "direct_answer",
        PedagogicalIntent::AnswerThenPivot => "answer_then_pivot",
        PedagogicalIntent::SessionClose => "session_close",
    }
}

/// Position of `intent` in `ALL_INTENTS`. Used as the array index for
/// the typed-but-non-Hash lookup table inside `TomlPromptPack`.
fn intent_index(intent: PedagogicalIntent) -> usize {
    ALL_INTENTS
        .iter()
        .position(|v| *v == intent)
        .expect("ALL_INTENTS covers every PedagogicalIntent variant")
}

fn parse_intent_key(s: &str) -> Option<PedagogicalIntent> {
    ALL_INTENTS.iter().find(|v| intent_key(**v) == s).copied()
}

// ─── Placeholder validation ─────────────────────────────────────────────────

/// Scan `content` for `{ident}` placeholders and panic if any token is
/// not in `allowed`. Identifier rule: ASCII alpha or `_` first char,
/// then ASCII alphanumeric or `_`. Anything else inside `{...}`
/// (e.g. `{Hello, world}`) is left alone — translators can use brace
/// characters in narrative text without false positives.
fn validate_placeholders(field: &str, content: &str, allowed: &[&str]) {
    let bytes = content.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            let start = i + 1;
            let mut end = start;
            while end < bytes.len() && bytes[end] != b'}' {
                end += 1;
            }
            if end < bytes.len() {
                let token = &content[start..end];
                if is_placeholder_ident(token) && !allowed.contains(&token) {
                    panic!(
                        "prompt pack: field {field} contains unknown placeholder {{{token}}}; allowed: {allowed:?}"
                    );
                }
                i = end + 1;
            } else {
                break;
            }
        } else {
            i += 1;
        }
    }
}

fn is_placeholder_ident(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
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
    fn german_pack_knowledge_intro_substitutes_age() {
        let pack = german_pack();
        let s = pack.knowledge_intro(8);
        assert!(s.contains("8-jähriges"), "got: {s}");
        assert!(!s.contains("{age}"));
    }

    #[test]
    #[should_panic(expected = "unknown placeholder")]
    fn corrupted_german_pack_with_unknown_placeholder_panics_at_load() {
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
language_name = "Deutsch"
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

[labels]
child = "Kind"
primer = "Primer"

[question_detection]
factual_prefixes = []
"#,
            INTENT_KEYS = all_intents_zeroed_toml(),
        );
        let _ = TomlPromptPack::from_toml_str(Locale::German, &body);
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
            "what does ",
            "how does ",
            "how do ",
            "how is ",
            "how are ",
        ];
        let got: Vec<&str> = pack.factual_prefixes().iter().map(String::as_str).collect();
        assert_eq!(got, want);
    }

    #[test]
    #[should_panic(expected = "unknown placeholder")]
    fn unknown_placeholder_in_base_is_a_loud_panic() {
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

[labels]
child = "Child"
primer = "Primer"

[question_detection]
factual_prefixes = []
"#,
            INTENT_KEYS = all_intents_zeroed_toml(),
        );
        let _ = TomlPromptPack::from_toml_str(Locale::English, &body);
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

[labels]
child = "Child"
primer = "Primer"

[question_detection]
factual_prefixes = []
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
}
