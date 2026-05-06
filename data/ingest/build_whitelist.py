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
# Wikipedia editors. Level/4 has ~1000 entries; the science subset is
# what we want. Note that the article *titles* are stable across both
# wikis — Simple English Wikipedia uses the same titles for the same
# concepts. We pull from English to get the curated list, then fetch
# leads from Simple English in the main pipeline.
_VITAL_ARTICLES_PAGE = "Wikipedia:Vital articles/Level/4/Science"
_API_URL = "https://en.wikipedia.org/w/api.php"

_LINK_RE = re.compile(r"\[\[([^\]\|#]+?)(?:\|[^\]]*)?\]\]")
_NAMESPACE_PREFIXES = (
    "File:", "Image:", "Category:", "Talk:", "Wikipedia:",
    "Help:", "Template:", "Portal:", "User:", "Module:",
)


def fetch_vital_articles_wikitext(http_client) -> str:
    params = {
        "action": "query",
        "prop": "revisions",
        "rvprop": "content",
        "rvslots": "main",
        "titles": _VITAL_ARTICLES_PAGE,
        "format": "json",
    }
    resp = http_client.get(_API_URL, params=params, timeout=30.0)
    resp.raise_for_status()
    data = resp.json()
    page = next(iter(data["query"]["pages"].values()))
    return page["revisions"][0]["slots"]["main"]["*"]


def extract_titles(wikitext: str) -> list[str]:
    """Pull article titles out of `[[Title]]` and `[[Title|display]]` links.

    Filters out namespace-prefixed links (categories, files, templates).
    Preserves order; deduplicates.
    """
    seen: set[str] = set()
    out: list[str] = []
    for m in _LINK_RE.finditer(wikitext):
        title = m.group(1).strip()
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
    wikitext = fetch_vital_articles_wikitext(sess)
    titles = extract_titles(wikitext)
    if args.limit is not None:
        titles = titles[: args.limit]
    for t in titles:
        print(t)
    print(
        f"\n# {len(titles)} candidates fetched from {_VITAL_ARTICLES_PAGE}",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
