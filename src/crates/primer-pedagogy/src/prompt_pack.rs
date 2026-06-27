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
//!
//! Literal braces (issue #20): `{{` / `}}` are the escape for a literal
//! `{` / `}`, like Rust `format!`. They are skipped by the scanner and
//! unescaped at render time, so a translator can write `{{Beispiel}}` in
//! narrative text without tripping the placeholder validator. A single
//! `{Beispiel}` is still a placeholder attempt and is rejected.

use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

use primer_core::conversation::PedagogicalIntent;
use primer_core::error::{PrimerError, Result};
use primer_core::i18n::Locale;
use primer_core::learner::EngagementState;
use serde::Deserialize;

/// Lifecycle status of a prompt pack. Set explicitly in `[meta] status`
/// or implicitly absent (which means `Stable`). The loader emits a
/// one-time warning per `(process, locale)` pair on `Preview` packs to
/// flag that the strings have not been through native-speaker review.
///
/// Allow-list: only `"stable"` and `"preview"` are accepted as TOML
/// values. Adding a new variant is a deliberate, two-place change:
/// the enum and the validator's `from_toml_str` arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackStatus {
    /// Default. All user-visible strings have been reviewed by a native speaker.
    /// Absent `[meta] status` in a pack TOML maps to `Stable`.
    Stable,
    /// Machine-translated draft awaiting native-speaker review. `load_cached`
    /// emits a one-time `tracing::warn!` per `(process, locale)` pair when a
    /// `Preview` pack loads, so logs make the unreviewed status obvious.
    Preview,
}

impl PackStatus {
    /// Parse the optional `[meta] status` value into the enum.
    /// Absence (`None`) and `"stable"` both map to `Stable`; only
    /// `"preview"` maps to `Preview`. Any other string is a load-time
    /// error so the validator catches typos.
    fn from_meta(raw: Option<&str>) -> Result<Self> {
        match raw {
            None | Some("stable") => Ok(Self::Stable),
            Some("preview") => Ok(Self::Preview),
            Some(other) => Err(PrimerError::Config(format!(
                "prompt pack: unknown [meta] status {other:?}; allowed: \"stable\", \"preview\""
            ))),
        }
    }
}

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
    /// Render the break-suggestion guidance section with `{minutes}`
    /// substituted. Renders only when the per-turn intent is `SuggestBreak`.
    /// Locale-keyed: each pack's TOML template owns its unit word
    /// ("minutes" / "Minuten") so adding a new locale is purely additive.
    fn break_suggestion_intro(&self, minutes: u32) -> String;
    /// Child-facing apology streamed when a turn was truncated by the
    /// context limit and the Primer is about to retry with a smaller
    /// prompt. Locale-keyed; no placeholders.
    fn memory_limit_retry(&self) -> &str;
    /// Child-facing cue streamed when context-limit retries are exhausted
    /// and the Primer accepts the partial reply. Locale-keyed; no
    /// placeholders.
    fn memory_limit_soft_stop(&self) -> &str;
    fn child_label(&self) -> &str;
    fn primer_label(&self) -> &str;
    /// Lowercased prefixes that mark a child's input as a direct
    /// factual lookup. Empty for locales (e.g. Japanese) where prefix
    /// matching doesn't apply — `decide_intent` falls back to the
    /// LLM-based classifier in that case.
    fn factual_prefixes(&self) -> &[String];
    /// Display strings for the three voice-mode UI states
    /// (LISTEN / LATENT_THINK / SPEAK). Locale-keyed. Consumed by the
    /// GUI's `get_voice_state_copy` Tauri command. No placeholders —
    /// every field is a literal display string. Empty fields are a
    /// pack-shape error caught at load time, so callers can render the
    /// returned references unconditionally.
    fn voice_state_labels(&self) -> &VoiceStateLabels;
    /// Lifecycle status of this pack. `Stable` for packs reviewed by a
    /// native speaker (the default when `[meta] status` is absent).
    /// `Preview` for machine-translated content awaiting review — the
    /// loader emits a one-time warning when these load.
    fn status(&self) -> PackStatus;
}

/// Display strings for the voice-mode UI states.
///
/// Locale-keyed copy for the three states the voice loop cycles through —
/// LISTEN, LATENT_THINK, SPEAK. Each state has a short label (above the
/// indicator) and a longer hint (below). Populated from the
/// `[voice_state]` table of the active prompt pack; the GUI consumes
/// these via `PromptPack::voice_state_labels`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceStateLabels {
    pub listen_label: String,
    pub listen_hint: String,
    pub thinking_label: String,
    pub thinking_hint: String,
    pub speak_label: String,
    pub speak_hint: String,
}

/// Per-locale packs embedded at compile time so a binary can ship
/// without any data files alongside it. Override at runtime via
/// `PRIMER_PROMPTS_DIR`.
const EN_TOML: &str = include_str!("../prompts/en.toml");
const DE_TOML: &str = include_str!("../prompts/de.toml");
const HI_TOML: &str = include_str!("../prompts/hi.toml");

fn embedded_pack(locale: Locale) -> &'static str {
    match locale {
        Locale::English => EN_TOML,
        Locale::German => DE_TOML,
        Locale::Hindi => HI_TOML,
    }
}

/// Per-process gate: tracks which preview locales have already emitted
/// their one-time warning. Populated by `emit_preview_warning_if_first`;
/// consulted by `load_cached`.
fn preview_warned_gate() -> &'static Mutex<HashSet<Locale>> {
    static GATE: OnceLock<Mutex<HashSet<Locale>>> = OnceLock::new();
    GATE.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Emit the preview-status warning for `locale` if it hasn't been emitted
/// before in this process. Idempotent across calls; the warning text
/// names the locale's pack_id so `tail -f` of logs can tell apart
/// concurrent preview locales.
fn emit_preview_warning_if_first(locale: Locale) {
    // Decide whether to emit inside a tight scope so the mutex guard is
    // released before `tracing::warn!` runs (a synchronously-writing
    // subscriber would otherwise hold the gate for the warn's duration).
    // Poison fallback: treat as "first time" and warn anyway — the spec
    // requires degrading gracefully, never silencing.
    let is_first = match preview_warned_gate().lock() {
        Ok(mut seen) => seen.insert(locale),
        Err(_) => true,
    };
    if is_first {
        tracing::warn!(
            target: "primer::prompt_pack",
            locale = locale.pack_id(),
            "prompt pack is in preview status — machine-translated content awaiting native-speaker review. \
             This locale is not in Locale::ALL and is not advertised to end users."
        );
    }
}

#[cfg(test)]
pub(super) fn reset_preview_warn_once_for_test(locale: Locale) {
    let mut seen = preview_warned_gate()
        .lock()
        .expect("preview gate mutex poisoned");
    seen.remove(&locale);
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
    static HI_PACK: OnceLock<Arc<dyn PromptPack>> = OnceLock::new();
    let pack = match locale {
        Locale::English => {
            if let Some(p) = EN_PACK.get() {
                Arc::clone(p)
            } else {
                let p = load(locale)?;
                let _ = EN_PACK.set(Arc::clone(&p));
                p
            }
        }
        Locale::German => {
            if let Some(p) = DE_PACK.get() {
                Arc::clone(p)
            } else {
                let p = load(locale)?;
                let _ = DE_PACK.set(Arc::clone(&p));
                p
            }
        }
        Locale::Hindi => {
            if let Some(p) = HI_PACK.get() {
                Arc::clone(p)
            } else {
                let p = load(locale)?;
                let _ = HI_PACK.set(Arc::clone(&p));
                p
            }
        }
    };
    if pack.status() == PackStatus::Preview {
        emit_preview_warning_if_first(locale);
    }
    Ok(pack)
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
    break_suggestion_intro_template: String,
    memory_limit_retry: String,
    memory_limit_soft_stop: String,
    child_label: String,
    primer_label: String,
    factual_prefixes: Vec<String>,
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
    PedagogicalIntent::SuggestBreak,
    PedagogicalIntent::ProbeReasoning,
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
        PedagogicalIntent::SuggestBreak => "suggest_break",
        PedagogicalIntent::ProbeReasoning => "probe_reasoning",
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
///
/// `{{` / `}}` are the escape for a literal `{` / `}` (issue #20): a
/// doubled open-brace is skipped here and unescaped at render time by
/// [`render_template`], so narrative like `{{Beispiel}}` is never flagged.
/// A single `{Beispiel}` is still treated as a placeholder attempt and
/// rejected — translators must double-up to get a literal.
fn validate_placeholders(field: &str, content: &str, allowed: &[&str]) -> Result<()> {
    let bytes = content.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            // Escaped literal `{{` — not a placeholder delimiter.
            if bytes.get(i + 1) == Some(&b'{') {
                i += 2;
                continue;
            }
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

/// Render `template`, substituting `{key}` for the matching `vars` value
/// and treating `{{` / `}}` as literal `{` / `}` (the issue-#20
/// brace-escape, familiar from Rust `format!`).
///
/// A single left-to-right pass — substituted values are emitted as-is and
/// never re-scanned, so `{{name}}` always renders as the literal `{name}`
/// even when `name` is a known var. A naive `str::replace` chain cannot do
/// this: `{{name}}` contains the substring `{name}`, which `replace` would
/// wrongly interpolate.
///
/// A `{...}` whose contents match no var is emitted verbatim (e.g.
/// narrative `{Hello, world}` that survives [`validate_placeholders`]).
/// Unterminated `{` and lone `}` are emitted verbatim rather than erroring
/// — validation already runs at load time, so the renderer is lenient.
fn render_template(template: &str, vars: &[(&str, &str)]) -> String {
    let bytes = template.as_bytes();
    let mut out = String::with_capacity(template.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => {
                if bytes.get(i + 1) == Some(&b'{') {
                    out.push('{');
                    i += 2;
                    continue;
                }
                let start = i + 1;
                let mut end = start;
                while end < bytes.len() && bytes[end] != b'}' {
                    end += 1;
                }
                if end >= bytes.len() {
                    // Unterminated `{` — emit the remainder verbatim.
                    out.push_str(&template[i..]);
                    break;
                }
                let key = &template[start..end];
                match vars.iter().find(|(k, _)| *k == key) {
                    Some((_, val)) => out.push_str(val),
                    None => out.push_str(&template[i..=end]),
                }
                i = end + 1;
            }
            b'}' => {
                // `}}` is a literal `}`; a lone `}` is emitted as-is.
                if bytes.get(i + 1) == Some(&b'}') {
                    i += 1;
                }
                out.push('}');
                i += 1;
            }
            _ => {
                // Copy the run of non-brace bytes in one go. Braces are
                // ASCII, so the run boundaries are always char boundaries.
                let run_start = i;
                while i < bytes.len() && bytes[i] != b'{' && bytes[i] != b'}' {
                    i += 1;
                }
                out.push_str(&template[run_start..i]);
            }
        }
    }
    out
}

/// Unescape `{{` / `}}` to literal braces in a field that carries no
/// interpolated placeholders (validated with an empty allowlist). Defined
/// in terms of [`render_template`] with no vars so the escape rule is
/// identical everywhere.
fn unescape_braces(s: &str) -> String {
    render_template(s, &[])
}

#[cfg(test)]
mod tests;
