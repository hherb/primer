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
import dataclasses
import json
import random
import re
import time
import unicodedata
import urllib.parse
from pathlib import Path

from retry import RetrySettings, retry_http_get


# Slug character class. Lowercase ASCII alphanumerics survive; everything
# else collapses to a hyphen. Pre-folding (casefold + NFD-strip) is what
# turns Unicode input into this character set; see :func:`slugify`.
_NON_ALNUM = re.compile(r"[^a-z0-9]+")


def slugify(title: str) -> str:
    """Convert a Wikipedia article title to a URL-safe lowercase slug.

    Pipeline:

    1. NFC-normalise so precomposed and decomposed Unicode forms map
       to the same slug.
    2. Apply Unicode case folding (``str.casefold``). Critically this
       turns ß → ss and capital ẞ → ss; without it German titles like
       ``Straße`` would silently produce the corrupted slug ``strae``
       (because ß has no NFD decomposition and is then stripped by
       the ASCII regex).
    3. NFD-decompose to separate combining diacritics, then drop them
       (best-effort transliteration: ä → a, ö → o, ü → u, é → e, etc.).
    4. Collapse runs of non-alphanumerics to a single hyphen.
    5. Trim leading and trailing hyphens.

    The diacritic-strip uses the simple base-letter form, not the
    long German transliteration (ä→ae, ö→oe, ü→ue). For
    children's-curriculum article titles this is the right trade-off:
    cleaner-looking ids and very low collision risk in any single
    whitelist (and any real collision is caught loudly by
    :func:`_assert_unique_slugs` before HTTP traffic).

    Raises:
        ValueError: when the input is empty or has no alphanumeric
            chars after normalisation. Empty slugs would silently
            collide on the passage ``id`` and corrupt the seed corpus.
    """
    if not title:
        raise ValueError("slugify: empty title")
    nfc = unicodedata.normalize("NFC", title)
    # casefold handles ß → ss; for ASCII-only titles it is identical
    # to str.lower, so the existing English path is unchanged.
    folded = nfc.casefold()
    nfd = unicodedata.normalize("NFD", folded)
    ascii_only = "".join(c for c in nfd if not unicodedata.combining(c))
    slug = _NON_ALNUM.sub("-", ascii_only).strip("-")
    if not slug:
        raise ValueError(f"slugify: no alphanumerics in title: {title!r}")
    return slug


def _assert_unique_slugs(titles: list[str]) -> None:
    """Reject whitelists where two distinct titles slugify to the same id.

    `read_whitelist` already rejects byte-exact duplicates, but two
    different surface forms can collide post-slugify (e.g. ``"Foo bar"``
    vs ``"foo-bar"``, ``"DNA"`` vs ``"dna"``, ``"Straße"`` vs
    ``"Strasse"`` — all of which slip past the whitelist parser's
    exact-string check). A collision would silently drop the second
    passage at load time because the loader's idempotent-id-skip rule
    treats the second `id` as already-present. Better to catch it
    loudly here, before any HTTP traffic.
    """
    seen: dict[str, str] = {}
    for title in titles:
        slug = slugify(title)
        if slug in seen:
            raise ValueError(
                f"slug collision: {title!r} and {seen[slug]!r} both "
                f"produce id slug {slug!r}; rename one in the whitelist"
            )
        seen[slug] = title


def _assert_unique_passage_ids(pairs: list[tuple[str, dict]]) -> None:
    """Reject result sets where two input titles produce the same id.

    Complements `_assert_unique_slugs`, which only inspects input slugs
    (pre-resolution). Sources with `redirects=1` enabled (Klexikon and
    Simple English MediaWiki both do) can collapse two distinct input
    titles to the same canonical title at fetch time — e.g. on
    Klexikon `Atom` and `Molekül` both resolve to `Atome und Moleküle`,
    producing the same passage id `wiki-klexikon:de:atome-und-molekule`
    for two whitelist lines. Without this check, the second passage
    would silently overwrite the first in the JSONL (sorted-by-id
    write loop) or be silently dropped at load time. We raise loudly
    so the developer can drop or rename one of the colliding inputs.
    """
    seen: dict[str, str] = {}
    for input_title, passage in pairs:
        pid = passage["id"]
        if pid in seen:
            raise RuntimeError(
                f"passage id collision: input titles {seen[pid]!r} and "
                f"{input_title!r} both resolved to id {pid!r} after "
                f"redirect resolution; drop or rename one in the whitelist"
            )
        seen[pid] = input_title


def read_whitelist(path: Path) -> list[str]:
    """Parse a whitelist file: one article title per line, comments OK.

    - Lines starting with ``#`` (after stripping) are ignored.
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


# ── Wikitext stripper (used by the Klexikon fetch strategy) ───────────


# Link prefixes whose entire ``[[...]]`` block is dropped (rather than
# flattened to display text). Two classes:
#
# - Image links: ``Datei:`` (German), ``Bild:`` (older German),
#   ``File:`` (English), ``Image:`` (English). Captions can contain
#   nested ``[[link]]`` calls so balanced-bracket scanning is needed.
# - Category links: ``Kategorie:`` (German), ``Category:`` (English).
#   Always render invisibly in the wiki UI; in plain text they leak as
#   raw ``Kategorie:Foo`` strings. Categories don't carry captions, so
#   the same balanced-bracket scanner handles them safely.
_DROP_BLOCK_PREFIXES = (
    "Datei:",
    "Bild:",
    "File:",
    "Image:",
    "Kategorie:",
    "Category:",
)

# Maximum length of any prefix in `_DROP_BLOCK_PREFIXES`. Used as the
# look-ahead window when scanning for a drop-block opener.
_DROP_BLOCK_PREFIX_LOOKAHEAD = max(len(p) for p in _DROP_BLOCK_PREFIXES)


def _strip_balanced_drop_blocks(wikitext: str) -> str:
    """Remove ``[[<prefix>:...]]`` image and category blocks.

    Image captions can themselves contain wiki links (``[[Datei:Foo
    .jpg|mini|see [[Bar]]]]``). Regex can't naturally handle balanced
    brackets, so we walk the text manually, tracking the ``[[`` /
    ``]]`` depth from the opening of any drop-prefixed link until we
    return to depth 0. Everything inside (and including the outer
    brackets) is dropped.
    """
    out: list[str] = []
    i = 0
    n = len(wikitext)
    while i < n:
        if wikitext.startswith("[[", i):
            inner = wikitext[i + 2 : i + 2 + _DROP_BLOCK_PREFIX_LOOKAHEAD]
            matched_prefix = next(
                (p for p in _DROP_BLOCK_PREFIXES if inner.startswith(p)),
                None,
            )
            if matched_prefix is not None:
                depth = 1
                j = i + 2
                while j < n and depth > 0:
                    if wikitext.startswith("[[", j):
                        depth += 1
                        j += 2
                    elif wikitext.startswith("]]", j):
                        depth -= 1
                        j += 2
                    else:
                        j += 1
                # `j` is now just past the matching `]]` (or end of
                # input on malformed wikitext). Skip the whole block.
                i = j
                continue
        out.append(wikitext[i])
        i += 1
    return "".join(out)


# Match a balanced (single-pair) wiki link: `[[target]]` or
# `[[target|display]]`. Targets and display text must not contain
# brackets or vertical bars (we strip image blocks first so the
# remaining links are the simple variety).
_WIKILINK_PIPED_RE = re.compile(r"\[\[[^\[\]\|]*\|([^\[\]\|]+?)\]\]")
_WIKILINK_PLAIN_RE = re.compile(r"\[\[([^\[\]\|]+?)\]\]")

# Templates `{{...}}`. Single-level only — nested templates are rare
# in Klexikon leads.
_TEMPLATE_RE = re.compile(r"\{\{[^{}]*\}\}")

# References `<ref>...</ref>`, `<ref name="x">...</ref>`, and the
# self-closing `<ref name="x"/>` form.
_REF_PAIR_RE = re.compile(r"<ref(?:\s[^>]*)?>.*?</ref>", re.DOTALL | re.IGNORECASE)
_REF_SELF_RE = re.compile(r"<ref(?:\s[^>]*)?/>", re.IGNORECASE)

# HTML comments.
_HTML_COMMENT_RE = re.compile(r"<!--.*?-->", re.DOTALL)

# `<gallery>...</gallery>` is a MediaWiki extension that renders a
# grid of images. Inside the block, each line is `Filename.jpg|caption`
# (NOT a `[[File:...]]` bracketed link, so the drop-block scanner
# above does not catch it). Whole block is dropped. The opening tag
# may carry attributes (`<gallery mode="packed">`).
_GALLERY_RE = re.compile(r"<gallery(?:\s[^>]*)?>.*?</gallery>", re.DOTALL | re.IGNORECASE)

# Bold (`'''...'''`) and italic (`''...''`) markers. Order matters:
# strip the longer marker first so `'''bold'''` doesn't leave a
# dangling `'`.
_BOLD_RE = re.compile(r"'''(.+?)'''")
_ITALIC_RE = re.compile(r"''(.+?)''")

# Run of three or more consecutive newlines → collapse to two
# (preserve paragraph breaks; drop stranded blanks left by image
# block removal).
_BLANKLINE_RUN_RE = re.compile(r"\n{3,}")

# Run of two or more spaces or tabs *within a line* → collapse to a
# single space. Leaves newlines alone so paragraph breaks survive.
# Removes the visible artefact left by template/ref removal where the
# surrounding spaces collapse together (e.g. ``Vor  war es anders.``
# from ``Vor {{template}} war es anders.``).
_INLINE_WS_RUN_RE = re.compile(r"[ \t]{2,}")


def strip_klexikon_wikitext(wikitext: str) -> str:
    """Convert MediaWiki wikitext (lead-section shape) to plain text.

    Pipeline:

    1. Remove HTML comments and ``<ref>`` markers (both pair and
       self-closing forms) — they contain neither curriculum content
       nor useful retrieval signal.
    2. Remove image blocks (``[[Datei:...]]`` / ``[[File:...]]`` etc.)
       via balanced-bracket scanning. Image captions can contain
       nested ``[[link]]`` calls; the whole block is dropped.
    3. Flatten remaining wiki links: ``[[a|b]]`` → ``b``;
       ``[[a]]`` → ``a``.
    4. Drop ``{{templates}}``.
    5. Strip bold (``'''x'''`` → ``x``) and italic (``''x''`` → ``x``)
       markers.
    6. Collapse runs of 3+ newlines to a paragraph break, then trim.

    Pure function — no I/O, no module-level state. Tested directly in
    ``tests/test_wikitext_strip.py``.
    """
    text = _HTML_COMMENT_RE.sub("", wikitext)
    text = _REF_PAIR_RE.sub("", text)
    text = _REF_SELF_RE.sub("", text)
    text = _GALLERY_RE.sub("", text)
    text = _strip_balanced_drop_blocks(text)
    text = _WIKILINK_PIPED_RE.sub(r"\1", text)
    text = _WIKILINK_PLAIN_RE.sub(r"\1", text)
    text = _TEMPLATE_RE.sub("", text)
    text = _BOLD_RE.sub(r"\1", text)
    text = _ITALIC_RE.sub(r"\1", text)
    text = _INLINE_WS_RUN_RE.sub(" ", text)
    text = _BLANKLINE_RUN_RE.sub("\n\n", text)
    return text.strip()


# ── Per-source Wikipedia configuration ────────────────────────────────


# Allowed fetch strategies. A typo in `WikiSource.fetch_strategy`
# would silently fall through to whichever default branch the
# dispatch picks; validating at construction makes the error fire
# near the typo.
_VALID_FETCH_STRATEGIES = frozenset({"text_extracts", "klexikon_wikitext"})


@dataclasses.dataclass(frozen=True)
class WikiSource:
    """Per-source Wikipedia ingest configuration.

    All fields are required. ``frozen=True`` so a preset cannot be
    mutated mid-run (mutation would silently change every passage
    produced thereafter — a class of bug we'd rather make impossible).

    Fields:
        pack_id: short locale identifier matching the corresponding
            ``primer_core::i18n::Locale::pack_id``. Used as the middle
            segment of the passage id (``<id_prefix>:<pack_id>:<slug>``)
            and as the file suffix in the auto-seed discovery
            (``wiki_passages.<pack_id>.jsonl``).
        api_url: full URL to the MediaWiki ``api.php`` endpoint.
        web_base_url: prefix for the user-facing wiki URL. Used by
            strategies (like ``klexikon_wikitext``) where the API
            response doesn't include a ``fullurl`` field — the
            canonical URL is built by :func:`_klexikon_canonical_url`,
            which underscore-substitutes spaces and percent-encodes
            non-ASCII bytes per RFC 3986.
        id_prefix: the source-family prefix in the passage id.
            Distinct per source family; ``wiki-simple`` for Simple
            English Wikipedia; ``wiki-klexikon`` for Klexikon.
        human_label: human-readable source name used in the
            ``attribution`` string per the CC-BY-SA attribution
            requirement (``"'Title' from <human_label>, licensed
            under <license>"``).
        license: SPDX-style license identifier embedded in every
            passage record. Per-source: Simple English Wikipedia
            declares CC-BY-SA-3.0 for parity with the existing seed
            corpus; Klexikon declares CC-BY-SA-4.0 per its site
            footer.
        topic_tags: extra free-form tags appended to every passage's
            ``topics`` list (alongside the literal ``"wikipedia"``
            prefix and the slug suffix). For Simple English the tags
            include the language identifier (``simple-english``) and
            the curated subject area (``science``); for Klexikon the
            only tag is the source name (``klexikon``).
        disambiguation_patterns: tuple of compiled regex patterns
            whose presence in the lead's first 300 chars marks the
            article as a disambiguation page. Disambiguation phrasings
            are language-specific; each preset carries its own list.
        fetch_strategy: discriminator selecting the lead-fetch
            implementation. ``"text_extracts"`` uses
            ``action=query&prop=extracts`` (batched, fast — Simple
            English path). ``"klexikon_wikitext"`` uses
            ``action=parse&prop=wikitext&section=0`` and runs the
            result through :func:`strip_klexikon_wikitext` (one HTTP
            call per article — Klexikon path).
        batch_size: number of titles per HTTP request. ``20`` for the
            TextExtracts strategy (the API's per-request cap); ``1``
            for the per-page parse strategy.
    """
    pack_id: str
    api_url: str
    web_base_url: str
    id_prefix: str
    human_label: str
    license: str
    topic_tags: tuple[str, ...]
    disambiguation_patterns: tuple[re.Pattern, ...]
    fetch_strategy: str
    batch_size: int

    def __post_init__(self) -> None:
        if self.fetch_strategy not in _VALID_FETCH_STRATEGIES:
            raise ValueError(
                f"unknown fetch_strategy: {self.fetch_strategy!r}; "
                f"valid options: {sorted(_VALID_FETCH_STRATEGIES)}"
            )
        if self.batch_size < 1:
            raise ValueError(
                f"batch_size must be >= 1; got {self.batch_size!r}"
            )


# Disambiguation phrasings used by Simple English Wikipedia and
# English Wikipedia. Detected against the start of the extract.
_EN_DISAMBIGUATION_PATTERNS: tuple[re.Pattern, ...] = (
    re.compile(r"\bcan mean\b", re.IGNORECASE),
    re.compile(r"\bmay refer to\b", re.IGNORECASE),
    re.compile(r"\bcan refer to\b", re.IGNORECASE),
    re.compile(r"\bis a disambiguation\b", re.IGNORECASE),
)


# Disambiguation phrasings used in German wikis (Klexikon, de-Wikipedia).
# The most common form is ``<title> steht für: ...``;
# ``Begriffsklärung`` is the literal MediaWiki category and sometimes
# appears in the lead too.
_DE_DISAMBIGUATION_PATTERNS: tuple[re.Pattern, ...] = (
    re.compile(r"\bsteht für\b", re.IGNORECASE),
    re.compile(r"\bist eine Begriffsklärung\b", re.IGNORECASE),
    re.compile(r"\bBegriffsklärungsseite\b", re.IGNORECASE),
    re.compile(r"\bkann sich beziehen auf\b", re.IGNORECASE),
    re.compile(r"\bist mehrdeutig\b", re.IGNORECASE),
)


SIMPLE_ENGLISH = WikiSource(
    pack_id="en",
    api_url="https://simple.wikipedia.org/w/api.php",
    web_base_url="https://simple.wikipedia.org/wiki/",
    id_prefix="wiki-simple",
    human_label="Simple English Wikipedia",
    license="CC-BY-SA-3.0",
    topic_tags=("simple-english", "science"),
    disambiguation_patterns=_EN_DISAMBIGUATION_PATTERNS,
    fetch_strategy="text_extracts",
    batch_size=20,
)


KLEXIKON = WikiSource(
    pack_id="de",
    api_url="https://klexikon.zum.de/api.php",
    web_base_url="https://klexikon.zum.de/wiki/",
    id_prefix="wiki-klexikon",
    human_label="Klexikon",
    license="CC-BY-SA-4.0",
    topic_tags=("klexikon",),
    disambiguation_patterns=_DE_DISAMBIGUATION_PATTERNS,
    fetch_strategy="klexikon_wikitext",
    batch_size=1,
)


# Lookup table for the ``--language`` CLI flag. Adding a new preset
# means appending one row; the CLI argparse choices are derived from
# this dict so a typo'd flag fails loudly.
_SOURCES_BY_PACK_ID: dict[str, WikiSource] = {
    SIMPLE_ENGLISH.pack_id: SIMPLE_ENGLISH,
    KLEXIKON.pack_id: KLEXIKON,
}


def to_passage(record: dict, *, source: WikiSource) -> dict:
    """Convert a fetched-article record to a SeedPassage-compatible dict.

    Input shape: ``{"title": str, "lead_text": str, "canonical_url": str}``.
    Output shape: matches ``primer_kb_load::SeedPassage`` exactly so
    the JSONL drops into the existing loader without modification.

    The slug (lowercased, transliterated) goes into ``id`` and
    ``source``; the original-cased title (preserving diacritics) is
    embedded in the human-readable ``attribution`` string. The
    canonical URL is structured into ``source_url`` (carried through
    to the ``sources`` table) rather than embedded in ``attribution``.

    Raises:
        ValueError: propagated from :func:`slugify` when the title is
            empty or has no alphanumeric chars.
    """
    title = record["title"]
    slug = slugify(title)
    return {
        "id": f"{source.id_prefix}:{source.pack_id}:{slug}",
        "source": f"{source.id_prefix}:{source.pack_id}:{slug}",
        "license": source.license,
        "attribution": (
            f"'{title}' from {source.human_label}, "
            f"licensed under {source.license}"
        ),
        "source_url": record["canonical_url"],
        "text": _strip_math_artifacts(record["lead_text"]),
        "topics": ["wikipedia", *source.topic_tags, slug],
    }


def _strip_math_artifacts(text: str) -> str:
    """Remove MediaWiki MathJax fallback blocks from extract text.

    The extracts API returns LaTeX placeholders like
    ``{\\displaystyle \\rho ={\\frac {m}{V}}}`` plus several lines of
    indented unicode math layout when an article contains formulas.
    These render as garbage in plain-text retrieval. We split on
    blank lines and drop any resulting paragraph that contains the
    ``\\displaystyle`` marker — a deliberately coarse rule that strips
    both the LaTeX line and the surrounding unicode preview block in
    one cut.
    """
    parts = text.split("\n\n")
    kept = [p for p in parts if "\\displaystyle" not in p]
    return "\n\n".join(kept).strip()


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
