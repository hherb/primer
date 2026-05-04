# Primer Internationalisation Design

*Technical design document. May 2026.*
*Priority: Do this before further implementation work.*

---

ADDENDUM BY HORST 2026-05-04: consider dual voice models for voice context switching between languages, instead of forcing the same model to do both

## The Problem

The Primer is currently English-only. Every system prompt, every child-facing message, every age-band language guidance, and several pieces of classification logic contain hardcoded English. The codebase has approximately 70+ English strings/blocks and 3 areas of English-specific logic.

This is not a cosmetic issue. It is a structural one that gets harder to fix with every line of code written. The prompt builder's 100+ lines of Socratic philosophy, the age-band vocabulary guidance, the factual-question detector — all of these are load-bearing, and retrofitting i18n onto them after further development will mean rewriting code that was already tested and working.

The Primer's humanitarian mission requires multilingual support. Most of the world's children do not speak English. The regions where the Primer could have the greatest impact — sub-Saharan Africa, South Asia, Southeast Asia, Latin America — are also the most linguistically diverse. A Primer that only works in English is a Primer for the privileged.

Additionally, a multilingual Primer opens a natural pathway to language teaching (see the Polyglott project, temporarily shelved while awaiting better multilingual local models). The architectural decisions made now will determine whether that integration is straightforward or requires another rewrite.

---

## What Needs to Change

The audit identified three categories of work:

### Category 1: Externalisable Strings (~50 items)

Strings that can be extracted from code and placed in language-specific resource files. These include:

- **Child-facing messages** (greeting, session close, break suggestion, error recovery) — ~10 strings. These need warm, age-appropriate translation, not mechanical conversion.
- **Parent-facing messages** (first-run banner, persistence notice, CLI help) — ~5 strings. Informational, standard i18n.
- **Developer-facing messages** (error output, diagnostic labels) — ~20 strings. Low priority but should be externalised for consistency.
- **Stub backend responses** — ~2 strings. Test/fallback, but should be translatable.

### Category 2: Pedagogically Load-Bearing Prompts (~15 blocks)

System prompts sent to the LLM that encode the Primer's entire teaching philosophy. These cannot be mechanically translated — they must be *rewritten* for each language by someone who understands both the pedagogy and the target language's educational norms. They include:

- **Core system prompt** (100+ lines): The Socratic principles, the tone, the vocabulary discipline, the "never give a direct answer" instruction.
- **Pedagogical intent instructions** (8 branches): One per intent (SocraticQuestion, Scaffolding, Encouragement, etc.).
- **Engagement-state notes** (2-3 conditional blocks): Frustration handling, disengagement response.
- **Knowledge context framing** (3 blocks): How the LLM should interpret RAG passages, session summaries, and retrieved prior turns.
- **Age-band language guidance** (4 blocks): What vocabulary and sentence complexity is appropriate for each age group — this is the most language-dependent component.
- **Vocabulary discipline block**: How to introduce technical terms — contains English-specific examples and pedagogy.

### Category 3: Language-Dependent Logic (3 areas)

Code that makes assumptions about English structure:

1. **`is_factual_question()`** — Matches against English question prefixes ("what is", "how does", etc.). Other languages form questions differently (particles, inversion, tone, morphology).

2. **Age-band language guidance** — Rules like "never use a word with more than three syllables" are meaningless in German (where compound words are routine), Japanese (where syllable structure differs fundamentally), or Mandarin (where character count is the relevant metric).

3. **Vocabulary examples** — Lists of "technical" words ("plasma", "molecule", "conductor") are English-specific. Each language has its own set of words that are technical-for-children.

---

## Proposed Architecture

### Principle: TOML Prompt Packs, Not Code

System prompts and child-facing strings should live in TOML files, not in Rust source. This achieves several things:

- Translators can work on TOML files without touching Rust code
- Prompt iteration doesn't require recompilation
- Multiple languages can be tested side-by-side without code changes
- The prompts become a data product that can be independently versioned, reviewed, and contributed to

### Directory Structure

```
src/
  crates/
    primer-core/
      src/
        locale.rs          # Locale type, PromptPack trait
    primer-pedagogy/
      src/
        prompt_builder.rs  # Reads from PromptPack, no hardcoded strings
      prompts/
        en.toml            # English prompt pack (the reference)
        de.toml            # German
        es.toml            # Spanish
        ja.toml            # Japanese
        zh.toml            # Mandarin
        template.toml      # Annotated template for new translations
    primer-cli/
      messages/
        en.toml            # CLI messages (errors, banners, help text)
        de.toml
        ...
```

### The PromptPack Format

Each language gets a single TOML file containing all pedagogically load-bearing text. The file is structured to mirror the prompt builder's logic:

```toml
# prompts/en.toml — English Prompt Pack
# 
# This file contains all text the Primer sends to the LLM or displays
# to the child. It is the ONLY place language-specific content lives.
# The Rust code reads from this file; it contains no English strings.
#
# TRANSLATORS: Do not translate mechanically. These prompts encode
# pedagogical intent. Read the annotation for each section, understand
# what it is trying to achieve, and write the equivalent in your
# language's natural pedagogical register. If a concept doesn't
# translate directly, adapt it — the pedagogy matters more than
# word-for-word fidelity.

[meta]
language = "en"
language_name = "English"
# BCP 47 tag for speech pipeline integration
bcp47 = "en-US"
# Direction: "ltr" or "rtl" — for future display rendering
direction = "ltr"
# Who wrote/reviewed this translation and when
authors = ["Daniel Osei (fictional)"]
reviewed_by = []
last_updated = "2026-05-04"

# --- SYSTEM PROMPT COMPONENTS ---

[system_prompt]
# The base identity and principles. {name} and {age} are interpolated.
# This is the most important block — it defines who the Primer is.
base = """
You are the Primer — a patient, curious, Socratic learning companion \
for a child named {name}, age {age}.

Your core principles:
- NEVER give a direct answer when you can ask a guiding question instead.
- Ask questions that lead {name} toward discovering the answer themselves.
...
"""

# --- PEDAGOGICAL INTENT INSTRUCTIONS ---
# One entry per PedagogicalIntent variant. The key must match the
# Rust enum variant name (snake_case).

[intent]
socratic_question = """\
Your next response should be a guiding question that leads \
toward understanding."""

comprehension_check = """\
Your next response should probe whether the child truly understands \
or is repeating what they've heard. Ask them to explain it differently, \
apply it to a new situation, or find a flaw in a deliberately wrong \
statement."""

scaffolding = """\
The child is struggling. Your next response should offer a concrete \
example, a story, or an analogy that makes the concept tangible. \
Reduce the abstraction level."""

encouragement = """\
The child is frustrated. Your next response should be encouraging \
without being patronising. Acknowledge the difficulty. Normalise \
confusion. Suggest a different angle of approach."""

extension = """\
The child has demonstrated understanding. Your next response should \
extend the concept — introduce a complication, a counterexample, \
or a connection to a different domain."""

direct_answer = """\
This is a factual question. Answer it directly and clearly, then \
follow with a Socratic question that builds on the answer."""

answer_then_pivot = """\
Provide the factual answer briefly, then pivot to a question that \
makes the child think about *why* the fact matters or what would \
change if it were different."""

session_close = """\
Suggest that this is a good stopping point. Summarise what was \
explored today (not what was 'learned' — what was *explored*). \
Leave the child with one question to think about until next time."""

# --- ENGAGEMENT STATE NOTES ---

[engagement]
frustrated = """\
IMPORTANT: The child appears frustrated. Be especially gentle. \
Offer to approach the topic differently or switch topics entirely."""

disengaging = """\
NOTE: The child may be losing interest. Consider suggesting a break \
or pivoting to a topic they find more engaging."""

# --- AGE-BAND LANGUAGE GUIDANCE ---
# These are the most language-dependent sections. Each language
# needs its OWN complexity metrics — do not translate the English
# syllable rules into other languages. Rewrite from scratch based
# on what constitutes age-appropriate language in YOUR language.

[language_guidance]
# Annotation: For the youngest children. In English, this means
# short words, short sentences, concrete anchoring. In German,
# compound words are natural even for young children — adjust
# accordingly. In Japanese, use hiragana-heavy text and avoid
# kanji beyond the child's grade level.
ages_0_6 = """
- Use only words a young child uses at home or kindergarten.
- Sentences are short — aim for 6 to 10 words.
- Never use a word with more than three syllables unless you have \
just defined it through a concrete everyday example, and the child \
has shown they grasped the example.
- Anchor every idea to something the child can see, touch, hear, \
or do: food, toys, pets, body, weather, family.
- Avoid abstract nouns unless you have grounded them in a physical \
thing first.
"""

ages_7_9 = """..."""
ages_10_12 = """..."""
ages_13_plus = """..."""

# --- VOCABULARY DISCIPLINE ---
# The principle is universal (introduce technical words through
# plain-language analogy first). The EXAMPLES are language-specific.
# Replace the example words and the example sentence with equivalents
# natural in your language.

[vocabulary]
discipline = """
- Before using any technical or unusual word (examples at this age: \
{example_technical_words}), first explain the idea in plain everyday \
words using a concrete analogy {name} already knows (food, toys, \
animals, weather, family, body). Only use the technical word once \
the plain-language idea is clear.
...
"""
# Comma-separated list of words considered "technical" for young
# children in this language. Used in the vocabulary discipline
# prompt and potentially in vocabulary tracking.
example_technical_words = "plasma, molecule, conductor, insulator, vibration, frequency"

# Example sentence showing the "weave-in" technique in this language.
# English: "the air pushes the charges away — it repels them"
# German: "die Luft drückt die Ladungen weg — sie stößt sie ab"
weave_in_example = "the air pushes the charges away — it repels them"

# --- KNOWLEDGE CONTEXT FRAMING ---

[context]
rag_intro = """\
Relevant factual context (use to ground your responses, but do not \
quote directly — rephrase for a {age}-year-old):

{passages}"""

summary_intro = """\
Earlier in this conversation (long-term memory across many turns):

{summary}"""

retrieved_turns_intro = """\
Relevant prior moments from this same session (retrieved by topic, \
not in time order; use as background, not as the active conversation):

{lines}"""

# --- SPEAKER LABELS ---
# Used in prompt construction for turn history

[labels]
child = "Child"
primer = "Primer"

# --- CHILD-FACING MESSAGES ---

[messages]
greeting = "Hello, {name}. What are you curious about today?"
session_close = "That was a good conversation. Until next time."
break_suggestion = "We've been talking for a while. Want to take a break?"
no_response = "(no response generated)"
error = "Something went wrong. Let's try that again."
```

### The Locale Type

Add to `primer-core`:

```rust
/// Language/locale configuration for a Primer session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Locale {
    /// BCP 47 language tag (e.g., "en-US", "de-DE", "ja-JP")
    pub language_tag: String,
    /// Short code for prompt pack lookup (e.g., "en", "de", "ja")
    pub prompt_pack: String,
    /// Text direction
    pub direction: TextDirection,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum TextDirection {
    Ltr,
    Rtl,
}
```

Add `locale: Locale` to `LearnerProfile`. This persists with the child — a returning child doesn't need to re-specify their language. It also enables future features: a bilingual child could have two profiles, one per language.

### The PromptPack Trait

The prompt builder currently constructs prompts by concatenating hardcoded strings. Replace this with a trait:

```rust
pub trait PromptPack: Send + Sync {
    fn system_prompt_base(&self, name: &str, age: u8) -> String;
    fn intent_instruction(&self, intent: PedagogicalIntent) -> String;
    fn engagement_note(&self, state: &EngagementState) -> String;
    fn language_guidance(&self, age: u8) -> String;
    fn vocabulary_discipline(&self, name: &str, age: u8) -> String;
    fn rag_context_frame(&self, passages: &str, age: u8) -> String;
    fn summary_frame(&self, summary: &str) -> String;
    fn retrieved_turns_frame(&self, lines: &str) -> String;
    fn speaker_label_child(&self) -> &str;
    fn speaker_label_primer(&self) -> &str;
    fn greeting(&self, name: &str) -> String;
    fn session_close_message(&self) -> &str;
    fn break_suggestion(&self) -> &str;
}
```

A `TomlPromptPack` struct implements this trait by loading and interpolating from a TOML file. The prompt builder receives a `&dyn PromptPack` and uses it instead of hardcoded strings.

### Refactoring Language-Dependent Logic

**`is_factual_question()`** — Move this into the prompt pack or a language-specific module:

```toml
# In the TOML prompt pack:
[question_detection]
# Prefixes that indicate a factual question in this language.
# Used by decide_intent() to route to DirectAnswer.
# These are lowercased and matched against the start of the child's input.
factual_prefixes = [
    "what is ",
    "what are ",
    "what's ",
    "what does ",
    "how does ",
    "how do ",
    "how is ",
    "how are ",
]
```

For languages where prefix-matching doesn't work (Japanese, where question particles appear at the end; Mandarin, where tone and context determine question type), the prompt pack can set `factual_prefixes = []` and instead rely on the LLM-based classifier. The `decide_intent()` function should check: if factual_prefixes is non-empty, use prefix matching; if empty, delegate to the comprehension/classifier pipeline.

**Age-band language guidance** — Already handled: each TOML file contains its own `[language_guidance]` section, written from scratch for that language. The English "three syllable" rule stays in `en.toml`; the German equivalent (whatever it is — perhaps based on compound-word depth or Flesch-Reading-Ease adapted for German) goes in `de.toml`.

**Vocabulary examples** — Already handled: the `example_technical_words` field in each TOML file contains language-appropriate technical vocabulary.

---

## LLM System Prompt Language

A critical design question: **in what language should the system prompt be written?**

Option A: System prompt always in English, with an instruction to respond in the target language. ("You are the Primer. Respond in German. The child speaks German.")

Option B: System prompt in the target language. The entire TOML file is in the target language.

Option C: Hybrid — structural/meta instructions in English (the LLM understands these best), pedagogical instructions and child-facing templates in the target language.

**Recommendation: Option C for now, with a path to Option B.**

Current LLMs (Claude, Llama, Qwen, Gemma) understand English system prompts best. Writing the meta-instructions in English ("Your next response should be a guiding question") and the child-facing examples in the target language gives the best of both worlds. As multilingual model quality improves — especially for local models — the system prompt can transition fully to the target language.

In the TOML format, this looks like:

```toml
[intent]
# The instruction is in English (for the LLM).
# The example is in the target language (for tone calibration).
socratic_question = """\
Your next response should be a guiding question that leads toward \
understanding. Respond in German. Example tone: \
'Das ist eine gute Beobachtung. Aber was würde passieren, wenn...?'"""
```

This is pragmatic and can be revised per-language as models improve.

---

## Speech Pipeline Integration

The locale must propagate through the full stack:

```
CLI --language de
  → LearnerProfile.locale = Locale { language_tag: "de-DE", ... }
    → PromptPack loaded from prompts/de.toml
    → WhisperSTT configured for German
    → PiperTTS configured for German voice
    → Knowledge base queries in German (future)
```

The `--language` CLI flag selects everything. No per-component language configuration.

For the speech pipeline specifically:

- **Whisper** is multilingual out of the box — it detects language automatically or can be constrained to a specific language. Pass the BCP 47 tag.
- **Piper** requires a language-specific voice model. Each supported language needs at least one Piper voice. Voice models are separate downloads. The TOML prompt pack should reference the default voice for its language.
- **Silero VAD** is language-agnostic (it detects voice activity, not language). No changes needed.

---

## LearnerProfile Changes

```rust
pub struct LearnerProfile {
    pub id: LearnerId,
    pub name: String,
    pub age: u8,
    pub locale: Locale,          // NEW
    pub concepts: Vec<ConceptState>,
    pub preferences: LearningPreferences,
    pub engagement_snapshot: Option<EngagementState>,
}
```

The locale is stored with the child. A child who speaks German always gets German, without needing to specify it each session. This also enables:

- Bilingual children with separate profiles per language
- Future: mixed-language sessions (the Polyglott use case)
- Future: tracking vocabulary acquisition per language

Schema migration: add a `locale` column to the `learners` table (default `"en"` for existing profiles). This is a simple v5 migration.

---

## What to Do Now (Before Further Implementation)

The goal is not to translate everything immediately. The goal is to **establish the architecture** so that all future code is written against the `PromptPack` trait rather than hardcoded strings. Then translations can be added incrementally.

### Step 1: Create the PromptPack trait and TomlPromptPack implementation

Extract the current English strings from `prompt_builder.rs` into `prompts/en.toml`. Implement `TomlPromptPack` that loads and interpolates from TOML. Modify `prompt_builder.rs` to use `&dyn PromptPack` instead of hardcoded strings.

**Test:** All existing tests pass with the TOML-loaded English pack producing identical output to the current hardcoded strings.

### Step 2: Add Locale to LearnerProfile and CLI

Add `--language` flag (default "en"). Add `locale` to `LearnerProfile`. Schema v5 migration. Wire locale through to prompt builder.

### Step 3: Extract child-facing messages

Move greeting, session close, break suggestion, and error messages to a separate messages TOML file per language. Wire through CLI.

### Step 4: Refactor `is_factual_question()`

Move prefix list to the TOML prompt pack. Add fallback-to-classifier path for languages where prefix matching doesn't work.

### Step 5: Create `template.toml`

An annotated template that a translator can copy to create a new language pack. Each section has comments explaining the pedagogical intent, what matters about the phrasing, and what can be freely adapted.

### Step 6: Create `de.toml` (German)

First translation. Horst can do this — native speaker, understands the pedagogy, can validate that the German Primer feels right. German is a good first test because it differs enough from English (compound words, V2 word order, gendered nouns, different question formation) to expose any remaining English assumptions in the code.

---

## What NOT to Do

- **Do not use a generic i18n framework (like `fluent` or `gettext`).** These are designed for UI strings, not for LLM prompts. The Primer's prompts are not UI — they are pedagogically load-bearing text that needs to be edited as prose, not as key-value pairs. TOML files with full prose blocks are the right format.

- **Do not attempt machine translation of prompts.** The system prompts encode teaching philosophy. "Be warm. Be patient. Never condescend." cannot be run through Google Translate and expected to produce the same pedagogical effect. Each language needs a native speaker who understands the Primer's approach to write (not translate) the prompts.

- **Do not add languages without a native-speaking educator reviewing them.** A badly translated prompt pack is worse than no translation — it produces a Primer that speaks the child's language but teaches badly. Quality control per language is essential.

- **Do not block on this.** The architecture change (Steps 1-4) is a few days of work. Creating `de.toml` is a weekend. Further languages can be added by contributors at any time after the architecture is in place. The point is to stop building on the English-only foundation now, not to achieve full multilingual support before proceeding.

---

## Relationship to Polyglott

The Polyglott project (multi-language tutor, temporarily shelved) requires exactly this architecture. Once the Primer has `PromptPack` support and locale-aware speech pipeline integration, the Polyglott use case becomes a specialised prompt pack that manages two languages simultaneously — the child's native language and the language being learned.

The key architectural requirement for Polyglott: the ability to switch the prompt pack's response language mid-session while keeping the system prompt's meta-instructions stable. This is naturally supported by the hybrid approach (English meta-instructions, target-language content) and can be extended later to full target-language system prompts as model quality improves.

The structural difficulties Horst encountered with Polyglott — context-switching reliability in multilingual conversation — are partly a model quality issue (local models struggle with consistent code-switching) and partly an architecture issue (the system prompt must clearly delineate which language to use when). The TOML prompt pack approach gives explicit control over these instructions per language, which is the architectural prerequisite for reliable multilingual behaviour regardless of model quality.

---

## Effort Estimate

| Step | Scope | Effort | Dependency |
|------|-------|--------|------------|
| 1. PromptPack trait + en.toml | Architecture | 3–5 days | None |
| 2. Locale in LearnerProfile + CLI | Plumbing | 1–2 days | Step 1 |
| 3. Extract child-facing messages | Cleanup | 1 day | Step 1 |
| 4. Refactor is_factual_question() | Logic | 1 day | Step 1 |
| 5. Create template.toml | Documentation | Half day | Step 1 |
| 6. Create de.toml (German) | Translation | 2–3 days | Steps 1–5 |
| Per additional language | Translation | 2–3 days each | Steps 1–5 |

**Total for architecture + German:** ~10 days of focused work.

This should be done before further implementation of the pedagogical engine, the comprehension classifier, or the vocabulary tracking system — all of which will generate new strings that will need to be externalised.
