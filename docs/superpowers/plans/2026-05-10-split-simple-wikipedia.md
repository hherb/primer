# Split simple_wikipedia.py Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Split `data/ingest/simple_wikipedia.py` (970 lines, well past the 500-line guideline) into focused submodules under `data/ingest/wiki/`, with `simple_wikipedia.py` reduced to a thin CLI entry point + back-compat shim. Pure mechanical refactor — no behaviour change. The 135 Python tests are the safety net and must stay green throughout.

**Architecture:** Three concern-bound submodules in a new `wiki/` package — `source.py` (config + identity: `WikiSource`, presets, slugify, whitelist parser, passage emitter), `strip.py` (Klexikon wikitext → plain text), `fetch.py` (HTTP dispatch + per-strategy fetchers). The existing `simple_wikipedia.py` keeps the `main` pipeline orchestrator + CLI argparse + `__main__` block (so `python simple_wikipedia.py --language en` still works) plus a wildcard re-export of every public/private name the existing tests reach for (so no test edits are required).

**Tech Stack:** Python 3.10+, pytest. No new dependencies. The `data/ingest/` package directory uses no `__init__.py` today (each module is top-level on the test sys.path); we add a single `data/ingest/wiki/__init__.py` to make `wiki/` a real package.

---

## File Structure

**New files (created):**
- `data/ingest/wiki/__init__.py` — empty package marker
- `data/ingest/wiki/source.py` — `WikiSource`, `_VALID_FETCH_STRATEGIES`, `SIMPLE_ENGLISH`, `KLEXIKON`, `_EN_DISAMBIGUATION_PATTERNS`, `_DE_DISAMBIGUATION_PATTERNS`, `_SOURCES_BY_PACK_ID`, `slugify`, `_NON_ALNUM`, `_assert_unique_slugs`, `_assert_unique_passage_ids`, `read_whitelist`, `to_passage`, `_strip_math_artifacts`
- `data/ingest/wiki/strip.py` — `strip_klexikon_wikitext`, `_strip_balanced_drop_blocks`, `_DROP_BLOCK_PREFIXES`, `_DROP_BLOCK_PREFIX_LOOKAHEAD`, all wikitext regexes (`_WIKILINK_*`, `_TEMPLATE_RE`, `_REF_*`, `_HTML_COMMENT_RE`, `_GALLERY_RE`, `_BOLD_RE`, `_ITALIC_RE`, `_BLANKLINE_RUN_RE`, `_INLINE_WS_RUN_RE`)
- `data/ingest/wiki/fetch.py` — `fetch_lead`, `fetch_leads`, `_fetch_lead_via_text_extracts`, `_fetch_leads_via_text_extracts`, `_fetch_lead_via_klexikon`, `_fetch_leads_via_klexikon`, `_klexikon_canonical_url`, `_check_disambiguation`, `_DISAMBIGUATION_HEAD_CHARS`, `_RETRY_SETTINGS`, `_DEFAULT_USER_AGENT`

**Modified files:**
- `data/ingest/simple_wikipedia.py` — reduced to `main`, CLI helpers (`_parse_args`, `_default_whitelist_path`, `_default_output_path`, `_SHORT_LEAD_WORD_THRESHOLD`), `__main__` block, and re-exports of every name tests use.

**Tests:** No test files are modified. The 135 existing tests in `data/ingest/tests/` import from `simple_wikipedia` and that import path must keep resolving every name they currently fetch.

---

## Task 1: Baseline + branch

**Files:** none modified.

- [ ] **Step 1: Confirm clean main + branch off**

```bash
cd /Users/hherb/src/primer
git status                      # expect: on main, clean
git checkout -b refactor/split-simple-wikipedia
```

- [ ] **Step 2: Run the existing test suite to capture the baseline**

```bash
cd /Users/hherb/src/primer/data/ingest
.venv/bin/pytest tests/ -q
```

Expected: `135 passed`. If any fail, stop — the baseline is broken and refactoring is not safe.

---

## Task 2: Create empty `wiki/` package

**Files:**
- Create: `data/ingest/wiki/__init__.py`

- [ ] **Step 1: Create the package directory + `__init__.py`**

```bash
mkdir -p /Users/hherb/src/primer/data/ingest/wiki
```

Then write `data/ingest/wiki/__init__.py`:

```python
"""Wikipedia-shaped ingest pipeline submodules.

Split out of the original ``simple_wikipedia.py`` to keep individual
files focused and under the 500-line project guideline. Three modules:

- :mod:`wiki.source` — domain model and identity (``WikiSource`` config
  dataclass, the ``SIMPLE_ENGLISH`` and ``KLEXIKON`` presets, slug
  helpers, whitelist parser, ``to_passage`` emitter).
- :mod:`wiki.strip` — wikitext → plain text for the Klexikon fetch
  strategy. Pure functions, no I/O.
- :mod:`wiki.fetch` — HTTP-fetch dispatch and per-strategy fetchers.
  All network calls flow through here.

The CLI entry point (``main`` orchestrator + argparse) stays in
``simple_wikipedia.py``, which also re-exports every name the test
suite reaches for so the split is invisible to existing tests.
"""
```

- [ ] **Step 2: Confirm tests still pass (no behaviour change yet)**

```bash
cd /Users/hherb/src/primer/data/ingest
.venv/bin/pytest tests/ -q
```

Expected: `135 passed`.

- [ ] **Step 3: Commit**

```bash
cd /Users/hherb/src/primer
git add data/ingest/wiki/__init__.py
git commit -m "refactor(ingest): create empty wiki/ package for upcoming split"
```

---

## Task 3: Extract `wiki/strip.py`

The wikitext stripper is the most independent piece — pure functions, no `WikiSource` knowledge, no HTTP. Extracting it first lets the next two extractions import from it cleanly without a dependency loop.

**Files:**
- Create: `data/ingest/wiki/strip.py`
- Modify: `data/ingest/simple_wikipedia.py` (delete the moved code, add `from wiki.strip import *` re-export shim)

- [ ] **Step 1: Write `data/ingest/wiki/strip.py`**

Copy these regions from the current `simple_wikipedia.py` verbatim (preserving comments and docstrings) into the new file:

- The module docstring (rewritten to describe just the stripper — see below).
- `_DROP_BLOCK_PREFIXES`, `_DROP_BLOCK_PREFIX_LOOKAHEAD` (lines 176–187).
- `_strip_balanced_drop_blocks` (lines 190–228).
- `_WIKILINK_PIPED_RE`, `_WIKILINK_PLAIN_RE`, `_TEMPLATE_RE`, `_REF_PAIR_RE`, `_REF_SELF_RE`, `_HTML_COMMENT_RE`, `_GALLERY_RE`, `_BOLD_RE`, `_ITALIC_RE`, `_BLANKLINE_RUN_RE`, `_INLINE_WS_RUN_RE` (lines 235–273).
- `strip_klexikon_wikitext` (lines 276–309).

Required imports at top of the new file:

```python
"""Klexikon MediaWiki → plain-text stripper.

Klexikon's MediaWiki has no TextExtracts extension, so the Klexikon
fetch strategy retrieves raw wikitext via ``action=parse&prop=wikitext
&section=0`` and converts it to plain text here. Pure functions, no
I/O — tested directly in ``tests/test_wikitext_strip.py``.

The pipeline (in :func:`strip_klexikon_wikitext`) handles real-world
wikitext idioms found in Klexikon leads: image and category drop-blocks
with balanced-bracket scanning (image captions can themselves contain
nested links), plain and piped wiki links, templates, references in
both pair and self-closing forms, HTML comments, ``<gallery>`` blocks,
bold/italic markers, and whitespace cleanup.
"""
from __future__ import annotations

import re
```

Drop the section-header comment line `# ── Wikitext stripper ...` from the original since each module now stands on its own.

- [ ] **Step 2: Replace the moved code in `simple_wikipedia.py` with a re-export shim**

Delete lines 163–309 (the `# ── Wikitext stripper ──` section through end of `strip_klexikon_wikitext`) and replace with:

```python
# ── Wikitext stripper (moved to wiki.strip) ──────────────────────────
# Re-exported for back-compat: tests import `strip_klexikon_wikitext`
# directly from `simple_wikipedia`. The implementation now lives in
# wiki/strip.py; importing it here keeps `from simple_wikipedia import
# strip_klexikon_wikitext` resolving without a test edit.
from wiki.strip import (  # noqa: E402,F401
    _BLANKLINE_RUN_RE,
    _BOLD_RE,
    _DROP_BLOCK_PREFIXES,
    _DROP_BLOCK_PREFIX_LOOKAHEAD,
    _GALLERY_RE,
    _HTML_COMMENT_RE,
    _INLINE_WS_RUN_RE,
    _ITALIC_RE,
    _REF_PAIR_RE,
    _REF_SELF_RE,
    _TEMPLATE_RE,
    _WIKILINK_PIPED_RE,
    _WIKILINK_PLAIN_RE,
    _strip_balanced_drop_blocks,
    strip_klexikon_wikitext,
)
```

(`E402` suppresses the "module-level import not at top" lint, since we have other code above; `F401` suppresses the "imported but unused" lint for the names that exist solely for re-export.)

- [ ] **Step 3: Run the wikitext-strip tests to verify the move**

```bash
cd /Users/hherb/src/primer/data/ingest
.venv/bin/pytest tests/test_wikitext_strip.py -q
```

Expected: same number of test passes as before (count whatever `pytest -q tests/test_wikitext_strip.py` printed at baseline).

- [ ] **Step 4: Run the full suite to verify nothing else broke**

```bash
cd /Users/hherb/src/primer/data/ingest
.venv/bin/pytest tests/ -q
```

Expected: `135 passed`.

- [ ] **Step 5: Commit**

```bash
cd /Users/hherb/src/primer
git add data/ingest/wiki/strip.py data/ingest/simple_wikipedia.py
git commit -m "refactor(ingest): extract wikitext stripper to wiki/strip.py"
```

---

## Task 4: Extract `wiki/source.py`

Domain model + identity helpers — the `WikiSource` dataclass, both presets, disambiguation patterns, slugify, dup-checks, whitelist parser, `to_passage` emitter, and the `_strip_math_artifacts` helper that `to_passage` calls.

**Files:**
- Create: `data/ingest/wiki/source.py`
- Modify: `data/ingest/simple_wikipedia.py` (delete moved code, add re-export shim)

- [ ] **Step 1: Write `data/ingest/wiki/source.py`**

Required imports + module docstring at the top:

```python
"""Per-source Wikipedia ingest configuration + identity helpers.

The :class:`WikiSource` dataclass bundles per-source ingest parameters
(URL, license, fetch strategy, disambiguation patterns) so the same
fetch / passage-emit / JSONL-write code path can serve any
MediaWiki-shaped source. Two presets ship today:

- :data:`SIMPLE_ENGLISH` — Simple English Wikipedia.
- :data:`KLEXIKON` — German children's wiki at ``klexikon.zum.de``.
  Selected over regular ``de.wikipedia.org`` because its hand-written
  ages-8-13 vocabulary fits the Primer's audience the way Simple
  English fits the English audience.

Adding a new source is purely additive: declare a ``WikiSource``
preset, hand-author the whitelist, and call into the pipeline
(:func:`simple_wikipedia.main`) with ``source=<preset>``.

The slug helpers (:func:`slugify`, :func:`_assert_unique_slugs`,
:func:`_assert_unique_passage_ids`), whitelist parser
(:func:`read_whitelist`), and passage emitter (:func:`to_passage`) live
here too because they form the "input identity" surface that operates
on a ``WikiSource``.
"""
from __future__ import annotations

import dataclasses
import re
import unicodedata
from pathlib import Path
```

Then copy these regions from the current `simple_wikipedia.py` verbatim (preserving comments and docstrings):

- `_NON_ALNUM` (line 44).
- `slugify` (lines 47–87).
- `_assert_unique_slugs` (lines 90–110).
- `_assert_unique_passage_ids` (lines 113–136).
- `read_whitelist` (lines 139–160).
- `_VALID_FETCH_STRATEGIES` (line 319).
- `WikiSource` dataclass (lines 322–396).
- `_EN_DISAMBIGUATION_PATTERNS` (lines 401–406).
- `_DE_DISAMBIGUATION_PATTERNS` (lines 413–419).
- `SIMPLE_ENGLISH` preset (lines 422–433).
- `KLEXIKON` preset (lines 436–447).
- `_SOURCES_BY_PACK_ID` (lines 453–456).
- `to_passage` (lines 459–489).
- `_strip_math_artifacts` (lines 492–506).

Drop the `# ── Per-source Wikipedia configuration ──` and similar section-header comments since each module now stands on its own.

- [ ] **Step 2: Replace the moved code in `simple_wikipedia.py` with a re-export shim**

The current file's structure: top-of-file imports → slugify region → wikitext-stripper region (already moved in Task 3) → WikiSource region → fetch region → main + CLI. After Task 3 the wikitext shim is inline; now we need a shim for the source/identity material too.

Delete the moved blocks (lines 44, 47–160, 312–506 of the original — adjusted for the lines already deleted in Task 3) and replace with a re-export shim. Place the new shim **above** the (still in-flight) Task-3 wikitext shim for readability:

```python
# ── Configuration + identity (moved to wiki.source) ──────────────────
# Re-exported for back-compat: tests import slugify, WikiSource,
# SIMPLE_ENGLISH, KLEXIKON, to_passage, etc. directly from
# `simple_wikipedia`. The implementation now lives in wiki/source.py.
from wiki.source import (  # noqa: E402,F401
    KLEXIKON,
    SIMPLE_ENGLISH,
    WikiSource,
    _DE_DISAMBIGUATION_PATTERNS,
    _EN_DISAMBIGUATION_PATTERNS,
    _NON_ALNUM,
    _SOURCES_BY_PACK_ID,
    _VALID_FETCH_STRATEGIES,
    _assert_unique_passage_ids,
    _assert_unique_slugs,
    _strip_math_artifacts,
    read_whitelist,
    slugify,
    to_passage,
)
```

After this step, the `simple_wikipedia.py` top half should be: module docstring → top-of-file imports (`argparse`, `json`, `random`, `time`, `Path`, retry stuff still needed by `main`) → wiki.source re-export shim → wiki.strip re-export shim → fetch region (still untouched) → `main` + CLI.

The fetch region still references `_RETRY_SETTINGS`, `time`, `random`, `urllib.parse`, etc. — those imports stay at the top of the file; Task 5 will move them when fetch is extracted.

- [ ] **Step 3: Run the slug/source/passage/whitelist tests**

```bash
cd /Users/hherb/src/primer/data/ingest
.venv/bin/pytest tests/test_slugify.py tests/test_wiki_source.py tests/test_passage_emit.py tests/test_whitelist_parser.py -q
```

Expected: same per-file counts as baseline.

- [ ] **Step 4: Run the full suite**

```bash
cd /Users/hherb/src/primer/data/ingest
.venv/bin/pytest tests/ -q
```

Expected: `135 passed`.

- [ ] **Step 5: Commit**

```bash
cd /Users/hherb/src/primer
git add data/ingest/wiki/source.py data/ingest/simple_wikipedia.py
git commit -m "refactor(ingest): extract WikiSource + identity helpers to wiki/source.py"
```

---

## Task 5: Extract `wiki/fetch.py`

HTTP dispatch and per-strategy fetchers — the `text_extracts` strategy (Simple English path) and the `klexikon_wikitext` strategy (Klexikon path), the public `fetch_lead` / `fetch_leads` dispatch, the canonical-URL builder, and the disambiguation-page guard.

**Files:**
- Create: `data/ingest/wiki/fetch.py`
- Modify: `data/ingest/simple_wikipedia.py` (delete moved code, add re-export shim)

- [ ] **Step 1: Write `data/ingest/wiki/fetch.py`**

Required imports + module docstring:

```python
"""Wikipedia-shaped fetch dispatch + per-strategy fetchers.

Two strategies are wired today, dispatched on
``WikiSource.fetch_strategy``:

- ``"text_extracts"`` — uses the MediaWiki TextExtracts extension
  (``action=query&prop=extracts``). Batched at up to 20 titles per
  request (the API's per-IP cap). Used by the Simple English
  Wikipedia preset.
- ``"klexikon_wikitext"`` — uses ``action=parse&prop=wikitext
  &section=0`` and runs the result through
  :func:`wiki.strip.strip_klexikon_wikitext`. One HTTP request per
  article (the parse API has no batching). Used by the Klexikon
  preset.

All HTTP calls go through :func:`retry.retry_http_get` so HTTP 429 /
5xx responses get exponential backoff with jitter. Network errors
(``requests.exceptions.*``) propagate unchanged.
"""
from __future__ import annotations

import random
import time
import urllib.parse

from retry import RetrySettings, retry_http_get
from wiki.source import WikiSource
from wiki.strip import strip_klexikon_wikitext
```

Then copy these regions from the current `simple_wikipedia.py` verbatim:

- `_DISAMBIGUATION_HEAD_CHARS` (line 514).
- `_check_disambiguation` (lines 517–530).
- `_fetch_lead_via_text_extracts` (lines 536–580).
- `_fetch_leads_via_text_extracts` (lines 583–641).
- `_klexikon_canonical_url` (lines 647–664).
- `_fetch_lead_via_klexikon` (lines 667–729).
- `_fetch_leads_via_klexikon` (lines 732–753).
- `fetch_lead` (lines 759–777).
- `fetch_leads` (lines 780–811).
- `_DEFAULT_USER_AGENT` (line 816).
- `_RETRY_SETTINGS` (line 823).

Drop the section-header comments (`# ── Strategy: ... ──`) since the new file is single-purpose.

- [ ] **Step 2: Replace the moved code in `simple_wikipedia.py` with a re-export shim**

Delete the moved blocks and replace the fetch region with:

```python
# ── HTTP fetch dispatch (moved to wiki.fetch) ─────────────────────────
# Re-exported for back-compat: tests import fetch_lead, fetch_leads,
# _klexikon_canonical_url directly from `simple_wikipedia`. The
# implementation now lives in wiki/fetch.py.
from wiki.fetch import (  # noqa: E402,F401
    _DEFAULT_USER_AGENT,
    _DISAMBIGUATION_HEAD_CHARS,
    _RETRY_SETTINGS,
    _check_disambiguation,
    _fetch_lead_via_klexikon,
    _fetch_lead_via_text_extracts,
    _fetch_leads_via_klexikon,
    _fetch_leads_via_text_extracts,
    _klexikon_canonical_url,
    fetch_lead,
    fetch_leads,
)
```

After this step, `simple_wikipedia.py` is: module docstring → `argparse`, `json`, `time`, `Path` imports (the only ones still used by `main` + CLI) → wiki.source re-export → wiki.strip re-export → wiki.fetch re-export → `_SHORT_LEAD_WORD_THRESHOLD` → `main` → CLI helpers → `__main__` block.

The `random` and `urllib.parse` imports at the top of `simple_wikipedia.py` are now dead (only fetch.py uses them) — remove them. The `RetrySettings` import is also dead — remove it.

- [ ] **Step 3: Run the fetch + pipeline tests**

```bash
cd /Users/hherb/src/primer/data/ingest
.venv/bin/pytest tests/test_fetch_lead.py tests/test_pipeline.py -q
```

Expected: same per-file counts as baseline.

- [ ] **Step 4: Run the full suite**

```bash
cd /Users/hherb/src/primer/data/ingest
.venv/bin/pytest tests/ -q
```

Expected: `135 passed`.

- [ ] **Step 5: Commit**

```bash
cd /Users/hherb/src/primer
git add data/ingest/wiki/fetch.py data/ingest/simple_wikipedia.py
git commit -m "refactor(ingest): extract HTTP fetch dispatch to wiki/fetch.py"
```

---

## Task 6: Trim `simple_wikipedia.py` module docstring + final pass

The module docstring still describes the original monolith. Update it to reflect the new role: CLI entry point + back-compat shim.

**Files:**
- Modify: `data/ingest/simple_wikipedia.py`

- [ ] **Step 1: Rewrite the module docstring**

Replace the current 25-line docstring at the top of `simple_wikipedia.py` with:

```python
"""Wikipedia-shaped ingest pipeline — CLI entry point + pipeline glue.

The implementation was split into three focused submodules under
:mod:`wiki` (see ``wiki/__init__.py``); this module retains:

- :func:`main` — the pipeline orchestrator (whitelist → batched fetch
  → JSONL).
- The argparse CLI helpers (:func:`_parse_args`,
  :func:`_default_whitelist_path`, :func:`_default_output_path`) and
  the ``if __name__ == "__main__"`` entry point.
- Back-compat re-exports of every public/private name imported by the
  test suite, so ``from simple_wikipedia import slugify`` etc. keep
  resolving without a test edit.

To regenerate a JSONL seed corpus:

.. code-block:: bash

    python3 simple_wikipedia.py --language en   # Simple English Wikipedia
    python3 simple_wikipedia.py --language de   # Klexikon (German children's wiki)

See ``data/ingest/README.md`` for the full live-run workflow and
``wiki/__init__.py`` for the submodule layout.
"""
```

- [ ] **Step 2: Verify the file is now under the 500-line guideline**

```bash
wc -l /Users/hherb/src/primer/data/ingest/simple_wikipedia.py
wc -l /Users/hherb/src/primer/data/ingest/wiki/*.py
```

Expected:
- `simple_wikipedia.py`: under 250 lines.
- `wiki/source.py`: roughly 370 lines.
- `wiki/strip.py`: roughly 180 lines.
- `wiki/fetch.py`: roughly 330 lines.

If any submodule exceeds 500, that is a sign the split needs further refinement — flag rather than ignore.

- [ ] **Step 3: Run the full suite one more time**

```bash
cd /Users/hherb/src/primer/data/ingest
.venv/bin/pytest tests/ -q
```

Expected: `135 passed`.

- [ ] **Step 4: Spot-check the live CLI parse path (no HTTP)**

```bash
cd /Users/hherb/src/primer/data/ingest
.venv/bin/python -c "from simple_wikipedia import main, KLEXIKON, SIMPLE_ENGLISH, slugify, strip_klexikon_wikitext, fetch_lead; print('all imports resolve')"
.venv/bin/python simple_wikipedia.py --help
```

Expected: the import line prints `all imports resolve`; the `--help` output shows the same flags as before. No HTTP traffic — `--help` exits before any pipeline work.

- [ ] **Step 5: Commit**

```bash
cd /Users/hherb/src/primer
git add data/ingest/simple_wikipedia.py
git commit -m "refactor(ingest): rewrite simple_wikipedia.py docstring for new role"
```

---

## Task 7: Doc updates

CLAUDE.md describes `simple_wikipedia.py` as 927 lines and lists the split as a deferred follow-up. After this PR ships, that line should describe the actual layout.

**Files:**
- Modify: `CLAUDE.md`
- Modify (if needed): `data/ingest/README.md`
- Modify (if needed): `README.md`, `ROADMAP.md` — only if the split is user-facing news.

- [ ] **Step 1: Update the CLAUDE.md note about file size**

Find the bullet starting "**`simple_wikipedia.py` is 927 lines**" in CLAUDE.md and rewrite it. Suggested replacement:

```markdown
- **`data/ingest/simple_wikipedia.py` is the CLI entry point + back-compat shim** (~200 lines after the 2026-05-10 split). The actual ingest implementation lives under `data/ingest/wiki/`: `wiki/source.py` (`WikiSource` dataclass + presets + slugify + whitelist parser + `to_passage` emitter), `wiki/strip.py` (Klexikon wikitext → plain text), `wiki/fetch.py` (HTTP dispatch + per-strategy fetchers). Tests under `data/ingest/tests/` import from `simple_wikipedia` (the back-compat shim re-exports every public/private name); when adding a new test, prefer the submodule import path (`from wiki.fetch import fetch_lead`) for new code.
```

- [ ] **Step 2: Check `data/ingest/README.md` for any stale references**

```bash
grep -n "simple_wikipedia.py" /Users/hherb/src/primer/data/ingest/README.md
```

If any of those references describe the old 927-line file or list functions that have moved, update them. The CLI command lines (`python3 simple_wikipedia.py --language en`) are unchanged and stay.

- [ ] **Step 3: Decide whether `README.md` / `ROADMAP.md` need a mention**

This is internal cleanup — not a user-facing feature. Default: leave them alone. If a "what shipped this session" bullet feels useful in `ROADMAP.md`, a one-liner under Phase 0.3 housekeeping is fine.

- [ ] **Step 4: Run the test suite once more**

```bash
cd /Users/hherb/src/primer/data/ingest
.venv/bin/pytest tests/ -q
```

Expected: `135 passed`.

- [ ] **Step 5: Commit**

```bash
cd /Users/hherb/src/primer
git add CLAUDE.md
# Add data/ingest/README.md / ROADMAP.md / README.md only if they were edited.
git commit -m "docs: update CLAUDE.md for the wiki/ submodule layout"
```

---

## Task 8: Push + open PR

**Files:** none modified.

- [ ] **Step 1: Push the branch**

```bash
cd /Users/hherb/src/primer
git push -u origin refactor/split-simple-wikipedia
```

- [ ] **Step 2: Open the PR**

```bash
gh pr create --title "refactor(ingest): split simple_wikipedia.py into wiki/ submodules" --body "$(cat <<'EOF'
## Summary

- Splits `data/ingest/simple_wikipedia.py` (970 lines, well past the 500-line project guideline) into three focused submodules under `data/ingest/wiki/`:
  - `wiki/source.py` — `WikiSource` dataclass, `SIMPLE_ENGLISH` / `KLEXIKON` presets, slugify, whitelist parser, `to_passage` emitter.
  - `wiki/strip.py` — Klexikon wikitext → plain text. Pure functions.
  - `wiki/fetch.py` — HTTP dispatch + per-strategy fetchers + retry settings.
- `simple_wikipedia.py` is now the CLI entry point + back-compat shim (~200 lines): `main` orchestrator, argparse, `__main__`, and re-exports of every name the existing 135 Python tests reach for.
- Pure mechanical refactor — zero behaviour change. The 135 tests stay green throughout the split (verified after each commit).

## Test plan

- [ ] `pytest data/ingest/tests/ -q` shows `135 passed` against the final state.
- [ ] `python -c "from simple_wikipedia import main, KLEXIKON, SIMPLE_ENGLISH, slugify, strip_klexikon_wikitext, fetch_lead"` resolves cleanly (back-compat shim exports verified).
- [ ] `python simple_wikipedia.py --help` prints the same flags as before.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

- [ ] **Step 3: Confirm CI green**

Watch the PR for CI status; if any test fails on CI but passed locally, that's a real bug worth investigating before merging.

---

## Self-review checklist

After all 8 tasks complete:

1. **Spec coverage:** every name in the original `simple_wikipedia.py` has been placed in exactly one submodule OR remains in `simple_wikipedia.py` as a CLI/orchestrator concern. Tests pass without edits.
2. **No placeholders:** every step lists exact line ranges, exact commands, and complete code blocks.
3. **Type consistency:** `WikiSource` is defined in `wiki/source.py`; `wiki/fetch.py` imports it; `simple_wikipedia.py` re-exports it. The dataclass field signatures are unchanged.
4. **Import cycles:** `wiki/source.py` has zero deps on the other two; `wiki/strip.py` has zero deps on the other two; `wiki/fetch.py` depends on both `wiki.source` (for the `WikiSource` type and the disambiguation patterns via the source argument) and `wiki.strip` (for `strip_klexikon_wikitext`). No cycles.
