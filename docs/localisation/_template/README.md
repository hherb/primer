# <Language name in English> (`<pack_id>`)

> **Template.** Copy this file to `docs/localisation/<pack_id>/README.md` and replace every `<…>` placeholder. Delete sections that don't apply yet — they can be added in follow-up PRs.

## Identity

| Field | Value |
|---|---|
| `pack_id` (ISO-639-1) | `<xx>` |
| `Locale::*` variant | `Locale::<LanguageName>` |
| `bcp47` | `<xx-YY>` |
| Native name | <Español / Deutsch / 日本語 / …> |
| Child-directed register | <`tú` / `du` / plain-form / …> |

## Status

| Layer | State | Notes |
|---|---|---|
| Prompt pack | ⬜ / ✅ | <link to PR or commit> |
| `Locale` variant + inference-error strings | ⬜ / ✅ | |
| KB seed corpus | ⬜ / ✅ | <N passages from <source>, license <…>> |
| Retrieval benchmark + sweep tests | ⬜ / ✅ | <N queries / N strict mappings> |
| Default voice (Piper) | ⬜ / ✅ | <model id> |
| Default STT (Whisper) | ⬜ / ✅ | <`small.en` / `small` / `medium`> |

## Pedagogical adaptation notes

What changed from the English reference pack, and why. Examples to cover:

- **Register choice.** Why `<chosen pronoun/form>`, not the alternative.
- **Age-band complexity metric.** What replaces the English syllable rule. How "technical vocabulary" is identified in your language.
- **Vocabulary examples used in the system prompt.** The English pack lists `plasma`, `molecule`, `conductor`, etc. as technical-for-children. Yours should list words that actually pose vocabulary difficulty for `<age band>` children in your language.
- **Idiom / cultural anchors.** The English pack anchors abstract ideas in "food, toys, pets, body, weather, family". Adapt to whatever your culture's child-everyday anchor set is.
- **Politeness / softening particles.** Languages with mandatory softeners (Japanese sentence-final particles, etc.) — note where they fit and where they don't.

## Knowledge-base corpus

- **Source(s):** <e.g. Klexikon, Vikidia, hand-drafted CC0>
- **License(s):** <e.g. CC-BY-SA-4.0>
- **Passage count:** <N>
- **Ingestion path:** <ingest pipeline / hand-authored>
- **Update cadence:** <one-shot / regenerable from upstream>

If the corpus is auto-generated from an upstream source, list the whitelist file and the command to regenerate it.

## Retrieval benchmark

- **Queries:** <N child-style queries>
- **Strict canonical-id mappings:** <N>
- **Clusters covered:** <space / body / how-things-work / life / earth & weather / …>
- **Known failing queries (BM25-only):** <listed in `KNOWN_FAILING_QUERIES_<LANG>` with rationale>
- **Known failing queries (hybrid):** <listed in `KNOWN_FAILING_QUERIES_<LANG>_HYBRID` with rationale>
- **Production defaults:** <`top_k=5, min_score=0.5` or whatever sweep recommended>

## Voice

- **Piper voice:** <model id> — picked because <reason: child-friendly, clear, gentle, region-matches…>
- **Whisper model:** <small / small.en / medium> — picked because <reason>
- **espeak-ng phoneme coverage:** <complete / partial / known gaps>
- **Known voice issues:** <e.g. specific phoneme that's mispronounced; words that need manual fixup>

## Tested models

Hands-on observations from running this locale against local Ollama models. Each entry is a snapshot — re-test after model updates.

### Criteria

- **Language adherence** — does the model stay in <language>, or drift back to English?
- **Age appropriateness** — is the vocabulary suited to a child (~7-12), or adult prose?
- **Socratic discipline** — does it ask more than it tells, or fall into lecture mode?
- **Register** — does it use the right child-directed form (<chosen register>)?
- **Latency** — perceived response time on the test machine.

### Models

| Model | Language adherence | Age appropriateness | Register | Latency | Verdict |
|---|---|---|---|---|---|
| `<model-tag>` | <observation> | <observation> | <observation> | <observation> | <recommendation> |

### How to add an entry

```bash
~/.cargo/bin/cargo run --bin primer -- \
  --backend ollama --model <model-tag> \
  --language <pack_id> --name <child-name> --age <age>
```

Try a mix of: a curiosity opener, a frustration cue, a factual question, and a comprehension-check follow-up. Append a row to the table above (or a section below for longer notes).

## Open issues for this locale

<Link to GitHub issues tagged `locale:<pack_id>`>
