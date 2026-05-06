"""Tests for to_passage — record → SeedPassage-compatible dict."""
import pytest
from simple_wikipedia import to_passage


def test_basic_record():
    record = {
        "title": "Photosynthesis",
        "lead_text": "Photosynthesis is a process used by plants and other organisms.",
        "canonical_url": "https://simple.wikipedia.org/wiki/Photosynthesis",
    }
    p = to_passage(record)
    assert p == {
        "id": "wiki-simple:en:photosynthesis",
        "source": "wiki-simple:en:photosynthesis",
        "license": "CC-BY-SA-3.0",
        "attribution": "'Photosynthesis' from Simple English Wikipedia, licensed under CC-BY-SA-3.0",
        "source_url": "https://simple.wikipedia.org/wiki/Photosynthesis",
        "text": "Photosynthesis is a process used by plants and other organisms.",
        "topics": ["wikipedia", "simple-english", "science", "photosynthesis"],
    }


def test_multiword_title():
    record = {
        "title": "Black hole",
        "lead_text": "A black hole is a region of spacetime.",
        "canonical_url": "https://simple.wikipedia.org/wiki/Black_hole",
    }
    p = to_passage(record)
    assert p["id"] == "wiki-simple:en:black-hole"
    assert p["topics"] == ["wikipedia", "simple-english", "science", "black-hole"]


def test_attribution_uses_original_title_capitalisation():
    # The slug is lowercased; the attribution preserves the title verbatim.
    record = {
        "title": "DNA",
        "lead_text": "Deoxyribonucleic acid (DNA) is a molecule.",
        "canonical_url": "https://simple.wikipedia.org/wiki/DNA",
    }
    p = to_passage(record)
    assert "'DNA'" in p["attribution"]
    assert p["id"] == "wiki-simple:en:dna"


def test_short_lead_raises():
    # Sanity guard: lead < 30 words is suspicious. The pipeline warns
    # rather than fails on these (whitelist hand-review), but `to_passage`
    # itself does not enforce length — that's the pipeline's job. Verify
    # `to_passage` happily produces a record for short text so the
    # responsibility is clear.
    record = {
        "title": "Test",
        "lead_text": "Short.",
        "canonical_url": "https://example.com/Test",
    }
    p = to_passage(record)
    assert p["text"] == "Short."
    assert p["id"] == "wiki-simple:en:test"
