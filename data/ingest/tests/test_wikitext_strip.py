"""Tests for the wikitext → plain-text stripper.

The stripper is the load-bearing piece of the Klexikon (German children's
wiki) ingest path. Klexikon's MediaWiki has no TextExtracts extension, so
we fetch ``action=parse&prop=wikitext&section=0`` and strip wiki markup
ourselves to produce a clean plain-text lead.

Each test pins one canonical wikitext idiom seen in real Klexikon
articles. If a future Klexikon edition introduces a new markup form we
don't strip (e.g. tables, math templates), the symptom would be raw
markup leaking into the seed corpus — these tests catch that as soon as
the pattern is added to a fixture.
"""
import pytest
from simple_wikipedia import strip_klexikon_wikitext


# ─── Wiki link conversion ────────────────────────────────────────────────


def test_strip_simple_wiki_link_keeps_target_text():
    # `[[Wetter]]` → "Wetter": no display alternative, the target
    # serves as the display text. This is the dominant link form in
    # Klexikon leads.
    assert strip_klexikon_wikitext("Das [[Wetter]] ist anders.") == (
        "Das Wetter ist anders."
    )


def test_strip_piped_wiki_link_keeps_display_text():
    # `[[Tropen|tropisch]]` → "tropisch": piped form where the target
    # differs from the display text (used to attach an inflected form
    # of the German noun to the canonical lemma article).
    assert strip_klexikon_wikitext("Im [[Tropen|tropisch]]en Regenwald.") == (
        "Im tropischen Regenwald."
    )


def test_strip_multiple_links_in_one_sentence():
    src = "Die [[Pflanzen]] brauchen [[Sonne]] und [[Wasser]]."
    assert strip_klexikon_wikitext(src) == (
        "Die Pflanzen brauchen Sonne und Wasser."
    )


# ─── Image blocks ────────────────────────────────────────────────────────


def test_strip_simple_datei_image_block():
    # `[[Datei:Foo.jpg|mini|caption]]` → "" (whole block is dropped).
    src = "[[Datei:Foo.jpg|mini|Caption text]]\n\nBody paragraph."
    assert strip_klexikon_wikitext(src) == "Body paragraph."


def test_strip_datei_image_block_with_nested_link_in_caption():
    # Real Klexikon Klima page: the image caption itself contains a
    # `[[Thailand]]` link. The whole image block — including the
    # nested link — must be removed; the body paragraph below must
    # survive intact.
    src = (
        "[[Datei:Foo.jpg|mini|[[Thailand]]: Hier im [[Tropen|tropisch]]"
        "en [[Regenwald]] wachsen [[Pflanzen]] sehr gut.]]\n"
        "Wenn man vom Klima spricht."
    )
    assert strip_klexikon_wikitext(src) == "Wenn man vom Klima spricht."


def test_strip_handles_alternative_image_prefixes():
    # Older / English-style prefixes still appear in some Klexikon
    # articles. All four should be recognised as image blocks.
    for prefix in ("Datei", "Bild", "File", "Image"):
        src = f"[[{prefix}:foo.jpg|mini|caption]]\nBody text."
        assert strip_klexikon_wikitext(src) == "Body text.", (
            f"prefix {prefix!r} should be recognised as an image block"
        )


# ─── Templates ──────────────────────────────────────────────────────────


def test_strip_inline_template():
    # `{{template}}` → "". Templates rarely appear in Klexikon leads
    # but when they do they're typically date or formatting helpers
    # that are useless in plain text.
    assert strip_klexikon_wikitext("Vor {{Jahr|2020}} war es anders.") == (
        "Vor war es anders."
    )


# ─── Categories ─────────────────────────────────────────────────────────


def test_strip_german_category_link():
    # `[[Kategorie:Foo]]` is a navigational link to a category page.
    # The MediaWiki convention is to render it invisibly at the page
    # bottom; in plain text it serves no curriculum purpose. Must be
    # dropped entirely (not flattened to display text), otherwise raw
    # `Kategorie:Foo` strings leak into the seed corpus.
    src = (
        "Echter Inhalt des Artikels.\n\n"
        "[[Kategorie:Tiere und Natur]]\n"
        "[[Kategorie:Erdkunde]]"
    )
    out = strip_klexikon_wikitext(src)
    assert "Kategorie" not in out
    assert "Echter Inhalt" in out


def test_strip_english_category_link():
    # English MediaWiki convention; appears in articles cross-edited
    # from English Wikipedia. Same rule as German categories.
    src = "Real article body.\n\n[[Category:Science]]"
    out = strip_klexikon_wikitext(src)
    assert "Category" not in out
    assert "Real article body" in out


# ─── Gallery blocks ─────────────────────────────────────────────────────


def test_strip_gallery_block():
    # MediaWiki's `<gallery>...</gallery>` extension renders a grid of
    # images. Inside the block, each line is `Filename.jpg|caption`
    # — NOT the `[[File:...]]` bracketed form. The whole block (start
    # tag through close tag) must be removed; the prose around it
    # must survive intact.
    src = (
        "First paragraph stays.\n\n"
        "<gallery>\n"
        "Foo.jpg|First caption\n"
        "Bar.jpg|Second caption with a link [[Sun]]\n"
        "</gallery>\n\n"
        "Second paragraph also stays."
    )
    out = strip_klexikon_wikitext(src)
    assert "<gallery" not in out
    assert "</gallery>" not in out
    assert "Foo.jpg" not in out
    assert "First caption" not in out
    assert "First paragraph stays." in out
    assert "Second paragraph also stays." in out


def test_strip_gallery_block_with_attributes():
    # `<gallery>` opening tags often carry attributes
    # (`<gallery mode="packed">`, etc.). The strip rule must match
    # those too.
    src = (
        "Body text.\n"
        '<gallery mode="packed" widths="200">\n'
        "Foo.jpg|caption\n"
        "</gallery>"
    )
    out = strip_klexikon_wikitext(src)
    assert "gallery" not in out
    assert "Body text." in out


# ─── References and HTML ────────────────────────────────────────────────


def test_strip_reference_tags_and_content():
    # `<ref>...</ref>` → "". The cited content is rarely useful and
    # the markup itself is garbage in retrieval text.
    src = "Klima ist langfristig.<ref>Quelle: Beispiel</ref> Wetter ist kurz."
    assert strip_klexikon_wikitext(src) == "Klima ist langfristig. Wetter ist kurz."


def test_strip_self_closing_reference_tag():
    src = 'Wetter ist kurz.<ref name="x"/> Klima ist lang.'
    assert strip_klexikon_wikitext(src) == "Wetter ist kurz. Klima ist lang."


def test_strip_html_comment():
    src = "<!-- editor note -->Real text."
    assert strip_klexikon_wikitext(src) == "Real text."


# ─── Bold and italic ────────────────────────────────────────────────────


def test_strip_bold_and_italic_markup():
    # `'''bold'''` → "bold"; `''italic''` → "italic". The text content
    # survives; the apostrophes are the wiki-markup wrapper.
    src = "Das ist '''wichtig''' und ''sehr'' interessant."
    assert strip_klexikon_wikitext(src) == "Das ist wichtig und sehr interessant."


# ─── Whitespace ─────────────────────────────────────────────────────────


def test_collapses_whitespace_left_after_stripping():
    # When an image block + a trailing newline are removed, the
    # resulting text should not have leading whitespace or stranded
    # blank lines. Verify the cleanup step works end-to-end.
    src = (
        "[[Datei:Foo.jpg|mini|caption]]\n\n\n"
        "First paragraph.\n\n\n\n"
        "Second paragraph."
    )
    out = strip_klexikon_wikitext(src)
    # Tolerate paragraph break (single \n\n) but no triple-blanks.
    assert out.startswith("First paragraph")
    assert out.endswith("Second paragraph.")
    assert "\n\n\n" not in out


def test_stripper_preserves_unicode_diacritics():
    # ä, ö, ü, ß must survive in the output text — only markup is
    # stripped. (The slug step would transliterate; the lead text
    # stays in native German.)
    src = "Der Bär läuft über die [[Straße]] zum Fluss."
    assert strip_klexikon_wikitext(src) == (
        "Der Bär läuft über die Straße zum Fluss."
    )


def test_stripper_produces_plain_text_on_real_klexikon_lead_shape():
    # Smoke test against the real shape of the Klexikon "Klima" article
    # lead. Combines image caption with nested links + plain prose.
    src = (
        "[[Datei:Thai rain forest.jpg|mini|[[Thailand]]: Hier im "
        "[[Tropen|tropisch]]en [[Regenwald]] wachsen [[Pflanzen]] sehr gut.]]\n"
        "Wenn man vom Klima spricht, ist gemeint, dass es irgendwo "
        "normalerweise warm oder kalt ist. Das [[Wetter]] ist etwas "
        "Ähnliches, aber vom Wetter spricht man, wenn man an einen Tag "
        "oder wenige [[Woche]]n denkt."
    )
    out = strip_klexikon_wikitext(src)
    # The image and its nested links must be entirely gone.
    assert "Datei:" not in out
    assert "[[" not in out
    assert "]]" not in out
    # Body links must be flattened to their display text.
    assert "Wetter" in out
    assert "Wochen" in out
    # No raw markup tokens leaked.
    assert "|" not in out
    # Starts at the prose, not at a leading newline or stranded space.
    assert out.startswith("Wenn man vom Klima spricht")
