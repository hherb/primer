//! `TomlPromptPack`: the TOML-backed `PromptPack` implementation, its
//! raw deserialisation types, and the load-time validation that turns a
//! malformed pack into a loud startup error.
//!
//! `from_toml_str` parses a pack body, cross-checks its `[meta]` block
//! against `Locale`'s projections, scans every field for unknown
//! placeholders, and materialises the fixed-size per-intent instruction
//! array. Verbatim fields are brace-unescaped at load time; the three
//! `*_template` fields stay raw and are rendered per call.

use std::collections::BTreeMap;

use primer_core::conversation::PedagogicalIntent;
use primer_core::error::{PrimerError, Result};
use primer_core::i18n::Locale;
use primer_core::learner::EngagementState;
use serde::Deserialize;

use super::intents::{ALL_INTENTS, intent_index, intent_key, parse_intent_key};
use super::render::{render_template, unescape_braces, validate_placeholders};
use super::{PackStatus, PromptPack, VoiceStateLabels};

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
    break_suggestion_intro_template: String,
    memory_limit_retry: String,
    memory_limit_soft_stop: String,
    child_label: String,
    primer_label: String,
    factual_prefixes: Vec<String>,
    confusion_openers: Vec<String>,
    request_openers: Vec<String>,
    voice_state_labels: VoiceStateLabels,
    status: PackStatus,
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

        let status = PackStatus::from_meta(raw.meta.status.as_deref())?;

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
        validate_placeholders(
            "sections.vocab_review_intro",
            &raw.sections.vocab_review_intro,
            &[],
        )?;
        validate_placeholders(
            "sections.break_suggestion_intro",
            &raw.sections.break_suggestion_intro,
            &["minutes"],
        )?;
        // The context-limit apology / soft-stop are streamed to the child
        // unconditionally on the truncation path (unlike summary_intro etc.,
        // which are omitted when empty), so an empty value would silently
        // stream nothing. Require both to be present and non-empty at load
        // time — the same fail-loud discipline as `voice_state`.
        validate_non_empty(
            "sections.memory_limit_retry",
            &raw.sections.memory_limit_retry,
        )?;
        validate_non_empty(
            "sections.memory_limit_soft_stop",
            &raw.sections.memory_limit_soft_stop,
        )?;
        validate_placeholders("labels.child", &raw.labels.child, &[])?;
        validate_placeholders("labels.primer", &raw.labels.primer, &[])?;
        // No placeholders allowed in any voice_state field — every value
        // is a literal display string. Empty values are a pack-shape
        // error because consumers render the strings without checking
        // whether they're populated.
        validate_voice_state_section(&raw.voice_state)?;
        validate_placeholders(
            "voice_state.listen_label",
            &raw.voice_state.listen_label,
            &[],
        )?;
        validate_placeholders("voice_state.listen_hint", &raw.voice_state.listen_hint, &[])?;
        validate_placeholders(
            "voice_state.thinking_label",
            &raw.voice_state.thinking_label,
            &[],
        )?;
        validate_placeholders(
            "voice_state.thinking_hint",
            &raw.voice_state.thinking_hint,
            &[],
        )?;
        validate_placeholders("voice_state.speak_label", &raw.voice_state.speak_label, &[])?;
        validate_placeholders("voice_state.speak_hint", &raw.voice_state.speak_hint, &[])?;

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
                    Some(v) => arr[i] = unescape_braces(v),
                    None => {
                        return Err(PrimerError::Config(format!(
                            "prompt pack: missing intent '{key}'"
                        )));
                    }
                }
            }
            arr
        };

        // Verbatim fields (everything except the three `*_template` fields)
        // are unescaped at load time so `{{`/`}}` become literal braces in
        // the strings consumers read by reference (issue #20). The templated
        // fields stay raw — they are unescaped during `render_template` at
        // call time, which also performs placeholder substitution.
        Ok(Self {
            locale,
            base_template: raw.system_prompt.base,
            language_guidance: LanguageGuidanceBands {
                ages_0_6: unescape_braces(&raw.language_guidance.ages_0_6),
                ages_7_9: unescape_braces(&raw.language_guidance.ages_7_9),
                ages_10_12: unescape_braces(&raw.language_guidance.ages_10_12),
                ages_13_plus: unescape_braces(&raw.language_guidance.ages_13_plus),
            },
            intents,
            engagement_frustrated: unescape_braces(&raw.engagement.frustrated),
            engagement_disengaging: unescape_braces(&raw.engagement.disengaging),
            knowledge_intro_template: raw.sections.knowledge_intro,
            summary_intro: unescape_braces(&raw.sections.summary_intro),
            retrieved_intro: unescape_braces(&raw.sections.retrieved_intro),
            vocab_review_intro: unescape_braces(&raw.sections.vocab_review_intro),
            break_suggestion_intro_template: raw.sections.break_suggestion_intro,
            memory_limit_retry: unescape_braces(&raw.sections.memory_limit_retry),
            memory_limit_soft_stop: unescape_braces(&raw.sections.memory_limit_soft_stop),
            child_label: unescape_braces(&raw.labels.child),
            primer_label: unescape_braces(&raw.labels.primer),
            factual_prefixes: raw
                .question_detection
                .factual_prefixes
                .iter()
                .map(|p| unescape_braces(p))
                .collect(),
            confusion_openers: raw
                .assertion_detection
                .confusion_openers
                .iter()
                .map(|p| unescape_braces(p))
                .collect(),
            request_openers: raw
                .assertion_detection
                .request_openers
                .iter()
                .map(|p| unescape_braces(p))
                .collect(),
            voice_state_labels: VoiceStateLabels {
                listen_label: unescape_braces(&raw.voice_state.listen_label),
                listen_hint: unescape_braces(&raw.voice_state.listen_hint),
                thinking_label: unescape_braces(&raw.voice_state.thinking_label),
                thinking_hint: unescape_braces(&raw.voice_state.thinking_hint),
                speak_label: unescape_braces(&raw.voice_state.speak_label),
                speak_hint: unescape_braces(&raw.voice_state.speak_hint),
            },
            status,
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
        // Single-pass render (issue #20): substitutes `{name}`/`{age}`/
        // `{language_guidance}` and unescapes `{{`/`}}`. The band text is
        // validated placeholder-free (empty allowlist), so it carries no
        // `{age}` to re-substitute — single-pass is equivalent to the old
        // sequential `replace` chain for every shipping pack while also
        // honouring brace escapes the chain could not.
        let lg = self.language_guidance(age);
        let age_str = age.to_string();
        render_template(
            &self.base_template,
            &[("language_guidance", lg), ("name", name), ("age", &age_str)],
        )
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
        render_template(&self.knowledge_intro_template, &[("age", &age.to_string())])
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
    fn break_suggestion_intro(&self, minutes: u32) -> String {
        render_template(
            &self.break_suggestion_intro_template,
            &[("minutes", &minutes.to_string())],
        )
    }
    fn memory_limit_retry(&self) -> &str {
        &self.memory_limit_retry
    }
    fn memory_limit_soft_stop(&self) -> &str {
        &self.memory_limit_soft_stop
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
    fn confusion_openers(&self) -> &[String] {
        &self.confusion_openers
    }
    fn request_openers(&self) -> &[String] {
        &self.request_openers
    }
    fn voice_state_labels(&self) -> &VoiceStateLabels {
        &self.voice_state_labels
    }
    fn status(&self) -> PackStatus {
        self.status
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
    #[serde(default)]
    assertion_detection: AssertionDetectionSection,
    voice_state: VoiceStateSection,
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
    #[serde(default)]
    status: Option<String>,
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
    break_suggestion_intro: String,
    memory_limit_retry: String,
    memory_limit_soft_stop: String,
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

/// Openers that classify a child turn's speech-act for intent routing.
/// Optional section (`#[serde(default)]` at the use site) so packs that
/// predate it — and synthetic test packs — load with empty lists, which
/// disables the exclusions and falls back to the broader claim routing.
/// The shipping en/de/hi packs populate both lists; a test guards that.
#[derive(Deserialize, Default)]
struct AssertionDetectionSection {
    #[serde(default)]
    confusion_openers: Vec<String>,
    #[serde(default)]
    request_openers: Vec<String>,
}

#[derive(Deserialize)]
struct VoiceStateSection {
    listen_label: String,
    listen_hint: String,
    thinking_label: String,
    thinking_hint: String,
    speak_label: String,
    speak_hint: String,
}

/// Reject an empty value in any `[voice_state]` field. Renders unconditionally
/// (no `Option<&str>` plumbing in consumers), so an empty string would silently
/// produce a missing UI label rather than a clear pack-shape error at load time.
fn validate_voice_state_section(section: &VoiceStateSection) -> Result<()> {
    for (field, value) in [
        ("voice_state.listen_label", &section.listen_label),
        ("voice_state.listen_hint", &section.listen_hint),
        ("voice_state.thinking_label", &section.thinking_label),
        ("voice_state.thinking_hint", &section.thinking_hint),
        ("voice_state.speak_label", &section.speak_label),
        ("voice_state.speak_hint", &section.speak_hint),
    ] {
        if value.is_empty() {
            return Err(PrimerError::Config(format!(
                "prompt pack: field {field} must not be empty"
            )));
        }
    }
    Ok(())
}

/// Reject an empty pack field at load time. Used for strings that are
/// rendered unconditionally (so an empty value would silently produce
/// nothing) rather than omitted-when-empty.
fn validate_non_empty(field: &str, value: &str) -> Result<()> {
    if value.is_empty() {
        return Err(PrimerError::Config(format!(
            "prompt pack: field {field} must not be empty"
        )));
    }
    Ok(())
}
