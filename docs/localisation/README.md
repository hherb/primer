# Localisation

The Primer is a children's Socratic learning companion. Its humanitarian mission requires it to speak the languages children actually grow up with. This directory is the contributor's home for that work.

If you want to **add a new language**, **fix a translation**, or **evaluate which local LLMs work well for an existing locale**, start here.

## What "localisation" means in the Primer

A locale in the Primer is not a `.po` file. It is a bundle of artefacts that together make the Primer work in a target language:

| Layer | Where it lives | What a contributor does |
|---|---|---|
| **Pedagogical prompt pack** | [`src/crates/primer-pedagogy/prompts/<pack>.toml`](../../src/crates/primer-pedagogy/prompts/) | Rewrite the Socratic system prompt, age-band guidance, intent instructions, engagement notes, and UI state copy in the target language. Not a mechanical translation — the pedagogy must work in the language's natural register. |
| **Locale enum + inference-error strings** | [`src/crates/primer-core/src/i18n.rs`](../../src/crates/primer-core/src/i18n.rs) | Add a `Locale` variant, a `render_<lang>` match arm, and tests. Small but load-bearing Rust change. |
| **Knowledge-base seed corpus** | [`data/seed/`](../../data/seed/) and the ingestion pipeline at [`data/ingest/`](../../data/ingest/) | Curate a children's-wiki source (Klexikon-style) or hand-draft passages, and ship the resulting JSONL. |
| **Retrieval benchmark** | [`src/tests/common/<lang>.rs`](../../src/tests/common/) plus per-locale sweep/quality tests | Author 20+ child-style queries with canonical-id mappings; tune retrieval defaults against that benchmark. |
| **Voice assets mapping** | [`src/crates/primer-speech/src/voice_loop/locale_defaults.rs`](../../src/crates/primer-speech/src/voice_loop/) | Pick the default Piper voice and Whisper model for the locale. |
| **Tested-models notes** | This directory, under your language code | Document which local Ollama models adhere to the language, hit the right reading level, and feel right pedagogically. |

The bar for "the Primer supports language X" is that all six layers exist and pass the validation tests. The prompt pack alone makes the LLM speak the language; the rest is what makes it speak the language *well*.

## Layout

```
docs/localisation/
├── README.md               ← you are here
├── CONTRIBUTING.md         ← step-by-step contributor manual
├── _template/              ← copy this when adding a new locale
│   └── README.md
├── en/                     ← English (the reference)
│   └── README.md
└── de/                     ← German
    └── README.md
```

Each language directory holds at minimum a `README.md` status page. Longer notes (terminology choices, model evaluations, register decisions, dialect handling, …) live as additional pages in the same directory.

## Supported locales today

| Code | Language | Prompt pack | KB seed | Voice (TTS/STT) | Status |
|---|---|---|---|---|---|
| `en` | English | ✅ | ✅ 56 hand-drafted + 35 Simple-English-Wiki | ✅ `en_GB-alba-medium` + Whisper `small.en` | Reference locale — [details](en/README.md) |
| `de` | German | ✅ | ✅ 66 Klexikon articles | ✅ `de_DE-thorsten-medium` + Whisper `small` | Working — [details](de/README.md) |
| `hi` | Hindi (हिन्दी) | 🟡 preview (machine-translated) | ❌ | ✅ `hi_IN-rohan-medium` + Whisper `small` | Preview — excluded from `Locale::ALL` — [details](hi/README.md) |

Add your locale to this table when you open the PR.

## Where to go next

- **You want to add a new language → [CONTRIBUTING.md](CONTRIBUTING.md).**
- **You want to fix or refine an existing translation →** open the relevant pack TOML in [`src/crates/primer-pedagogy/prompts/`](../../src/crates/primer-pedagogy/prompts/) and read the placeholder rules in [CONTRIBUTING.md § Validation rules](CONTRIBUTING.md#validation-rules).
- **You want to evaluate which LLMs work for a locale →** see the language's status page (e.g. [de/README.md](de/README.md)) and add a row to its tested-models table.
- **You want the high-level rationale →** [`docs/background_research/i18n_design.md`](../background_research/i18n_design.md) is the long-form design doc.

## Principles

These are non-negotiable for any locale contribution:

1. **Pedagogy travels; English doesn't.** The Socratic method, the "ask more than tell" discipline, the "never give a direct answer when you can ask a guiding question" rule — these are language-independent. The specific examples, vocabulary lists, syllable rules, and politeness register are not. Adapt, don't translate.
2. **No condescension, no engagement-maximising.** Every locale must preserve the Primer's refusal to keep children hooked. If the target language has a strong child-directed register that softens this, keep the firmness anyway.
3. **Child-appropriate vocabulary is locale-specific.** German children handle compound words from age 4; English children find three-syllable words hard at age 6; Japanese children grasp ideograms in patterns English children never encounter. Each pack's age-band guidance is written from scratch — never translated.
4. **Honour the language's child-directed register.** German children are universally addressed with `du`, never `Sie`. Spanish children with `tú`, French with `tu`, Japanese with plain-form or polite-form depending on family norm — pick the register a teacher would use with that age group and use it consistently.
5. **All learner data stays local; the cloud sees only turns the user opted into.** Translation work changes nothing about this — but mention it if your locale needs region-specific privacy guidance.
