"""Tests for fetch_lead — uses an injected http_client so the test never
talks to the real network."""
import json
from pathlib import Path
import pytest
from wiki.fetch import _klexikon_canonical_url, fetch_lead, fetch_leads
from wiki.source import KLEXIKON, SIMPLE_ENGLISH


FIXTURES = Path(__file__).parent / "fixtures"


class FakeResponse:
    def __init__(
        self,
        payload: dict,
        status_code: int = 200,
        headers: dict[str, str] | None = None,
    ):
        self._payload = payload
        self.status_code = status_code
        # Default to empty dict so retry_http_get's
        # ``getattr(resp, "headers", {}).get("Retry-After")`` finds an
        # empty header set on the existing fixtures. Tests that need
        # to set Retry-After pass an explicit ``headers={...}``.
        self.headers = headers or {}

    def json(self) -> dict:
        return self._payload

    def raise_for_status(self) -> None:
        if self.status_code >= 400:
            raise RuntimeError(f"fake http error {self.status_code}")


class FakeHttpClient:
    """A `requests.Session`-compatible fake that returns canned payloads.

    Maps the requested `titles` query parameter to a response payload.
    """

    def __init__(self, responses: dict[str, dict]):
        self.responses = responses
        self.calls: list[dict] = []

    def get(self, url: str, params: dict, timeout: float | None = None):
        self.calls.append({"url": url, "params": params})
        title = params["titles"]
        if title not in self.responses:
            return FakeResponse({"query": {"pages": {"-1": {"missing": ""}}}})
        return FakeResponse(self.responses[title])


def _load_fixture(name: str) -> dict:
    return json.loads((FIXTURES / name).read_text(encoding="utf-8"))


def test_fetch_lead_returns_title_text_url():
    client = FakeHttpClient({"Photosynthesis": _load_fixture("photosynthesis.json")})
    result = fetch_lead("Photosynthesis", http_client=client, source=SIMPLE_ENGLISH)
    assert result["title"] == "Photosynthesis"
    assert "Photosynthesis is a process" in result["lead_text"]
    assert result["canonical_url"] == "https://simple.wikipedia.org/wiki/Photosynthesis"


def test_fetch_lead_uses_correct_api_endpoint():
    client = FakeHttpClient({"Photosynthesis": _load_fixture("photosynthesis.json")})
    fetch_lead("Photosynthesis", http_client=client, source=SIMPLE_ENGLISH)
    assert len(client.calls) == 1
    call = client.calls[0]
    assert "simple.wikipedia.org" in call["url"]
    assert call["params"]["action"] == "query"
    assert call["params"]["prop"] == "extracts|info"
    assert call["params"]["exintro"] == 1
    assert call["params"]["explaintext"] == 1
    assert call["params"]["inprop"] == "url"
    assert call["params"]["titles"] == "Photosynthesis"


def test_fetch_lead_missing_article_raises():
    # Article not in the fake's response map → the fake returns the
    # "missing" sentinel structure, which fetch_lead must detect.
    client = FakeHttpClient({})
    with pytest.raises(RuntimeError, match="not found"):
        fetch_lead("DoesNotExist", http_client=client, source=SIMPLE_ENGLISH)


def test_fetch_lead_empty_extract_raises():
    payload = {
        "query": {"pages": {"99": {"title": "Stub", "extract": "", "fullurl": "https://x"}}}
    }
    client = FakeHttpClient({"Stub": payload})
    with pytest.raises(RuntimeError, match="empty extract"):
        fetch_lead("Stub", http_client=client, source=SIMPLE_ENGLISH)


def test_fetch_lead_disambiguation_page_raises_can_mean():
    # Real-world example: Simple English Wikipedia "Base" article is a
    # disambiguation page whose extract starts with "Base can mean: ...".
    # The pipeline must reject this loudly so the developer fixes the
    # whitelist (e.g. to `Base (chemistry)`).
    payload = {
        "query": {
            "pages": {
                "111": {
                    "title": "Base",
                    "extract": "Base can mean various things in different fields including mathematics, sport, architecture, biology, and chemistry. See specific articles for each meaning.",
                    "fullurl": "https://simple.wikipedia.org/wiki/Base",
                }
            }
        }
    }
    client = FakeHttpClient({"Base": payload})
    with pytest.raises(RuntimeError, match="disambiguation page"):
        fetch_lead("Base", http_client=client, source=SIMPLE_ENGLISH)


def test_fetch_lead_disambiguation_page_raises_may_refer_to():
    # Variant disambiguation marker. Simple Wiki uses both phrasings.
    payload = {
        "query": {
            "pages": {
                "222": {
                    "title": "Saturn",
                    "extract": "Saturn may refer to: the planet, the Roman god, the rocket family, or the Saturn computer game console.",
                    "fullurl": "https://simple.wikipedia.org/wiki/Saturn",
                }
            }
        }
    }
    client = FakeHttpClient({"Saturn": payload})
    with pytest.raises(RuntimeError, match="disambiguation page"):
        fetch_lead("Saturn", http_client=client, source=SIMPLE_ENGLISH)


def test_fetch_lead_prose_may_refer_to_in_body_is_not_disambiguation():
    # Regression for issue #41: a genuine article whose lead merely
    # *contains* "may refer to" later in a sentence (not as the
    # lead-opening "<title> may refer to:") must NOT be rejected as a
    # disambiguation page. The pre-fix rule searched the whole 300-char
    # head and falsely flagged this article; the lead-anchored rule
    # only matches the marker right after the title-as-subject, so this
    # passes through. "Reference (computer science)" is the regression
    # fixture title the issue asks to keep unblocked across edits.
    lead = (
        "In computer science, a reference is a value that may refer to "
        "data stored elsewhere in memory, such as a variable or an object."
    )
    payload = {
        "query": {
            "pages": {
                "333": {
                    "title": "Reference (computer science)",
                    "extract": lead,
                    "fullurl": (
                        "https://simple.wikipedia.org/wiki/"
                        "Reference_(computer_science)"
                    ),
                }
            }
        }
    }
    client = FakeHttpClient({"Reference (computer science)": payload})
    result = fetch_lead(
        "Reference (computer science)",
        http_client=client,
        source=SIMPLE_ENGLISH,
    )
    assert result["title"] == "Reference (computer science)"
    assert "may refer to" in result["lead_text"]


def test_fetch_lead_explicit_disambiguation_marker_still_raises_in_body():
    # The explicit self-declaration marker ("is a disambiguation") is
    # deliberately NOT lead-anchored — it only ever appears on a
    # disambiguation page, so matching it anywhere in the head stays
    # correct and guards against a false negative. Pins the two-category
    # split introduced for issue #41.
    payload = {
        "query": {
            "pages": {
                "444": {
                    "title": "Mercury",
                    "extract": (
                        "Mercury is the name of several different things. "
                        "This article is a disambiguation page listing the "
                        "planet, the element, and the Roman god."
                    ),
                    "fullurl": "https://simple.wikipedia.org/wiki/Mercury",
                }
            }
        }
    }
    client = FakeHttpClient({"Mercury": payload})
    with pytest.raises(RuntimeError, match="disambiguation page"):
        fetch_lead("Mercury", http_client=client, source=SIMPLE_ENGLISH)


def test_fetch_leads_rejects_oversized_batch():
    # Single-batch contract: caller is responsible for chunking. The API
    # would silently truncate a 21-title query, so we surface the bug
    # loudly here.
    titles = [f"Title{i}" for i in range(21)]
    with pytest.raises(ValueError, match="exceeds API cap"):
        fetch_leads(titles, http_client=FakeHttpClient({}), source=SIMPLE_ENGLISH)


def test_fetch_leads_empty_input_returns_empty_dict_no_http_call():
    client = FakeHttpClient({})
    out = fetch_leads([], http_client=client, source=SIMPLE_ENGLISH)
    assert out == {}
    assert client.calls == [], "empty input must not trigger an HTTP request"


# ─── retry wiring (issue #38) ────────────────────────────────────────
# Pins that fetch_lead routes through retry.retry_http_get for the
# 429 / 5xx path. One test is enough — every strategy fetcher uses
# retry_http_get the same way, and the pure-helper coverage in
# tests/test_retry.py is what defends behaviour at scale.


class FlakyFakeHttpClient:
    """Test fake that returns one 429 then the canned payload.

    Distinct from ``FakeHttpClient`` because we need a status-code
    sequence, not a static-by-title map. Used only by the integration
    test that proves the strategy fetcher routes through retry_http_get.
    """

    def __init__(self, payload_after_one_429: dict):
        self._payload = payload_after_one_429
        self._calls_so_far = 0
        self.calls: list[dict] = []

    def get(self, url: str, params: dict, timeout: float | None = None):
        self.calls.append({"url": url, "params": params})
        self._calls_so_far += 1
        if self._calls_so_far == 1:
            return FakeResponse({}, status_code=429, headers={"Retry-After": "0"})
        return FakeResponse(self._payload)


def test_fetch_lead_retries_on_429_then_succeeds(monkeypatch):
    """Integration test: a 429 on the first call is retried by
    retry_http_get, and the second call's payload is returned. Pins
    the wiring through the public fetch_lead API.

    Stubs time.sleep via monkeypatch so the test runs at full speed
    even though Retry-After=0 (which the helper would otherwise pass
    to time.sleep(0) — harmless, but the monkeypatch is the standard
    pytest pattern for any test that exercises a code path that
    calls a real-world side effect).
    """
    monkeypatch.setattr("time.sleep", lambda _: None)
    client = FlakyFakeHttpClient(
        payload_after_one_429=_load_fixture("photosynthesis.json")
    )
    result = fetch_lead("Photosynthesis", http_client=client, source=SIMPLE_ENGLISH)
    assert result["title"] == "Photosynthesis"
    assert "Photosynthesis is a process" in result["lead_text"]
    assert len(client.calls) == 2  # One 429, one success.


# ─── Klexikon source (parse&prop=wikitext&section=0 strategy) ───────────
# Klexikon's MediaWiki has no TextExtracts extension, so the pipeline
# uses a different fetch shape: `action=parse` instead of
# `action=query`. The response carries `parse.wikitext.*` (raw
# wikitext for the lead section), which the stripper converts to
# plain text.


def _klexikon_parse_payload(title: str, wikitext: str) -> dict:
    """Build a fake `action=parse` response shaped like the real
    Klexikon API output."""
    return {
        "parse": {
            "title": title,
            "wikitext": {"*": wikitext},
        },
    }


class KlexikonFakeHttpClient:
    """Fake HTTP client for the Klexikon `action=parse` flow.

    The real Klexikon API returns a JSON envelope with `parse.title`
    and `parse.wikitext.*`; canonical URLs are constructed by the
    fetcher (the API doesn't return them inline for the parse action).
    Maps the requested `page` query parameter to a canned payload.
    """

    def __init__(self, responses: dict[str, dict]):
        self.responses = responses
        self.calls: list[dict] = []

    def get(self, url: str, params: dict, timeout: float | None = None):
        self.calls.append({"url": url, "params": params})
        page = params.get("page", "")
        if page not in self.responses:
            return FakeResponse({"error": {"code": "missingtitle", "info": f"{page} missing"}})
        return FakeResponse(self.responses[page])


def test_fetch_lead_klexikon_strategy_uses_parse_action_against_klexikon_zum_de():
    payload = _klexikon_parse_payload(
        title="Klima",
        wikitext=(
            "Wenn man vom Klima spricht, ist gemeint, dass es irgendwo "
            "normalerweise warm oder kalt ist. Das [[Wetter]] ist etwas "
            "Ähnliches, aber vom Wetter spricht man, wenn man an einen "
            "Tag oder wenige [[Woche]]n denkt."
        ),
    )
    client = KlexikonFakeHttpClient({"Klima": payload})
    result = fetch_lead("Klima", http_client=client, source=KLEXIKON)
    assert result["title"] == "Klima"
    # Wiki links must be flattened.
    assert "[[" not in result["lead_text"]
    assert "Wetter" in result["lead_text"]
    # The canonical URL is constructed deterministically from the
    # source's web-page base + the title (URL-encoded with spaces →
    # underscores per MediaWiki convention).
    assert result["canonical_url"] == "https://klexikon.zum.de/wiki/Klima"
    # Critical: the request must have gone to klexikon.zum.de.
    assert len(client.calls) == 1
    assert "klexikon.zum.de" in client.calls[0]["url"]
    assert "simple.wikipedia.org" not in client.calls[0]["url"]
    # And it must have used the parse action, not the query action.
    assert client.calls[0]["params"]["action"] == "parse"
    assert client.calls[0]["params"]["section"] == 0
    assert client.calls[0]["params"]["page"] == "Klima"


def test_fetch_lead_klexikon_strips_image_block():
    # Real Klexikon "Klima" page lead opens with a Datei: image block
    # whose caption itself contains [[link]]s. The fetcher must drop
    # the entire image block; otherwise raw wikitext leaks into the
    # passage.
    payload = _klexikon_parse_payload(
        title="Klima",
        wikitext=(
            "[[Datei:Thai_rain_forest.jpg|mini|[[Thailand]]: Hier im "
            "[[Tropen|tropisch]]en [[Regenwald]] wachsen [[Pflanzen]] "
            "sehr gut.]]\n"
            "Wenn man vom Klima spricht, ist gemeint, dass es irgendwo "
            "normalerweise warm oder kalt ist."
        ),
    )
    client = KlexikonFakeHttpClient({"Klima": payload})
    result = fetch_lead("Klima", http_client=client, source=KLEXIKON)
    text = result["lead_text"]
    assert "Datei:" not in text
    assert "[[" not in text
    assert "]]" not in text
    assert text.startswith("Wenn man vom Klima spricht")


def test_fetch_lead_klexikon_handles_titles_with_spaces():
    # MediaWiki titles can contain spaces; the canonical URL uses
    # underscores. Verify the URL construction handles both.
    payload = _klexikon_parse_payload(
        title="Roter Riese",
        wikitext="Ein Roter Riese ist ein sehr großer [[Stern]].",
    )
    client = KlexikonFakeHttpClient({"Roter Riese": payload})
    result = fetch_lead("Roter Riese", http_client=client, source=KLEXIKON)
    assert result["title"] == "Roter Riese"
    assert result["canonical_url"] == "https://klexikon.zum.de/wiki/Roter_Riese"


def test_fetch_lead_klexikon_missing_page_raises():
    # MediaWiki's parse action returns an `error.code=missingtitle`
    # block when the page doesn't exist. The fetcher must surface
    # that as a `RuntimeError` so a typo'd whitelist entry fails
    # loudly during ingest, not silently as an empty passage.
    client = KlexikonFakeHttpClient({})  # nothing in the map
    with pytest.raises(RuntimeError, match="not found|missing"):
        fetch_lead("DoesNotExist", http_client=client, source=KLEXIKON)


# ─── _klexikon_canonical_url unit tests (issue #55) ───────────────────
# RFC 3986 reserves the URL path component to ASCII; non-ASCII octets
# must be percent-encoded. Modern browsers handle either form, but the
# canonical form is the percent-encoded one and that's what the
# `source_url` field — which flows into the `sources` table and any
# attribution UI — should carry.


def test_klexikon_canonical_url_percent_encodes_diacritics():
    # German titles routinely contain ä/ö/ü/é. The canonical form must
    # percent-encode them: "Vögel" → "V%C3%B6gel".
    assert (
        _klexikon_canonical_url(KLEXIKON, "Vögel")
        == "https://klexikon.zum.de/wiki/V%C3%B6gel"
    )


def test_klexikon_canonical_url_percent_encodes_eszett():
    # ß is non-ASCII (U+00DF); must percent-encode to %C3%9F.
    assert (
        _klexikon_canonical_url(KLEXIKON, "Größe")
        == "https://klexikon.zum.de/wiki/Gr%C3%B6%C3%9Fe"
    )


def test_klexikon_canonical_url_replaces_spaces_with_underscores_first():
    # MediaWiki canonical paths use underscores for spaces; this rule
    # is independent of percent-encoding and must still apply.
    assert (
        _klexikon_canonical_url(KLEXIKON, "Roter Riese")
        == "https://klexikon.zum.de/wiki/Roter_Riese"
    )


def test_klexikon_canonical_url_underscore_is_preserved_unencoded():
    # _ is an unreserved character (RFC 3986 §2.3) and must NOT be
    # percent-encoded. urllib.parse.quote already handles this; this
    # test pins the invariant against future regressions.
    assert (
        _klexikon_canonical_url(KLEXIKON, "Schwarzes Loch")
        == "https://klexikon.zum.de/wiki/Schwarzes_Loch"
    )


def test_klexikon_canonical_url_ascii_titles_unchanged():
    # Pure-ASCII titles must round-trip byte-for-byte; the change must
    # not introduce churn in the Simple-English-style URL shape.
    assert (
        _klexikon_canonical_url(KLEXIKON, "Klima")
        == "https://klexikon.zum.de/wiki/Klima"
    )


def test_klexikon_canonical_url_namespace_colon_is_preserved_unencoded():
    # MediaWiki canonical URLs leave namespace separators (`Datei:`,
    # `Bild:`, `Kategorie:`) unescaped. The `safe=":"` flag passed to
    # urllib.parse.quote is what preserves this — without it `:` would
    # be percent-encoded to `%3A`. Pin that design choice here so a
    # future "tighten the safe set" change can't silently break
    # namespace-prefixed canonical URLs.
    assert (
        _klexikon_canonical_url(KLEXIKON, "Datei:Beispiel")
        == "https://klexikon.zum.de/wiki/Datei:Beispiel"
    )


def test_fetch_lead_klexikon_disambiguation_steht_fuer_raises():
    # German disambiguation pages typically use "<title> steht für: ...".
    # Klexikon's curation makes this rare but possible; catch it.
    payload = _klexikon_parse_payload(
        title="Saturn",
        wikitext=(
            "Saturn steht für: den Planeten Saturn, den römischen Gott "
            "Saturn, oder verschiedene andere Bedeutungen."
        ),
    )
    client = KlexikonFakeHttpClient({"Saturn": payload})
    with pytest.raises(RuntimeError, match="disambiguation page"):
        fetch_lead("Saturn", http_client=client, source=KLEXIKON)


def test_fetch_lead_klexikon_prose_steht_fuer_in_body_is_not_disambiguation():
    # Regression for issue #41 (German side): "steht für" is also a
    # common prose construction ("X steht für Y" = "X stands for Y"),
    # not only a disambiguation marker. An article whose lead uses it
    # mid-sentence — not as the lead-opening "<title> steht für:" —
    # must pass through. The lead-anchored rule only flags the marker
    # right after the title-as-subject.
    payload = _klexikon_parse_payload(
        title="Ampel",
        wikitext=(
            "Eine Ampel regelt den Verkehr an Kreuzungen. "
            "Die Farbe Rot steht für Halt und die Farbe Grün für Fahren."
        ),
    )
    client = KlexikonFakeHttpClient({"Ampel": payload})
    result = fetch_lead("Ampel", http_client=client, source=KLEXIKON)
    assert result["title"] == "Ampel"
    assert "steht für" in result["lead_text"]
