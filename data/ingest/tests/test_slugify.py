"""Tests for slugify — pure function, no I/O."""
import pytest
from simple_wikipedia import _assert_unique_slugs, slugify


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


def test_assert_unique_slugs_passes_for_distinct_titles():
    # No collisions; should not raise.
    _assert_unique_slugs(["Photosynthesis", "Gravity", "Black hole"])


def test_assert_unique_slugs_rejects_case_only_collision():
    # `read_whitelist` only catches byte-exact dups, so two case-different
    # entries pass through to the slug-collision check. Both produce "dna".
    with pytest.raises(ValueError, match="slug collision"):
        _assert_unique_slugs(["DNA", "dna"])


def test_assert_unique_slugs_rejects_punctuation_collision():
    # "Foo bar" and "foo-bar" both collapse to "foo-bar".
    with pytest.raises(ValueError, match="slug collision"):
        _assert_unique_slugs(["Foo bar", "foo-bar"])


def test_assert_unique_slugs_error_message_names_both_titles():
    # Use a slug-only collision (different surface form, different slug
    # casing) so the message is discriminable: both titles AND the
    # produced slug appear, helping the developer find the offending
    # whitelist entries.
    with pytest.raises(ValueError) as ei:
        _assert_unique_slugs(["Foo bar", "foo-bar"])
    msg = str(ei.value)
    assert "'Foo bar'" in msg
    assert "'foo-bar'" in msg
    assert "slug collision" in msg
