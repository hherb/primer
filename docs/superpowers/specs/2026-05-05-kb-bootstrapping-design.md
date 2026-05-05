# Knowledge Base Bootstrapping — Design Spec

**Date:** 2026-05-05
**Phase:** Phase 0.2
**Author:** Claude (with Horst Herb)
**Status:** Draft, pending user review

## Goal

Populate the per-locale FTS5 knowledge base (`passages_<pack_id>`) with high-quality, redistributable content covering the breadth of topics children naturally ask about — space, dinosaurs, weather, the human body, plants, animals, how things work — so the Primer's responses are grounded in verified passages rather than parametric LLM knowledge alone. The Primer must work fully offline once the KB is loaded, and the entire content pipeline must respect copyright in a way that lets us redistribute the resulting database.

After this work:

- A small **curated seed corpus** ships with the repository (~50–100 passages per locale, JSONL, hand-drafted by Claude and human-edited) and auto-loads on first open of an empty `passages_<pack_id>` table.
- A **Python ingestion pipeline** at `tools/kb-ingest/` produces a fuller corpus on demand from CC-BY / CC-BY-SA / CC0 / public-domain sources (Simple English Wikipedia, OpenStax, PMC OA-CC-BY subset, Wikibooks, Wikidata, NASA/NOAA/NIH, Project Gutenberg). Output is a SQLite file matching the existing schema; users drop it at `--knowledge-db <path>`.
- A new **`primer-kb-load` Rust binary** ingests JSONL files into the KB without requiring Python — used by both the auto-seed path and by users who fetch a pre-built JSONL but don't want to run the Python pipeline.
- A new lightweight **`sources` table** in the KB DB records `(id, license, attribution, retrieved_at)` per source, so licence compliance travels with the data and a `--print-attribution` CLI flag can produce the credit list at any time.
- A new **retrieval-quality integration test** asserts canonical child queries ("why is the sky blue", "how do plants eat", etc.) return passages containing the expected key terms (Rayleigh, photosynthesis, …) — also drives `RetrievalParams` default tuning.
- The existing `SqliteKnowledgeBase` API and per-locale table layout are preserved unchanged. All bootstrapping is additive.

## Non-goals

- **No embedding-based / semantic retrieval.** FTS5 + BM25 only. Embeddings are a Phase 4 concern (per ROADMAP "embedding-based semantic search" line for Frithjof).
- **No on-device fetch from the live internet.** Ingestion is a developer/build-time activity; the runtime never reaches out to download passages. (The Primer is offline-first by design.)
- **No live updates / incremental sync.** Re-running ingestion produces a fresh DB; merging deltas into a live DB is deferred. The seed corpus is replaced wholesale on the next release.
- **No image / diagram extraction.** Text-only passages. Diagrams are a Phase 3 (display) concern.
- **No translation pipeline.** Each locale ingests its own native sources. We do not auto-translate English into other locales — that would degrade quality and corrupt BM25 stats.
- **No per-passage age grading.** Every passage is "approximately suitable for ages 6–12". Finer-grained pitch happens at the LLM layer, not the corpus layer.
- **No NSFW / adult-content classifier.** Hard category and keyword blacklist only (Wikipedia categories tagged sexuality, violence, weapons, etc.). Conservative — false positives are fine, false negatives are not.
- **No CK-12, MIT OCW, Khan Academy.** All carry NC clauses that prevent redistribution under our rule.
- **No proprietary content.** Britannica, Encarta, Encyclopedia.com, etc. are out.
- **No per-source FTS5 tokenizer customisation.** All locales use SQLite's default `unicode61` for now (matches the existing comment in `primer-knowledge/src/lib.rs`). Per-locale tokenizers are a future concern.
- **No de-duplication across sources.** Two sources covering "photosynthesis" each contribute their passage; BM25 surfaces whichever scores higher. Cross-source dedup is a polish item.
- **No ranking-of-source priors** ("prefer OpenStax over Wikipedia"). All passages compete on BM25 alone in v1.
- **No `RetrievalParams` plumbing changes.** The same struct (top_k, min_score, source_filter) works fine; this work just tunes its defaults.

## Pedagogical principles preserved

- **The Primer asks more questions than it answers.** Better RAG retrieval lets the Primer ground its Socratic questions in real specifics ("the article mentions xylem — do you know what those are?") rather than vague hand-waving. Retrieval quality directly amplifies the Socratic stance.
- **All learner data stays local.** Nothing about the corpus changes this; ingestion does not transmit any child data anywhere.
- **Comprehension is verified through transfer questions and application — not assumed.** A grounded passage gives the Primer concrete facts to test transfer against.
- **The Primer does not maximise engagement.** Corpus content is factual reference material, not gamified hooks.

## Licence policy

We accept content under licences that allow redistribution. We require source citation regardless of licence (always good practice, also a hard requirement under most CC licences).

| Licence class | Decision | Rationale |
|---|---|---|
| Public domain (PD) / CC0 | ✅ Accept | No restrictions. |
| CC-BY (attribution only) | ✅ Accept | Stated rule applies directly. |
| CC-BY-SA (attribution + share-alike) | ✅ Accept | ShareAlike obligation cascades to redistributors but does not prevent redistribution. Aligns with the project's AGPL spirit — encourage dissemination, copyleft on derivatives. |
| US-Federal-Government works (NASA / NOAA / NIH / USGS) | ✅ Accept | PD in the US, treated as PD in practice elsewhere; cite the originating agency. |
| CC-BY-NC, CC-BY-NC-SA, CC-BY-NC-ND | ❌ Reject | NC clause prevents commercial redistribution; the Primer device may eventually ship commercially. |
| CC-BY-ND | ❌ Reject | NoDerivatives prevents the chunking / reformatting we need. |
| All-rights-reserved, "free to read but not redistribute", "view-only" | ❌ Reject | Doesn't satisfy the redistribution rule. |
| Crown Copyright / OGL (UK gov) | ⚠️ Case-by-case | OGL is OK; Crown Copyright varies. Default reject pending review. |

The licence tag for each ingested passage is stored as a string in the new `sources` table — not as an enum — so we can record exact terms without needing a code change for every new licence variant.

### Implications of accepting CC-BY-SA

Anyone who redistributes the resulting `.db` file must keep it under CC-BY-SA. A future commercial release of the Primer device that ships the DB will need to make the corpus available under CC-BY-SA at distribution time. This is acceptable to the project. We do **not** attempt to produce a "pure CC-BY-only" build variant — too much rope, too little gain.

## Architecture

Three deliverables, layered:

```
┌─────────────────────────────────────────────────────────────┐
│  RUNTIME (existing, unchanged except auto-seed on open)     │
│  primer-knowledge: SqliteKnowledgeBase::open_for_locale()   │
└──────────────────▲──────────────────────────────────────────┘
                   │ reads
                   │
┌──────────────────┴──────────────────────────────────────────┐
│  BUILD ARTEFACTS                                            │
│  data/seed/seed_passages.<pack_id>.jsonl  (in-repo)         │
│  primer-corpus-<pack_id>-YYYYMMDD.db      (release artefact)│
└──────────────────▲──────────────────────────────────────────┘
                   │ produced by
       ┌───────────┴────────────┐
       │                        │
┌──────┴───────────┐   ┌────────┴────────────────────────────┐
│ tools/kb-ingest/ │   │ src/crates/primer-kb-load/          │
│ (Python)         │   │ Rust bin: ingest JSONL → SQLite     │
│ download/parse/  │   │ used by both auto-seed and          │
│ filter/chunk/    │   │ "I downloaded the JSONL but no      │
│ emit JSONL       │   │  Python on my box" path             │
└──────────────────┘   └─────────────────────────────────────┘
```

### Storage: the `sources` table

A new table tracks per-source metadata. Schema migration follows the same idempotent column-add pattern `primer-storage` uses. Versioning: introduce a `user_version` to `primer-knowledge`'s schema (it currently has none — `schema.rs` doesn't exist for this crate). Set initial version to 1; the `sources` table migration is v2.

```sql
CREATE TABLE IF NOT EXISTS sources(
    id            TEXT PRIMARY KEY,         -- e.g. "wikipedia:en:Rayleigh_scattering"
    license       TEXT NOT NULL,            -- e.g. "CC-BY-SA-4.0"
    attribution   TEXT NOT NULL,            -- human-readable credit line
    source_url    TEXT,                     -- canonical URL, if applicable
    retrieved_at  INTEGER NOT NULL          -- unix epoch
);
```

Existing `passages_<pack>.source` becomes a *logical* (not enforced) FK into `sources.id` — FTS5 virtual tables can't enforce FKs, but the loader writes both rows in one transaction.

A small Rust API addition to `SqliteKnowledgeBase`:

```rust
pub fn upsert_source(&self, src: &SourceMeta) -> Result<()>;
pub fn list_sources(&self) -> Result<Vec<SourceMeta>>;  // for --print-attribution
```

`SourceMeta` lives in `primer-core::knowledge`.

### The seed corpus

Format: JSON Lines, one passage per line, in `data/seed/seed_passages.<pack_id>.jsonl`. Each line:

```json
{"id":"seed:en:rayleigh-scattering","source":"seed:en:rayleigh-scattering","license":"CC0-1.0","attribution":"The Primer seed corpus, Claude + Horst Herb, 2026","text":"The sky looks blue because…","topics":["physics","optics","weather"]}
```

`topics` is informational only — not stored in the DB schema, just useful for human review and for partitioning the corpus.

50–100 passages targeting the topics children most often ask about:

- **Space:** sun, moon, planets (each major one), stars, black holes, galaxies, comets, eclipses, why we have day and night, tides
- **Earth & weather:** seasons, rain, snow, wind, lightning, rainbows, the water cycle, volcanoes, earthquakes, why the sky is blue, why sunsets are red
- **Life on earth:** how plants eat (photosynthesis), how seeds grow, food chains, dinosaurs (multiple), evolution basics, mammals/reptiles/amphibians/birds, insects, where babies come from (age-appropriate, no genitalia), how animals see at night
- **The human body:** heart, lungs, brain, digestion, skeleton, immune system, why we sleep, why we dream
- **How things work:** magnets, electricity, batteries, gears, wheels, levers, sound, light, mirrors, lenses, fire, ice, salt, soap
- **Maths/logic primers:** what numbers are, what zero means, infinity (kid-friendly), prime numbers
- **History anchors:** evolution timeline, dinosaurs, Egyptians, Romans, the moon landing, electricity discovery — each light, each clearly factual

Each passage:

- **300–500 words.** Long enough for genuine substance, short enough that a child's attention can survive being read it (which is what RAG retrieval triggers).
- **Plain language.** Roughly Simple English Wikipedia register. Technical terms introduced with a parenthetical or follow-up sentence.
- **No moralising, no value-laden framing.** Just the facts.
- **CC0 licensed** — Claude and Horst write it from scratch, no derivative obligation.

Claude drafts; Horst edits. Drafting goes in batches of ~10 passages on a single topic (e.g. "Space — first 10") so review is tractable. Each batch is a separate commit.

### Auto-seed-on-empty path

In `SqliteKnowledgeBase::open_for_locale`, after `migrate_or_create`, count rows in `passages_<pack>_content`. If zero **and** the binary can locate `data/seed/seed_passages.<pack_id>.jsonl` (relative to the working dir or `CARGO_MANIFEST_DIR`-derived in dev / next to the binary in release), load it.

Discovery rules:

1. `$PRIMER_SEED_DIR/seed_passages.<pack_id>.jsonl` if env var is set.
2. `$XDG_DATA_HOME/primer/seed/seed_passages.<pack_id>.jsonl`.
3. Cargo dev path: `<CARGO_MANIFEST_DIR>/../../data/seed/seed_passages.<pack_id>.jsonl`.
4. Otherwise, log `tracing::info!` "no seed corpus found for locale X; KB starts empty" and continue. **Empty KB is valid** (existing behaviour). No hard error.

The auto-seed is one-shot; subsequent opens skip the load because rows are present.

### `primer-kb-load` Rust binary

New crate `primer-kb-load` with a single binary:

```bash
cargo run --bin primer-kb-load -- \
    --knowledge-db ./primer-en.db \
    --locale en \
    --jsonl ./seed_passages.en.jsonl
```

Reads JSONL, opens `SqliteKnowledgeBase::open_for_locale`, calls `upsert_source` then `insert_passage` per line in a single transaction, prints a one-line summary.

This is what the auto-seed path also calls internally (extracted as a library function `primer_kb_load::load_jsonl(kb, path) -> Result<usize>`).

### Python ingestion pipeline (`tools/kb-ingest/`)

Layout:

```
tools/kb-ingest/
    README.md
    pyproject.toml          # uv- or pip-installable
    sources.toml            # declarative source registry
    kb_ingest/
        __init__.py
        cli.py              # `kb-ingest fetch | parse | filter | chunk | emit | all`
        download.py         # cache-aware downloads, resumable
        parsers/
            wikipedia.py    # mwparserfromhell + mwxml
            openstax.py     # trafilatura on official HTML exports
            pmc_oa.py       # NXML parsing, CC-BY filter
            wikibooks.py
            project_gutenberg.py
            gov_html.py     # generic HTML scrape for NASA/NOAA/NIH pages
        filter.py           # NSFW category blacklist, keyword screen, length floor
        chunk.py            # paragraph-aware chunker, ~300-700 words
        license.py          # licence detection + sources.toml lookup
        emit.py             # JSONL writer matching seed format
    tests/                  # parser unit tests on small fixtures
```

`sources.toml` is the declarative source registry:

```toml
[[source]]
id = "wikipedia-simple-en"
kind = "wikipedia"
url = "https://dumps.wikimedia.org/simplewiki/latest/simplewiki-latest-pages-articles.xml.bz2"
locale = "en"
license = "CC-BY-SA-4.0"
attribution = "Simple English Wikipedia contributors, https://simple.wikipedia.org/"
parser = "wikipedia"
include_categories = []   # empty = all
exclude_categories = ["Sexuality", "Violence", "Pornography", "Weapons"]

[[source]]
id = "openstax-biology-2e"
kind = "openstax"
url = "https://openstax.org/details/books/biology-2e"
locale = "en"
license = "CC-BY-4.0"
attribution = "OpenStax, Biology 2e, Rice University, https://openstax.org/details/books/biology-2e"
parser = "openstax"
```

The parser library choices:

- **`mwparserfromhell` + `mwxml`** for wiki dumps — the standard tools, mature, handle the templates and infoboxes correctly.
- **`trafilatura`** for HTML extraction — robust against the cruft on OpenStax/government pages.
- **`lxml`** for PMC NXML.
- **`requests`** for downloads with `If-Modified-Since` cache headers.
- **`tqdm`** for progress.
- **`pydantic`** for `sources.toml` and JSONL schema validation.

CLI surface:

```bash
kb-ingest fetch --source wikipedia-simple-en        # download to ~/.primer/kb-cache/
kb-ingest parse --source wikipedia-simple-en        # produce raw text dump
kb-ingest filter --source wikipedia-simple-en       # apply NSFW + length filters
kb-ingest chunk --source wikipedia-simple-en        # paragraph-aware chunking
kb-ingest emit --source wikipedia-simple-en --out simplewiki-en.jsonl
kb-ingest all   --locale en --out primer-corpus-en.jsonl
```

Each stage is idempotent and cached; re-running `all` after one source's parse changes only re-runs that source.

### Filtering: NSFW and quality

A two-stage filter:

1. **Category blacklist** (Wikipedia-specific): drop any article whose category set intersects a blacklist (`Sexuality`, `Pornography`, `Weapons`, `Violence`, `Drugs`, `Suicide`, `Genocide`, `Torture`, `Nudity`, …). The full list lives in `tools/kb-ingest/kb_ingest/filter.py` and is reviewed by humans.
2. **Keyword screen** for sources without categories (OpenStax, gov pages): a small list of substrings; if any appears, drop the passage. False-positive bias.

Quality filters:
- Drop articles tagged stub, disambiguation, redirect, list-of, or with `<200` words of body text.
- Drop articles whose first paragraph is below a readability floor (Flesch reading ease < 50, optional, behind a flag — not run by default since OpenStax college texts will sometimes fall below).

### Chunking

Paragraph-aware. Iterate paragraphs in order; greedily accumulate into a chunk until ~500 words; flush; new chunk starts with the article title as a prefix line so context survives retrieval. Hard cap 700 words. Drop chunks <100 words.

The article title prefix matters for FTS5: a query "rayleigh scattering" might otherwise miss a deep paragraph that doesn't repeat "rayleigh".

### Distribution model

| Asset | Where | Size |
|---|---|---|
| Seed corpus JSONL (per locale) | In `data/seed/` in repo | ~50–500 KB |
| Pre-built English corpus DB | GitHub Release artefact | est. 200 MB–1 GB |
| `tools/kb-ingest/` source | In repo | ~50 KB |
| `primer-kb-load` binary | In workspace | tiny |

Releases gain a step: run `kb-ingest all --locale en` on a maintainer's box, upload the resulting `.db` and `.jsonl` to the release. CI does **not** run ingestion (too slow, too much disk).

### Retrieval quality test

New file `src/crates/primer-knowledge/tests/retrieval_quality.rs`:

```rust
// Loads data/seed/seed_passages.en.jsonl into a temp DB,
// runs canonical queries, asserts top-K passages contain
// expected key terms.
const QUERIES: &[(&str, &[&str])] = &[
    ("why is the sky blue", &["rayleigh", "scattering", "wavelength"]),
    ("how do plants eat", &["photosynthesis", "chlorophyll"]),
    ("how do magnets work", &["magnet", "iron", "field"]),
    ("why do we have seasons", &["axis", "tilt", "earth"]),
    ("what makes lightning", &["charge", "cloud", "electric"]),
    // …~15 total
];
```

Each query: `kb.retrieve(query, &RetrievalParams::default())`, assert at least one of the top-3 passages contains all required key terms (case-insensitive substring). Tests run on the seed corpus, so they're deterministic and don't require the full pipeline.

This test also drives `RetrievalParams::default()` tuning (the explicit ROADMAP item). If a query needs `top_k = 7` to surface the right passage, that's a signal to bump the default.

## Implementation order

1. **Spec review** (this document). Land after Horst's review.
2. **Schema migration** in `primer-knowledge`: add `sources` table + user_version (v1 → v2). One PR, additive, idempotent. Tests for round-trip.
3. **`SourceMeta` + `upsert_source` / `list_sources` API**, plus `Passage.license`/`Passage.attribution` carried through the trait. (Trait change is small; storage backend is the only impl.)
4. **`primer-kb-load` crate + binary.** Reads JSONL, writes via the new API. Library entry point `load_jsonl(kb, path) -> Result<usize>`. Unit tests.
5. **Auto-seed-on-empty** in `SqliteKnowledgeBase::open_for_locale`. The discovery rules above; falls back to "log + continue" if no seed found. Test fixture: a 3-passage minimal seed.
6. **Seed corpus drafting (Claude → Horst editing).** Batches of ~10 passages per topic cluster. Five clusters: space, earth/weather, life, body, how-things-work. Each batch a separate PR. (The two not-so-on-the-critical-path clusters — maths primers, history — get drafted last.)
7. **Retrieval quality integration test.** Lands once the seed corpus has ≥30 passages so queries have something to find.
8. **`tools/kb-ingest/` Python pipeline.** Lands in pieces: download + sources.toml first; then Wikipedia parser; then OpenStax; then PMC; then filter + chunk + emit. Each parser ships with a small fixture-based unit test.
9. **First full English corpus build.** Maintainer runs `kb-ingest all --locale en`, publishes the `.db` and `.jsonl` as a GitHub Release artefact. Document in `README.md` how to use a downloaded corpus.
10. **`--print-attribution` CLI flag** on `primer-cli`. Reads `kb.list_sources()`, prints the credit list. (Required for licence compliance under CC-BY and CC-BY-SA when the device ships.)

The auto-seed path (steps 1–6) is independently shippable and gives the Primer a usable, child-pitched corpus on first run with zero external dependencies. The Python pipeline (steps 8–9) is bonus headroom for users who want a deeper corpus — and for the eventual hardware build.

## Risks and mitigations

- **Risk: Seed corpus drafted by an LLM contains subtle errors that get retrieved as "ground truth".** Mitigation: human edit pass on every passage. Each passage cites a real-world primary source URL in a comment block in the JSONL — invisible to retrieval, but lets reviewers verify facts. We may eventually swap LLM-drafted passages for direct PD/CC-BY excerpts as the corpus matures.
- **Risk: Wikipedia category blacklist isn't sufficient — children's curiosity finds adjacent topics.** Mitigation: conservative blacklist plus per-source allowlist for high-stakes domains. Periodic spot-checks. The retrieval test set should include "tricky" queries (e.g. "where do babies come from") to verify the response is age-appropriate.
- **Risk: BM25 surfaces a noisy passage when the dump has stylistic outliers (tables, lists, infobox residue).** Mitigation: chunker drops chunks under 100 words; parser strips wiki templates; quality filter rejects stub-flavoured content. Iterative — the retrieval test is the truth signal.
- **Risk: The full English corpus is too big to ship comfortably (~1 GB after all sources).** Mitigation: seed corpus is the default; full corpus is opt-in; users who want it download a release artefact. Offer a "core" subset (Simple Wikipedia + curated OpenStax chapters only, ~200 MB) as the recommended midweight option.
- **Risk: A future commercial Primer device ships the DB and inadvertently violates CC-BY-SA by not making the corpus available.** Mitigation: the `sources` table + `--print-attribution` flag mean the attribution travels with the data; the build pipeline produces a `LICENSE-CORPUS.md` summary alongside the `.db`. Hardware-release checklist will include "verify CC-BY-SA corpus is available for download per the licence".
- **Risk: A locale gets bootstrapped from a rough auto-translated seed and corrupts FTS quality.** Mitigation: explicit non-goal — no auto-translation. Each locale gets its own native sources or stays empty (which is a valid state).
- **Risk: The seed corpus diverges from the Python-pipeline corpus over time** (different writing styles, conflicting facts). Mitigation: the seed corpus is the canonical fallback for "common child questions". Pipeline content layers on top; BM25 picks the more relevant passage per query. Both tagged with their source so manual review is possible.

## Open questions for review

1. **Seed corpus drafting cadence.** Drafting 50–100 passages is ~3–5 sessions of Claude work + 3–5 sessions of Horst editing. Is the "five-cluster, ~10-passage batches" cadence right, or should the first batch be smaller (3–5 passages of one topic) so the workflow can be debugged before scaling up?
2. **Distribution of the full corpus.** GitHub Releases caps at 2 GB per asset, which is fine for a single-locale `.db` but might pinch if we bundle multiple locales. Hugging Face Datasets is a natural alternative for the corpus artefact; the repo stays code-only. Preference?
3. **Where does `tools/kb-ingest/` live in the workspace?** I'd put it as a top-level repo directory (sibling of `src/` and `docs/`), not inside `src/` — it's not Rust and not part of the cargo workspace. Confirm.
4. **CI coverage.** Should the retrieval-quality test run in CI? It depends on the seed corpus JSONL existing; if the JSONL is in-repo, yes. If it lives only in releases, it has to be a manual check.
5. **`--print-attribution` UX.** Default to a one-line summary ("KB contains 2,341 passages from 4 sources; full credits at <path>"); `--print-attribution full` dumps the entire list. Acceptable, or should the full list be the default?
