# Simple English Wikipedia ingestion (Phase 0.2 MVP)

**Status:** Approved 2026-05-06.
**Author:** Horst Herb + Claude Code (paired brainstorming).
**Phase:** 0.2 — Knowledge-base bootstrapping.
**Predecessor:** [2026-05-05-kb-bootstrapping-design.md](2026-05-05-kb-bootstrapping-design.md), [2026-05-05-hybrid-retrieval-design.md](2026-05-05-hybrid-retrieval-design.md).

## Goal

Ship an MVP end-to-end ingestion of ~30-50 Simple English Wikipedia Science articles into the existing hybrid knowledge base. The output augments the hand-drafted CC0 seed corpus (which already covers all five planned clusters at 55 passages) with a thin layer of additional breadth — concepts the seed corpus doesn't reach. The pipeline plus a curated whitelist ship in-repo; the generated JSONL is committed and auto-loads on a fresh KB alongside `seed_passages.en.jsonl`.

This is explicitly a **starting point**, not a finished system. Expect iteration as we learn what works in practice; the design is biased toward minimal infrastructure and a tight feedback loop.

## Non-goals

- Other locales. The schema is already locale-keyed (`passages_<pack>` per Phase 0.2.5); German Wikipedia ingestion is a future PR with no design change.
- Section-based or multi-passage chunking. Lead-only is the MVP rule. If field testing shows leads alone are insufficient, layering on section-level chunks is additive (different ID suffix, no breaking change).
- Other Wikipedia portals (Geography, Biology, History). Same script + a different whitelist; trivial follow-up once the pipeline is proven.
- Tuning `RetrievalParams` / `HybridParams` defaults against the broader corpus. Gated on this work landing — listed in ROADMAP.md as the next Phase 0.2 task.
- Surfacing CC-BY-SA-3.0 attribution to end-users at runtime. The `attribution` field already exists in the schema and the data preserves it; UX surfacing is a future concern.
- Crawling, category-tree walking, full XML dump processing. These all increase scope without paying off at MVP size.

## Architecture

A standalone Python ingestion script + curated whitelist + a small Rust auto-seed extension.

```
data/ingest/
├── build_whitelist.py                      # one-time helper; pulls Vital Articles
├── simple_wikipedia.py                     # main ingest; reads whitelist, fetches, emits JSONL
├── simple_wikipedia_whitelist.txt          # committed, hand-reviewed (30-50 entries)
├── tests/                                  # pytest tests (pure-function + recorded-fixture)
│   ├── fixtures/
│   ├── test_slugify.py
│   ├── test_whitelist_parser.py
│   ├── test_passage_emit.py
│   └── test_pipeline.py
├── requirements.txt                        # requests, pytest
└── README.md                               # usage docs

data/seed/
├── seed_passages.en.jsonl                  # existing; CC0; auto-loaded
└── wiki_passages.en.jsonl                  # generated output; CC-BY-SA-3.0; auto-loaded

src/crates/primer-kb-load/src/lib.rs        # extend auto_seed_if_empty to load all
                                            # *.<pack>.jsonl files in the seed dir
```

Ingestion is a one-time developer step run on a workstation with internet. The output is committed to git. Every other developer (and CI) gets the data for free, offline. This mirrors the existing `seed_passages.en.jsonl` pattern exactly.

## Components

### `build_whitelist.py` (one-time helper)

Fetches `Wikipedia:Vital_articles/Level/4/Science_and_technology` via the MediaWiki API (the curated "core 1000" list maintained by Wikipedia editors, of which ~150 are Science). Parses out article titles, prints them to stdout. The developer reviews, deletes inappropriate entries (medical conditions, controversial figures, articles too abstract for primary-school children, etc.), and saves the trimmed list to `simple_wikipedia_whitelist.txt`.

Not run in CI. Re-run by hand when expanding the whitelist for a future PR.

### `simple_wikipedia.py` (main ingest)

Pure functions where possible, network injected for testability. Public API:

- `read_whitelist(path: Path) -> list[str]` — parse the whitelist file. Strip blanks; skip lines starting with `#`. Preserve order (for diff stability with hand edits).
- `slugify(title: str) -> str` — `"Black hole"` → `"black-hole"`; NFC-normalise; lowercase; replace runs of non-alphanumeric with `-`; trim leading/trailing hyphens. Mirrors the existing `primer-cli::slug` pattern (Unicode-aware) but adapted for hyphen-separation rather than empty-separator (article slugs are user-visible in URLs, names are not).
- `fetch_lead(title: str, *, http_client) -> dict` — `GET https://simple.wikipedia.org/w/api.php?action=query&prop=extracts|info&exintro=1&explaintext=1&inprop=url&titles=<title>&format=json`. Returns `{"title": str, "lead_text": str, "canonical_url": str}`. Loud failure (raises) when the API returns no pages or an empty extract — a missing article is a whitelist bug we want to notice. `http_client` is injected so unit tests can substitute a fake.
- `to_passage(record: dict) -> dict` — converts the fetched record into the JSONL schema. See **Schema** below.
- `main(whitelist_path: Path, output_path: Path, http_client=None)` — orchestrates the run. Default `http_client` is a `requests.Session` with the User-Agent set per Wikipedia etiquette and a 100 ms inter-request sleep. Output is sorted by `id` for deterministic diffs.

The script must:
- Set a descriptive `User-Agent` per [Wikipedia API etiquette](https://www.mediawiki.org/wiki/API:Etiquette) — at minimum a tool name, version, and contact (e.g. `PrimerSeedBuilder/0.1 (contact: <maintainer-email-or-repo-url>)`). The exact contact value is set at implementation time once the maintainer decides what to expose publicly.
- Sleep 100 ms between fetches (belt and braces against rate limiting; ≤50 articles × 100 ms = 5 s total wallclock — negligible).
- Sanity-check each fetched lead has ≥30 words; warn (not fail) on shorter results so a human can review whether the article was misnamed or has unusual structure.

### `auto_seed_if_empty` extension (Rust)

Currently in `src/crates/primer-kb-load/src/lib.rs`, `auto_seed_if_empty` discovers a single file `seed_passages.<pack>.jsonl`. Extend it to:

1. Walk the discovered seed directory.
2. Load every regular file matching `*.<pack>.jsonl` in lexicographic order.
3. Each call to `load_jsonl` is already idempotent on passage `id`; ordering does not affect final state.

The discovery search path (`$PRIMER_SEED_DIR` → `$XDG_DATA_HOME/primer/seed/` → walking up from `CARGO_MANIFEST_DIR` for `data/seed/`) does not change.

## Schema

The output JSONL exactly matches the existing `primer-kb-load` loader:

```json
{
  "id": "wiki-simple:en:photosynthesis",
  "source": "wiki-simple:en:photosynthesis",
  "license": "CC-BY-SA-3.0",
  "attribution": "'Photosynthesis' from Simple English Wikipedia (https://simple.wikipedia.org/wiki/Photosynthesis), licensed under CC-BY-SA-3.0",
  "text": "Photosynthesis is a process used by plants and other organisms...",
  "topics": ["wikipedia", "simple-english", "science", "photosynthesis"]
}
```

**ID namespacing:** `wiki-simple:en:<slug>`. Distinct from `seed:en:<slug>` so the two corpora can never collide on `id`. The `<slug>` is `slugify(title)` — see component spec.

**License:** Wikipedia content is dual-licensed CC-BY-SA-3.0 and GFDL. We declare `CC-BY-SA-3.0` per Wikipedia's preferred attribution convention.

**Attribution string format:** `"'<title>' from Simple English Wikipedia (<canonical_url>), licensed under CC-BY-SA-3.0"`. The title is the *original* Wikipedia title (preserved capitalisation), not the slug. The `canonical_url` is the value returned by the API's `inprop=url` query (definitive, redirect-resolved).

**Topics:** static list `["wikipedia", "simple-english", "science", <slug>]`. The `topics` field is metadata only — the BM25 + dense-vector retrieval engine reads `text`, not `topics`. Static topics are fine for MVP; richer topic extraction (e.g. from Wikipedia categories) is a follow-up.

## Testing strategy

**TDD discipline applies.** Tests first, watch them fail, implement to green, then commit each pure function. The Python codebase is small enough that we should never have an untested function.

### Python tests (pytest, `data/ingest/tests/`)

- `test_slugify.py`:
  - ASCII title round-trips
  - Spaces become hyphens
  - Punctuation stripped (`"E. coli"` → `"e-coli"`, `"AC/DC"` → `"ac-dc"` — *note: this is a generic slugify rule; "AC/DC" is not in scope as a science article and serves only as an edge-case example*)
  - Unicode preserved where applicable, NFC-normalised so `"é"` precomposed and decomposed map to the same slug
  - Trailing/leading hyphens trimmed
  - Empty input or all-non-alphanumeric input raises (we don't want silent ID collisions)
- `test_whitelist_parser.py`:
  - Comments (`#`) and blank lines skipped
  - Leading/trailing whitespace stripped per line
  - Order preserved
  - Duplicates raise (catch hand-edit mistakes)
- `test_passage_emit.py`:
  - `to_passage(canned_record)` produces the exact expected JSON dict, including the attribution string format
  - Edge cases: title with apostrophe, title with non-ASCII characters
- `test_pipeline.py`:
  - Full pipeline run on a 3-article fixture using a fake `http_client` that returns canned API responses
  - Output JSONL matches a checked-in expected file byte-for-byte (catches ordering, formatting drift)
  - Loud failure on a fixture that returns an empty extract

### Rust tests (extend `primer-kb-load`)

- New: two seed files in the discovered dir → both files' passages all load
- New: ordering of file discovery doesn't affect final state (idempotent)
- Existing single-file behaviour preserved (move existing test if needed; keep coverage)
- New: a non-locale-matching file in the seed dir (e.g. `wiki_passages.de.jsonl` when locale is English) is *not* loaded

### Integration / smoke

- Run `simple_wikipedia.py` against the live API on the real whitelist → verify ~30-50 records emitted to `data/seed/wiki_passages.en.jsonl`, sorted by id, deterministic on re-run.
- `cargo test -p primer-kb-load` passes including the existing 50+ canonical-query retrieval test (no regressions from the wiki layer).
- Add 5-10 new canonical queries to `retrieval_quality.rs` that the wiki layer should satisfy. Suggested probes for queries the seed corpus does *not* answer well (so the test demonstrably exercises the wiki addition):
  - "what is gravity"
  - "what is an atom"
  - "how does a computer work"
  - "what is a virus"
  - "what is energy"
  - "what is a number" / "how do we count"
  - "why is the sky blue" (currently only adjacent in seed via `light` and `rainbows`)
  - These are placeholders; final list assembled against the actual whitelist.
- Smoke run: `cargo run --bin primer -- --backend cloud --age 9 --no-persist --verbose` and ask a few questions whose answers are in the wiki corpus. Observe via verbose logs that wiki passages are being retrieved.

## Acceptance criteria

1. `python3 data/ingest/build_whitelist.py > /tmp/cand.txt` runs without errors against the live API and emits ≥100 candidate titles.
2. Hand-reviewed whitelist `data/ingest/simple_wikipedia_whitelist.txt` contains 30-50 entries appropriate for a children's product. Reviewed by Horst, not Claude.
3. `python3 data/ingest/simple_wikipedia.py` reads the whitelist, fetches articles, emits deterministic JSONL to `data/seed/wiki_passages.en.jsonl`. Each record has CC-BY-SA-3.0 license + URL-bearing attribution. Sorted by `id`.
4. `pytest data/ingest/tests/` passes (4 test files, ≥10 individual tests).
5. `cargo test -p primer-kb-load` passes including the new multi-file auto-seed tests + ≥5 new canonical retrieval queries hitting the wiki layer.
6. `cargo run --bin primer -- --age 9 --no-persist --verbose` (stub backend acceptable; cloud preferred for smoke) shows wiki passages being retrieved on relevant queries.
7. `README.md`, `ROADMAP.md`, `CLAUDE.md` updated to reflect Wikipedia ingestion shipped.
8. Whole branch is one logical commit per task (TDD discipline).

## Risks

- **MediaWiki API rate limit.** ~50 req/s with proper `User-Agent`. For ≤50 articles this is a non-issue; we add the User-Agent and 100 ms sleep regardless.
- **Article title drift.** A whitelisted title that gets renamed in Wikipedia returns no pages. The script raises on this so the developer notices and updates the whitelist. (Trade-off: the run aborts mid-list. Acceptable for MVP; a future iteration could collect failures and report at the end.)
- **Lead extraction quirks.** The `extracts` API generally returns clean plain text but template-heavy or math-heavy articles can produce odd artifacts. Sanity check (≥30 words, warn on shorter) catches the obvious cases; humans review suspicious entries.
- **License attribution drift.** Per CC-BY-SA-3.0 we must credit Wikipedia and link to the article. The `attribution` field carries this through the data layer; verifying that the runtime UX surfaces it appropriately is a future concern explicitly out of scope here.
- **Diff churn on regeneration.** Sorting by `id` and pinning `User-Agent` makes the JSONL reproducible. Real article-content changes will produce real diffs, which is the correct behaviour.
- **License compatibility with the project's AGPL-3.0.** AGPL is GPL-compatible and CC-BY-SA-3.0 content can be redistributed alongside AGPL code (different works, different licenses, attributed correctly). Each passage's `license` field declares CC-BY-SA-3.0 explicitly. No issue at the data layer.

## Iteration plan

This is explicitly a starting point. Anticipated learnings that may produce follow-up PRs:

- Lead-only chunking may miss too much detail on important articles → consider section-level chunking for the largest articles only.
- Static topic tags may be too coarse for downstream filtering → enrich with Wikipedia categories.
- A whitelist of 30-50 may be too thin to demonstrate retrieval improvement on diverse child queries → expand to ~150 (still hand-curated) or move to category-tree crawling.
- Per-article fetches may become tedious if we expand significantly → switch to XML dump processing.

None of these need to be solved up front. The design holds at ≤50 articles; we iterate when we hit a real wall.

## Open decisions

None. All major axes are pinned by this spec.
