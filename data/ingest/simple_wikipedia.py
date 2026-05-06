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
