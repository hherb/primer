"""Wikipedia-shaped fetch dispatch + per-strategy fetchers.

Two strategies are wired today, dispatched on
``WikiSource.fetch_strategy``:

- ``"text_extracts"`` -- uses the MediaWiki TextExtracts extension
  (``action=query&prop=extracts``). Batched at up to 20 titles per
  request (the API's per-IP cap). Used by the Simple English
  Wikipedia preset.
- ``"klexikon_wikitext"`` -- uses ``action=parse&prop=wikitext
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

from .source import WikiSource
from .strip import strip_klexikon_wikitext


# Length of the head-of-extract slice that disambiguation patterns are
# matched against. Disambiguation pages always announce themselves up
# front; matching the whole extract would slow the regex pass and add
# false positives where a body paragraph happens to contain a phrase
# like "may refer to".
_DISAMBIGUATION_HEAD_CHARS = 300


# Default user-agent for live runs. Per Wikipedia API etiquette, this
# must include the tool name, version, and a contact identifier.
_DEFAULT_USER_AGENT = "PrimerSeedBuilder/0.1 (contact: my.list.subscriptions@gmail.com)"


# Retry settings used by every strategy fetcher's HTTP-call wrapper.
# Single source of truth so tuning is one constant edit, not three.
# See ``retry.py`` for the underlying defaults; pin them here only if a
# per-source override is ever needed.
_RETRY_SETTINGS = RetrySettings.default()


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
            f"use a more specific title (e.g. {title!r} -> "
            f"{title!r} (specific topic)). Lead starts: {head[:120]!r}"
        )


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

    Returns a dict keyed by INPUT title (not the canonical title --
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


def _klexikon_canonical_url(source: WikiSource, title: str) -> str:
    """Construct the canonical web URL for a Klexikon article.

    MediaWiki canonicalises article paths by replacing spaces with
    underscores. Non-ASCII characters in the title (e.g. a-umlaut,
    o-umlaut, u-umlaut, eszett) are percent-encoded per RFC 3986 --
    ``Voegel`` becomes ``V%C3%B6gel``. The ``parse`` API doesn't return
    a ``fullurl`` field, so we construct it from the source's
    ``web_base_url`` and the (possibly redirected) page title.

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
    pipeline loop in :func:`simple_wikipedia.main` already paces
    requests via ``inter_batch_sleep_s``; with ``source.batch_size ==
    1`` that becomes a per-request throttle.
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
