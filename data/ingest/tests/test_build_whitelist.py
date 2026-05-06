"""Tests for the whitelist-builder helper."""
from build_whitelist import extract_titles


def test_extracts_simple_links():
    wikitext = "Some prose with [[Photosynthesis]] and [[Black hole]] mentions."
    assert extract_titles(wikitext) == ["Photosynthesis", "Black hole"]


def test_handles_pipe_aliased_links():
    wikitext = "[[Black hole|black holes]] are dense."
    assert extract_titles(wikitext) == ["Black hole"]


def test_filters_namespace_prefixes():
    wikitext = (
        "[[Photosynthesis]] [[File:Sun.jpg]] [[Category:Astronomy]] "
        "[[Wikipedia:Vital articles]] [[Talk:Black hole]] [[Black hole]]"
    )
    assert extract_titles(wikitext) == ["Photosynthesis", "Black hole"]


def test_dedupes_repeated_titles():
    wikitext = "[[Photosynthesis]] later mentioned again as [[Photosynthesis]]"
    assert extract_titles(wikitext) == ["Photosynthesis"]


def test_skips_anchor_only_links():
    # [[#section]] is an in-page anchor, not an article. Our regex's
    # `[^\]\|#]+?` excludes anything containing #.
    wikitext = "[[#references]] [[Photosynthesis]]"
    assert extract_titles(wikitext) == ["Photosynthesis"]


def test_strips_leading_colon_before_namespace_check():
    # `[[:Category:Foo]]` is the "linked-as-text, not categorised"
    # form. The leading colon must be stripped before namespace
    # filtering so `:Category:Foo` and `Category:Foo` both get rejected.
    wikitext = "[[:Category:Astronomy|astronomy]] [[Photosynthesis]]"
    assert extract_titles(wikitext) == ["Photosynthesis"]


def test_filters_interwiki_and_special_prefixes():
    # Interwiki prefixes (m:, meta:, s:, etc.) point to sister wikis
    # and are never article candidates. `Special:` is the special
    # namespace. None should leak through.
    wikitext = (
        "[[Photosynthesis]] [[m:List of articles]] "
        "[[Special:Recentchanges]] [[meta:Hub]] [[Black hole]]"
    )
    assert extract_titles(wikitext) == ["Photosynthesis", "Black hole"]
