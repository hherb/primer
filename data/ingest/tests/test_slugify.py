"""Tests for slugify — pure function, no I/O."""
import pytest
from wiki.source import (
    _assert_unique_passage_ids,
    _assert_unique_slugs,
    slugify,
)


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


# ─── German orthography ──────────────────────────────────────────────────
# These tests pin the slugify behaviour for German titles. The previous
# implementation used `str.lower()` + NFD-strip-combining, which silently
# DROPPED ß (no NFD decomposition + non-ASCII → stripped by the
# alphanumeric regex). `Straße` became `strae` — a silent corruption
# that would have produced unfindable passage ids in a German seed
# corpus. The cure is `str.casefold()` which performs proper Unicode
# case folding (ß → ss, capital ẞ → ss, etc.).


def test_slugify_eszett_becomes_ss():
    # The standard German transliteration: ß → ss. Critical for the
    # German Wikipedia ingest because many science article titles use ß
    # (Größe, Maß, Schließe, etc.).
    assert slugify("Straße") == "strasse"


def test_slugify_capital_eszett_becomes_ss():
    # The capital ẞ (U+1E9E, added to Unicode in 2008) is rare but
    # exists in modern German typography. casefold() handles both.
    assert slugify("STRAẞE") == "strasse"


def test_slugify_umlauts_decompose_to_base_letter():
    # ä → a, ö → o, ü → u via NFD + combining-strip.
    # Note: the German "long form" transliteration would be ä→ae, ö→oe,
    # ü→ue. We deliberately use the simpler base-letter form here for
    # consistency with the existing English NFD path; the trade-off is
    # that "Müller" and "Muller" would produce the same slug. For
    # children's-curriculum article titles this is acceptable; the
    # _assert_unique_slugs check would catch any real collision in the
    # whitelist.
    assert slugify("Ökologie") == "okologie"
    assert slugify("Übermorgen") == "ubermorgen"
    assert slugify("Ärger") == "arger"


def test_slugify_compound_german_word_with_eszett_and_umlaut():
    # "Größe" combines both: ö-umlaut + ß. casefold() folds ß → ss
    # FIRST, then NFD strips the ö-combining mark.
    assert slugify("Größe") == "grosse"


def test_assert_unique_slugs_catches_eszett_post_fold_collision():
    # `Straße` and `Strasse` are distinct German words (the latter is
    # the spelling used in Switzerland and in pre-reform German), but
    # they slugify to the same id under casefold(). Catch the collision
    # loudly rather than silently dropping the second passage at load
    # time.
    with pytest.raises(ValueError, match="slug collision"):
        _assert_unique_slugs(["Straße", "Strasse"])


# ─── Post-resolution duplicate-id check ────────────────────────────────
# `_assert_unique_passage_ids` complements `_assert_unique_slugs`:
# the slug check runs on input titles BEFORE the API resolves redirects;
# the passage-id check runs on the resolved passages AFTER the API
# follows `redirects=1`. Two distinct input titles can collapse to the
# same canonical title at fetch time (the Klexikon Atom/Molekül
# scenario where both resolve to "Atome und Moleküle"); without this
# check, the second passage silently overwrites or drops the first.


def _passage_with_id(pid: str) -> dict:
    """Test helper: minimal passage dict with the given id."""
    return {"id": pid, "text": "x"}


def test_assert_unique_passage_ids_passes_for_distinct_ids():
    # No collisions; should not raise.
    _assert_unique_passage_ids(
        [
            ("Photosynthesis", _passage_with_id("wiki:en:photosynthesis")),
            ("Gravity", _passage_with_id("wiki:en:gravity")),
            ("Black hole", _passage_with_id("wiki:en:black-hole")),
        ]
    )


def test_assert_unique_passage_ids_catches_redirect_collision():
    # The Atom/Molekül scenario: on Klexikon, both input titles resolve
    # to the canonical "Atome und Moleküle" page, producing the same id.
    # The pre-resolution slug check passes (atom != molekul), so this
    # post-resolution check is the only line of defence.
    colliding_id = "wiki-klexikon:de:atome-und-molekule"
    with pytest.raises(RuntimeError, match="passage id collision"):
        _assert_unique_passage_ids(
            [
                ("Atom", _passage_with_id(colliding_id)),
                ("Molekül", _passage_with_id(colliding_id)),
            ]
        )


def test_assert_unique_passage_ids_error_names_both_inputs_and_id():
    # The error message should be actionable: it must let the developer
    # find the offending whitelist entries by name AND see which
    # canonical id they collapsed to.
    colliding_id = "wiki-klexikon:de:atome-und-molekule"
    with pytest.raises(RuntimeError) as ei:
        _assert_unique_passage_ids(
            [
                ("Atom", _passage_with_id(colliding_id)),
                ("Molekül", _passage_with_id(colliding_id)),
            ]
        )
    msg = str(ei.value)
    assert "'Atom'" in msg
    assert "'Molekül'" in msg
    assert colliding_id in msg
    assert "redirect resolution" in msg
