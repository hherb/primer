"""Wikipedia-shaped ingest pipeline -- CLI entry point + pipeline glue.

The implementation lives in three focused submodules under :mod:`wiki`
(see ``wiki/__init__.py``); this module retains only:

- :func:`main` -- the pipeline orchestrator (whitelist -> batched
  fetch -> JSONL).
- The argparse CLI helpers (:func:`_parse_args`,
  :func:`_default_whitelist_path`, :func:`_default_output_path`) and
  the ``if __name__ == "__main__"`` entry point.

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

from wiki.fetch import _DEFAULT_USER_AGENT, fetch_leads
from wiki.source import (
    KLEXIKON,
    SIMPLE_ENGLISH,
    WikiSource,
    _SOURCES_BY_PACK_ID,
    _assert_unique_passage_ids,
    _assert_unique_slugs,
    read_whitelist,
    to_passage,
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
