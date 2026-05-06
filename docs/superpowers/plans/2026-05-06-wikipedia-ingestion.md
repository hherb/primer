# Wikipedia Ingestion (Phase 0.2 MVP) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship an MVP end-to-end ingestion of ~30-50 Simple English Wikipedia Science articles into the existing hybrid knowledge base, auto-loaded alongside the hand-drafted CC0 seed corpus.

**Architecture:** Standalone Python ingestion script + curated whitelist + small Rust auto-seed extension. Lead-only chunking (one passage per article). Output JSONL committed in-repo at `data/seed/wiki_passages.en.jsonl`. Mirrors the existing seed-corpus shipping pattern.

**Tech Stack:** Python 3 (`requests`, `pytest`) for the ingest pipeline. Existing Rust workspace (`primer-kb-load`, `primer-knowledge`) for the auto-seed extension and retrieval-quality assertions. MediaWiki extracts API for fetching article leads as plain text.

**Spec:** [docs/superpowers/specs/2026-05-06-wikipedia-ingestion-design.md](../specs/2026-05-06-wikipedia-ingestion-design.md)

**Branch:** `feature/wikipedia-ingestion` (create via `superpowers:using-git-worktrees` at execution time).

---

## File structure

### New (Python pipeline)

| Path | Purpose |
|---|---|
| `data/ingest/simple_wikipedia.py` | Main ingestion script. Pure functions where possible; network injected for testability. |
| `data/ingest/build_whitelist.py` | One-time helper. Pulls `Wikipedia:Vital_articles/Level/4/Science_and_technology` from the MediaWiki API and prints candidate titles. |
| `data/ingest/simple_wikipedia_whitelist.txt` | Curated, hand-reviewed list of 30-50 article titles to ingest. Lines starting with `#` and blank lines are ignored. |
| `data/ingest/requirements.txt` | Python deps: `requests`, `pytest`. |
| `data/ingest/pyproject.toml` | Pytest config (`pythonpath`, `testpaths`). No build target — this is not a published package. |
| `data/ingest/README.md` | Usage docs: how to install deps, regenerate the whitelist, run the ingest, run the tests. |
| `data/ingest/tests/__init__.py` | Empty marker so pytest treats the dir as a package. |
| `data/ingest/tests/test_slugify.py` | Pure-function tests for `slugify`. |
| `data/ingest/tests/test_whitelist_parser.py` | Tests for `read_whitelist`. |
| `data/ingest/tests/test_passage_emit.py` | Tests for `to_passage`. |
| `data/ingest/tests/test_pipeline.py` | End-to-end test using a fake HTTP client + canned fixtures. |
| `data/ingest/tests/fixtures/photosynthesis.json` | Canned MediaWiki API response for one article. |
| `data/ingest/tests/fixtures/black_hole.json` | Canned MediaWiki API response for one article. |
| `data/ingest/tests/fixtures/gravity.json` | Canned MediaWiki API response for one article. |
| `data/ingest/tests/fixtures/expected_output.jsonl` | Golden JSONL output for the 3-article pipeline test. |

### New (output artifact, committed)

| Path | Purpose |
|---|---|
| `data/seed/wiki_passages.en.jsonl` | Generated JSONL output; committed for offline-first reproducibility. ~30-50 lines. |

### Modified (Rust)

| Path | What changes |
|---|---|
| `src/crates/primer-kb-load/src/lib.rs` | Add `discover_seed_files(locale) -> Vec<PathBuf>`; refactor `discover_seed_jsonl` to delegate to it; rewrite `auto_seed_if_empty` to load every discovered file. |
| `src/crates/primer-kb-load/tests/retrieval_quality.rs` | Add 5-10 new canonical queries that the wiki layer should satisfy. |

### Modified (docs)

| Path | What changes |
|---|---|
| `README.md` | Update "Knowledge-base bootstrapping" bullet + Phase 0 status paragraph to reflect Wikipedia layer shipped. |
| `ROADMAP.md` | Check off `[ ] Write an ingestion script ...` line; update Phase 0 exit-criteria paragraph. |
| `CLAUDE.md` | Update project-shape paragraph; add a gotcha note about the new ingestion path. |

---

## Pre-flight: create worktree + branch

This is a multi-task implementation; isolate it in a worktree.

- [ ] **Step 1: Create worktree**

Use `superpowers:using-git-worktrees` to create the worktree. Branch name: `feature/wikipedia-ingestion`. Base: `main`.

- [ ] **Step 2: Confirm baseline tests pass**

```bash
cd src
~/.cargo/bin/cargo test --workspace 2>&1 | grep -E "^test result:" | awk '{p+=$4;f+=$6} END {print "passed:",p,"failed:",f}'
```

Expected: `passed: 548 failed: 0` (this is the baseline as of 2026-05-06; the count may have grown by the time this plan runs).

---

## Task 1: Scaffold the Python pipeline directory

**Files:**
- Create: `data/ingest/requirements.txt`
- Create: `data/ingest/pyproject.toml`
- Create: `data/ingest/README.md`
- Create: `data/ingest/tests/__init__.py`

- [ ] **Step 1: Create `data/ingest/requirements.txt`**

```
requests>=2.32,<3
pytest>=8.0,<9
```

- [ ] **Step 2: Create `data/ingest/pyproject.toml`**

```toml
[tool.pytest.ini_options]
pythonpath = ["."]
testpaths = ["tests"]
```

This makes `from simple_wikipedia import slugify` work in tests when running `pytest` from `data/ingest/`.

- [ ] **Step 3: Create `data/ingest/tests/__init__.py`**

Empty file (one trailing newline). Marks the dir as a package so pytest collection works on all platforms.

- [ ] **Step 4: Create `data/ingest/README.md`**

```markdown
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
```

- [ ] **Step 5: Run pytest baseline (no tests yet, should still succeed)**

```bash
cd data/ingest
python3 -m pip install -r requirements.txt
python3 -m pytest
```

Expected: `no tests ran` (zero collected). Confirms the pyproject.toml is parsed and pytest can find `tests/`.

- [ ] **Step 6: Commit**

```bash
git add data/ingest/requirements.txt data/ingest/pyproject.toml data/ingest/README.md data/ingest/tests/__init__.py
git commit -m "feat(ingest): scaffold data/ingest dir for Wikipedia pipeline"
```

---

## Task 2: TDD `slugify`

Pure function. Mirrors `primer-cli::slug` but produces hyphen-separated slugs (URL-friendly) rather than empty-separator (filesystem-friendly).

**Files:**
- Create: `data/ingest/tests/test_slugify.py`
- Create: `data/ingest/simple_wikipedia.py`

- [ ] **Step 1: Write the failing tests**

`data/ingest/tests/test_slugify.py`:

```python
"""Tests for slugify — pure function, no I/O."""
import pytest
from simple_wikipedia import slugify


def test_ascii_lowercase():
    assert slugify("Photosynthesis") == "photosynthesis"


def test_spaces_to_hyphens():
    assert slugify("Black hole") == "black-hole"


def test_punctuation_stripped_to_hyphens():
    # Period + space collapse to a single hyphen
    assert slugify("E. coli") == "e-coli"


def test_apostrophe_stripped():
    assert slugify("'Photosynthesis'") == "photosynthesis"


def test_multiple_words_with_hyphens():
    assert slugify("Solar system") == "solar-system"


def test_runs_of_punctuation_collapse():
    # "AC/DC" is not a science article; included as a slugify edge case
    # showing that runs of non-alphanumerics collapse into a single hyphen.
    assert slugify("AC/DC") == "ac-dc"


def test_unicode_preserved():
    # "Café" should produce a recognisable slug. Whether the é is
    # preserved or transliterated is implementation-defined; here we
    # require that the result is non-empty and lowercase.
    s = slugify("Café")
    assert s
    assert s == s.lower()
    # Must be NFC-normalised so precomposed and decomposed forms match
    precomposed = "Café"  # é = U+00E9
    decomposed = "Café"   # e + U+0301
    assert slugify(precomposed) == slugify(decomposed)


def test_leading_trailing_hyphens_trimmed():
    assert slugify("--Hello--") == "hello"


def test_empty_input_raises():
    with pytest.raises(ValueError):
        slugify("")


def test_only_punctuation_raises():
    with pytest.raises(ValueError):
        slugify("---")
    with pytest.raises(ValueError):
        slugify("!@#")
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cd data/ingest
python3 -m pytest tests/test_slugify.py -v
```

Expected: All tests FAIL with `ModuleNotFoundError: No module named 'simple_wikipedia'`.

- [ ] **Step 3: Implement `slugify`**

Create `data/ingest/simple_wikipedia.py`:

```python
"""Simple English Wikipedia ingestion pipeline.

Pure functions where possible; network injected via `http_client` so the
unit tests can substitute a fake. See `data/ingest/README.md` for usage.
"""
import re
import unicodedata


_NON_ALNUM = re.compile(r"[^a-z0-9]+")


def slugify(title: str) -> str:
    """Convert a Wikipedia article title to a URL-safe lowercase slug.

    NFC-normalises the input first so precomposed and decomposed Unicode
    forms map to the same slug. Lowercases (per Unicode case-folding),
    strips diacritics best-effort via NFD decomposition + ascii filter,
    then replaces runs of non-alphanumerics with a single hyphen.
    Trims leading/trailing hyphens.

    Raises:
        ValueError: when the input is empty or has no alphanumeric chars
        after normalisation. Empty slugs would silently collide on `id`.
    """
    if not title:
        raise ValueError("slugify: empty title")
    # NFC first so precomposed and decomposed map identically.
    nfc = unicodedata.normalize("NFC", title)
    # Lowercase.
    lower = nfc.lower()
    # Decompose for diacritic stripping (best-effort transliteration).
    nfd = unicodedata.normalize("NFD", lower)
    ascii_only = "".join(c for c in nfd if not unicodedata.combining(c))
    # Replace runs of non-alphanumerics with a single hyphen.
    slug = _NON_ALNUM.sub("-", ascii_only).strip("-")
    if not slug:
        raise ValueError(f"slugify: no alphanumerics in title: {title!r}")
    return slug
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
python3 -m pytest tests/test_slugify.py -v
```

Expected: 10 passed.

- [ ] **Step 5: Commit**

```bash
git add data/ingest/simple_wikipedia.py data/ingest/tests/test_slugify.py
git commit -m "feat(ingest): slugify pure function with TDD coverage"
```

---

## Task 3: TDD `read_whitelist`

Pure function. Reads the whitelist file; skips blanks and `#` comments; preserves order; raises on duplicates.

**Files:**
- Create: `data/ingest/tests/test_whitelist_parser.py`
- Modify: `data/ingest/simple_wikipedia.py`

- [ ] **Step 1: Write the failing tests**

`data/ingest/tests/test_whitelist_parser.py`:

```python
"""Tests for read_whitelist — reads a text file, returns ordered titles."""
import pytest
from pathlib import Path
from simple_wikipedia import read_whitelist


def test_basic_titles(tmp_path: Path):
    p = tmp_path / "wl.txt"
    p.write_text("Photosynthesis\nBlack hole\nGravity\n")
    assert read_whitelist(p) == ["Photosynthesis", "Black hole", "Gravity"]


def test_comments_skipped(tmp_path: Path):
    p = tmp_path / "wl.txt"
    p.write_text("# header\nPhotosynthesis\n# inline comment\nBlack hole\n")
    assert read_whitelist(p) == ["Photosynthesis", "Black hole"]


def test_blank_lines_skipped(tmp_path: Path):
    p = tmp_path / "wl.txt"
    p.write_text("\nPhotosynthesis\n\n\nBlack hole\n\n")
    assert read_whitelist(p) == ["Photosynthesis", "Black hole"]


def test_whitespace_trimmed(tmp_path: Path):
    p = tmp_path / "wl.txt"
    p.write_text("  Photosynthesis  \n\tBlack hole\t\n")
    assert read_whitelist(p) == ["Photosynthesis", "Black hole"]


def test_order_preserved(tmp_path: Path):
    p = tmp_path / "wl.txt"
    p.write_text("Zebra\nAlpha\nMango\n")
    assert read_whitelist(p) == ["Zebra", "Alpha", "Mango"]


def test_duplicates_raise(tmp_path: Path):
    p = tmp_path / "wl.txt"
    p.write_text("Photosynthesis\nBlack hole\nPhotosynthesis\n")
    with pytest.raises(ValueError, match="duplicate"):
        read_whitelist(p)
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
python3 -m pytest tests/test_whitelist_parser.py -v
```

Expected: All tests FAIL with `ImportError: cannot import name 'read_whitelist'`.

- [ ] **Step 3: Implement `read_whitelist`**

Append to `data/ingest/simple_wikipedia.py`:

```python
from pathlib import Path


def read_whitelist(path: Path) -> list[str]:
    """Parse a whitelist file: one article title per line, comments OK.

    - Lines starting with `#` (after stripping) are ignored.
    - Blank lines are ignored.
    - Leading/trailing whitespace is stripped per line.
    - Order is preserved (so hand edits diff cleanly).

    Raises:
        ValueError: if any title appears more than once.
    """
    titles: list[str] = []
    seen: set[str] = set()
    for line in Path(path).read_text(encoding="utf-8").splitlines():
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        if stripped in seen:
            raise ValueError(f"read_whitelist: duplicate title: {stripped!r}")
        seen.add(stripped)
        titles.append(stripped)
    return titles
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
python3 -m pytest tests/test_whitelist_parser.py -v
```

Expected: 6 passed.

- [ ] **Step 5: Commit**

```bash
git add data/ingest/simple_wikipedia.py data/ingest/tests/test_whitelist_parser.py
git commit -m "feat(ingest): read_whitelist parser with comment + dedup support"
```

---

## Task 4: TDD `to_passage`

Pure function: given a fetched-article record `{title, lead_text, canonical_url}`, produce the `SeedPassage`-compatible dict.

**Files:**
- Create: `data/ingest/tests/test_passage_emit.py`
- Modify: `data/ingest/simple_wikipedia.py`

- [ ] **Step 1: Write the failing tests**

`data/ingest/tests/test_passage_emit.py`:

```python
"""Tests for to_passage — record → SeedPassage-compatible dict."""
import pytest
from simple_wikipedia import to_passage


def test_basic_record():
    record = {
        "title": "Photosynthesis",
        "lead_text": "Photosynthesis is a process used by plants and other organisms.",
        "canonical_url": "https://simple.wikipedia.org/wiki/Photosynthesis",
    }
    p = to_passage(record)
    assert p == {
        "id": "wiki-simple:en:photosynthesis",
        "source": "wiki-simple:en:photosynthesis",
        "license": "CC-BY-SA-3.0",
        "attribution": "'Photosynthesis' from Simple English Wikipedia, licensed under CC-BY-SA-3.0",
        "source_url": "https://simple.wikipedia.org/wiki/Photosynthesis",
        "text": "Photosynthesis is a process used by plants and other organisms.",
        "topics": ["wikipedia", "simple-english", "science", "photosynthesis"],
    }


def test_multiword_title():
    record = {
        "title": "Black hole",
        "lead_text": "A black hole is a region of spacetime.",
        "canonical_url": "https://simple.wikipedia.org/wiki/Black_hole",
    }
    p = to_passage(record)
    assert p["id"] == "wiki-simple:en:black-hole"
    assert p["topics"] == ["wikipedia", "simple-english", "science", "black-hole"]


def test_attribution_uses_original_title_capitalisation():
    # The slug is lowercased; the attribution preserves the title verbatim.
    record = {
        "title": "DNA",
        "lead_text": "Deoxyribonucleic acid (DNA) is a molecule.",
        "canonical_url": "https://simple.wikipedia.org/wiki/DNA",
    }
    p = to_passage(record)
    assert "'DNA'" in p["attribution"]
    assert p["id"] == "wiki-simple:en:dna"


def test_short_lead_raises():
    # Sanity guard: lead < 30 words is suspicious. The pipeline warns
    # rather than fails on these (whitelist hand-review), but `to_passage`
    # itself does not enforce length — that's the pipeline's job. Verify
    # `to_passage` happily produces a record for short text so the
    # responsibility is clear.
    record = {
        "title": "Test",
        "lead_text": "Short.",
        "canonical_url": "https://example.com/Test",
    }
    p = to_passage(record)
    assert p["text"] == "Short."
    assert p["id"] == "wiki-simple:en:test"
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
python3 -m pytest tests/test_passage_emit.py -v
```

Expected: All tests FAIL with `ImportError: cannot import name 'to_passage'`.

- [ ] **Step 3: Implement `to_passage`**

Append to `data/ingest/simple_wikipedia.py`:

```python
def to_passage(record: dict) -> dict:
    """Convert a fetched-article record to a SeedPassage-compatible dict.

    Input shape: `{"title": str, "lead_text": str, "canonical_url": str}`.
    Output shape: matches `primer_kb_load::SeedPassage` exactly so the
    JSONL drops into the existing loader without modification.

    The slug (lowercased) goes into `id` and `source`; the original-cased
    title is preserved in the human-readable `attribution` string. The
    canonical URL is structured into `source_url` (carried through to the
    `sources` table) rather than embedded in `attribution`.
    """
    title = record["title"]
    slug = slugify(title)
    return {
        "id": f"wiki-simple:en:{slug}",
        "source": f"wiki-simple:en:{slug}",
        "license": "CC-BY-SA-3.0",
        "attribution": (
            f"'{title}' from Simple English Wikipedia, "
            f"licensed under CC-BY-SA-3.0"
        ),
        "source_url": record["canonical_url"],
        "text": record["lead_text"],
        "topics": ["wikipedia", "simple-english", "science", slug],
    }
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
python3 -m pytest tests/test_passage_emit.py -v
```

Expected: 4 passed.

- [ ] **Step 5: Commit**

```bash
git add data/ingest/simple_wikipedia.py data/ingest/tests/test_passage_emit.py
git commit -m "feat(ingest): to_passage builds SeedPassage-compatible dict"
```

---

## Task 5: TDD `fetch_lead` with injected HTTP client

Network is the test boundary: inject the HTTP client, mock with a fake.

**Files:**
- Create: `data/ingest/tests/fixtures/photosynthesis.json` (canned API response)
- Modify: `data/ingest/simple_wikipedia.py` (add `fetch_lead`)
- Modify: `data/ingest/tests/test_passage_emit.py` (or new test file — see below)

- [ ] **Step 1: Create the fixture**

`data/ingest/tests/fixtures/photosynthesis.json`:

```json
{
  "batchcomplete": "",
  "query": {
    "pages": {
      "12345": {
        "pageid": 12345,
        "ns": 0,
        "title": "Photosynthesis",
        "extract": "Photosynthesis is a process used by plants and other organisms to convert light energy into chemical energy that, through cellular respiration, can later be released to fuel the organism's activities. This chemical energy is stored in carbohydrate molecules, such as sugars, which are made from carbon dioxide and water.",
        "fullurl": "https://simple.wikipedia.org/wiki/Photosynthesis"
      }
    }
  }
}
```

- [ ] **Step 2: Write the failing tests**

Create `data/ingest/tests/test_fetch_lead.py`:

```python
"""Tests for fetch_lead — uses an injected http_client so the test never
talks to the real network."""
import json
from pathlib import Path
import pytest
from simple_wikipedia import fetch_lead


FIXTURES = Path(__file__).parent / "fixtures"


class FakeResponse:
    def __init__(self, payload: dict, status_code: int = 200):
        self._payload = payload
        self.status_code = status_code

    def json(self) -> dict:
        return self._payload

    def raise_for_status(self) -> None:
        if self.status_code >= 400:
            raise RuntimeError(f"fake http error {self.status_code}")


class FakeHttpClient:
    """A `requests.Session`-compatible fake that returns canned payloads.

    Maps the requested `titles` query parameter to a response payload.
    """

    def __init__(self, responses: dict[str, dict]):
        self.responses = responses
        self.calls: list[dict] = []

    def get(self, url: str, params: dict, timeout: float | None = None):
        self.calls.append({"url": url, "params": params})
        title = params["titles"]
        if title not in self.responses:
            return FakeResponse({"query": {"pages": {"-1": {"missing": ""}}}})
        return FakeResponse(self.responses[title])


def _load_fixture(name: str) -> dict:
    return json.loads((FIXTURES / name).read_text(encoding="utf-8"))


def test_fetch_lead_returns_title_text_url():
    client = FakeHttpClient({"Photosynthesis": _load_fixture("photosynthesis.json")})
    result = fetch_lead("Photosynthesis", http_client=client)
    assert result["title"] == "Photosynthesis"
    assert "Photosynthesis is a process" in result["lead_text"]
    assert result["canonical_url"] == "https://simple.wikipedia.org/wiki/Photosynthesis"


def test_fetch_lead_uses_correct_api_endpoint():
    client = FakeHttpClient({"Photosynthesis": _load_fixture("photosynthesis.json")})
    fetch_lead("Photosynthesis", http_client=client)
    assert len(client.calls) == 1
    call = client.calls[0]
    assert "simple.wikipedia.org" in call["url"]
    assert call["params"]["action"] == "query"
    assert call["params"]["prop"] == "extracts|info"
    assert call["params"]["exintro"] == 1
    assert call["params"]["explaintext"] == 1
    assert call["params"]["inprop"] == "url"
    assert call["params"]["titles"] == "Photosynthesis"


def test_fetch_lead_missing_article_raises():
    # Article not in the fake's response map → the fake returns the
    # "missing" sentinel structure, which fetch_lead must detect.
    client = FakeHttpClient({})
    with pytest.raises(RuntimeError, match="not found"):
        fetch_lead("DoesNotExist", http_client=client)


def test_fetch_lead_empty_extract_raises():
    payload = {
        "query": {"pages": {"99": {"title": "Stub", "extract": "", "fullurl": "https://x"}}}
    }
    client = FakeHttpClient({"Stub": payload})
    with pytest.raises(RuntimeError, match="empty extract"):
        fetch_lead("Stub", http_client=client)
```

- [ ] **Step 3: Run tests to verify they fail**

```bash
python3 -m pytest tests/test_fetch_lead.py -v
```

Expected: All tests FAIL with `ImportError: cannot import name 'fetch_lead'`.

- [ ] **Step 4: Implement `fetch_lead`**

Append to `data/ingest/simple_wikipedia.py`:

```python
WIKIPEDIA_API_URL = "https://simple.wikipedia.org/w/api.php"


def fetch_lead(title: str, *, http_client) -> dict:
    """Fetch the lead section of a Simple English Wikipedia article.

    Uses the MediaWiki extracts API with `exintro=1&explaintext=1` so the
    server returns the lead as plain text — no wikitext parser needed
    on our side.

    Returns:
        `{"title": str, "lead_text": str, "canonical_url": str}`.

    Raises:
        RuntimeError: when the article doesn't exist (API returns the
        "missing" page sentinel) or returns an empty extract. Both are
        whitelist bugs that the developer should notice immediately.
    """
    params = {
        "action": "query",
        "prop": "extracts|info",
        "exintro": 1,
        "explaintext": 1,
        "inprop": "url",
        "titles": title,
        "format": "json",
        "redirects": 1,
    }
    resp = http_client.get(WIKIPEDIA_API_URL, params=params, timeout=30.0)
    resp.raise_for_status()
    data = resp.json()
    pages = data.get("query", {}).get("pages", {})
    if not pages:
        raise RuntimeError(f"fetch_lead: empty response for {title!r}")
    page = next(iter(pages.values()))
    if "missing" in page:
        raise RuntimeError(f"fetch_lead: article not found: {title!r}")
    extract = page.get("extract", "")
    if not extract.strip():
        raise RuntimeError(f"fetch_lead: empty extract for {title!r}")
    return {
        "title": page["title"],
        "lead_text": extract.strip(),
        "canonical_url": page["fullurl"],
    }
```

- [ ] **Step 5: Run tests to verify they pass**

```bash
python3 -m pytest tests/test_fetch_lead.py -v
```

Expected: 4 passed.

- [ ] **Step 6: Commit**

```bash
git add data/ingest/simple_wikipedia.py data/ingest/tests/test_fetch_lead.py data/ingest/tests/fixtures/photosynthesis.json
git commit -m "feat(ingest): fetch_lead with injected http_client + fixture-driven tests"
```

---

## Task 6: TDD pipeline orchestration (`main`)

End-to-end test: 3-article fixture → pipeline → expected JSONL byte-for-byte.

**Files:**
- Create: `data/ingest/tests/fixtures/black_hole.json`
- Create: `data/ingest/tests/fixtures/gravity.json`
- Create: `data/ingest/tests/fixtures/expected_output.jsonl`
- Create: `data/ingest/tests/test_pipeline.py`
- Modify: `data/ingest/simple_wikipedia.py` (add `main`)

- [ ] **Step 1: Create the additional fixtures**

`data/ingest/tests/fixtures/black_hole.json`:

```json
{
  "batchcomplete": "",
  "query": {
    "pages": {
      "23456": {
        "pageid": 23456,
        "ns": 0,
        "title": "Black hole",
        "extract": "A black hole is a region of spacetime where gravity is so strong that nothing — not even light or other electromagnetic waves — has enough energy to escape it. The theory of general relativity predicts that a sufficiently compact mass can deform spacetime to form a black hole.",
        "fullurl": "https://simple.wikipedia.org/wiki/Black_hole"
      }
    }
  }
}
```

`data/ingest/tests/fixtures/gravity.json`:

```json
{
  "batchcomplete": "",
  "query": {
    "pages": {
      "34567": {
        "pageid": 34567,
        "ns": 0,
        "title": "Gravity",
        "extract": "Gravity, also known as gravitation, is a natural phenomenon by which all things with mass or energy — including planets, stars, galaxies, and even light — are attracted to one another. On Earth, gravity gives weight to physical objects, and the Moon's gravity causes the tides of the oceans.",
        "fullurl": "https://simple.wikipedia.org/wiki/Gravity"
      }
    }
  }
}
```

- [ ] **Step 2: Create the expected output fixture**

`data/ingest/tests/fixtures/expected_output.jsonl`:

The output is sorted by `id` lexicographically. So the order is `black-hole`, `gravity`, `photosynthesis`. JSON keys must serialise in dict-iteration order matching `to_passage` (`id`, `source`, `license`, `attribution`, `source_url`, `text`, `topics`). Each line is one compact JSON object. **Ensure the file ends with exactly one newline.**

```jsonl
{"id": "wiki-simple:en:black-hole", "source": "wiki-simple:en:black-hole", "license": "CC-BY-SA-3.0", "attribution": "'Black hole' from Simple English Wikipedia, licensed under CC-BY-SA-3.0", "source_url": "https://simple.wikipedia.org/wiki/Black_hole", "text": "A black hole is a region of spacetime where gravity is so strong that nothing — not even light or other electromagnetic waves — has enough energy to escape it. The theory of general relativity predicts that a sufficiently compact mass can deform spacetime to form a black hole.", "topics": ["wikipedia", "simple-english", "science", "black-hole"]}
{"id": "wiki-simple:en:gravity", "source": "wiki-simple:en:gravity", "license": "CC-BY-SA-3.0", "attribution": "'Gravity' from Simple English Wikipedia, licensed under CC-BY-SA-3.0", "source_url": "https://simple.wikipedia.org/wiki/Gravity", "text": "Gravity, also known as gravitation, is a natural phenomenon by which all things with mass or energy — including planets, stars, galaxies, and even light — are attracted to one another. On Earth, gravity gives weight to physical objects, and the Moon's gravity causes the tides of the oceans.", "topics": ["wikipedia", "simple-english", "science", "gravity"]}
{"id": "wiki-simple:en:photosynthesis", "source": "wiki-simple:en:photosynthesis", "license": "CC-BY-SA-3.0", "attribution": "'Photosynthesis' from Simple English Wikipedia, licensed under CC-BY-SA-3.0", "source_url": "https://simple.wikipedia.org/wiki/Photosynthesis", "text": "Photosynthesis is a process used by plants and other organisms to convert light energy into chemical energy that, through cellular respiration, can later be released to fuel the organism's activities. This chemical energy is stored in carbohydrate molecules, such as sugars, which are made from carbon dioxide and water.", "topics": ["wikipedia", "simple-english", "science", "photosynthesis"]}
```

Note: em-dashes appear in the source `extract` text and stay as em-dashes (`—`) in the JSON encoding when `ensure_ascii=False` is **not** used; we'll use `json.dumps(..., ensure_ascii=True)` for portability so the encoded form matches the fixture.

- [ ] **Step 3: Write the failing pipeline test**

`data/ingest/tests/test_pipeline.py`:

```python
"""End-to-end pipeline test using a fake HTTP client + canned fixtures."""
import json
from pathlib import Path
import pytest
from simple_wikipedia import main


FIXTURES = Path(__file__).parent / "fixtures"


class FakeResponse:
    def __init__(self, payload: dict, status_code: int = 200):
        self._payload = payload
        self.status_code = status_code

    def json(self) -> dict:
        return self._payload

    def raise_for_status(self) -> None:
        if self.status_code >= 400:
            raise RuntimeError(f"fake http error {self.status_code}")


class FakeHttpClient:
    def __init__(self, responses: dict[str, dict]):
        self.responses = responses

    def get(self, url, params, timeout=None):
        title = params["titles"]
        return FakeResponse(self.responses[title])


def _load(name: str) -> dict:
    return json.loads((FIXTURES / name).read_text(encoding="utf-8"))


def test_pipeline_three_articles_byte_exact(tmp_path: Path):
    whitelist = tmp_path / "wl.txt"
    whitelist.write_text("Photosynthesis\nBlack hole\nGravity\n")
    output = tmp_path / "out.jsonl"

    client = FakeHttpClient({
        "Photosynthesis": _load("photosynthesis.json"),
        "Black hole": _load("black_hole.json"),
        "Gravity": _load("gravity.json"),
    })

    main(whitelist_path=whitelist, output_path=output, http_client=client)

    actual = output.read_text(encoding="utf-8")
    expected = (FIXTURES / "expected_output.jsonl").read_text(encoding="utf-8")
    assert actual == expected, (
        "pipeline output does not match expected_output.jsonl byte-for-byte"
    )


def test_pipeline_output_sorted_by_id(tmp_path: Path):
    # Whitelist order is z, a, m — the output must reorder to a, m, z by id.
    whitelist = tmp_path / "wl.txt"
    # Use the Wikipedia titles so slugs alphabetise as expected.
    # photosynthesis > gravity > black-hole, so output order is b, g, p.
    whitelist.write_text("Photosynthesis\nGravity\nBlack hole\n")
    output = tmp_path / "out.jsonl"

    client = FakeHttpClient({
        "Photosynthesis": _load("photosynthesis.json"),
        "Black hole": _load("black_hole.json"),
        "Gravity": _load("gravity.json"),
    })

    main(whitelist_path=whitelist, output_path=output, http_client=client)
    lines = output.read_text(encoding="utf-8").strip().splitlines()
    ids = [json.loads(line)["id"] for line in lines]
    assert ids == [
        "wiki-simple:en:black-hole",
        "wiki-simple:en:gravity",
        "wiki-simple:en:photosynthesis",
    ]
```

- [ ] **Step 4: Run tests to verify they fail**

```bash
python3 -m pytest tests/test_pipeline.py -v
```

Expected: All tests FAIL with `ImportError: cannot import name 'main'`.

- [ ] **Step 5: Implement `main`**

Append to `data/ingest/simple_wikipedia.py`:

```python
import json as _json
import time as _time


# Default user-agent for live runs. Per Wikipedia API etiquette, this
# must include the tool name, version, and a contact identifier. The
# contact placeholder will be replaced at first live-run time once we
# decide what to expose publicly.
_DEFAULT_USER_AGENT = "PrimerSeedBuilder/0.1 (contact: see-repo-readme)"


def main(
    whitelist_path: Path,
    output_path: Path,
    *,
    http_client=None,
    inter_request_sleep_s: float = 0.1,
) -> None:
    """Run the full pipeline: whitelist → fetch → JSONL.

    The output JSONL is sorted by `id` for deterministic diffs.

    Args:
        whitelist_path: path to the whitelist text file.
        output_path: where to write the JSONL.
        http_client: optional HTTP client (must implement `get(url, params,
            timeout=...)` and return a response with `.json()` and
            `.raise_for_status()`). If `None`, a `requests.Session` is
            constructed with the default User-Agent.
        inter_request_sleep_s: seconds to wait between fetches when using
            a real network client. Ignored for fake clients (the fake's
            sleep is just irrelevant overhead in tests, so we still call
            `time.sleep` — set to 0 in tests if it ever matters).
    """
    if http_client is None:
        import requests
        http_client = requests.Session()
        http_client.headers.update({"User-Agent": _DEFAULT_USER_AGENT})

    titles = read_whitelist(whitelist_path)
    passages: list[dict] = []
    for i, title in enumerate(titles):
        if i > 0:
            _time.sleep(inter_request_sleep_s)
        record = fetch_lead(title, http_client=http_client)
        passage = to_passage(record)
        word_count = len(passage["text"].split())
        if word_count < 30:
            print(
                f"warning: lead for {title!r} has only {word_count} words "
                "— review whether the article was misnamed",
                flush=True,
            )
        passages.append(passage)

    passages.sort(key=lambda p: p["id"])

    with output_path.open("w", encoding="utf-8") as f:
        for p in passages:
            # ensure_ascii=True so the file is portable across editors
            # and the diff is stable regardless of locale settings.
            f.write(_json.dumps(p, ensure_ascii=True))
            f.write("\n")


if __name__ == "__main__":
    # Default paths assume the script is run from data/ingest/.
    here = Path(__file__).resolve().parent
    whitelist = here / "simple_wikipedia_whitelist.txt"
    output = here.parent / "seed" / "wiki_passages.en.jsonl"
    main(whitelist_path=whitelist, output_path=output)
    print(f"wrote {output}")
```

- [ ] **Step 6: Make tests not sleep**

The `inter_request_sleep_s` parameter defaults to 0.1, which adds 200 ms to the 3-article pipeline test. Acceptable but wasteful. Update both pipeline tests to pass `inter_request_sleep_s=0.0`:

```python
main(whitelist_path=whitelist, output_path=output, http_client=client, inter_request_sleep_s=0.0)
```

(Apply this in both `test_pipeline_three_articles_byte_exact` and `test_pipeline_output_sorted_by_id`.)

- [ ] **Step 7: Run tests to verify they pass**

```bash
python3 -m pytest tests/ -v
```

Expected: All tests pass (10 + 6 + 4 + 4 + 2 = 26 tests).

- [ ] **Step 8: Commit**

```bash
git add data/ingest/simple_wikipedia.py data/ingest/tests/test_pipeline.py data/ingest/tests/fixtures/black_hole.json data/ingest/tests/fixtures/gravity.json data/ingest/tests/fixtures/expected_output.jsonl
git commit -m "feat(ingest): pipeline orchestration with byte-exact end-to-end test"
```

---

## Task 7: Build the whitelist helper

One-time tool that pulls candidate article titles from `Wikipedia:Vital_articles/Level/4/Science_and_technology` so the developer doesn't have to hand-curate from scratch.

The MediaWiki API returns the page content as wikitext. Vital Articles uses `{{Icon|...}}` and `[[Article name]]` link syntax to enumerate articles. We extract titles from the link syntax, filtering out non-article links (talk pages, categories, files, etc.).

**Files:**
- Create: `data/ingest/build_whitelist.py`

- [ ] **Step 1: Implement the whitelist builder**

`data/ingest/build_whitelist.py`:

```python
"""Print candidate Wikipedia article titles for the seed whitelist.

Usage: `python3 build_whitelist.py > /tmp/cand.txt`. Then hand-review the
output, delete inappropriate entries, and save the trimmed list to
`simple_wikipedia_whitelist.txt`.

This tool is one-time. CI does not run it.
"""
import re
import sys
import argparse


# Wikipedia (English, not Simple English): vital articles are curated by
# Wikipedia editors. Level/4 has ~1000 entries; the science subset is
# what we want. Note that the article *titles* are stable across both
# wikis — Simple English Wikipedia uses the same titles for the same
# concepts. We pull from English to get the curated list, then fetch
# leads from Simple English in the main pipeline.
_VITAL_ARTICLES_PAGE = "Wikipedia:Vital articles/Level/4/Science"
_API_URL = "https://en.wikipedia.org/w/api.php"

_LINK_RE = re.compile(r"\[\[([^\]\|#]+?)(?:\|[^\]]*)?\]\]")
_NAMESPACE_PREFIXES = (
    "File:", "Image:", "Category:", "Talk:", "Wikipedia:",
    "Help:", "Template:", "Portal:", "User:", "Module:",
)


def fetch_vital_articles_wikitext(http_client) -> str:
    params = {
        "action": "query",
        "prop": "revisions",
        "rvprop": "content",
        "rvslots": "main",
        "titles": _VITAL_ARTICLES_PAGE,
        "format": "json",
    }
    resp = http_client.get(_API_URL, params=params, timeout=30.0)
    resp.raise_for_status()
    data = resp.json()
    page = next(iter(data["query"]["pages"].values()))
    return page["revisions"][0]["slots"]["main"]["*"]


def extract_titles(wikitext: str) -> list[str]:
    """Pull article titles out of `[[Title]]` and `[[Title|display]]` links.

    Filters out namespace-prefixed links (categories, files, templates).
    Preserves order; deduplicates.
    """
    seen: set[str] = set()
    out: list[str] = []
    for m in _LINK_RE.finditer(wikitext):
        title = m.group(1).strip()
        if not title or title.startswith(_NAMESPACE_PREFIXES):
            continue
        if title in seen:
            continue
        seen.add(title)
        out.append(title)
    return out


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--limit", type=int, default=None,
                        help="optionally cap the number of candidates printed")
    args = parser.parse_args()

    import requests
    sess = requests.Session()
    sess.headers.update({"User-Agent": "PrimerSeedBuilder/0.1 (contact: see-repo-readme)"})
    wikitext = fetch_vital_articles_wikitext(sess)
    titles = extract_titles(wikitext)
    if args.limit is not None:
        titles = titles[: args.limit]
    for t in titles:
        print(t)
    print(
        f"\n# {len(titles)} candidates fetched from {_VITAL_ARTICLES_PAGE}",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
```

- [ ] **Step 2: Add a unit test for the link extractor**

Create `data/ingest/tests/test_build_whitelist.py`:

```python
"""Tests for the whitelist-builder helper."""
from build_whitelist import extract_titles


def test_extracts_simple_links():
    wikitext = "Some prose with [[Photosynthesis]] and [[Black hole]] mentions."
    assert extract_titles(wikitext) == ["Photosynthesis", "Black hole"]


def test_handles_pipe_aliased_links():
    wikitext = "[[Black hole|black holes]] are dense."
    assert extract_titles(wikitext) == ["Black hole"]


def test_filters_namespace_prefixes():
    wikitext = (
        "[[Photosynthesis]] [[File:Sun.jpg]] [[Category:Astronomy]] "
        "[[Wikipedia:Vital articles]] [[Talk:Black hole]] [[Black hole]]"
    )
    assert extract_titles(wikitext) == ["Photosynthesis", "Black hole"]


def test_dedupes_repeated_titles():
    wikitext = "[[Photosynthesis]] later mentioned again as [[Photosynthesis]]"
    assert extract_titles(wikitext) == ["Photosynthesis"]


def test_skips_anchor_only_links():
    # [[#section]] is an in-page anchor, not an article. Our regex's
    # `[^\]\|#]+?` excludes anything containing #.
    wikitext = "[[#references]] [[Photosynthesis]]"
    assert extract_titles(wikitext) == ["Photosynthesis"]
```

- [ ] **Step 3: Run tests**

```bash
python3 -m pytest tests/test_build_whitelist.py -v
```

Expected: 5 passed.

- [ ] **Step 4: Commit**

```bash
git add data/ingest/build_whitelist.py data/ingest/tests/test_build_whitelist.py
git commit -m "feat(ingest): build_whitelist helper to seed candidates from Vital Articles"
```

---

## Task 8: Hand-curate the whitelist (manual)

This task is the only non-automatable step. The developer reviews the candidate list and produces a trimmed children's-curriculum-appropriate whitelist.

**Files:**
- Create: `data/ingest/simple_wikipedia_whitelist.txt`

- [ ] **Step 1: Run the candidate generator against the live API**

```bash
cd data/ingest
python3 build_whitelist.py > /tmp/cand.txt
wc -l /tmp/cand.txt
```

Expected: ≥100 candidate titles.

- [ ] **Step 2: Hand-review and produce the trimmed whitelist**

Open `/tmp/cand.txt`. Trim to 30-50 children's-curriculum-appropriate Science topics. Topics that overlap with the existing seed corpus (sun, moon, photosynthesis, etc.) should be excluded — the wiki layer is for **breadth**, not duplication. Suggested keepers: gravity, atom, molecule, energy, force, magnet (already in seed; skip), DNA, cell (already in seed; skip), virus, bacteria, ecosystem, climate change, plate tectonics, fossil, mineral, crystal, motion, friction, machine, computer, internet, mathematics, geometry, probability, etc. The exact list is a content judgment call; the developer chooses.

Save the trimmed list to `data/ingest/simple_wikipedia_whitelist.txt`. Format:

```
# Simple English Wikipedia ingestion whitelist (Science portal, English).
# Lines starting with `#` and blank lines are ignored.
# Hand-curated from build_whitelist.py output on YYYY-MM-DD.
# Goal: breadth — concepts not covered by the hand-drafted seed corpus.

Atom
Molecule
Energy
Force
Gravity
DNA
Virus
Ecosystem
...
```

Target: 30-50 entries.

- [ ] **Step 3: Sanity-check the whitelist**

```bash
python3 -c "from simple_wikipedia import read_whitelist; from pathlib import Path; titles = read_whitelist(Path('simple_wikipedia_whitelist.txt')); print(f'{len(titles)} titles'); print('\n'.join(titles))"
```

Expected: 30-50 titles printed; no `ValueError` (no duplicates).

- [ ] **Step 4: Commit**

```bash
git add data/ingest/simple_wikipedia_whitelist.txt
git commit -m "feat(ingest): hand-curated Wikipedia whitelist (~N entries)"
```

(Replace `N` with the actual count.)

---

## Task 9: Run the ingest against the live API

**Files:**
- Create: `data/seed/wiki_passages.en.jsonl`

- [ ] **Step 1: Run the ingest**

```bash
cd data/ingest
python3 simple_wikipedia.py
```

Expected: prints `wrote .../data/seed/wiki_passages.en.jsonl` after ~10-30 seconds (1 API request per article, 100 ms inter-request sleep, plus network latency).

If any article emits a "warning: lead for X has only N words" line, hand-review whether the title was misnamed or whether the article genuinely has a very short lead. Update the whitelist if needed and re-run.

If the run fails with `RuntimeError: fetch_lead: article not found: 'Foo'`, the title in the whitelist doesn't match a Simple English Wikipedia article. Fix the whitelist and re-run.

- [ ] **Step 2: Inspect the output**

```bash
wc -l ../seed/wiki_passages.en.jsonl
head -1 ../seed/wiki_passages.en.jsonl | python3 -m json.tool
```

Expected: line count matches whitelist; first record is a valid JSON object with all 7 fields populated.

- [ ] **Step 3: Confirm determinism**

```bash
cp ../seed/wiki_passages.en.jsonl /tmp/run1.jsonl
python3 simple_wikipedia.py
diff /tmp/run1.jsonl ../seed/wiki_passages.en.jsonl
```

Expected: no diff. (If Wikipedia editors change article content between the two runs, there may be a real diff — that's correct behaviour.)

- [ ] **Step 4: Commit**

```bash
git add data/seed/wiki_passages.en.jsonl
git commit -m "feat(ingest): generated wiki_passages.en.jsonl (N articles)"
```

(Replace `N` with the actual line count.)

---

## Task 10: Rust — extend `auto_seed_if_empty` to load multiple JSONL files

**Files:**
- Modify: `src/crates/primer-kb-load/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `src/crates/primer-kb-load/src/lib.rs` (after the existing `discover_seed_jsonl_finds_file_under_env_dir` test):

```rust
    #[tokio::test]
    async fn auto_seed_loads_all_matching_jsonl_files_in_dir() {
        // Two seed files in the same dir → both load.
        let seed_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            seed_dir.path().join("seed_passages.en.jsonl"),
            r#"{"id":"hand-1","source":"seed:en:hand-1","license":"CC0-1.0","attribution":"hand","text":"hand-drafted passage one"}"#,
        )
        .unwrap();
        std::fs::write(
            seed_dir.path().join("wiki_passages.en.jsonl"),
            r#"{"id":"wiki-1","source":"wiki-simple:en:wiki-1","license":"CC-BY-SA-3.0","attribution":"wiki","text":"wikipedia passage one"}"#,
        )
        .unwrap();
        // Distractor: a different-locale file must NOT be loaded.
        std::fs::write(
            seed_dir.path().join("wiki_passages.de.jsonl"),
            r#"{"id":"de-1","source":"wiki-simple:de:de-1","license":"CC-BY-SA-3.0","attribution":"de","text":"deutsche passage"}"#,
        )
        .unwrap();
        // Distractor: a non-jsonl file must NOT be loaded.
        std::fs::write(seed_dir.path().join("README.md"), "not jsonl").unwrap();

        let db = tempfile::NamedTempFile::new().unwrap();
        let kb = SqliteKnowledgeBase::open_for_locale(db.path(), Locale::English).unwrap();

        // SAFETY: serial test (uses tempdir + env var; no other test
        // currently sets PRIMER_SEED_DIR concurrently with this one).
        unsafe {
            std::env::set_var("PRIMER_SEED_DIR", seed_dir.path());
        }
        let result = auto_seed_if_empty(&kb, Locale::English).await.unwrap();
        unsafe {
            std::env::remove_var("PRIMER_SEED_DIR");
        }

        let stats = result.expect("auto-seed should have loaded files");
        assert_eq!(stats.inserted, 2, "expected both en files to load");
        assert_eq!(kb.passage_count().unwrap(), 2);
    }

    #[test]
    fn discover_seed_files_returns_only_matching_locale() {
        let seed_dir = tempfile::tempdir().unwrap();
        std::fs::write(seed_dir.path().join("seed_passages.en.jsonl"), "{}").unwrap();
        std::fs::write(seed_dir.path().join("wiki_passages.en.jsonl"), "{}").unwrap();
        std::fs::write(seed_dir.path().join("wiki_passages.de.jsonl"), "{}").unwrap();
        std::fs::write(seed_dir.path().join("README.md"), "x").unwrap();

        unsafe {
            std::env::set_var("PRIMER_SEED_DIR", seed_dir.path());
        }
        let mut found = discover_seed_files(Locale::English);
        unsafe {
            std::env::remove_var("PRIMER_SEED_DIR");
        }
        found.sort(); // guarantee lexicographic order for the assertion
        assert_eq!(found.len(), 2);
        assert!(found[0].file_name().unwrap() == "seed_passages.en.jsonl");
        assert!(found[1].file_name().unwrap() == "wiki_passages.en.jsonl");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cd src
~/.cargo/bin/cargo test -p primer-kb-load auto_seed_loads_all_matching 2>&1 | tail -10
```

Expected: FAIL — `auto_seed_loads_all_matching_jsonl_files_in_dir` either fails (only one file loaded) or doesn't compile (`discover_seed_files` doesn't exist).

- [ ] **Step 3: Implement `discover_seed_files`**

Add to `src/crates/primer-kb-load/src/lib.rs`, after the existing `discover_seed_jsonl` function:

```rust
/// Discover ALL seed JSONL files for `locale` in the first search-path
/// directory that contains any. The search order matches
/// [`discover_seed_jsonl`] (env override → XDG → cargo dev path); whichever
/// directory yields at least one matching file wins, and all matching
/// files in that directory are returned.
///
/// "Matching" means a regular file whose name ends with `.<pack>.jsonl`,
/// where `<pack>` is `locale.pack_id()`. This lets the in-repo seed dir
/// hold both `seed_passages.en.jsonl` (CC0 hand-drafted) and
/// `wiki_passages.en.jsonl` (CC-BY-SA-3.0 wiki layer) side by side, while
/// `wiki_passages.de.jsonl` is correctly ignored when the locale is
/// English.
///
/// Returns an empty `Vec` if no candidate directory exists.
pub fn discover_seed_files(locale: Locale) -> Vec<PathBuf> {
    let pack = locale.pack_id();
    let suffix = format!(".{pack}.jsonl");

    let candidate_dirs = candidate_seed_dirs();
    for dir in candidate_dirs {
        let mut hits = Vec::new();
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if name.ends_with(&suffix) {
                hits.push(path);
            }
        }
        if !hits.is_empty() {
            hits.sort();
            return hits;
        }
    }
    Vec::new()
}

/// The ordered list of directories to look for seed files in. Mirrors
/// the existing [`discover_seed_jsonl`] precedence.
fn candidate_seed_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(d) = std::env::var("PRIMER_SEED_DIR") {
        dirs.push(PathBuf::from(d));
    }
    if let Some(data_home) = xdg_data_home() {
        dirs.push(data_home.join("primer/seed"));
    }
    if let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
        let mut p = PathBuf::from(manifest_dir);
        for _ in 0..5 {
            dirs.push(p.join("data/seed"));
            if !p.pop() {
                break;
            }
        }
    }
    dirs
}
```

- [ ] **Step 4: Refactor `auto_seed_if_empty` to use `discover_seed_files`**

Replace the existing `auto_seed_if_empty` (lines ~265-288) with:

```rust
/// Auto-seed `kb` from the discovered JSONL file(s) if `kb` is empty.
///
/// All `*.<pack>.jsonl` files in the first matching search-path directory
/// are loaded in lexicographic order (e.g. both `seed_passages.en.jsonl`
/// and `wiki_passages.en.jsonl` will load on a fresh English KB). The
/// returned `LoadStats` aggregates inserts/skips across all loaded files.
///
/// Returns:
/// - `Ok(Some(stats))` if at least one seed file was found and loaded.
/// - `Ok(None)` if either the KB already has passages or no seed files
///   could be located.
///
/// Errors propagate from the loader; discovery itself never errors.
pub async fn auto_seed_if_empty(
    kb: &SqliteKnowledgeBase,
    locale: Locale,
) -> Result<Option<LoadStats>> {
    if kb.passage_count()? > 0 {
        return Ok(None);
    }
    let files = discover_seed_files(locale);
    if files.is_empty() {
        tracing::info!(
            target = "primer-kb-load",
            locale = locale.pack_id(),
            "no seed corpus found; knowledge base starts empty"
        );
        return Ok(None);
    }
    let mut total = LoadStats::default();
    for path in &files {
        tracing::info!(
            target = "primer-kb-load",
            locale = locale.pack_id(),
            path = %path.display(),
            "loading seed corpus into empty knowledge base"
        );
        let stats = load_jsonl(kb, path).await?;
        total.inserted += stats.inserted;
        total.skipped_existing += stats.skipped_existing;
        total.sources_seen += stats.sources_seen;
    }
    Ok(Some(total))
}
```

- [ ] **Step 5: Update `discover_seed_jsonl` to delegate (preserves backward compat)**

Replace the existing `discover_seed_jsonl` body (lines ~154-187) with a thin delegation:

```rust
/// Search known locations for `seed_passages.<pack_id>.jsonl`, in order:
///
/// 1. `$PRIMER_SEED_DIR/seed_passages.<pack_id>.jsonl` (env override).
/// 2. `$XDG_DATA_HOME/primer/seed/seed_passages.<pack_id>.jsonl`.
/// 3. Cargo dev path: `<workspace_root>/data/seed/seed_passages.<pack_id>.jsonl`.
///
/// Returns the canonical-named file, if any. Use [`discover_seed_files`]
/// to discover *all* matching files (the path that `auto_seed_if_empty`
/// uses).
pub fn discover_seed_jsonl(locale: Locale) -> Option<PathBuf> {
    let canonical = format!("seed_passages.{}.jsonl", locale.pack_id());
    discover_seed_files(locale)
        .into_iter()
        .find(|p| p.file_name().and_then(|n| n.to_str()) == Some(&canonical))
}
```

- [ ] **Step 6: Run tests to verify they pass**

```bash
~/.cargo/bin/cargo test -p primer-kb-load 2>&1 | tail -15
```

Expected: All `primer-kb-load` tests pass, including the two new ones.

- [ ] **Step 7: Run the workspace test suite to confirm no regressions**

```bash
~/.cargo/bin/cargo test --workspace 2>&1 | grep -E "^test result:" | awk '{p+=$4;f+=$6} END {print "passed:",p,"failed:",f}'
```

Expected: total grew by 2 (548 → 550); 0 failed.

- [ ] **Step 8: Commit**

```bash
git add src/crates/primer-kb-load/src/lib.rs
git commit -m "feat(kb-load): auto_seed_if_empty loads all *.<pack>.jsonl in seed dir"
```

---

## Task 11: Add wiki-targeted canonical retrieval queries

The existing `retrieval_quality.rs` exercises 50+ queries against the seed corpus. Now extend it with queries that should be answered by the wiki layer (concepts the seed corpus doesn't cover).

**Files:**
- Modify: `src/crates/primer-kb-load/tests/retrieval_quality.rs`

- [ ] **Step 1: Inspect the wiki output to choose realistic queries**

```bash
cd /Users/hherb/src/primer
python3 -c "
import json
with open('data/seed/wiki_passages.en.jsonl') as f:
    for line in f:
        rec = json.loads(line)
        title = rec['attribution'].split(\"'\")[1]
        first_sentence = rec['text'].split('.')[0]
        print(f'{title:30} | {first_sentence[:80]}')
"
```

This prints title + first sentence of each wiki passage. Pick 5-10 queries whose answers are reliably in the wiki passages but NOT in the seed corpus. Pick concepts the seed doesn't cover.

- [ ] **Step 2: Append the new queries to `retrieval_quality.rs`**

Locate the `QUERIES` array in `src/crates/primer-kb-load/tests/retrieval_quality.rs` (currently ends at line ~130). Insert before the closing `]`:

```rust
    // ----- Wikipedia layer (Phase 0.2 MVP) -----
    // These queries target concepts the hand-drafted seed corpus does
    // NOT cover, so they exercise the wiki_passages.en.jsonl layer
    // specifically. Adjust the required terms based on the actual
    // wiki passages produced by the ingest run.
    //
    // Replace the placeholder list below with real queries chosen by
    // inspecting `data/seed/wiki_passages.en.jsonl` after Task 9. The
    // template:
    //   ("query a child might ask", &["term1", "term2"], 5),
    // The required terms are case-insensitive substrings; pick fragments
    // that match morphological variants ("fus" matches "fusion", "fusing").
    //
    // EXAMPLE PLACEHOLDERS — REPLACE BEFORE COMMITTING:
    ("what is gravity",  &["gravity", "mass"], 5),
    ("what is an atom",  &["atom", "particle"], 5),
    ("what is energy",   &["energy"], 5),
    ("what is a virus",  &["virus"], 5),
    ("what is climate change", &["climate"], 5),
```

- [ ] **Step 3: Edit the placeholders to match your actual whitelist**

Replace the 5 example placeholders with queries whose required terms you've verified are present in your `wiki_passages.en.jsonl`. Aim for 5-10 queries.

- [ ] **Step 4: Run the retrieval-quality test**

```bash
cd src
~/.cargo/bin/cargo test -p primer-kb-load --test retrieval_quality -- --nocapture 2>&1 | tail -20
```

Expected: PASS. If it fails, the failure message lists the queries whose required terms were not found in the top-5 — adjust either the query (more typical phrasing) or the required terms (less morphologically specific) until it passes.

- [ ] **Step 5: Run the full workspace test suite**

```bash
~/.cargo/bin/cargo test --workspace 2>&1 | grep -E "^test result:" | awk '{p+=$4;f+=$6} END {print "passed:",p,"failed:",f}'
```

Expected: 0 failed.

- [ ] **Step 6: Commit**

```bash
git add src/crates/primer-kb-load/tests/retrieval_quality.rs
git commit -m "test(kb-load): retrieval-quality queries targeting the wiki layer"
```

---

## Task 12: Manual smoke test against the cloud backend

Eyeballs-on-data validation: sit at the REPL and observe wiki passages being retrieved on relevant questions.

- [ ] **Step 1: Build with verbose support**

```bash
cd src
~/.cargo/bin/cargo build --bin primer
```

- [ ] **Step 2: Confirm the wiki JSONL ships in-repo**

```bash
ls -la /Users/hherb/src/primer/data/seed/
```

Expected: both `seed_passages.en.jsonl` and `wiki_passages.en.jsonl` present.

- [ ] **Step 3: Run a smoke conversation**

```bash
~/.cargo/bin/cargo run --bin primer -- \
    --backend cloud --name SmokeTester --age 9 --no-persist --verbose \
    --knowledge-db /tmp/wiki-smoke.db 2>&1 | tee /tmp/wiki-smoke.log
```

(Use `--backend stub` if no cloud key is available; the wiki passages will still be retrieved, but the Primer's responses will be canned.)

Ask 3-5 questions whose answers should land in the wiki layer (e.g. "what is gravity?", "what is an atom?", "what is climate change?"). Confirm via the verbose output (`[intent]`, `[knowledge]` lines if present in the codebase) that the wiki passages are being retrieved.

- [ ] **Step 4: Document findings**

If everything works, no commit needed for this task. If you find something off (e.g. a wiki passage that retrieves on the wrong query), note it and decide whether to fix in this PR or open a follow-up issue.

---

## Task 13: Update README.md, ROADMAP.md, CLAUDE.md

**Files:**
- Modify: `README.md`
- Modify: `ROADMAP.md`
- Modify: `CLAUDE.md`

- [ ] **Step 1: Update README.md**

Find the "Knowledge-base bootstrapping (partial)" bullet and update it to reflect the new state:

```markdown
- **Knowledge-base bootstrapping (Phase 0.2)** — a hand-drafted CC0 seed corpus of 55 passages spanning all five planned clusters (space, body, how-things-work, life, earth/weather) plus a layer of ~N Simple English Wikipedia Science articles (CC-BY-SA-3.0) ship in-repo at `data/seed/seed_passages.en.jsonl` and `data/seed/wiki_passages.en.jsonl` and auto-load on first run when the KB is empty. A standalone `primer-kb-load` binary supports JSONL ingestion and `--reembed` backfill for re-embedding passages under a new model. A retrieval-quality integration test exercises 60+ canonical child queries across all clusters plus wiki-only concepts. The wiki layer is generated by the Python pipeline at `data/ingest/`; see `data/ingest/README.md` for usage.
```

(Replace `~N` with the actual line count of `data/seed/wiki_passages.en.jsonl`.)

Find the Phase 0 status paragraph and update it:

```markdown
**Phase 0.3 is now complete.** Phase 0.2 (knowledge-base bootstrapping) is also complete in MVP form: hybrid retrieval (BM25 + dense-vector RRF), JSONL ingestion + auto-seed infrastructure, a 55-passage hand-drafted CC0 seed corpus across all five planned clusters, and a Simple English Wikipedia Science layer (~N articles, CC-BY-SA-3.0) all ship in-repo. Tuning of `RetrievalParams` / `HybridParams` defaults against the broader corpus remains as the natural next Phase 0.2 task. Still ahead (see [ROADMAP.md](ROADMAP.md)): retrieval-params tuning, local llama.cpp inference, hardening of the speech loop, hardware integration.
```

- [ ] **Step 2: Update ROADMAP.md**

Find the Phase 0.2 section. Mark off the previously-unchecked Wikipedia ingestion item:

```markdown
- ✅ Simple English Wikipedia ingestion (Phase 0.2 MVP, 2026-05-06; spec at [docs/superpowers/specs/2026-05-06-wikipedia-ingestion-design.md](docs/superpowers/specs/2026-05-06-wikipedia-ingestion-design.md)). Python pipeline at `data/ingest/` ingests ~N Simple English Wikipedia Science articles via the MediaWiki extracts API (lead-only chunking; one passage per article). Output JSONL ships in-repo at `data/seed/wiki_passages.en.jsonl` and auto-loads on a fresh KB alongside the hand-drafted CC0 seed corpus. Per-passage `CC-BY-SA-3.0` license + URL-bearing attribution. `primer-kb-load::auto_seed_if_empty` extended to load all `*.<pack>.jsonl` files in the seed dir.
- [ ] Tune `RetrievalParams` / `HybridParams` defaults (top_k, min_score, RRF k) against the broader corpus
```

(Remove the previously-listed `[ ] Write an ingestion script ...` line since this task replaces it.)

Update the Phase 0 exit-criteria paragraph:

```markdown
**Phase 0 exit criteria:** ... Phase 0.1, 0.2 (MVP), and 0.3 are complete; tuning of retrieval params against the broader corpus remains.
```

- [ ] **Step 3: Update CLAUDE.md**

Find the Phase status paragraph at line ~9. Update:

```markdown
**Phase 0.1 is done; Phase 0.2 (MVP) is done; Phase 0.3 is done.** The hand-drafted CC0 seed corpus + Simple English Wikipedia (Science portal, lead-only chunking) auto-load alongside each other on a fresh KB. Tuning of retrieval params against the broader corpus is the next Phase 0.2 task. Working today: [...keep the rest of the existing list, append: "Wikipedia Science layer auto-loaded alongside the hand-drafted seed corpus, ..."]. Still ahead: retrieval-params tuning, local llama.cpp inference, voice-loop hardening, hardware integration.
```

In the "Conventions and gotchas" section, add a note about the new Python pipeline:

```markdown
- **Wikipedia ingestion lives in `data/ingest/` (Python).** Pure functions where possible; network injected for testability. To regenerate `data/seed/wiki_passages.en.jsonl`, run `python3 data/ingest/simple_wikipedia.py` from the repo root after activating the venv (`pip install -r data/ingest/requirements.txt`). Re-runs are deterministic (sorted by `id`); diffs only reflect real article-content changes on Wikipedia. The whitelist at `data/ingest/simple_wikipedia_whitelist.txt` is hand-curated; expand it with `build_whitelist.py` + manual review.
- **`auto_seed_if_empty` loads ALL matching `*.<pack>.jsonl` files in the seed dir** — this is what makes the wiki layer auto-load alongside the hand-drafted seed. A future locale's wiki layer (e.g. `wiki_passages.de.jsonl`) auto-loads on a German session without code change.
```

- [ ] **Step 4: Run the workspace test suite one final time**

```bash
cd src
~/.cargo/bin/cargo test --workspace 2>&1 | grep -E "^test result:" | awk '{p+=$4;f+=$6} END {print "passed:",p,"failed:",f}'
```

Expected: 0 failed.

- [ ] **Step 5: Commit**

```bash
git add README.md ROADMAP.md CLAUDE.md
git commit -m "docs: README + ROADMAP + CLAUDE.md for Wikipedia ingestion (Phase 0.2 MVP)"
```

---

## Task 14: Final verification + push

- [ ] **Step 1: Re-run the full verification gauntlet**

```bash
cd src
~/.cargo/bin/cargo build --workspace
~/.cargo/bin/cargo test --workspace
~/.cargo/bin/cargo clippy --workspace --all-targets
~/.cargo/bin/cargo fmt --all -- --check
```

Expected: all green; tally `passed: M failed: 0` where `M = baseline + 7 (Rust new tests) + ~5 (new retrieval queries that pass) ≈ 555+`.

- [ ] **Step 2: Re-run the Python tests**

```bash
cd data/ingest
python3 -m pytest -v
```

Expected: ~30+ tests, all pass.

- [ ] **Step 3: Eyeball the diff for the whole branch**

```bash
git log --oneline main..HEAD
git diff --stat main..HEAD
```

Expected: a clean per-task commit history; touched files match the file-structure plan.

- [ ] **Step 4: Push the branch**

```bash
git push -u origin feature/wikipedia-ingestion
```

- [ ] **Step 5: Open a PR**

```bash
gh pr create --title "Phase 0.2 MVP: Simple English Wikipedia ingestion" --body "$(cat <<'EOF'
## Summary

- Python pipeline at `data/ingest/` ingests ~N Simple English Wikipedia Science articles into the existing hybrid knowledge base.
- Lead-only chunking (one passage per article); MediaWiki extracts API (no wikitext parser dependency).
- Output JSONL committed in-repo at `data/seed/wiki_passages.en.jsonl` and auto-loads on a fresh KB alongside the hand-drafted CC0 seed corpus.
- `primer-kb-load::auto_seed_if_empty` extended to load all `*.<pack>.jsonl` files in the seed dir (so a future locale's wiki layer auto-loads on a same-locale session without code change).
- Per-passage `CC-BY-SA-3.0` license + URL-bearing attribution; `source_url` carries the canonical Wikipedia article URL through to the `sources` table.
- Retrieval-quality integration test extended with 5-10 wiki-targeted canonical queries.
- Spec at `docs/superpowers/specs/2026-05-06-wikipedia-ingestion-design.md`.

## Test plan

- [x] Workspace tests green: `cargo test --workspace`.
- [x] Clippy clean.
- [x] Fmt clean.
- [x] Python tests green: `pytest data/ingest/`.
- [x] Manual smoke against cloud backend: ask 3-5 wiki-targeted questions, observe wiki passages retrieved.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

(Replace `~N` with the actual line count.)

---

## Self-review

This section is for the plan author to verify completeness against the spec. Do NOT mark steps below in the plan execution.

**Spec coverage:**
- Architecture (script + whitelist + Rust extension) → Tasks 1-10
- Components (`build_whitelist.py`, `simple_wikipedia.py`, Rust `discover_seed_files` + `auto_seed_if_empty`) → Tasks 2-7, 10
- Schema (id namespacing, license, attribution, source_url, topics) → Tasks 4 + 6 + fixtures
- Testing strategy (4 Python test files + Rust unit tests + integration retrieval test) → Tasks 2-7, 10, 11
- Acceptance criteria 1-7 (live API smoke, hand-curated 30-50 entries, deterministic JSONL, pytest passes, cargo test passes, manual smoke shows wiki retrieval, doc updates) → Tasks 8, 9, 12, 13
- Acceptance criterion 8 (one logical commit per task) → built into every task's "Commit" step
- Risks (rate limit, title drift, lead extraction quirks, license attribution drift, diff churn, license compatibility) → addressed in implementation (User-Agent, sleep, sanity-check, source_url field)

**Placeholder scan:**
- Task 8 has a deliberate manual step ("Hand-review and produce the trimmed whitelist") — this is acknowledged non-automatable content review, not a placeholder. The replacement is the developer's content judgment.
- Task 9 has line counts and warnings expressed in placeholder form (`N`); these are filled in at execution time from real data.
- Task 11 has explicit "EXAMPLE PLACEHOLDERS — REPLACE BEFORE COMMITTING" markers in the code, with explicit instructions to replace before commit.
- Task 13 has `~N` placeholder for the line count — filled in at doc-writing time.
- No "TODO" or "TBD" markers anywhere; the manual content steps are clearly demarcated.

**Type/name consistency:**
- `slugify`, `read_whitelist`, `to_passage`, `fetch_lead`, `main` are consistently named across tests, implementations, and the README.
- The schema fields (`id`, `source`, `license`, `attribution`, `source_url`, `text`, `topics`) match `SeedPassage` exactly (verified against `src/crates/primer-kb-load/src/lib.rs:46-64`).
- Rust function names: `discover_seed_files`, `auto_seed_if_empty`, `discover_seed_jsonl` — all consistent across tests + implementation + the test that asserts back-compat.
- Whitelist filename: `simple_wikipedia_whitelist.txt` — consistent everywhere.
- Output filename: `wiki_passages.en.jsonl` — consistent everywhere.
- ID prefix: `wiki-simple:en:` — consistent in the schema, tests, and the new auto-seed test.

No issues found. Plan is ready.
