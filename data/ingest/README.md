# Wikipedia-shaped ingestion

One-time tooling that ingests children's-curriculum articles from
Wikipedia-shaped sources into the Primer's hybrid knowledge base.

Two sources ship today:

- **Simple English Wikipedia** (`--language en`, default) — uses the
  MediaWiki TextExtracts API for batched 20-titles-per-request lead
  fetches. Whitelist: `simple_wikipedia_whitelist.txt`. Output:
  `../seed/wiki_passages.en.jsonl`. License: CC-BY-SA-3.0.
- **Klexikon** (`--language de`) — German children's wiki at
  `klexikon.zum.de`, hand-written for ages 8-13. Klexikon's MediaWiki
  has no TextExtracts extension, so the pipeline uses
  `action=parse&prop=wikitext&section=0` (one HTTP call per article)
  and runs the result through an in-house wikitext stripper.
  Whitelist: `klexikon_whitelist.txt`. Output:
  `../seed/wiki_passages.de.jsonl`. License: CC-BY-SA-4.0.

Both sources share the same fetch / passage-emit / JSONL-write code
path, parameterised by a `WikiSource` frozen dataclass. Adding a new
source means: declare a `WikiSource` preset (with the right
`fetch_strategy`), hand-curate a whitelist, run the pipeline. See
[`wiki/source.py`](wiki/source.py) for the full presets.

## Submodule layout

`simple_wikipedia.py` is the CLI entry point and pipeline orchestrator
(`main`); the actual ingest code lives under `wiki/`:

- [`wiki/source.py`](wiki/source.py) — `WikiSource` dataclass, the
  `SIMPLE_ENGLISH` and `KLEXIKON` presets, slug helpers
  (`slugify`, `_assert_unique_slugs`, `_assert_unique_passage_ids`),
  whitelist parser (`read_whitelist`), passage emitter
  (`to_passage`).
- [`wiki/strip.py`](wiki/strip.py) — Klexikon wikitext → plain text
  (`strip_klexikon_wikitext` and helpers). Pure functions.
- [`wiki/fetch.py`](wiki/fetch.py) — HTTP fetch dispatch
  (`fetch_lead`, `fetch_leads`) and per-strategy fetchers, plus the
  retry-settings constant (`_RETRY_SETTINGS`).

`simple_wikipedia.py` re-exports every name imported by the existing
test suite, so `from simple_wikipedia import slugify` etc. keep
resolving. New code should import from the submodule directly
(`from wiki.fetch import fetch_lead`).

`wiki/__init__.py` deliberately exposes no re-exports — import from
the specific submodule (`from wiki.source import KLEXIKON`,
`from wiki.fetch import fetch_lead`), not from the package
(`from wiki import KLEXIKON` will fail).

## Prerequisites

- Internet access — the ingest scripts fetch from the live MediaWiki
  APIs. There is no offline mode.
- A populated whitelist: `simple_wikipedia_whitelist.txt` for English
  (use `build_whitelist.py` to seed candidates from Wikipedia's Vital
  Articles list, then trim by hand) or `klexikon_whitelist.txt` for
  German (hand-author since Klexikon is small and the relevant subset
  is even smaller).

## Setup

```bash
cd data/ingest
python3 -m venv .venv
source .venv/bin/activate
pip install -r requirements.txt
```

## Run the tests

```bash
pytest
```

## Regenerate the English whitelist (rare; only when expanding coverage)

```bash
python3 build_whitelist.py > /tmp/cand.txt
# Hand-review /tmp/cand.txt; trim to children's-curriculum-appropriate
# entries; save the trimmed list to simple_wikipedia_whitelist.txt.
```

## Re-run the ingest

```bash
# Simple English Wikipedia (default).
python3 simple_wikipedia.py --language en
# Writes ../seed/wiki_passages.en.jsonl. Commit the diff.

# Klexikon (German children's wiki).
python3 simple_wikipedia.py --language de
# Writes ../seed/wiki_passages.de.jsonl. Commit the diff.
```

Override the defaults:

```bash
python3 simple_wikipedia.py \
    --language de \
    --whitelist /path/to/custom_whitelist.txt \
    --output /path/to/custom_output.jsonl
```

## Schema

The output JSONL exactly matches `primer-kb-load`'s `SeedPassage`
schema. English (Simple English Wikipedia):

```json
{
  "id": "wiki-simple:en:photosynthesis",
  "source": "wiki-simple:en:photosynthesis",
  "license": "CC-BY-SA-3.0",
  "attribution": "'Photosynthesis' from Simple English Wikipedia, licensed under CC-BY-SA-3.0",
  "source_url": "https://simple.wikipedia.org/wiki/Photosynthesis",
  "text": "Photosynthesis is a process used by plants...",
  "topics": ["wikipedia", "simple-english", "science", "photosynthesis"]
}
```

German (Klexikon):

```json
{
  "id": "wiki-klexikon:de:klima",
  "source": "wiki-klexikon:de:klima",
  "license": "CC-BY-SA-4.0",
  "attribution": "'Klima' from Klexikon, licensed under CC-BY-SA-4.0",
  "source_url": "https://klexikon.zum.de/wiki/Klima",
  "text": "Wenn man vom Klima spricht, ist gemeint, dass es...",
  "topics": ["wikipedia", "klexikon", "klima"]
}
```

The canonical web URL goes in the structured `source_url` field
(carried through to the `sources` table); the human-readable
`attribution` is the credit string per the source's CC-BY-SA license.

## Per-article licensing

Each passage declares the license its source publishes under:
`CC-BY-SA-3.0` for Simple English Wikipedia (parity with the existing
Phase 0.2 corpus); `CC-BY-SA-4.0` for Klexikon (per the site's About
page and per-page footer). The Primer's overall code license
(AGPL-3.0) is unaffected; the data layer carries each passage's
license alongside its content.
