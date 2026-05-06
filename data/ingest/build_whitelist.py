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
# Wikipedia editors. Note that the article *titles* are stable across both
# wikis — Simple English Wikipedia uses the same titles for the same
# concepts. We pull from English to get the curated list, then fetch
# leads from Simple English in the main pipeline.
#
# "Science" used to be a single umbrella sub-list; in 2026 it has been
# split into ~11 Level-4 sub-lists. We aggregate across the science-
# adjacent ones; the developer trims to children's-curriculum-relevant
# entries by hand afterwards.
_VITAL_ARTICLES_PAGES = (
    "Wikipedia:Vital articles/Level 4/Physical sciences",
    "Wikipedia:Vital articles/Level 4/Biological and health sciences",
)
_API_URL = "https://en.wikipedia.org/w/api.php"

_LINK_RE = re.compile(r"\[\[([^\]\|#]+?)(?:\|[^\]]*)?\]\]")
# Wikipedia namespace prefixes that mark a link as non-article. A leading
# colon is also a "linked-as-text" marker (e.g. `[[:Category:Foo]]`); we
# strip it before matching so `Category:Foo` and `:Category:Foo` are both
# rejected. The interwiki prefixes (`m:`, `s:`, `c:`, `d:`, `meta:`) point
# to other Wikimedia projects and are never article candidates.
_NAMESPACE_PREFIXES = (
    "File:", "Image:", "Category:", "Talk:", "Wikipedia:",
    "Help:", "Template:", "Portal:", "User:", "Module:",
    "Special:", "Media:",
    "m:", "meta:", "s:", "c:", "d:", "wikt:", "b:", "n:", "q:", "v:",
)


def fetch_vital_articles_wikitext(http_client, page: str) -> str:
    """Fetch the wikitext of a single Vital Articles list page.

    Follows redirects via `redirects=1` so renamed pages still resolve.
    """
    params = {
        "action": "query",
        "prop": "revisions",
        "rvprop": "content",
        "rvslots": "main",
        "titles": page,
        "format": "json",
        "redirects": 1,
    }
    resp = http_client.get(_API_URL, params=params, timeout=30.0)
    resp.raise_for_status()
    data = resp.json()
    page_data = next(iter(data["query"]["pages"].values()))
    return page_data["revisions"][0]["slots"]["main"]["*"]


def extract_titles(wikitext: str) -> list[str]:
    """Pull article titles out of `[[Title]]` and `[[Title|display]]` links.

    Filters out namespace-prefixed links (categories, files, templates).
    Preserves order; deduplicates.
    """
    seen: set[str] = set()
    out: list[str] = []
    for m in _LINK_RE.finditer(wikitext):
        title = m.group(1).strip().lstrip(":")
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

    aggregated: list[str] = []
    seen: set[str] = set()
    for page in _VITAL_ARTICLES_PAGES:
        wikitext = fetch_vital_articles_wikitext(sess, page)
        for title in extract_titles(wikitext):
            if title not in seen:
                seen.add(title)
                aggregated.append(title)

    if args.limit is not None:
        aggregated = aggregated[: args.limit]
    for t in aggregated:
        print(t)
    print(
        f"\n# {len(aggregated)} candidates fetched from "
        f"{len(_VITAL_ARTICLES_PAGES)} Vital Articles pages",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
