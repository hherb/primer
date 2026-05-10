"""Per-source Wikipedia ingest configuration + identity helpers.

The :class:`WikiSource` dataclass bundles per-source ingest parameters
(URL, license, fetch strategy, disambiguation patterns) so the same
fetch / passage-emit / JSONL-write code path can serve any
MediaWiki-shaped source. Two presets ship today:

- :data:`SIMPLE_ENGLISH` -- Simple English Wikipedia.
- :data:`KLEXIKON` -- German children's wiki at ``klexikon.zum.de``.
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
       turns ss -> ss and capital ss -> ss; without it German titles like
       ``Strasse`` would silently produce the corrupted slug ``strae``
       (because ss has no NFD decomposition and is then stripped by
       the ASCII regex).
    3. NFD-decompose to separate combining diacritics, then drop them
       (best-effort transliteration: a-umlaut -> a, o-umlaut -> o,
       u-umlaut -> u, e-acute -> e, etc.).
    4. Collapse runs of non-alphanumerics to a single hyphen.
    5. Trim leading and trailing hyphens.

    The diacritic-strip uses the simple base-letter form, not the
    long German transliteration (a-umlaut -> ae, o-umlaut -> oe,
    u-umlaut -> ue). For children's-curriculum article titles this is
    the right trade-off: cleaner-looking ids and very low collision
    risk in any single whitelist (and any real collision is caught
    loudly by :func:`_assert_unique_slugs` before HTTP traffic).

    Raises:
        ValueError: when the input is empty or has no alphanumeric
            chars after normalisation. Empty slugs would silently
            collide on the passage ``id`` and corrupt the seed corpus.
    """
    if not title:
        raise ValueError("slugify: empty title")
    nfc = unicodedata.normalize("NFC", title)
    # casefold handles eszett -> ss; for ASCII-only titles it is identical
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
    vs ``"foo-bar"``, ``"DNA"`` vs ``"dna"``, ``"Strasse"`` vs
    ``"Strasse"`` -- all of which slip past the whitelist parser's
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
    titles to the same canonical title at fetch time -- e.g. on
    Klexikon `Atom` and `Molekuel` both resolve to `Atome und Molekuele`,
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
    produced thereafter -- a class of bug we'd rather make impossible).

    Fields:
        pack_id: short locale identifier matching the corresponding
            ``primer_core::i18n::Locale::pack_id``. Used as the middle
            segment of the passage id (``<id_prefix>:<pack_id>:<slug>``)
            and as the file suffix in the auto-seed discovery
            (``wiki_passages.<pack_id>.jsonl``).
        api_url: full URL to the MediaWiki ``api.php`` endpoint.
        web_base_url: prefix for the user-facing wiki URL. Used by
            strategies (like ``klexikon_wikitext``) where the API
            response doesn't include a ``fullurl`` field -- the
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
            ``action=query&prop=extracts`` (batched, fast -- Simple
            English path). ``"klexikon_wikitext"`` uses
            ``action=parse&prop=wikitext&section=0`` and runs the
            result through :func:`strip_klexikon_wikitext` (one HTTP
            call per article -- Klexikon path).
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
# The most common form is ``<title> steht fuer: ...``;
# ``Begriffsklaerung`` is the literal MediaWiki category and sometimes
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
    ``\\displaystyle`` marker -- a deliberately coarse rule that strips
    both the LaTeX line and the surrounding unicode preview block in
    one cut.
    """
    parts = text.split("\n\n")
    kept = [p for p in parts if "\\displaystyle" not in p]
    return "\n\n".join(kept).strip()
