"""Klexikon MediaWiki -> plain-text stripper.

Klexikon's MediaWiki has no TextExtracts extension, so the Klexikon
fetch strategy retrieves raw wikitext via ``action=parse&prop=wikitext
&section=0`` and converts it to plain text here. Pure functions, no
I/O -- tested directly in ``tests/test_wikitext_strip.py``.

The pipeline (in :func:`strip_klexikon_wikitext`) handles real-world
wikitext idioms found in Klexikon leads: image and category drop-blocks
with balanced-bracket scanning (image captions can themselves contain
nested links), plain and piped wiki links, templates, references in
both pair and self-closing forms, HTML comments, ``<gallery>`` blocks,
bold/italic markers, and whitespace cleanup.
"""
from __future__ import annotations

import re


# Link prefixes whose entire ``[[...]]`` block is dropped (rather than
# flattened to display text). Two classes:
#
# - Image links: ``Datei:`` (German), ``Bild:`` (older German),
#   ``File:`` (English), ``Image:`` (English). Captions can contain
#   nested ``[[link]]`` calls so balanced-bracket scanning is needed.
# - Category links: ``Kategorie:`` (German), ``Category:`` (English).
#   Always render invisibly in the wiki UI; in plain text they leak as
#   raw ``Kategorie:Foo`` strings. Categories don't carry captions, so
#   the same balanced-bracket scanner handles them safely.
_DROP_BLOCK_PREFIXES = (
    "Datei:",
    "Bild:",
    "File:",
    "Image:",
    "Kategorie:",
    "Category:",
)

# Maximum length of any prefix in `_DROP_BLOCK_PREFIXES`. Used as the
# look-ahead window when scanning for a drop-block opener.
_DROP_BLOCK_PREFIX_LOOKAHEAD = max(len(p) for p in _DROP_BLOCK_PREFIXES)


def _strip_balanced_drop_blocks(wikitext: str) -> str:
    """Remove ``[[<prefix>:...]]`` image and category blocks.

    Image captions can themselves contain wiki links (``[[Datei:Foo
    .jpg|mini|see [[Bar]]]]``). Regex can't naturally handle balanced
    brackets, so we walk the text manually, tracking the ``[[`` /
    ``]]`` depth from the opening of any drop-prefixed link until we
    return to depth 0. Everything inside (and including the outer
    brackets) is dropped.
    """
    out: list[str] = []
    i = 0
    n = len(wikitext)
    while i < n:
        if wikitext.startswith("[[", i):
            inner = wikitext[i + 2 : i + 2 + _DROP_BLOCK_PREFIX_LOOKAHEAD]
            matched_prefix = next(
                (p for p in _DROP_BLOCK_PREFIXES if inner.startswith(p)),
                None,
            )
            if matched_prefix is not None:
                depth = 1
                j = i + 2
                while j < n and depth > 0:
                    if wikitext.startswith("[[", j):
                        depth += 1
                        j += 2
                    elif wikitext.startswith("]]", j):
                        depth -= 1
                        j += 2
                    else:
                        j += 1
                # `j` is now just past the matching `]]` (or end of
                # input on malformed wikitext). Skip the whole block.
                i = j
                continue
        out.append(wikitext[i])
        i += 1
    return "".join(out)


# Match a balanced (single-pair) wiki link: `[[target]]` or
# `[[target|display]]`. Targets and display text must not contain
# brackets or vertical bars (we strip image blocks first so the
# remaining links are the simple variety).
_WIKILINK_PIPED_RE = re.compile(r"\[\[[^\[\]\|]*\|([^\[\]\|]+?)\]\]")
_WIKILINK_PLAIN_RE = re.compile(r"\[\[([^\[\]\|]+?)\]\]")

# Templates `{{...}}`. Single-level only -- nested templates are rare
# in Klexikon leads.
_TEMPLATE_RE = re.compile(r"\{\{[^{}]*\}\}")

# References `<ref>...</ref>`, `<ref name="x">...</ref>`, and the
# self-closing `<ref name="x"/>` form.
_REF_PAIR_RE = re.compile(r"<ref(?:\s[^>]*)?>.*?</ref>", re.DOTALL | re.IGNORECASE)
_REF_SELF_RE = re.compile(r"<ref(?:\s[^>]*)?/>", re.IGNORECASE)

# HTML comments.
_HTML_COMMENT_RE = re.compile(r"<!--.*?-->", re.DOTALL)

# `<gallery>...</gallery>` is a MediaWiki extension that renders a
# grid of images. Inside the block, each line is `Filename.jpg|caption`
# (NOT a `[[File:...]]` bracketed link, so the drop-block scanner
# above does not catch it). Whole block is dropped. The opening tag
# may carry attributes (`<gallery mode="packed">`).
_GALLERY_RE = re.compile(r"<gallery(?:\s[^>]*)?>.*?</gallery>", re.DOTALL | re.IGNORECASE)

# Bold (`'''...'''`) and italic (`''...''`) markers. Order matters:
# strip the longer marker first so `'''bold'''` doesn't leave a
# dangling `'`.
_BOLD_RE = re.compile(r"'''(.+?)'''")
_ITALIC_RE = re.compile(r"''(.+?)''")

# Run of three or more consecutive newlines -> collapse to two
# (preserve paragraph breaks; drop stranded blanks left by image
# block removal).
_BLANKLINE_RUN_RE = re.compile(r"\n{3,}")

# Run of two or more spaces or tabs *within a line* -> collapse to a
# single space. Leaves newlines alone so paragraph breaks survive.
# Removes the visible artefact left by template/ref removal where the
# surrounding spaces collapse together (e.g. ``Vor  war es anders.``
# from ``Vor {{template}} war es anders.``).
_INLINE_WS_RUN_RE = re.compile(r"[ \t]{2,}")


def strip_klexikon_wikitext(wikitext: str) -> str:
    """Convert MediaWiki wikitext (lead-section shape) to plain text.

    Pipeline:

    1. Remove HTML comments and ``<ref>`` markers (both pair and
       self-closing forms) -- they contain neither curriculum content
       nor useful retrieval signal.
    2. Remove image blocks (``[[Datei:...]]`` / ``[[File:...]]`` etc.)
       via balanced-bracket scanning. Image captions can contain
       nested ``[[link]]`` calls; the whole block is dropped.
    3. Flatten remaining wiki links: ``[[a|b]]`` -> ``b``;
       ``[[a]]`` -> ``a``.
    4. Drop ``{{templates}}``.
    5. Strip bold (``'''x'''`` -> ``x``) and italic (``''x''`` -> ``x``)
       markers.
    6. Collapse runs of 3+ newlines to a paragraph break, then trim.

    Pure function -- no I/O, no module-level state. Tested directly in
    ``tests/test_wikitext_strip.py``.
    """
    text = _HTML_COMMENT_RE.sub("", wikitext)
    text = _REF_PAIR_RE.sub("", text)
    text = _REF_SELF_RE.sub("", text)
    text = _GALLERY_RE.sub("", text)
    text = _strip_balanced_drop_blocks(text)
    text = _WIKILINK_PIPED_RE.sub(r"\1", text)
    text = _WIKILINK_PLAIN_RE.sub(r"\1", text)
    text = _TEMPLATE_RE.sub("", text)
    text = _BOLD_RE.sub(r"\1", text)
    text = _ITALIC_RE.sub(r"\1", text)
    text = _INLINE_WS_RUN_RE.sub(" ", text)
    text = _BLANKLINE_RUN_RE.sub("\n\n", text)
    return text.strip()
