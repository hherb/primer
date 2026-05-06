"""Simple English Wikipedia ingestion pipeline.

Pure functions where possible; network injected via `http_client` so the
unit tests can substitute a fake. See `data/ingest/README.md` for usage.
"""
import re
import unicodedata
from pathlib import Path


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


def to_passage(record: dict) -> dict:
    """Convert a fetched-article record to a SeedPassage-compatible dict.

    Input shape: `{"title": str, "lead_text": str, "canonical_url": str}`.
    Output shape: matches `primer_kb_load::SeedPassage` exactly so the
    JSONL drops into the existing loader without modification.

    The slug (lowercased) goes into `id` and `source`; the original-cased
    title is preserved in the human-readable `attribution` string. The
    canonical URL is structured into `source_url` (carried through to the
    `sources` table) rather than embedded in `attribution`.

    Raises:
        ValueError: propagated from `slugify` when the title is empty or
        has no alphanumeric chars. The caller is responsible for ensuring
        the record dict's keys exist and are non-null — `to_passage` is
        an internal pipeline function and does not validate input shape.
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


import json as _json
import time as _time


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
            a real network client. Set to 0 in tests.
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
