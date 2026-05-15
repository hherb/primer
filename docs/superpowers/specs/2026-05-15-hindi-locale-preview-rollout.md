# Hindi Locale (Preview) Rollout — Design

**Status:** approved, ready to plan
**Date:** 2026-05-15
**Issue:** —
**Branch (planned):** `feat/locale-hindi-preview`
**Author:** Claude Opus 4.7 (1M context) with Horst Herb

---

## Background

The Primer ships two end-user locales today: `en` (English, reference) and `de` (German, hand-translated by a native speaker). NEXT_SESSION called out a "Hindi (`hi`) locale pack rollout" as the next attractive locale step, anticipating that the pattern established by Klexikon (German children's wiki) would translate to a Hindi equivalent such as Bal Vikipedia or Vikidia-Hindi.

Web research at session start refuted that assumption: **no Hindi children's wiki of the Klexikon / Simple-English-Wikipedia shape currently exists**. Vikidia covers 14 languages and Hindi is not among them. Bal Vikipedia is not a real site. The full `hi.wikipedia.org` is adult prose, vocabulary-mismatched with the Primer's target audience (ages 5–14). A bespoke Hindi children's-friendly corpus needs sourcing decisions that aren't ready: candidates include NCERT textbooks (licensing claims need verification), Pratham Books StoryWeaver (CC-BY varies per book), and Wikisource Hindi children's literature. None is shovel-ready.

Separately, voice-asset availability is **not** a blocker: Piper ships `hi_IN-rohan-medium` (single voice, medium tier, 63 MB) and Whisper falls back to the multilingual `ggml-small.bin` already used for German.

This spec scopes a **preview** locale rollout: the Hindi locale becomes structurally present in the codebase (enum, prompt pack, voice defaults, inference-error translations, docs) but is explicitly gated from end-user UI listings until (a) a Hindi native speaker has reviewed the machine-translated prompt pack and (b) a children's-friendly corpus path is decided. The preview gate is a pair of belts-and-braces firewalls: a `Locale::ALL` exclusion (UI invisibility) and a `[meta] status = "preview"` TOML field (loader warns; future readers know the strings are unreviewed).

## Goals

- A developer running `--language hi` against any backend can exercise the full pipeline end-to-end against a Hindi system prompt.
- The asset auto-download path resolves `hi` to Rohan + small.bin without further configuration.
- All existing tests stay green; new tests pin the preview semantics.
- The translation gap and corpus gap are explicitly documented (docs, CLAUDE.md gotcha, NEXT_SESSION note).
- No end user encounters Hindi via the CLI or GUI locale picker — `Locale::ALL` excludes Hindi until the translation review lands.
- The TOML preview gate is a load-bearing structural firewall (loader-level), not a checklist item that can be silently bypassed.

## Non-goals

- No Hindi corpus authored or ingested. `data/seed/` gains nothing Hindi-shaped.
- No `tests/common/hi.rs` queries — placeholder benchmark data is dead code without a corpus, and would invite drift. Defer to the corpus-decision PR.
- No retrieval-quality or sweep tests for `hi`. Same reason.
- No `WikiSource` preset for Hindi in `data/ingest/wiki/source.py` — we don't have an upstream `api_url` to pin.
- No GUI change. `Locale::ALL` is the single source of truth for the picker; excluding Hindi from it is the GUI gate.
- No native-speaker translation pass. That is its own work item with its own PR.
- No flip of the embedding/speech-feature defaults; orthogonal carryover items.

## Design

### Module / file map

| Layer | File | Change |
|---|---|---|
| Locale enum | `src/crates/primer-core/src/i18n.rs` | + `Hindi` variant; `name`/`bcp47`/`pack_id`/`from_pack_id` arms; `render_hindi` function with 6 translated error strings; **`ALL` unchanged** (still `[English, German]`); tests pinning identity + Devanagari content + ALL.len() == 2 |
| Prompt-pack validator | `src/crates/primer-pedagogy/src/prompt_pack.rs` | + optional `[meta] status` deserialisation; `PackStatus { Stable, Preview }` enum; allow-list validator; `PromptPack::status()` accessor; warn-once-per-locale `OnceLock` on Preview load; `Hindi => HI_TOML` arm in `embedded_pack` |
| Prompt pack TOML | `src/crates/primer-pedagogy/prompts/hi.toml` | **new** ~210 lines; structurally complete; machine-translated content adapted for Devanagari + Hindi register; `[meta] status = "preview"`; `# REVIEW:` markers above translation blocks |
| Voice defaults | `src/crates/primer-speech/src/voice_loop/locale_defaults.rs` | + `hi` tuple; tests pin Rohan + small.bin |
| Docs (status page) | `docs/localisation/hi/README.md` | **new** ~80 lines; mirrors `docs/localisation/de/README.md`; flags preview status, missing corpus, candidate sources |
| Docs (model eval) | `docs/locale/models/HINDI.md` | **new** ~25 lines; skeleton mirroring `docs/locale/models/GERMAN.md` with empty evaluation table |
| Docs (gotcha) | `CLAUDE.md` | + 1 paragraph explaining the preview-status convention + why Hindi is in the enum but not in ALL |

No changes to: storage schemas, knowledge-base layer, retrieval helpers, dialogue manager, classifier/extractor/comprehension crates, primer-engine wiring, primer-cli or primer-gui surface code.

### Locale enum surgery

```rust
pub enum Locale {
    #[default]
    English,
    German,
    Hindi,  // preview — see docs/localisation/hi/README.md
}

impl Locale {
    pub const ALL: &'static [Self] = &[Self::English, Self::German];
    // Hindi deliberately omitted until prompt-pack native-speaker review lands.
    // `from_pack_id("hi")` still works for developer-only --language hi.

    pub fn name(self) -> &'static str { match self { ..., Self::Hindi => "Hindi" } }
    pub fn bcp47(self) -> &'static str { match self { ..., Self::Hindi => "hi-IN" } }
    pub fn pack_id(self) -> &'static str { match self { ..., Self::Hindi => "hi" } }
    pub fn from_pack_id(s: &str) -> Option<Self> {
        match s { ..., "hi" => Some(Self::Hindi), _ => None }
    }
}

pub fn render_inference_error(err: &InferenceError, locale: &Locale) -> String {
    match locale {
        Locale::English => render_english(err),
        Locale::German => render_german(err),
        Locale::Hindi => render_hindi(err),
    }
}
```

`render_hindi` mirrors `render_german`'s shape — six match arms over `InferenceError` variants, each returning a short Devanagari string. Auth, RateLimited (with Retry-After variant), ServiceUnavailable, NetworkUnavailable, ModelNotFound { model } (interpolates the model name), and a generic Other.

### Prompt-pack status field

Add to the TOML schema:

```toml
[meta]
language = "hi"
language_name = "Hindi"
bcp47 = "hi-IN"
status = "preview"   # optional, default "stable"; allow-list: ["stable", "preview"]
```

Rust types:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackStatus { Stable, Preview }

impl PackStatus {
    pub const fn from_toml_str(s: Option<&str>) -> Result<Self, &'static str> {
        match s {
            None | Some("stable") => Ok(Self::Stable),
            Some("preview") => Ok(Self::Preview),
            Some(_) => Err("unknown status; allowed: stable, preview"),
        }
    }
}

pub trait PromptPack {
    // existing methods...
    fn status(&self) -> PackStatus;
}
```

Warn-once-per-locale on Preview load. Implementation: a `static PREVIEW_WARNED: OnceLock<Mutex<HashSet<Locale>>>` (or per-locale `OnceLock<()>` if we want to avoid `Mutex`; either works — pick whichever keeps the loader simpler). The warning fires exactly once per `(process, locale)` pair via `load_cached`; direct `load(locale)` (which doesn't memoise) MAY re-emit if called repeatedly, but no production code path does that. The warn message:

```
prompt pack 'hi' is in preview status — machine-translated content awaiting native-speaker review. Hindi is not in Locale::ALL and is not advertised to end users.
```

### Translation discipline for hi.toml

The German pack established the "adapt, don't translate" precedent. Apply that to Hindi:

- **Tense register**: `तुम` (familiar-respectful, common in child-directed speech). Decision is flagged in a top-of-file comment for native-speaker review; the alternative `आप` is more formal and felt to be too distant for the Primer's role as a learning companion.
- **Syllable rule**: drop the English "no more than three syllables" rule entirely — matra-stacking inflates Devanagari syllable counts and the rule maps poorly. Replace with: "use everyday Hindi words a child uses at home or in school; avoid Sanskrit-rooted technical Hindi (तत्सम / पारिभाषिक) unless first explained in plain language with a concrete example."
- **Vocabulary examples**: replace English "plasma / molecule / conductor / vibration" with Hindi-natural technical-vocabulary markers — `कण` (particle), `तरंग` (wave), `विद्युत-धारा` (electric current), `आवृत्ति` (frequency), `अणु` (molecule).
- **Number-vocabulary rules**: keep Arabic numerals in sentence-length rules (`6 से 10 शब्द`), since that's the convention in modern Hindi pedagogical writing.
- **`# REVIEW:` markers**: every translated block gets a `# REVIEW:` comment line above it identifying that block as machine-translated. A native-speaker reviewer can grep for `# REVIEW:` to see exactly what needs eyes.
- **Voice-state copy** (`[voice_state]`): four-character labels feel cramped in Devanagari (`सुन रहा हूँ…` is much wider than `Listening…`); keep them short but allow them to be slightly longer than the English. Hint strings stay one short sentence.

### Voice defaults

```rust
("hi", LocaleDefault {
    piper_voice_id: "hi_IN-rohan-medium",
    piper_onnx_url: "https://huggingface.co/rhasspy/piper-voices/resolve/main/hi/hi_IN/rohan/medium/hi_IN-rohan-medium.onnx",
    piper_config_url: "https://huggingface.co/rhasspy/piper-voices/resolve/main/hi/hi_IN/rohan/medium/hi_IN-rohan-medium.onnx.json",
    whisper_model_id: "ggml-small.bin",   // multilingual; English-only small.en omits Hindi
    whisper_url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin",
    approx_total_mb: 540,                  // 63 (Piper) + ~470 (Whisper) = ~533, round up for consistency with de
}),
```

Whisper Hindi quality on `small.bin` is acceptable but not great; this matches the documented German tradeoff and is consistent with the carried-forward "voice-loop hardening" Phase 2 polish item.

## Tests

TDD throughout. Tests first; watch them fail against the placeholder enum / placeholder TOML; then implement until green.

### `primer-core::i18n`

1. `locale_hindi_pack_id_and_bcp47` — pins `Locale::Hindi.pack_id() == "hi"`, `.bcp47() == "hi-IN"`, `.name() == "Hindi"`.
2. `from_pack_id_hi_roundtrip` — `Locale::from_pack_id("hi") == Some(Locale::Hindi)` and round-trip via `pack_id()`.
3. `all_excludes_hindi_until_translation_reviewed` — pins `Locale::ALL.len() == 2`, asserts `!Locale::ALL.contains(&Locale::Hindi)`. Comment explains the preview gate.
4. `hindi_inference_errors_contain_devanagari` — iterate over every `InferenceError` variant; assert `render_inference_error(err, &Locale::Hindi)` is non-empty and contains at least one char in `U+0900..=U+097F`.
5. Existing `locale_pack_id_round_trips_via_from_pack_id` keeps iterating over `Locale::ALL` so it still pins only en + de.

### `primer-pedagogy::prompt_pack`

6. `hindi_pack_loads_in_preview_status` — `prompt_pack::load(Locale::Hindi)?.status() == PackStatus::Preview`.
7. `stable_pack_status_defaults_to_stable_when_field_absent` — `prompt_pack::load(Locale::English)?.status() == PackStatus::Stable` and same for German. Confirms no need to touch en.toml / de.toml.
8. `pack_status_rejects_unknown_value` — feed a synthetic TOML with `status = "wip"` through `TomlPromptPack::from_toml_str` and assert load error.
9. `hindi_pack_loads_meta_identity_matches_enum` — pins `meta.language == "hi"`, `language_name == "Hindi"`, `bcp47 == "hi-IN"` (existing meta validator already enforces this; the test is a regression guard against accidental drift).
10. `hindi_pack_has_all_intent_arms` — every `PedagogicalIntent` variant has a non-empty key in hi.toml. (Existing validator enforces; test pins as belt-and-braces.)
11. `hindi_pack_voice_state_section_complete` — all six `[voice_state]` fields are non-empty. (Existing validator enforces.)
12. `preview_warning_emits_once_per_locale` — call `load_cached(Locale::Hindi)` twice in a single test using a captured-tracing subscriber; assert exactly one warning event with `target = "primer::prompt_pack"`.

### `primer-speech::voice_loop::locale_defaults`

13. `hindi_default_is_rohan_plus_small_multilingual` — `voice_default_for(&Locale::Hindi).unwrap().piper_voice_id == "hi_IN-rohan-medium"` and `whisper_model_id == "ggml-small.bin"`.
14. Existing `approx_total_mb_is_sane` automatically picks up the new entry — 540 is comfortably inside the 400–1600 MB sanity range.
15. Existing `all_urls_resolve_under_huggingface_co` automatically picks up the new URLs.

## Error handling

- TOML parse errors on hi.toml at compile-include-str time = compile failure (caught by `include_str!`).
- Unknown `status` value in any pack = `TomlPromptPack::from_toml_str` returns `PrimerError` with the loader's existing message shape; existing load tests cover the panic-then-shape contract.
- `from_pack_id("hi")` returning `Some(Hindi)` and an `--language hi` invocation: the rest of the pipeline (knowledge base, learner store, embeddings) is locale-agnostic on the string side — it already round-trips `learners.locale = "hi"` rows since schema v6 (no migration needed).
- If `hi.toml` is missing or shorter than the validator demands, load panics at construction time. This is the desired loud-failure mode for a structurally incomplete pack; existing behaviour.
- Warn-once-per-locale failure mode: if the `OnceLock` is somehow poisoned, fall back to "warn on every load" — never silence. We do not gate any production behaviour on the warning firing.

## Implementation order

Branch: `feat/locale-hindi-preview`. The precise commit split is an implementation-plan decision (writing-plans will sequence it), but the obvious natural split is:

1. **Code + tests (one commit, or split into validator-first then enum+pack if size warrants).** Touches `i18n.rs` (variant + match arms + `render_hindi`), `prompt_pack.rs` (`PackStatus` + `[meta] status` validator + warn-once + `embedded_pack` Hindi arm), `locale_defaults.rs` (Hindi tuple), and new `prompts/hi.toml`. Workspace tests green at commit boundary.

   The enum variant and the `embedded_pack` match arm cannot be split across commits without breaking the exhaustive match — they land together. The `PackStatus` validator change CAN land in a prior commit if a clean split is desired, because it has no exhaustive-match dependency.

2. **Docs (one commit).** New `docs/localisation/hi/README.md`, new `docs/locale/models/HINDI.md`, edited `CLAUDE.md`. No code change; no test change.

Every commit must independently pass `~/.cargo/bin/cargo build --workspace && ~/.cargo/bin/cargo test --workspace`.

Test-numbering map for the implementation plan (tests are listed in the Tests section):
- Tests 1–4 (`i18n` Hindi tests) land alongside the enum variant.
- Test 5 (existing `locale_pack_id_round_trips_via_from_pack_id`) needs no change — it iterates `Locale::ALL`.
- Tests 6, 9, 10, 11, 12 (`prompt_pack` Hindi tests) land alongside `hi.toml`.
- Tests 7, 8 (`prompt_pack` PackStatus tests on stable packs + unknown value) land with the validator change.
- Test 13 (`locale_defaults` Hindi test) lands alongside the Hindi tuple.
- Tests 14, 15 are existing tests on `locale_defaults` that automatically extend coverage to the new entry — no new code needed.

## Open decisions / risks

- **Translation quality**: Claude is not a native Hindi speaker. The `[meta] status = "preview"` field + `Locale::ALL` exclusion + `# REVIEW:` markers + the docs/localisation/hi/README.md status page are four overlapping firewalls. A native-speaker review PR will flip `status` to `"stable"` and add Hindi to `Locale::ALL` in a single commit.
- **Tense register (तुम vs. आप)**: tum is a defensible choice that matches the de.toml du-precedent, but Hindi register choice is more culturally loaded than German's. Native-speaker review may flip this.
- **Vocabulary-difficulty examples**: the machine-translated "what counts as technical Hindi for a 7-year-old" rule may not match what real Hindi 7-year-olds actually find hard. Out of scope for this PR; pilot study is a phase-3 concern.
- **Hindi-specific factual-question prefixes**: `[question_detection].factual_prefixes` — for the Hindi pack I can populate with plausible prefixes (`क्या है `, `क्या हैं `, `कैसे `) but native-speaker review should confirm these. If unsure, leave the array empty and fall back to the LLM-engagement-classifier path (the contributor guide explicitly mentions this as an acceptable choice for languages where prefix-matching is awkward).
- **Corpus path is unsolved**: documented as an open question on the `hi/README.md` status page; tracked separately.
- **No Hindi smoke testing in this session**: the spec scopes only structural / unit-test changes. Real-LLM smoke against `--language hi` is a follow-up.

## Validation plan

Before merging the planned PR:

- `~/.cargo/bin/cargo build --workspace` — clean.
- `~/.cargo/bin/cargo test --workspace` — expect +~10 new tests, ~787 total (up from 777).
- `~/.cargo/bin/cargo test -p primer-gui --features speech` — unchanged (no GUI surface touched).
- `~/.cargo/bin/cargo fmt --all -- --check` — clean.
- `RUSTFLAGS="-D warnings" ~/.cargo/bin/cargo clippy --workspace --all-targets` — clean.
- A manual smoke (not blocking): `~/.cargo/bin/cargo run --bin primer -- --backend stub --name Aarav --age 8 --language hi`. Expected: session starts, system prompt loaded from hi.toml, no panic, response is gibberish-relative-to-Hindi-pedagogy because the stub backend echoes regardless. (The point is the loading path, not real-LLM output.)
- A manual real-LLM smoke (recommended; not blocking the PR): `--backend cloud --language hi --no-persist --verbose` and a few child-style Hindi prompts via stdin. Document any obvious translation register issues in the hi/README.md status page.

## Acceptance criteria

- [ ] `Locale::Hindi` exists in `primer_core::i18n`; `from_pack_id("hi")` returns `Some(Hindi)`; `pack_id()`, `name()`, `bcp47()` return the pinned strings.
- [ ] `Locale::ALL` still has length 2 and does not contain Hindi.
- [ ] `render_inference_error(err, &Locale::Hindi)` returns Devanagari-containing strings for every variant.
- [ ] `prompt_pack::load(Locale::Hindi)?.status() == PackStatus::Preview`.
- [ ] `prompt_pack::load(Locale::English)?.status() == PackStatus::Stable` (regression).
- [ ] Loading a pack with `[meta] status = "wip"` is a load-time error.
- [ ] `voice_default_for(&Locale::Hindi)` returns Rohan + small.bin.
- [ ] First Hindi load emits exactly one `tracing::warn!` event with `target = "primer::prompt_pack"`.
- [ ] CLI/GUI locale picker still shows only English + German.
- [ ] `docs/localisation/hi/README.md` and `docs/locale/models/HINDI.md` exist and link from the localisation index.
- [ ] CLAUDE.md gotcha documents the preview-status convention.
- [ ] Workspace tests + clippy + fmt all clean.

## What's next (post-this-PR)

- Native-speaker review of `hi.toml`. Output: a PR that flips `[meta] status = "stable"`, adds Hindi to `Locale::ALL`, and removes the `# REVIEW:` markers. May or may not bundle a curated set of factual-prefix tokens for `[question_detection]`.
- Corpus decision PR: pick a Hindi children's-friendly source (NCERT, Pratham, Wikisource), add a `WikiSource` preset or hand-author a seed JSONL, ingest, ship `seed_passages.hi.jsonl` and/or `wiki_*.hi.jsonl`.
- Retrieval-quality + sweep tests for `hi`: depends on corpus.
- Real-LLM smoke and model-evaluation entries in `docs/locale/models/HINDI.md`.

These are independent of each other and can land in any order.
