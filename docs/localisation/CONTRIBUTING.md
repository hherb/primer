# Contributor Manual — Adding or Refining a Locale

This is the step-by-step manual. For the high-level overview see [README.md](README.md); for the long-form rationale see [`docs/background_research/i18n_design.md`](../background_research/i18n_design.md).

The Primer's locale system is intentionally strict: a broken or inconsistent pack fails loudly at startup rather than producing malformed prompts at runtime. The validator panics on placeholder typos, on `[meta]` drift, on empty `[voice_state]` fields, and on missing intent variants. The rules below are the friendly form of those panics — get them right the first time.

---

## Who can contribute

You should be:

- **Fluent in the target language**, ideally a native speaker — and comfortable thinking about how a 7-to-12-year-old child in that language register actually talks. The Primer is for children; adult prose is a regression.
- **Familiar with the Socratic method** at the level the Primer encodes (read the reference English pack at [`src/crates/primer-pedagogy/prompts/en.toml`](../../src/crates/primer-pedagogy/prompts/en.toml) end-to-end — every section is annotated).
- **Comfortable editing TOML files and reading Rust enough to add an enum variant.** The Rust change is mechanical; the TOML is the load-bearing creative work.

You do **not** need to be a Rust developer beyond that. You do **not** need ML/inference experience to contribute a prompt pack — but evaluating local models for your locale (see § Test against local models below) does help users.

---

## Prerequisites

```bash
# Toolchain (the workspace pins Rust 1.88; rustup proxies honour this — Homebrew rust does not)
~/.cargo/bin/cargo --version       # 1.88+

# Optional: uv for the Python ingestion pipeline (only if you're adding a KB seed)
uv --version
```

All `cargo` commands run from `src/` — the workspace root is `src/Cargo.toml`, not the repo root.

---

## The contribution path at a glance

```
1. Pick your locale identifier      (e.g. "es" + Locale::Spanish + "es-ES")
2. Author the prompt pack TOML       (the load-bearing creative work)
3. Add the Locale enum variant       (~20 lines of Rust)
4. Add inference-error translations  (one match arm in render_inference_error)
5. Add the prompt-pack dispatch      (one match arm + include_str! in prompt_pack.rs)
6. (Optional) Add KB seed passages   (Wikipedia-shaped ingest or hand-drafted JSONL)
7. (Optional) Add retrieval benchmark + sweep tests
8. (Optional) Map default voice + STT assets
9. Document your locale here         (docs/localisation/<code>/README.md)
10. Run the test suite               (cargo test --workspace)
```

Steps 1-5 are the minimum to claim "language X works in the Primer." Steps 6-8 are what make it work well — they're separable PRs but the locale is incomplete without them.

---

## 1. Pick your locale identifier

Three identifiers travel together:

| Identifier | Example | What it is |
|---|---|---|
| `pack_id` | `"es"` | ISO-639-1 two-letter code. Used as the prompt-pack filename (`es.toml`), as the SQLite `learners.locale` value, as the Whisper transcription-language tag, and in `concepts.concept_language_tag`. **Stable forever — never renamed, never reused for a different language.** |
| `name()` | `"Spanish"` | English name of the language. Used as the internal identity check. **Always the English name, never the native form (`"Español"` would break the meta validator).** |
| `bcp47` | `"es-ES"` | BCP 47 tag with region. Used by Whisper, Piper, and future TTS backends for accent selection. Pick the region most likely to match the children's-wiki seed source you'll ship (`es-ES` if Vikidia-style Spanish; `es-MX` if a Mexican source). |

Region choice matters for voice — most Piper voices are region-specific. If your locale will need separate child-directed voices per region (Castilian vs. Latin-American Spanish), open an issue first; that's a Polyglott-flavoured architectural conversation, not a single-PR problem.

---

## 2. Author the prompt pack TOML

This is the load-bearing creative work. The file lives at `src/crates/primer-pedagogy/prompts/<pack_id>.toml` and contains every pedagogically-significant string the Primer sends to the LLM.

### Get the structure right

Copy [`en.toml`](../../src/crates/primer-pedagogy/prompts/en.toml) as your starting **structure** (sections, keys, comments). Then rewrite the **content** from scratch. **Do not** translate the English text mechanically — read the section's purpose (every section has a comment), understand what the prompt is trying to achieve, and write the equivalent in your language's natural pedagogical register.

The German pack at [`de.toml`](../../src/crates/primer-pedagogy/prompts/de.toml) is the worked example of "structure copied; content rewritten." Read it alongside `en.toml` to see what "adapt, don't translate" means in practice. Notable adaptations in the German pack:

- The English "never use a word with more than three syllables" rule is **deleted** — German compounds make syllable count a useless metric. The replacement rule names Latin/Greek roots and 3-plus-element compounds as the technical-vocabulary markers.
- A non-existent-in-English `du` vs. `Sie` block tops the system prompt with an explicit "this is not negotiable" framing.
- Vocabulary examples ("plasma", "molecule") are replaced with language-natural German equivalents that actually pose vocabulary difficulty for German children at that age (`Schwingung`, `Trommelfell`, `Druckwelle`).

### The eight sections

#### `[meta]` — Identity

```toml
[meta]
language = "es"             # must equal Locale::Spanish.pack_id()
language_name = "Spanish"   # must equal Locale::Spanish.name() — ENGLISH name
bcp47 = "es-ES"             # must equal Locale::Spanish.bcp47()
```

All three are cross-checked against the Rust enum at load time. Drift is a hard load-time error — fix the TOML, never the validator.

#### `[system_prompt]` — The Socratic philosophy

The base system prompt (60-100 lines) encodes the Primer's entire teaching style. **Placeholders allowed: `{name}`, `{age}`, `{language_guidance}`.** A `{foo}` token that's not one of those three is a load-time error.

Rewrite, don't translate. Pay particular attention to:

- **Child-directed register.** Pick the form a teacher would use with a 7-to-12-year-old in your language (informal `tú`/`du`/`tu`/plain-form Japanese). Be explicit about it: a single instruction at the top ("Always address the child as `tú`, never as `usted`") is worth more than implicit consistency. Children spot register drift instantly.
- **The "never give a direct answer when you can ask" rule.** This is the heart of the Primer. Keep it forceful in your language.
- **The "factual question gets a direct answer THEN a Socratic pivot" exception.** Children's factual questions ("How far is the Moon?") deserve answers; the pivot is what makes it Socratic.
- **The "warm but never engagement-maximising" framing.** No emojis, no excess exclamation marks. If your culture has a strong "be nice to children" register that drifts toward engagement-maximising — push back. The Primer is allowed to say "that's enough for today."

#### `[language_guidance]` — Age-band rules

Four blocks: `ages_0_6`, `ages_7_9`, `ages_10_12`, `ages_13_plus`. **No placeholders allowed.**

These are the **most language-specific** sections in the entire pack. **Do not translate the English syllable rules.** Write your language's own age-appropriate complexity metric:

- **English** counts syllables and uses Latin/Greek roots as a technical-vocabulary marker.
- **German** ignores syllables (compounds are routine), uses 3-plus-element compounds and Latin/Greek roots as markers, and is explicit that abstract nouns (`Energie`, `Materie`) need a concrete anchor before introduction.
- **Japanese** would likely use kanji-grade level (`gakushū kanji`) as the marker.
- **Mandarin** would use character count and grade-level chosen-character sets.

If you don't know what the right metric is for your language, ask a primary-school teacher. The Primer ships with whatever you write here — get it right.

#### `[intent]` — Per-pedagogical-intent instructions

One key per `PedagogicalIntent` enum variant in snake_case. **All keys must be present** — a missing variant is a load-time error. **No placeholders allowed.** Current keys:

- `socratic_question` — the default "ask a guiding question" branch
- `comprehension_check` — probe whether the child genuinely understands
- `scaffolding` — child is struggling; offer concrete example/analogy
- `encouragement` — child is frustrated; gentle, non-condescending
- `extension` — child has shown understanding; deepen
- `direct_answer` — factual question; answer then Socratic pivot
- `answer_then_pivot` — already in direct-answer mode; keep the pivot rolling
- `session_close` — gentle wrap-up
- `suggest_break` — wallclock-driven break suggestion

Each is a single instruction (one to four sentences) that gets appended to the system prompt for that turn. Keep them tight — the LLM follows short clear instructions better than long ones.

#### `[engagement]` — Conditional engagement-state notes

Two keys: `frustrated` and `disengaging`. **No placeholders allowed.** Only appended when the engagement classifier reports the corresponding state. Short, gentle, non-condescending.

#### `[sections]` — Knowledge / memory section headers

Single-line headers that introduce the RAG context, the rolling summary, the retrieved older turns, the vocab-review list, and the break-suggestion line.

| Key | Placeholders allowed |
|---|---|
| `knowledge_intro` | `{age}` |
| `summary_intro` | none |
| `retrieved_intro` | none |
| `vocab_review_intro` | none |
| `break_suggestion_intro` | `{minutes}` |

**`break_suggestion_intro` is locale-aware in time-unit wording.** Substitute the right plural form for your language. Example: English uses `minutes` (always plural; you won't see `{minutes}=1` in practice). German uses `Minuten`. Russian needs a more elaborate plural (`минут` vs. `минуты` vs. `минута`); if your language has that complexity, encode the plural rule inside the template text rather than splitting into multiple keys.

#### `[labels]` — Speaker labels

`child` and `primer` — used in the retrieved-prior-moments section as `[Child] foo` / `[Primer] bar`. Pick what reads naturally as a transcript label (`Kind`/`Primer`, `Niño`/`Primer`, `子`/`プライマー`).

#### `[question_detection]` — Factual-question prefix list

An array of **lowercase** prefixes that mark a child's input as a direct factual lookup. The trailing space is mandatory — `"what is "` matches `"what is the moon"` but not `"whatever"`.

For languages where prefix matching doesn't work (Japanese particles, Mandarin tone, agglutinative morphology) **set this to an empty array `[]`.** Routing falls back to the LLM-based classifier with no behaviour loss.

**Intentionally excluded** in every locale: exploratory forms ("what if", "what about"), `"why"`-questions, and `"what does X mean"` (the last is handled by the vocabulary-discipline block of the system prompt — short-circuiting it to DirectAnswer would break the "teach the word back in plain language" pedagogy).

#### `[voice_state]` — GUI voice-mode display strings

Six fields, all required, all non-empty (an empty value is a load-time error):

```toml
[voice_state]
listen_label = "Listening…"
listen_hint = "take your time"
thinking_label = "Thinking…"
thinking_hint = "the Primer is working on a reply"
speak_label = "Speaking…"
speak_hint = "let the Primer finish"
```

Both fields stay short. The label fits one line; the hint is a soft reassurance to the child (`take your time`, `let the Primer finish`), not an instruction. **No placeholders allowed in any voice_state field.**

### Validation rules

The pack loader rejects:

- **Unknown placeholder tokens.** A `{name}` in a field where only `{age}` is allowed → load-time error with field name and offending token.
  - **Literal braces — double them up.** To put a literal `{` or `}` in narrative text (e.g. you want the prompt to literally read `{Beispiel}`), write `{{Beispiel}}` — `{{` and `}}` render as a single `{` / `}`, exactly like Rust/Python format strings. A *single* `{Beispiel}` is still treated as a placeholder attempt and rejected, so this is the escape hatch when you genuinely need braces. Applies to every field, including the verbatim ones with no allowed placeholders.
- **Missing intent keys.** Every `PedagogicalIntent` variant must have a TOML key — see § The eight sections above for the canonical list.
- **`[meta]` drift.** `meta.language`, `meta.language_name`, `meta.bcp47` must equal `Locale::*.pack_id() / name() / bcp47()` exactly. If you change one in the TOML you must change all three (and the Rust enum, if relevant).
- **Empty `[voice_state]` field.** All six fields must be non-empty.
- **TOML parse errors.** Standard `toml` crate errors; line numbers are reported.

When a validation rule fires, the error message names the field. Fix the TOML, not the validator.

---

## 3. Add the `Locale` enum variant

Open [`src/crates/primer-core/src/i18n.rs`](../../src/crates/primer-core/src/i18n.rs) and add your variant to the `Locale` enum and to its four projection methods (`ALL`, `name`, `bcp47`, `pack_id`, `from_pack_id`). The compiler's exhaustiveness check will tell you everywhere else needs an arm.

The pattern (Spanish shown):

```rust
pub enum Locale {
    #[default]
    English,
    German,
    Spanish,            // ← new variant
}

impl Locale {
    pub const ALL: &'static [Self] = &[Self::English, Self::German, Self::Spanish];

    pub fn name(self) -> &'static str {
        match self {
            Self::English => "English",
            Self::German => "German",
            Self::Spanish => "Spanish",
        }
    }
    pub fn bcp47(self) -> &'static str {
        match self {
            Self::English => "en-US",
            Self::German => "de-DE",
            Self::Spanish => "es-ES",
        }
    }
    pub fn pack_id(self) -> &'static str {
        match self {
            Self::English => "en",
            Self::German => "de",
            Self::Spanish => "es",
        }
    }
    pub fn from_pack_id(s: &str) -> Option<Self> {
        match s {
            "en" => Some(Self::English),
            "de" => Some(Self::German),
            "es" => Some(Self::Spanish),
            _ => None,
        }
    }
}
```

Add a unit test mirroring the existing `locale_german_pack_id_and_bcp47`.

### Stable identifier discipline

- **`pack_id` is forever.** Once you ship `"es"` for Spanish, you can never reuse `"es"` for anything else. SQLite databases out in the wild carry `learners.locale = "es"` — renaming it would orphan every Spanish learner record.
- **`name()` is also forever.** It's the internal identity check used by the prompt-pack meta validator.
- **`bcp47` can change** if you discover the region was wrong, but you'll need to coordinate with downstream speech assets.

---

## 4. Add the inference-error translation

In the same file, add a `render_<lang>` function and a match arm:

```rust
pub fn render_inference_error(err: &InferenceError, locale: &Locale) -> String {
    match locale {
        Locale::English => render_english(err),
        Locale::German => render_german(err),
        Locale::Spanish => render_spanish(err),
    }
}

fn render_spanish(err: &InferenceError) -> String {
    use InferenceError::*;
    match err {
        Auth => "…".into(),
        RateLimited { retry_after: Some(d) } => {
            let secs = d.as_secs();
            if secs == 1 { "… 1 segundo …".into() }
            else { format!("… {secs} segundos …") }
        }
        RateLimited { retry_after: None } => "…".into(),
        ServiceUnavailable => "…".into(),
        NetworkUnavailable => "…".into(),
        ModelNotFound { model } => format!("… '{model}' … ollama pull {model} …"),
        Other(_) => "…".into(),
    }
}
```

Six variants total. The English version is the reference. Two non-obvious rules:

- **Singular vs. plural for `RateLimited`.** Most languages need a different word for "1 second" vs. "N seconds." Encode the plural rule explicitly.
- **`Other`'s inner string is dev-facing.** Never render the inner `String` to users — surface a generic "something unexpected went wrong" message. The call site logs the inner string via `tracing::warn!`.

Add a test mirroring `german_rate_limited_singular_vs_plural` and `german_other_does_not_leak_inner_dev_string`. The test suite in [`primer-core/src/i18n.rs`](../../src/crates/primer-core/src/i18n.rs) is the contract — every locale gets parity coverage.

---

## 5. Wire the prompt-pack dispatch

Open [`src/crates/primer-pedagogy/src/prompt_pack.rs`](../../src/crates/primer-pedagogy/src/prompt_pack.rs) and:

1. Add `const ES_TOML: &str = include_str!("../prompts/es.toml");` next to the existing two.
2. Add a match arm to `embedded_pack`.
3. Add a `static ES_PACK: OnceLock<Arc<dyn PromptPack>> = OnceLock::new();` and the matching `load_cached` arm.

That's it. The compiler's exhaustiveness check on `Locale` will flag any spot you missed.

---

## 6. (Optional) Knowledge-base seed

Without a seed, retrieval returns no passages and the prompt builder omits the knowledge section — the conversation still works, but the Primer can't ground its answers. **Adding a seed is what makes the locale feel real.**

Two paths:

### Path A: Adapt the Wikipedia-shaped ingest

Best when a children's-wiki source exists in your language. Examples:

- **English** uses Simple English Wikipedia (CC-BY-SA-3.0).
- **German** uses Klexikon (CC-BY-SA-4.0; hand-curated for ages 8-13).
- **French** has Vikidia (CC-BY-SA-3.0). Similar shape to Klexikon.
- **Spanish** has Vikidia España (smaller).

Read [`data/ingest/README.md`](../../data/ingest/README.md) and the [`wiki/source.py`](../../data/ingest/wiki/source.py) presets. Adding a source = declaring a new `WikiSource` dataclass with the right `fetch_strategy`, hand-curating a whitelist of children's-curriculum topics, and running the pipeline.

The output is `data/seed/wiki_passages.<pack_id>.jsonl`. `auto_seed_if_empty` discovers and loads every matching `*.<pack>.jsonl` on a fresh KB — no Rust change needed.

### Path B: Hand-drafted CC0 passages

When no children's wiki exists, write passages by hand. The English seed (`seed_passages.en.jsonl`, 56 passages) is the worked example. Each row is JSON with `id`, `source`, `license`, `attribution`, `text`, `topics`. Aim for ~30-60 passages across the five planned clusters: space, body, how-things-work, life, earth/weather.

License the hand-drafted work CC0 so it can ship as part of the Primer.

### Either way

Update [`README.md`](README.md)'s supported-locales table with the corpus size.

---

## 7. (Optional) Retrieval benchmark + sweep tests

Without a benchmark, you can't claim retrieval quality. With one, the workspace's regression tests pin the locale's recall against today's defaults.

The pattern (German is the worked example):

1. **`src/tests/common/<lang>.rs`** — define `QUERIES_<LANG>: &[BenchmarkQuery]` with 20+ child-style queries and 15+ strict canonical-id mappings to your seed passages. Include sanity tests asserting ≥20 queries, ≥15 strict mappings, ≥3 per cluster, and that every `canonical_id` exists in the shipped corpus.
2. **`src/tests/retrieval_quality_<lang>.rs`** — BM25-only regression test against today's production defaults. Use `KNOWN_FAILING_QUERIES_<LANG>` to explicitly exclude paraphrases the BM25 leg can't handle; each entry documents why.
3. **`src/tests/retrieval_quality_hybrid_<lang>.rs`** — structural `StubEmbedder` sanity check (always built) + real-`fastembed` recall floor under `--features fastembed`.
4. **`src/tests/retrieval_sweep_<lang>.rs`** — 24-cell BM25 diagnostic sweep (the harness in [`tests/common/sweep.rs`](../../src/tests/common/sweep.rs) does the work — just hand it a `Bm25SweepConfig`).
5. **`src/tests/retrieval_sweep_hybrid_<lang>.rs`** — 54-cell hybrid diagnostic sweep.

Run the sweeps with `--ignored --nocapture` to discover the best defaults for your corpus. Document any genuine BM25-vs-hybrid corpus-coverage gap (rare cases where the dense leg can't bridge a paraphrase gap) in `KNOWN_FAILING_QUERIES_<LANG>_HYBRID` with a one-line rationale per entry.

---

## 8. (Optional) Voice assets mapping

Open [`src/crates/primer-speech/src/voice_loop/locale_defaults.rs`](../../src/crates/primer-speech/src/voice_loop/) and map your locale to:

- A **Piper voice model id** (e.g. `es_ES-mls_9972-low`). Find one at [rhasspy/piper-voices on Hugging Face](https://huggingface.co/rhasspy/piper-voices/tree/main). Prefer a clear, child-friendly voice — the Primer is gentle and patient; avoid newscaster or robotic voices.
- A **Whisper model size**. `small.en` for English; `small` (multilingual) for everything else. Bigger models are slower and offer marginal accuracy gains for child speech.

Build with `--features primer-cli/speech` (CLI) or `--features primer-gui/speech` (GUI) and smoke-test with `--speech --voice <model-id> --whisper-model <path>`.

**`espeak-ng` must be installed system-wide** for Piper to produce phonemes. macOS: `brew install espeak-ng`. Debian/Ubuntu: `apt install espeak-ng-data`. This is a build-system gotcha worth calling out in your locale's status page if your language has known phoneme-coverage issues.

**Whisper transcription language is set from the locale's `pack_id`** (ISO-639-1). If your `pack_id` is not exactly the ISO-639-1 code Whisper expects, the multilingual model will fall back to English transcription. The pin is in `primer-speech/src/whisper/tests.rs::pack_id_is_iso_639_1_for_whisper`.

---

## 9. Document your locale here

Copy [`_template/README.md`](_template/README.md) into `docs/localisation/<pack_id>/README.md` and fill it in. Then update the supported-locales table in [`docs/localisation/README.md`](README.md).

If you ran model evaluations (recommended — most users will run Ollama with whatever local model they already have), include a "Tested models" section like the one in [`de/README.md`](de/README.md).

---

## 10. Run the test suite

From `src/`:

```bash
~/.cargo/bin/cargo build --workspace
~/.cargo/bin/cargo test --workspace
~/.cargo/bin/cargo clippy --workspace --all-targets
~/.cargo/bin/cargo fmt --check
```

Then a smoke run:

```bash
~/.cargo/bin/cargo run --bin primer -- \
  --backend ollama --model <a-local-model> \
  --language <pack_id> --name TestChild --age 9
```

Try a curiosity opener, a frustration cue, a factual question. The Primer should: stay in your language, address the child in the right register, ask more than it tells, and offer a Socratic pivot after a direct answer.

---

## Iterating without recompiling

The prompt-pack loader honours `PRIMER_PROMPTS_DIR`:

```bash
export PRIMER_PROMPTS_DIR=/Users/me/src/primer/src/crates/primer-pedagogy/prompts/
~/.cargo/bin/cargo run --bin primer -- --language es --name TestChild --age 9
```

With `PRIMER_PROMPTS_DIR` set, the cache is bypassed — every CLI invocation re-reads your TOML. Edit, save, restart, observe. **A typo or missing field still panics at startup**, so iteration cycles are second-level (start, see error, fix, restart) rather than recompile-level.

For the embedded production build, no env var is needed — the TOML is `include_str!`'d at compile time and validated at first load.

---

## PR checklist

Before opening a PR:

- [ ] Prompt pack at `src/crates/primer-pedagogy/prompts/<pack_id>.toml` validates (no startup panic).
- [ ] `Locale::<NewLang>` added with `pack_id`, `name`, `bcp47`, `from_pack_id`, `ALL`. Tests added.
- [ ] `render_inference_error` covers all six `InferenceError` variants, with singular-vs-plural handled and no `Other`-inner-string leak. Tests added.
- [ ] `prompt_pack.rs` dispatch (`EN_TOML`/`DE_TOML`-style include + `embedded_pack` arm + `load_cached` cache slot) wired.
- [ ] `cargo build --workspace`, `cargo test --workspace`, `cargo clippy --workspace --all-targets`, `cargo fmt --check` all clean.
- [ ] Smoke test with `--language <pack_id>` against at least one real local model — language adheres, register is right, Socratic pivot fires on factual questions.
- [ ] `docs/localisation/<pack_id>/README.md` documents the locale.
- [ ] `docs/localisation/README.md` supported-locales table updated.
- [ ] If you shipped a KB seed: passage count, license, attribution, and ingest source recorded in your locale's status page.
- [ ] If you shipped a benchmark: corpus size, query count, strict-mapping count, and any `KNOWN_FAILING_QUERIES_<LANG>*` entries with rationale.

---

## Things that will trip you up

- **Cross-checking `meta.language_name` against the English name, not the native form.** `language_name = "Español"` for Spanish will fail the load — the value must be `"Spanish"`. The native form belongs in your `docs/localisation/<pack_id>/README.md`, not in the TOML's `[meta]`.
- **Empty `[voice_state]` field.** A blank line in the TOML survives parsing and fails at validation. Quote the value explicitly.
- **Forgetting to update `Locale::ALL`.** Tests that enumerate locales (sweep round-trips, prompt-pack load tests) will silently skip your new variant.
- **Translating the syllable rule.** Don't. The English syllable rule is specific to English. Rewrite from scratch using your language's complexity metric.
- **Translating the factual-prefix list.** Make sure the prefixes that fire `DirectAnswer` routing in your language are pedagogically appropriate — they should still leave `"why"`, `"what if"`, and `"what does X mean"` as Socratic-richer untouched. If prefix-matching doesn't fit your language at all, ship `factual_prefixes = []` and let the LLM classifier handle it.
- **Picking a region-mismatched Piper voice.** A `de_DE-thorsten-medium` voice with `bcp47 = "de-AT"` will work but sound off. Pick the voice's region tag and align everything else to it.
- **Building with Homebrew rust instead of rustup proxies.** The workspace pins Rust 1.88; Homebrew may shadow with an older toolchain. Always call `~/.cargo/bin/cargo`.

---

## Getting help

Open an issue tagged `i18n` or `locale:<pack_id>` if you're stuck. The fastest path to review is a draft PR with the prompt pack and the enum-variant change, even if the KB seed and voice mapping land in follow-up PRs.
