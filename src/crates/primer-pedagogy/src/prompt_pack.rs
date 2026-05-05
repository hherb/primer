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
//! `{placeholder}` tokens. A typo (`{nme}` instead of `{name}`) returns
//! a `PrimerError::Config` from `load`, surfacing as a loud startup
//! failure rather than a silent malformed prompt at runtime. The same
//! treatment applies to missing-intent and meta-inconsistency errors —
//! every pack-shape problem is a single error variant.

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

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
    /// Single-line intro for the spaced-repetition vocabulary review
    /// section. Renders only when `due_vocab` is non-empty. Locale-keyed.
    fn vocab_review_intro(&self) -> &str;
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

/// Load the prompt pack for `locale`, freshly parsing every call.
///
/// Lookup order:
/// 1. If `PRIMER_PROMPTS_DIR` is set, read `<dir>/<pack_id>.toml`.
/// 2. Otherwise, parse the compile-time-embedded pack.
///
/// Returns `Err` on I/O failure, TOML-parse failure, placeholder
/// validation failure, missing-intent variants, or meta-inconsistency
/// against `Locale`'s projections. All pack-shape errors are surfaced as
/// `PrimerError::Config` so a broken pack fails loudly at startup.
///
/// Use [`load_cached`] for the production hot path; reserve `load` for
/// tests and PRIMER_PROMPTS_DIR-driven translator iteration.
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

/// Load the prompt pack for `locale`, returning a process-wide cached
/// instance after the first successful load.
///
/// When `PRIMER_PROMPTS_DIR` is set the cache is bypassed so translator
/// iteration sees fresh content on every call. Otherwise every caller
/// shares the same `Arc<dyn PromptPack>`, sidestepping a per-session
/// re-parse of the embedded TOML for callers like `DialogueManager::new`
/// that construct the pack but never need to mutate it.
pub fn load_cached(locale: Locale) -> Result<Arc<dyn PromptPack>> {
    // PRIMER_PROMPTS_DIR is the translator-iteration escape hatch; honour
    // it by bypassing the cache so a re-saved TOML file is reflected on
    // the next `load_cached` call.
    if std::env::var_os("PRIMER_PROMPTS_DIR").is_some() {
        return load(locale);
    }
    static EN_PACK: OnceLock<Arc<dyn PromptPack>> = OnceLock::new();
    static DE_PACK: OnceLock<Arc<dyn PromptPack>> = OnceLock::new();
    match locale {
        Locale::English => {
            if let Some(p) = EN_PACK.get() {
                return Ok(Arc::clone(p));
            }
            let p = load(locale)?;
            let _ = EN_PACK.set(Arc::clone(&p));
            Ok(p)
        }
        Locale::German => {
            if let Some(p) = DE_PACK.get() {
                return Ok(Arc::clone(p));
            }
            let p = load(locale)?;
            let _ = DE_PACK.set(Arc::clone(&p));
            Ok(p)
        }
    }
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
    vocab_review_intro: String,
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

        // Cross-check the file's metadata against the Rust enum's
        // projections. The `Locale` enum is the single source of truth
        // for language id, display name, and BCP-47 tag; the TOML file
        // duplicates them as documentation for translators. A mismatch
        // is a structural pack error — fail loudly at load time rather
        // than letting a stale `[meta]` block drift silently.
        if raw.meta.language != locale.pack_id() {
            return Err(PrimerError::Config(format!(
                "prompt pack: meta.language {:?} does not match Locale::{:?}.pack_id() {:?}",
                raw.meta.language,
                locale,
                locale.pack_id()
            )));
        }
        if raw.meta.language_name != locale.name() {
            return Err(PrimerError::Config(format!(
                "prompt pack: meta.language_name {:?} does not match Locale::{:?}.name() {:?}",
                raw.meta.language_name,
                locale,
                locale.name()
            )));
        }
        if raw.meta.bcp47 != locale.bcp47() {
            return Err(PrimerError::Config(format!(
                "prompt pack: meta.bcp47 {:?} does not match Locale::{:?}.bcp47() {:?}",
                raw.meta.bcp47,
                locale,
                locale.bcp47()
            )));
        }

        // Per-field placeholder allowlists. A typo here returns Err
        // with the field name and offending token so a broken pack
        // fails loudly at startup rather than producing malformed
        // prompts at runtime.
        validate_placeholders(
            "system_prompt.base",
            &raw.system_prompt.base,
            &["name", "age", "language_guidance"],
        )?;
        validate_placeholders(
            "language_guidance.ages_0_6",
            &raw.language_guidance.ages_0_6,
            &[],
        )?;
        validate_placeholders(
            "language_guidance.ages_7_9",
            &raw.language_guidance.ages_7_9,
            &[],
        )?;
        validate_placeholders(
            "language_guidance.ages_10_12",
            &raw.language_guidance.ages_10_12,
            &[],
        )?;
        validate_placeholders(
            "language_guidance.ages_13_plus",
            &raw.language_guidance.ages_13_plus,
            &[],
        )?;
        for (key, value) in &raw.intent {
            validate_placeholders(&format!("intent.{key}"), value, &[])?;
        }
        validate_placeholders("engagement.frustrated", &raw.engagement.frustrated, &[])?;
        validate_placeholders("engagement.disengaging", &raw.engagement.disengaging, &[])?;
        validate_placeholders(
            "sections.knowledge_intro",
            &raw.sections.knowledge_intro,
            &["age"],
        )?;
        validate_placeholders("sections.summary_intro", &raw.sections.summary_intro, &[])?;
        validate_placeholders(
            "sections.retrieved_intro",
            &raw.sections.retrieved_intro,
            &[],
        )?;
        validate_placeholders("labels.child", &raw.labels.child, &[])?;
        validate_placeholders("labels.primer", &raw.labels.primer, &[])?;

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
            vocab_review_intro: raw.sections.vocab_review_intro,
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
    fn vocab_review_intro(&self) -> &str {
        &self.vocab_review_intro
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
    meta: MetaSection,
    system_prompt: SystemPromptSection,
    language_guidance: LanguageGuidanceBands,
    intent: BTreeMap<String, String>,
    engagement: EngagementSection,
    sections: SectionsSection,
    labels: LabelsSection,
    question_detection: QuestionDetectionSection,
}

/// File-level documentation for translators. Cross-checked at load time
/// against `Locale`'s projections — a mismatch is a load error so
/// translators can't silently let the file's metadata drift away from
/// the enum.
#[derive(Deserialize)]
struct MetaSection {
    language: String,
    language_name: String,
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
    vocab_review_intro: String,
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

/// Scan `content` for `{ident}` placeholders and return Err if any
/// token is not in `allowed`. Identifier rule: ASCII alpha or `_` first
/// char, then ASCII alphanumeric or `_`. Anything else inside `{...}`
/// (e.g. `{Hello, world}`) is left alone — translators can use brace
/// characters in narrative text without false positives.
fn validate_placeholders(field: &str, content: &str, allowed: &[&str]) -> Result<()> {
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
                    return Err(PrimerError::Config(format!(
                        "prompt pack: field {field} contains unknown placeholder {{{token}}}; allowed: {allowed:?}"
                    )));
                }
                i = end + 1;
            } else {
                break;
            }
        } else {
            i += 1;
        }
    }
    Ok(())
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

[labels]
child = "Kind"
primer = "Primer"

[question_detection]
factual_prefixes = []
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

[labels]
child = "Child"
primer = "Primer"

[question_detection]
factual_prefixes = []
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

    /// Build a minimal but structurally-valid pack body with overridable
    /// `[meta]` and `[question_detection]` blocks. Used by the meta-
    /// consistency and factual-prefix tests.
    fn synthetic_pack_body(
        meta_language: &str,
        meta_language_name: &str,
        meta_bcp47: &str,
        factual_prefixes_array: &str,
    ) -> String {
        format!(
            r#"
[meta]
language = "{meta_language}"
language_name = "{meta_language_name}"
bcp47 = "{meta_bcp47}"

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

[labels]
child = "Child"
primer = "Primer"

[question_detection]
factual_prefixes = {factual_prefixes_array}
"#,
            INTENT_KEYS = all_intents_zeroed_toml(),
        )
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
        let pack =
            TomlPromptPack::from_toml_str(Locale::English, &body).expect("synthetic pack loads");
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
}
