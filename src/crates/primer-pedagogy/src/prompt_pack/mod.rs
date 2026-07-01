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
//!
//! # Module layout
//!
//! - [`render`] — pure placeholder validation + brace-aware templating.
//! - [`intents`] — `PedagogicalIntent` ↔ TOML key mapping.
//! - [`toml_pack`] — the `TomlPromptPack` impl, raw deserialisation
//!   types, and load-time validation.
//! - [`loader`] — the [`load`] / [`load_cached`] entry points and the
//!   preview-status warning gate.
//!
//! The public surface (`PromptPack`, `PackStatus`, `VoiceStateLabels`,
//! `TomlPromptPack`, `load`, `load_cached`) is re-exported here so
//! external callers keep using `prompt_pack::<name>` paths unchanged.

use primer_core::conversation::PedagogicalIntent;
use primer_core::error::{PrimerError, Result};
use primer_core::i18n::Locale;
use primer_core::learner::EngagementState;

mod intents;
mod loader;
mod render;
mod toml_pack;

pub use loader::{load, load_cached};
pub use toml_pack::TomlPromptPack;

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
    pub(crate) fn from_meta(raw: Option<&str>) -> Result<Self> {
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
    /// Lowercased openers that mark a child turn as an epistemic hedge /
    /// non-answer ("I don't know", "I'm not sure"). Such a turn routes to
    /// `ComprehensionCheck` rather than `ProbeReasoning` — a child
    /// signalling confusion needs scaffolding, not a "how do you know?"
    /// probe. Empty disables the check for that locale (the turn falls
    /// through to the normal claim/Socratic routing).
    fn confusion_openers(&self) -> &[String];
    /// Lowercased openers that mark a child turn as a request or meta-talk
    /// directed at the Primer ("I want", "tell me", "let's"). Such a turn
    /// is not a probe-able claim, so it stays on the `SocraticQuestion`
    /// default instead of routing to `ProbeReasoning`. Empty disables the
    /// check for that locale.
    fn request_openers(&self) -> &[String];
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

#[cfg(test)]
mod tests;
