"""Tests for slugify — pure function, no I/O."""
import pytest
from simple_wikipedia import slugify


def test_ascii_lowercase():
    assert slugify("Photosynthesis") == "photosynthesis"


def test_spaces_to_hyphens():
    assert slugify("Black hole") == "black-hole"


def test_punctuation_stripped_to_hyphens():
    # Period + space collapse to a single hyphen
    assert slugify("E. coli") == "e-coli"


def test_apostrophe_stripped():
    assert slugify("'Photosynthesis'") == "photosynthesis"


def test_multiple_words_with_hyphens():
    assert slugify("Solar system") == "solar-system"


def test_runs_of_punctuation_collapse():
    # "AC/DC" is not a science article; included as a slugify edge case
    # showing that runs of non-alphanumerics collapse into a single hyphen.
    assert slugify("AC/DC") == "ac-dc"


def test_unicode_preserved():
    # "Café" should produce a recognisable slug. Whether the é is
    # preserved or transliterated is implementation-defined; here we
    # require that the result is non-empty and lowercase.
    s = slugify("Café")
    assert s
    assert s == s.lower()
    # Must be NFC-normalised so precomposed and decomposed forms match
    precomposed = "Café"  # é = U+00E9
    decomposed = "Café"   # e + U+0301
    assert slugify(precomposed) == slugify(decomposed)


def test_leading_trailing_hyphens_trimmed():
    assert slugify("--Hello--") == "hello"


def test_empty_input_raises():
    with pytest.raises(ValueError):
        slugify("")


def test_only_punctuation_raises():
    with pytest.raises(ValueError):
        slugify("---")
    with pytest.raises(ValueError):
        slugify("!@#")
