"""Pin the back-compat shim's re-exports to the submodule canonical objects.

`simple_wikipedia.py` re-exports every name the existing test suite
imports so the post-PR-#60 split stays invisible to consumers. The
contract is identity, not equality: ``simple_wikipedia.KLEXIKON is
wiki.source.KLEXIKON``. If a future edit accidentally rebinds a shim
name to a copy (or to a stale snapshot), monkeypatching the canonical
location would silently fail to affect the shim, and vice-versa. This
test fires loudly the moment that drift is introduced.
"""
from __future__ import annotations

import simple_wikipedia
from wiki import fetch as wiki_fetch
from wiki import source as wiki_source
from wiki import strip as wiki_strip


_SOURCE_NAMES = (
    "KLEXIKON",
    "SIMPLE_ENGLISH",
    "WikiSource",
    "_DE_DISAMBIGUATION_PATTERNS",
    "_EN_DISAMBIGUATION_PATTERNS",
    "_NON_ALNUM",
    "_SOURCES_BY_PACK_ID",
    "_VALID_FETCH_STRATEGIES",
    "_assert_unique_passage_ids",
    "_assert_unique_slugs",
    "_strip_math_artifacts",
    "read_whitelist",
    "slugify",
    "to_passage",
)

_STRIP_NAMES = (
    "_BLANKLINE_RUN_RE",
    "_BOLD_RE",
    "_DROP_BLOCK_PREFIXES",
    "_DROP_BLOCK_PREFIX_LOOKAHEAD",
    "_GALLERY_RE",
    "_HTML_COMMENT_RE",
    "_INLINE_WS_RUN_RE",
    "_ITALIC_RE",
    "_REF_PAIR_RE",
    "_REF_SELF_RE",
    "_TEMPLATE_RE",
    "_WIKILINK_PIPED_RE",
    "_WIKILINK_PLAIN_RE",
    "_strip_balanced_drop_blocks",
    "strip_klexikon_wikitext",
)

_FETCH_NAMES = (
    "_DEFAULT_USER_AGENT",
    "_DISAMBIGUATION_HEAD_CHARS",
    "_RETRY_SETTINGS",
    "_check_disambiguation",
    "_fetch_lead_via_klexikon",
    "_fetch_lead_via_text_extracts",
    "_fetch_leads_via_klexikon",
    "_fetch_leads_via_text_extracts",
    "_klexikon_canonical_url",
    "fetch_lead",
    "fetch_leads",
)


def test_source_names_resolve_to_canonical_objects():
    for name in _SOURCE_NAMES:
        assert getattr(simple_wikipedia, name) is getattr(wiki_source, name), (
            f"shim simple_wikipedia.{name} drifted from wiki.source.{name}"
        )


def test_strip_names_resolve_to_canonical_objects():
    for name in _STRIP_NAMES:
        assert getattr(simple_wikipedia, name) is getattr(wiki_strip, name), (
            f"shim simple_wikipedia.{name} drifted from wiki.strip.{name}"
        )


def test_fetch_names_resolve_to_canonical_objects():
    for name in _FETCH_NAMES:
        assert getattr(simple_wikipedia, name) is getattr(wiki_fetch, name), (
            f"shim simple_wikipedia.{name} drifted from wiki.fetch.{name}"
        )
