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
