"""Wikipedia-shaped ingest pipeline -- CLI entry point + pipeline glue.

The implementation was split into three focused submodules under
:mod:`wiki` (see ``wiki/__init__.py``); this module retains:

- :func:`main` -- the pipeline orchestrator (whitelist -> batched
  fetch -> JSONL).
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
from __future__ import annotations

import argparse
import json
import time
from pathlib import Path


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


# Threshold below which a passage's word count triggers a hand-review
# warning. Short Wikipedia leads usually mean a misnamed article (a
# stub or near-empty page); not a hard error because the developer
# may legitimately want a short article through.
_SHORT_LEAD_WORD_THRESHOLD = 30


def main(
    whitelist_path: Path,
    output_path: Path,
    *,
    source: WikiSource,
    http_client=None,
    inter_batch_sleep_s: float = 1.0,
) -> None:
    """Run the full pipeline: whitelist → batched fetch → JSONL.

    The output JSONL is sorted by ``id`` for deterministic diffs.

    Args:
        whitelist_path: path to the whitelist text file.
        output_path: where to write the JSONL.
        source: the per-source preset (``SIMPLE_ENGLISH`` or
            ``KLEXIKON`` today). Required.
        http_client: optional HTTP client (must implement
            ``get(url, params, timeout=...)`` and return a response
            with ``.json()`` and ``.raise_for_status()``). If
            ``None``, a ``requests.Session`` is constructed with the
            default User-Agent.
        inter_batch_sleep_s: seconds to wait between batches when
            using a real network client. Each batch fetches up to
            ``source.batch_size`` titles. For per-page strategies
            (``batch_size == 1``) this becomes a per-request
            throttle. Set to 0 in tests.
    """
    if http_client is None:
        import requests
        http_client = requests.Session()
        http_client.headers.update({"User-Agent": _DEFAULT_USER_AGENT})

    titles = read_whitelist(whitelist_path)
    _assert_unique_slugs(titles)

    records: dict[str, dict] = {}
    step = source.batch_size
    for i in range(0, len(titles), step):
        if i > 0:
            time.sleep(inter_batch_sleep_s)
        batch = titles[i : i + step]
        records.update(fetch_leads(batch, http_client=http_client, source=source))

    pairs: list[tuple[str, dict]] = []
    for title in titles:
        passage = to_passage(records[title], source=source)
        word_count = len(passage["text"].split())
        if word_count < _SHORT_LEAD_WORD_THRESHOLD:
            print(
                f"warning: lead for {title!r} has only {word_count} words "
                "— review whether the article was misnamed",
                flush=True,
            )
        pairs.append((title, passage))

    _assert_unique_passage_ids(pairs)

    passages = [p for _, p in pairs]
    passages.sort(key=lambda p: p["id"])

    with output_path.open("w", encoding="utf-8") as f:
        for p in passages:
            # ensure_ascii=True so the file is portable across editors
            # and the diff is stable regardless of locale settings.
            f.write(json.dumps(p, ensure_ascii=True))
            f.write("\n")


# ── CLI ───────────────────────────────────────────────────────────────


def _parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    """Parse CLI flags. Pure: no I/O, no side effects beyond
    argparse's own ``--help``-and-exit. Tested via direct invocation."""
    parser = argparse.ArgumentParser(
        description=(
            "Ingest Wikipedia-shaped articles into a JSONL seed corpus. "
            "Choose --language en for Simple English Wikipedia, "
            "or --language de for Klexikon (German children's wiki)."
        ),
    )
    parser.add_argument(
        "--language",
        default=SIMPLE_ENGLISH.pack_id,
        choices=sorted(_SOURCES_BY_PACK_ID.keys()),
        help="locale pack id (default: en = Simple English Wikipedia)",
    )
    parser.add_argument(
        "--whitelist",
        type=Path,
        default=None,
        help=(
            "path to the whitelist file. Default: "
            "<source>_whitelist.txt next to this script."
        ),
    )
    parser.add_argument(
        "--output",
        type=Path,
        default=None,
        help=(
            "path to the JSONL output. Default: "
            "../seed/wiki_passages.<language>.jsonl"
        ),
    )
    return parser.parse_args(argv)


def _default_whitelist_path(here: Path, source: WikiSource) -> Path:
    """Default whitelist path for ``source``.

    The Simple English path keeps the legacy filename
    (``simple_wikipedia_whitelist.txt``) for back-compat with existing
    docs and the committed file. Klexikon uses ``klexikon_whitelist.txt``.
    """
    if source is SIMPLE_ENGLISH:
        return here / "simple_wikipedia_whitelist.txt"
    if source is KLEXIKON:
        return here / "klexikon_whitelist.txt"
    return here / f"{source.pack_id}_whitelist.txt"


def _default_output_path(here: Path, source: WikiSource) -> Path:
    """Default output JSONL path: alongside the existing seed corpus,
    named per the auto-seed discovery convention
    (``wiki_passages.<pack_id>.jsonl``)."""
    return here.parent / "seed" / f"wiki_passages.{source.pack_id}.jsonl"


if __name__ == "__main__":
    args = _parse_args()
    here = Path(__file__).resolve().parent
    selected_source = _SOURCES_BY_PACK_ID[args.language]
    whitelist = args.whitelist or _default_whitelist_path(here, selected_source)
    output = args.output or _default_output_path(here, selected_source)
    main(whitelist_path=whitelist, output_path=output, source=selected_source)
    print(f"wrote {output}")
