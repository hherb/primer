# Wikipedia ingestion

One-time tooling that ingests Simple English Wikipedia Science articles into the Primer's hybrid knowledge base.

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

## Regenerate the whitelist (rare; only when expanding coverage)

```bash
python3 build_whitelist.py > /tmp/cand.txt
# Hand-review /tmp/cand.txt; trim to children's-curriculum-appropriate
# entries; save the trimmed list to simple_wikipedia_whitelist.txt.
```

## Re-run the ingest (rare; only when the whitelist changes or articles drift)

```bash
python3 simple_wikipedia.py
# Writes ../seed/wiki_passages.en.jsonl. Commit the diff.
```

## Schema

The output JSONL exactly matches `primer-kb-load`'s `SeedPassage` schema:

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

The canonical Wikipedia URL goes in the structured `source_url` field (carried through to the `sources` table); the human-readable `attribution` is the credit string per CC-BY-SA-3.0.

## Per-article licensing

Each passage declares `CC-BY-SA-3.0` per Wikipedia's preferred attribution convention. The Primer's overall code license (AGPL-3.0) is unaffected; the data layer carries each passage's license alongside its content.
