"""Tests for the WikiSource configuration object.

WikiSource bundles per-language Wikipedia ingest parameters so the same
pure-function pipeline (slugify → fetch → to_passage → JSONL) can serve
any Wikipedia-shaped source. Two presets ship today:

- ``SIMPLE_ENGLISH`` — Simple English Wikipedia (TextExtracts API; the
  Phase 0.2 path).
- ``KLEXIKON`` — German children's wiki at klexikon.zum.de (no
  TextExtracts; uses ``parse&prop=wikitext&section=0`` + a wikitext
  stripper). Selected over regular de.wikipedia.org because its
  hand-written ages-8-13 vocabulary fits the Primer's audience the way
  Simple English fits the English audience.

These tests pin the preset shapes against drift. A renamed field in
SIMPLE_ENGLISH would silently corrupt the existing
``wiki_passages.en.jsonl`` schema; a typo in KLEXIKON.api_url would
produce a 404 against the wrong host; etc. Cheap structural checks
here catch all of that before any HTTP traffic.
"""
from wiki.source import KLEXIKON, SIMPLE_ENGLISH, WikiSource


def test_simple_english_preset_matches_phase_0_2_schema():
    """The Phase 0.2 ingest produced ids like `wiki-simple:en:photosynthesis`,
    attributions like `'Foo' from Simple English Wikipedia, licensed
    under CC-BY-SA-3.0`, and queried `simple.wikipedia.org`. Pin all
    those so a future refactor cannot silently drift the established
    seed corpus schema."""
    assert SIMPLE_ENGLISH.pack_id == "en"
    assert SIMPLE_ENGLISH.api_url == "https://simple.wikipedia.org/w/api.php"
    assert SIMPLE_ENGLISH.id_prefix == "wiki-simple"
    assert SIMPLE_ENGLISH.human_label == "Simple English Wikipedia"
    assert SIMPLE_ENGLISH.license == "CC-BY-SA-3.0"
    # Topic tags include the language-edition identifier and the broad
    # subject curation marker (the English whitelist is curated to
    # children's science). Both are useful as searchable metadata.
    assert "simple-english" in SIMPLE_ENGLISH.topic_tags
    assert "science" in SIMPLE_ENGLISH.topic_tags
    # Strategy: TextExtracts (single-shot batched). batch_size=20 is
    # the API's per-request cap.
    assert SIMPLE_ENGLISH.fetch_strategy == "text_extracts"
    assert SIMPLE_ENGLISH.batch_size == 20


def test_klexikon_preset_uses_klexikon_zum_de_subdomain():
    """Klexikon (klexikon.zum.de) is a hand-curated children's wiki
    written for ages 8-13. Its license is CC BY-SA 4.0 (per the
    site's About page and per-page footer), not 3.0."""
    assert KLEXIKON.pack_id == "de"
    assert KLEXIKON.api_url == "https://klexikon.zum.de/api.php"
    assert KLEXIKON.id_prefix == "wiki-klexikon"
    assert KLEXIKON.human_label == "Klexikon"
    assert KLEXIKON.license == "CC-BY-SA-4.0"
    assert "klexikon" in KLEXIKON.topic_tags
    # Klexikon uses the parse API per-page (no TextExtracts, no
    # batching). batch_size=1 forces the outer pipeline loop to
    # iterate one title at a time, applying the inter-batch sleep
    # as a per-request throttle.
    assert KLEXIKON.fetch_strategy == "klexikon_wikitext"
    assert KLEXIKON.batch_size == 1


def test_presets_have_distinct_pack_ids():
    """Each preset must own a unique pack_id; the
    `<id_prefix>:<pack_id>:<slug>` id-shape relies on (id_prefix,
    pack_id) being unique per source."""
    assert SIMPLE_ENGLISH.pack_id != KLEXIKON.pack_id


def test_presets_have_distinct_api_urls():
    """Both presets must point at different MediaWiki endpoints — a
    typo'd shared URL would silently produce wrong-language passages."""
    assert SIMPLE_ENGLISH.api_url != KLEXIKON.api_url


def test_wiki_source_disambiguation_patterns_compile():
    """Each preset's `disambiguation_patterns` must be a non-empty
    tuple of compiled regex objects. An empty tuple would silently
    disable disambiguation detection for that language — a class of
    bug we'd rather catch structurally."""
    for preset in (SIMPLE_ENGLISH, KLEXIKON):
        assert preset.disambiguation_patterns, (
            f"{preset.pack_id}: disambiguation_patterns must be non-empty"
        )
        for pat in preset.disambiguation_patterns:
            assert hasattr(pat, "search"), (
                f"{preset.pack_id}: pattern is not a compiled regex: {pat!r}"
            )


def test_lead_anchored_marker_matches_at_lead_start():
    """A subject-predicate marker right after a short title-as-subject
    (the disambiguation-lead shape) is matched."""
    from wiki.source import _lead_anchored_marker

    pat = _lead_anchored_marker("may refer to")
    assert pat.search("Mercury may refer to: the planet, the element.")
    assert pat.search("may refer to: ...")  # marker at the very start


def test_lead_anchored_marker_ignores_marker_past_the_subject_window():
    """A marker buried past the subject window — the prose false-positive
    case from issue #41 — is NOT matched, even though the phrase is
    present."""
    from wiki.source import _DISAMBIGUATION_SUBJECT_MAX_CHARS, _lead_anchored_marker

    pat = _lead_anchored_marker("may refer to")
    # Pad the subject so the marker starts well past the window.
    prefix = "x" * (_DISAMBIGUATION_SUBJECT_MAX_CHARS + 5) + " "
    assert pat.search(prefix + "may refer to data") is None
    # A realistic prose lead that merely contains the phrase mid-sentence.
    assert (
        pat.search(
            "In computer science, a reference is a value that may refer to data."
        )
        is None
    )


def test_lead_anchored_marker_is_case_insensitive_and_first_line_only():
    """The marker match is case-insensitive (extracts vary in casing)
    and ``.`` excludes newlines, so a marker on a later line does not
    match through the title line."""
    from wiki.source import _lead_anchored_marker

    pat = _lead_anchored_marker("steht für")
    assert pat.search("Saturn STEHT FÜR: den Planeten.")
    # Marker on a second line is not reachable from the lead-start anchor.
    assert pat.search("Saturn\nsteht für: den Planeten.") is None


def test_wiki_source_is_a_frozen_dataclass():
    """WikiSource carries process-wide configuration; mutating an
    instance after construction would silently change every passage
    produced by the pipeline. Frozen dataclasses prevent that class
    of bug at runtime."""
    import dataclasses
    assert dataclasses.is_dataclass(WikiSource)
    fields = dataclasses.fields(WikiSource)
    assert fields, "WikiSource must declare at least one field"
    import pytest
    with pytest.raises(dataclasses.FrozenInstanceError):
        SIMPLE_ENGLISH.pack_id = "xx"  # type: ignore[misc]


def test_site_root_strips_article_path():
    # Issue #40: the umbrella source_url is the site root, derived from
    # web_base_url (which is the article-path prefix), not the full prefix.
    from wiki.source import _site_root

    assert _site_root("https://simple.wikipedia.org/wiki/") == "https://simple.wikipedia.org/"
    assert _site_root("https://klexikon.zum.de/wiki/") == "https://klexikon.zum.de/"


def test_site_root_preserves_port_and_handles_bare_root():
    from wiki.source import _site_root

    assert _site_root("http://localhost:8080/wiki/") == "http://localhost:8080/"
    assert _site_root("https://example.org/") == "https://example.org/"


def test_umbrella_id_is_id_prefix_and_pack_id():
    # Drift guard: the umbrella id the emitter builds must be exactly
    # "<id_prefix>:<pack_id>" so it matches each child's "<...>:<slug>" family.
    assert SIMPLE_ENGLISH.id_prefix + ":" + SIMPLE_ENGLISH.pack_id == "wiki-simple:en"
    assert KLEXIKON.id_prefix + ":" + KLEXIKON.pack_id == "wiki-klexikon:de"


def test_unknown_fetch_strategy_is_rejected():
    """A typo in `fetch_strategy` would silently fall through to
    whichever default branch the dispatch picks. Validate the field
    at construction time so the error fires near the typo, not
    deeply inside `fetch_lead`."""
    import pytest
    import re
    with pytest.raises(ValueError, match="fetch_strategy"):
        WikiSource(
            pack_id="xx",
            api_url="https://example.invalid/api.php",
            web_base_url="https://example.invalid/wiki/",
            id_prefix="wiki-bogus",
            human_label="Bogus",
            license="CC0",
            topic_tags=("bogus",),
            disambiguation_patterns=(re.compile(r"\bnope\b"),),
            fetch_strategy="not_a_real_strategy",
            batch_size=1,
        )
