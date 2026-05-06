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


def test_math_artifacts_stripped_from_density_passage():
    # Real-world example: Simple English Wikipedia "Density" lead
    # includes a multi-line MathJax fallback block that the extracts
    # API leaves as garbage in the plain-text output. The math
    # paragraph (containing `\\displaystyle`) must be dropped; the
    # neighbouring prose paragraphs must survive intact.
    record = {
        "title": "Density",
        "lead_text": (
            "Density is a measurement that compares the amount of matter an "
            "object has to its volume.\n\n"
            "Density is found by dividing the mass of an object by its volume:\n\n"
            "  \n    \n      \n        ρ\n        =\n        \n          "
            "\n            m\n            V\n          \n        \n      \n    \n"
            "    {\\displaystyle \\rho ={\\frac {m}{V}}}\n  \n\n"
            "where ρ is the density."
        ),
        "canonical_url": "https://simple.wikipedia.org/wiki/Density",
    }
    p = to_passage(record)
    assert "\\displaystyle" not in p["text"]
    assert "Density is a measurement" in p["text"]
    assert "where ρ is the density." in p["text"]


def test_math_artifacts_stripper_leaves_plain_text_untouched():
    # No `\\displaystyle` marker → text passes through unchanged
    # (modulo trailing whitespace stripping).
    record = {
        "title": "Photosynthesis",
        "lead_text": (
            "Photosynthesis is a process used by plants.\n\n"
            "It converts light energy into chemical energy.\n\n"
            "This is how plants make food."
        ),
        "canonical_url": "https://simple.wikipedia.org/wiki/Photosynthesis",
    }
    p = to_passage(record)
    expected = (
        "Photosynthesis is a process used by plants.\n\n"
        "It converts light energy into chemical energy.\n\n"
        "This is how plants make food."
    )
    assert p["text"] == expected
