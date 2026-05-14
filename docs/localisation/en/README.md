# English (`en`)

The reference locale. Everything else is measured against this.

## Identity

| Field | Value |
|---|---|
| `pack_id` (ISO-639-1) | `en` |
| `Locale::*` variant | `Locale::English` (default) |
| `bcp47` | `en-US` |
| Native name | English |
| Child-directed register | informal `you` |

## Status

| Layer | State | Notes |
|---|---|---|
| Prompt pack | ✅ | [`prompts/en.toml`](../../../src/crates/primer-pedagogy/prompts/en.toml) |
| `Locale::English` variant + inference-error strings | ✅ | [`primer-core/src/i18n.rs`](../../../src/crates/primer-core/src/i18n.rs) |
| KB seed corpus | ✅ | 56 CC0 hand-drafted + 35 Simple-English-Wikipedia (CC-BY-SA-3.0) = **91 passages** |
| Retrieval benchmark + sweep tests | ✅ | 91 queries / 24 strict canonical-id mappings; 100% loose / 100% strict on hybrid |
| Default voice (Piper) | ✅ | `en_GB-alba-medium` |
| Default STT (Whisper) | ✅ | `small.en` |

## Pedagogical adaptation notes

This pack is the reference. Adaptation notes for other locales should describe what they diverged from in this pack.

Key load-bearing pieces other locales must adapt rather than translate:

- **The syllable rule.** `ages_0_6` says "never use a word with more than three syllables unless you have just defined it through a concrete everyday example." This is meaningful for English (where syllable count tracks vocabulary difficulty reasonably well) but useless for languages where compounds are routine (German), tone-bearing morphemes are atomic (Mandarin), or kanji-grade is the relevant metric (Japanese).
- **The technical-vocabulary examples.** `plasma`, `molecule`, `conductor`, `insulator`, `shockwave`, `vibration`, `frequency`, `voltage`, `current`, `atom`, `particle` — these are the words English-speaking children at the `ages_7_9` band actually find hard. Other locales should list their language's analogues, not these.
- **The vocabulary-discipline block.** The "if the child asks 'what does X mean?' that's a signal X was introduced too soon" pedagogy is universal. The example word (`repel`) is English-specific; substitute one that has analogous Latin/Greek-root opacity in your language.
- **The factual-question prefix list.** English uses "what is", "how does", "what's", etc. with a trailing space. Other languages need to consider whether prefix-matching even makes sense in their morphology — see [CONTRIBUTING.md § Factual-question detection](../CONTRIBUTING.md#question_detection--factual-question-prefix-list).

## Knowledge-base corpus

- **Sources:**
  - Hand-drafted CC0 seed at [`data/seed/seed_passages.en.jsonl`](../../../data/seed/seed_passages.en.jsonl) — 56 passages across all five planned clusters (space, body, how-things-work, life, earth & weather).
  - Simple English Wikipedia layer at [`data/seed/wiki_passages.en.jsonl`](../../../data/seed/wiki_passages.en.jsonl) — 35 articles (physics fundamentals, chemistry, biology, earth science, health).
- **Licenses:** CC0 (seed) + CC-BY-SA-3.0 (wiki layer).
- **Total passage count:** 91.
- **Ingestion path:** Wiki layer regenerated via [`data/ingest/simple_wikipedia.py`](../../../data/ingest/simple_wikipedia.py) (`--language en`); whitelist at [`data/ingest/simple_wikipedia_whitelist.txt`](../../../data/ingest/simple_wikipedia_whitelist.txt).

## Retrieval benchmark

- **Queries:** 91 child-style English queries
- **Strict canonical-id mappings:** 24
- **Clusters covered:** all five (space, body, how-things-work, life, earth & weather), plus a `Wiki` cluster for the Simple-English-Wikipedia layer
- **Known failing queries (BM25-only):** `KNOWN_FAILING_QUERIES` carries the 2 paraphrases the BM25 leg can't bridge ("what is inside a tiny bug" → insects, "why does the brain need oxygen from the lungs" → brain) — both pass under hybrid.
- **Known failing queries (hybrid):** **empty.** After the issue #45 corpus expansion (`seed:en:flowers` added; stomach-growl sentence added to `seed:en:digestion`), hybrid hits **100% loose / 100% strict on all 91 queries / 24 strict-subset canonical mappings**.
- **Production defaults:**
  - BM25-only: `KB_FINAL_TOP_K = 5`, `KB_BM25_ONLY_MIN_SCORE = 0.5`
  - Hybrid: `KB_BM25_TOP_K = 30`, `KB_VECTOR_TOP_K = 30`, `KB_FINAL_TOP_K = 5`, `RRF_K = 60`
- **Sweep tests:** [`tests/retrieval_sweep.rs`](../../../src/tests/retrieval_sweep.rs) (24-cell BM25) and [`tests/retrieval_sweep_hybrid.rs`](../../../src/tests/retrieval_sweep_hybrid.rs) (54-cell hybrid, under `--features fastembed`).

## Voice

- **Piper voice:** `en_GB-alba-medium` — child-friendly British female voice, clear and gentle. Good prosody for the Primer's patient delivery.
- **Whisper model:** `small.en` — English-only model is faster and more accurate for English than the multilingual `small`.
- **espeak-ng phoneme coverage:** complete for British and American English.
- **Known voice issues:** none currently tracked.

## Tested models

The English locale is heavily exercised by cloud Claude models in development (Sonnet 4.6, Opus 4.7). For local Ollama models the team has not maintained a formal evaluation table — English speakers running locally have many model choices and the locale is forgiving. If you want to contribute one, follow the format in [`de/README.md § Tested models`](../de/README.md#tested-models).

## Open issues for this locale

GitHub issues tagged [`locale:en`](https://github.com/hherb/primer/issues?q=label%3Alocale%3Aen).
