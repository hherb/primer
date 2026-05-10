"""Wikipedia-shaped ingest pipeline (multi-source).

Pure functions where possible; network injected via ``http_client`` so
the unit tests can substitute a fake. The :class:`WikiSource` dataclass
bundles per-source ingest parameters (URL, license, fetch strategy,
disambiguation patterns) so the same fetch / passage-emit / JSONL-write
code path can serve any MediaWiki-shaped source. Two presets ship today:

- :data:`SIMPLE_ENGLISH` — Simple English Wikipedia. Uses MediaWiki's
  TextExtracts extension (``exintro=1&explaintext=1``) for batched
  20-titles-per-request lead fetches.
- :data:`KLEXIKON` — German children's wiki at ``klexikon.zum.de``.
  Selected over regular ``de.wikipedia.org`` because its hand-written
  ages-8-13 vocabulary fits the Primer's audience the way Simple
  English fits the English audience. The Klexikon MediaWiki has no
  TextExtracts extension, so the fetch strategy is
  ``action=parse&prop=wikitext&section=0`` (one request per article)
  followed by :func:`strip_klexikon_wikitext` to convert wiki markup
  to plain text.

Adding a new source is purely additive: declare a ``WikiSource``
preset, hand-author the whitelist, and run :func:`main` with
``source=<preset>``. See ``data/ingest/README.md`` for the live-run
workflow.
"""
from __future__ import annotations

import argparse
import json
import random
import time
import urllib.parse
from pathlib import Path

from retry import RetrySettings, retry_http_get


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


# Length of the head-of-extract slice that disambiguation patterns are
# matched against. Disambiguation pages always announce themselves up
# front; matching the whole extract would slow the regex pass and add
# false positives where a body paragraph happens to contain a phrase
# like "may refer to".
_DISAMBIGUATION_HEAD_CHARS = 300


def _check_disambiguation(title: str, extract: str, source: WikiSource) -> None:
    """Raise if the lead looks like a disambiguation page for this source.

    Pure helper, factored out so each fetch strategy shares one
    matcher (and so language-specific pattern dispatch lives in
    exactly one place).
    """
    head = extract[:_DISAMBIGUATION_HEAD_CHARS]
    if any(p.search(head) for p in source.disambiguation_patterns):
        raise RuntimeError(
            f"{title!r} returned a disambiguation page; "
            f"use a more specific title (e.g. {title!r} → "
            f"{title!r} (specific topic)). Lead starts: {head[:120]!r}"
        )


# ── Strategy: TextExtracts (Simple English Wikipedia) ─────────────────


def _fetch_lead_via_text_extracts(
    title: str, *, http_client, source: WikiSource
) -> dict:
    """Fetch one article via ``action=query&prop=extracts``.

    Returns ``{"title": str, "lead_text": str, "canonical_url": str}``.
    Raises :class:`RuntimeError` for missing pages, empty extracts,
    and disambiguation pages.
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
    resp = retry_http_get(
        http_client,
        source.api_url,
        params=params,
        timeout=30.0,
        settings=_RETRY_SETTINGS,
        sleep=time.sleep,
        jitter_fn=random.random,
    )
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
    _check_disambiguation(title, extract, source)
    return {
        "title": page["title"],
        "lead_text": extract.strip(),
        "canonical_url": page["fullurl"],
    }


def _fetch_leads_via_text_extracts(
    titles: list[str], *, http_client, source: WikiSource
) -> dict[str, dict]:
    """Batch-fetch up to ``source.batch_size`` titles in ONE request.

    Returns a dict keyed by INPUT title (not the canonical title —
    redirects are resolved via the API's redirect map and
    normalisation map).
    """
    if len(titles) > source.batch_size:
        raise ValueError(
            f"fetch_leads: batch of {len(titles)} exceeds API cap "
            f"of {source.batch_size}; chunk in the caller"
        )
    if not titles:
        return {}
    params = {
        "action": "query",
        "prop": "extracts|info",
        "exintro": 1,
        "explaintext": 1,
        "inprop": "url",
        "titles": "|".join(titles),
        "format": "json",
        "redirects": 1,
        "exlimit": "max",
    }
    resp = retry_http_get(
        http_client,
        source.api_url,
        params=params,
        timeout=60.0,
        settings=_RETRY_SETTINGS,
        sleep=time.sleep,
        jitter_fn=random.random,
    )
    resp.raise_for_status()
    data = resp.json()
    normalized = {n["from"]: n["to"] for n in data.get("query", {}).get("normalized", [])}
    redirects = {n["from"]: n["to"] for n in data.get("query", {}).get("redirects", [])}
    pages_by_title = {p.get("title"): p for p in data.get("query", {}).get("pages", {}).values()}

    out: dict[str, dict] = {}
    for title in titles:
        resolved = normalized.get(title, title)
        resolved = redirects.get(resolved, resolved)
        page = pages_by_title.get(resolved, {"missing": ""})
        if "missing" in page:
            raise RuntimeError(f"fetch_leads: article not found: {title!r}")
        extract = page.get("extract", "")
        if not extract.strip():
            raise RuntimeError(f"fetch_leads: empty extract for {title!r}")
        _check_disambiguation(title, extract, source)
        out[title] = {
            "title": page["title"],
            "lead_text": extract.strip(),
            "canonical_url": page["fullurl"],
        }
    return out


# ── Strategy: Klexikon parse + wikitext-strip ─────────────────────────


def _klexikon_canonical_url(source: WikiSource, title: str) -> str:
    """Construct the canonical web URL for a Klexikon article.

    MediaWiki canonicalises article paths by replacing spaces with
    underscores. Non-ASCII characters in the title (e.g. ``ä``, ``ö``,
    ``ü``, ``ß``) are percent-encoded per RFC 3986 — ``Vögel`` becomes
    ``V%C3%B6gel``. The ``parse`` API doesn't return a ``fullurl``
    field, so we construct it from the source's ``web_base_url`` and
    the (possibly redirected) page title.

    ``safe="/:"`` keeps the path separator unencoded for namespace
    prefixes (e.g. ``Datei:Foo``) that MediaWiki canonical URLs leave
    unescaped; underscores are already in the unreserved set so
    :func:`urllib.parse.quote` leaves them alone.
    """
    return source.web_base_url + urllib.parse.quote(
        title.replace(" ", "_"), safe="/:"
    )


def _fetch_lead_via_klexikon(
    title: str, *, http_client, source: WikiSource
) -> dict:
    """Fetch one article via ``action=parse&prop=wikitext&section=0``.

    Klexikon's MediaWiki has no TextExtracts, so we ask for the raw
    wikitext of the lead section and convert it to plain text via
    :func:`strip_klexikon_wikitext`.

    Returns ``{"title": str, "lead_text": str, "canonical_url": str}``.
    Raises :class:`RuntimeError` for missing pages, empty extracts,
    and disambiguation pages.
    """
    params = {
        "action": "parse",
        "page": title,
        "prop": "wikitext",
        "section": 0,
        "format": "json",
        "redirects": 1,
    }
    resp = retry_http_get(
        http_client,
        source.api_url,
        params=params,
        timeout=30.0,
        settings=_RETRY_SETTINGS,
        sleep=time.sleep,
        jitter_fn=random.random,
    )
    resp.raise_for_status()
    data = resp.json()
    if "error" in data:
        err = data["error"]
        # `missingtitle` is the standard error code for an unknown
        # article; surface it as a clear message so a typo'd whitelist
        # entry fails loudly.
        if err.get("code") == "missingtitle":
            raise RuntimeError(
                f"fetch_lead: article not found: {title!r} "
                f"(klexikon: {err.get('info', '')})"
            )
        raise RuntimeError(
            f"fetch_lead: klexikon API error for {title!r}: {err}"
        )
    parse = data.get("parse")
    if not parse:
        raise RuntimeError(f"fetch_lead: empty response for {title!r}")
    raw_wikitext = parse.get("wikitext", {}).get("*", "")
    if not raw_wikitext.strip():
        raise RuntimeError(f"fetch_lead: empty wikitext for {title!r}")
    plain = strip_klexikon_wikitext(raw_wikitext)
    if not plain.strip():
        raise RuntimeError(
            f"fetch_lead: wikitext stripped to empty for {title!r}"
        )
    _check_disambiguation(title, plain, source)
    canonical_title = parse.get("title", title)
    return {
        "title": canonical_title,
        "lead_text": plain,
        "canonical_url": _klexikon_canonical_url(source, canonical_title),
    }


def _fetch_leads_via_klexikon(
    titles: list[str], *, http_client, source: WikiSource
) -> dict[str, dict]:
    """Per-title loop for the Klexikon strategy (no batching).

    The MediaWiki ``parse`` action takes a single ``page`` parameter
    per call, so each title is its own HTTP request. The outer
    pipeline loop in :func:`main` already paces requests via
    ``inter_batch_sleep_s``; with ``source.batch_size == 1`` that
    becomes a per-request throttle.
    """
    if len(titles) > source.batch_size:
        raise ValueError(
            f"fetch_leads: batch of {len(titles)} exceeds size "
            f"of {source.batch_size}; chunk in the caller"
        )
    out: dict[str, dict] = {}
    for title in titles:
        out[title] = _fetch_lead_via_klexikon(
            title, http_client=http_client, source=source
        )
    return out


# ── Public fetch dispatch ─────────────────────────────────────────────


def fetch_lead(title: str, *, http_client, source: WikiSource) -> dict:
    """Fetch the lead section for one title, dispatching on strategy.

    Returns ``{"title": str, "lead_text": str, "canonical_url": str}``.

    Raises:
        RuntimeError: when the article is missing, the lead is empty,
            or the page is a disambiguation page (per the source's
            patterns).
    """
    if source.fetch_strategy == "text_extracts":
        return _fetch_lead_via_text_extracts(
            title, http_client=http_client, source=source
        )
    if source.fetch_strategy == "klexikon_wikitext":
        return _fetch_lead_via_klexikon(
            title, http_client=http_client, source=source
        )
    raise ValueError(f"unknown fetch_strategy: {source.fetch_strategy!r}")


def fetch_leads(
    titles: list[str], *, http_client, source: WikiSource
) -> dict[str, dict]:
    """Fetch leads for a batch of up to ``source.batch_size`` titles.

    Returns a dict keyed by INPUT title.

    Strategy ``text_extracts``: 1 HTTP request for the whole batch
    (TextExtracts API takes pipe-separated titles, returns merged
    pages; cap is 20).

    Strategy ``klexikon_wikitext``: 1 HTTP request per title
    (parse API has no batching). With ``source.batch_size == 1``
    this is the only path the outer pipeline uses; calling with
    a longer ``titles`` list raises rather than silently looping
    so the developer sees the misuse.

    Raises:
        ValueError: when ``len(titles) > source.batch_size``.
        RuntimeError: from any underlying per-title fetch.
    """
    if not titles:
        return {}
    if source.fetch_strategy == "text_extracts":
        return _fetch_leads_via_text_extracts(
            titles, http_client=http_client, source=source
        )
    if source.fetch_strategy == "klexikon_wikitext":
        return _fetch_leads_via_klexikon(
            titles, http_client=http_client, source=source
        )
    raise ValueError(f"unknown fetch_strategy: {source.fetch_strategy!r}")


# Default user-agent for live runs. Per Wikipedia API etiquette, this
# must include the tool name, version, and a contact identifier.
_DEFAULT_USER_AGENT = "PrimerSeedBuilder/0.1 (contact: my.list.subscriptions@gmail.com)"


# Retry settings used by every strategy fetcher's HTTP-call wrapper.
# Single source of truth so tuning is one constant edit, not three.
# See ``retry.py`` for the underlying defaults; pin them here only if a
# per-source override is ever needed.
_RETRY_SETTINGS = RetrySettings.default()


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
