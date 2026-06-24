# हिन्दी (`hi`) — PREVIEW

> **Preview status.** This locale is in the codebase but excluded from `Locale::ALL`, meaning end users do not encounter it via the CLI/GUI locale picker. The prompt pack is machine-translated and awaiting native-speaker review. See the open work items below before relying on this locale for a real session with a child.

## Identity

| Field | Value |
|---|---|
| `pack_id` (ISO-639-1) | `hi` |
| `Locale::*` variant | `Locale::Hindi` |
| `bcp47` | `hi-IN` |
| Native name | हिन्दी |
| Child-directed register | informal `तुम` (never `आप`, never `तू`) |

## Status

| Layer | State | Notes |
|---|---|---|
| Prompt pack | 🟡 preview | [`prompts/hi.toml`](../../../src/crates/primer-pedagogy/prompts/hi.toml) — machine-translated, awaiting native-speaker review. `[meta] status = "preview"`. |
| `Locale::Hindi` variant + inference-error strings | ✅ | [`primer-core/src/i18n.rs`](../../../src/crates/primer-core/src/i18n.rs). Six error variants translated to Devanagari (Auth, RateLimited, ServiceUnavailable, NetworkUnavailable, ModelNotFound, Other). |
| KB seed corpus | ❌ | No Hindi corpus exists in the codebase. See "Corpus" section below. |
| Retrieval benchmark + sweep tests | ❌ | Pending corpus. |
| Default voice (Piper) | ✅ | `hi_IN-rohan-medium` (only Hindi voice on rhasspy/piper-voices). |
| Default STT (Whisper) | ✅ | `small` (multilingual). |
| Locale::ALL membership | ❌ | Deliberately excluded; flipped together with prompt-pack review. |

## Preview gates

Two firewalls prevent end-user exposure:

1. **`Locale::ALL` exclusion.** CLI and GUI pickers iterate `Locale::ALL`. The Hindi variant is not in that slice. A developer can still pass `--language hi` explicitly.
2. **`[meta] status = "preview"` field.** The prompt-pack loader emits a one-time `tracing::warn!` on first cached load of any Preview pack so logs make the unreviewed status obvious.

Both flip when a native speaker has reviewed the prompt pack: in one PR, edit `[meta] status = "stable"` in `hi.toml`, add `Self::Hindi` to `Locale::ALL`, remove this preview section.

## Pedagogical adaptation notes

The prompt pack follows the same "adapt, don't translate" pattern as the German pack:

### Address — `तुम`, not `आप`

The Primer addresses the child as `तुम` (informal-respectful) throughout. The formal `आप` would be jarringly distant for a learning companion; the intimate `तू` is too casual outside close family and can read as rude in unfamiliar Hindi-speaking regions. `तुम` mirrors the `du` precedent the German pack established.

The system prompt opens with an explicit non-negotiable address block:

```
संबोधन — यह बात नहीं बदलनी चाहिए:
- तुम {name} से हमेशा अनौपचारिक "तुम" से बात करते हो। कभी "आप" नहीं।
```

A native-speaker reviewer may want to revisit this against regional usage (e.g. how it reads in Hyderabadi vs. Delhi vs. Mumbai Hindi).

### Complexity marker — Sanskrit-rooted vocabulary, not syllable count

The English "no more than three syllables" rule is **deleted** for Hindi. Devanagari matra-stacking inflates syllable counts in a way that makes them a useless pedagogical metric: `कण` (1 syllable) is plain-language at 8 years old; `इलेक्ट्रॉन` (4 syllables) is also plain-language in Devanagari but technical-vocabulary in pedagogy.

The Hindi `ages_7_9` band names two markers for technical vocabulary instead:

- **तत्सम (Sanskrit-rooted) terms** that have entered scientific Hindi but not everyday speech (`कण`, `अणु`, `तरंग`, `आवृत्ति`, `विद्युत-धारा`, `इलेक्ट्रॉन`, `कंपन`, `कर्णपटल`).
- **Long compound terms** (often Sanskrit-derived multi-element compounds).

Both require the everyday introduction described in the vocabulary-discipline block.

### Vocabulary examples

The English pack lists `plasma, molecule, conductor, insulator, shockwave, vibration, frequency, voltage, current, atom, particle` as technical-for-children at age 7–9. The Hindi pack lists the तत्सम equivalents: `कण, अणु, तरंग, आवृत्ति, विद्युत-धारा, वोल्टता, इलेक्ट्रॉन, कंपन, सदमे की लहर, वायुमंडल`. These are equivalents, not translations.

### Factual-question prefix matching

Hindi syntax typically places the question word at the end rather than the start, so prefix-matching is weaker than for English or German. The pack ships a starter list (`क्या है `, `क्या हैं `, `कैसे काम `, `कैसे होता `, `कैसे होती `, `कहाँ है `, `कौन है `) but the LLM-engagement-classifier fallback path is the safety net. A native-speaker reviewer should curate this list (or set it to `[]` and rely entirely on the classifier).

## Corpus

**There is currently no Hindi children's wiki of the Klexikon / Simple-English-Wikipedia shape.** Investigation at 2026-05-15:

- **Vikidia** ([vikidia.org](https://en.vikidia.org/wiki/Vikidia:About)) covers 14 languages; Hindi is not among them.
- **"Bal Vikipedia"** is not a real site.
- **`hi.wikipedia.org`** is adult prose — too dense and vocabulary-mismatched for ages 5–14.

Candidate sources that need verification before adoption:

- **NCERT textbooks** ([ncert.nic.in](https://ncert.nic.in/)) — Indian government textbooks. Class 1–10 textbooks are available in Hindi. Licensing terms claim "free to use for educational purposes" but the precise license (CC vs. govt-permissive vs. proprietary-but-free) needs spot-checking before ingest.
- **Pratham Books StoryWeaver** ([storyweaver.org.in](https://storyweaver.org.in/)) — large library of children's stories in many Indian languages, including Hindi. CC-BY licensing on most books but varies per book; ingest pipeline would need per-book license check.
- **Wikisource Hindi** ([hi.wikisource.org](https://hi.wikisource.org/)) — children's literature including Premchand and others; mostly literary, not encyclopedic.

A separate work item should pick a source, add a `WikiSource` preset (or hand-author a seed JSONL like the English path), and ship `seed_passages.hi.jsonl` and/or `wiki_*.hi.jsonl`.

## Voice

- **Recommended TTS — Supertonic 3** (issue #170). The multilingual Supertonic model covers Hindi at Piper-class CPU latency (Stage A.5 spike: RTF ≈ 0.18 for `hi`, model load ≈ 300 ms — see [`docs/devel/supertonic3-stage-a5-spike.md`](../../devel/supertonic3-stage-a5-spike.md)). This is the intended Hindi voice path; Piper's single Hindi voice is the fallback. **Licence note:** the Supertonic *weights* are OpenRAIL-M (the *code* is MIT). A deliberate licence read concluded the weights are usable as a default children's-tutor voice subject to four conditions (no weight redistribution, AI-voice disclosure, attribution + use-restriction pass-through, keep distinct from AGPL). The **licence gate is cleared** — see [`docs/devel/supertonic-openrail-license-assessment.md`](../../devel/supertonic-openrail-license-assessment.md). This is independent of the prompt-pack-review and corpus gates below, which remain open.
- **Fallback TTS — Piper voice:** [`hi_IN-rohan-medium`](https://huggingface.co/rhasspy/piper-voices/tree/main/hi/hi_IN/rohan/medium) — the only Hindi voice on rhasspy/piper-voices at the time of this writing (63 MB, medium tier). Requires the `espeak-ng` system dependency; Supertonic does not.
- **Whisper model:** `small` (multilingual). **Must be set explicitly via `WhisperStt::with_language("hi")`** — same gotcha as the German locale; without the language flag the multilingual model defaults to English and produces approximate-English transcripts of Hindi audio.
- **espeak-ng phoneme coverage:** sufficient for Hindi text-to-speech with Piper; the Hindi phoneme set is supported by the standard espeak-ng install. (Not needed for the Supertonic path.)

## Tested models

(Empty — populate as you smoke-test models against `--language hi`.) See [`docs/locale/models/HINDI.md`](../../locale/models/HINDI.md) for the model-evaluation log.

## Open items before this locale goes stable

- [ ] **Native-speaker prompt-pack review.** Grep `prompts/hi.toml` for `# REVIEW:` to see flagged blocks. Critical: tense register, age-band vocabulary markers, factual-prefix list, voice-state UI copy.
- [ ] **Corpus selection.** NCERT vs. Pratham vs. Wikisource. Confirm licensing per source.
- [ ] **`tests/common/hi.rs`** benchmark queries. Mirror the EN / DE shape with 20+ child-style queries.
- [ ] **Retrieval-quality + sweep tests.** Mirror `retrieval_quality_de.rs` and the hybrid sweep harness shape.
- [ ] **Real-LLM smoke testing** against at least three local Ollama models and Claude. Populate `docs/locale/models/HINDI.md`.
- [ ] **Flip `[meta] status = "stable"` in `hi.toml`** and add `Self::Hindi` to `Locale::ALL` — single commit, ships the locale to end users.

## Open issues for this locale

GitHub issues labelled [`locale:hi`](https://github.com/hherb/primer/issues?q=label%3Alocale%3Ahi).
