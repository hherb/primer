"""Tests for fetch_lead — uses an injected http_client so the test never
talks to the real network."""
import json
from pathlib import Path
import pytest
from simple_wikipedia import fetch_lead, fetch_leads


FIXTURES = Path(__file__).parent / "fixtures"


class FakeResponse:
    def __init__(self, payload: dict, status_code: int = 200):
        self._payload = payload
        self.status_code = status_code

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
    result = fetch_lead("Photosynthesis", http_client=client)
    assert result["title"] == "Photosynthesis"
    assert "Photosynthesis is a process" in result["lead_text"]
    assert result["canonical_url"] == "https://simple.wikipedia.org/wiki/Photosynthesis"


def test_fetch_lead_uses_correct_api_endpoint():
    client = FakeHttpClient({"Photosynthesis": _load_fixture("photosynthesis.json")})
    fetch_lead("Photosynthesis", http_client=client)
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
        fetch_lead("DoesNotExist", http_client=client)


def test_fetch_lead_empty_extract_raises():
    payload = {
        "query": {"pages": {"99": {"title": "Stub", "extract": "", "fullurl": "https://x"}}}
    }
    client = FakeHttpClient({"Stub": payload})
    with pytest.raises(RuntimeError, match="empty extract"):
        fetch_lead("Stub", http_client=client)


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
        fetch_lead("Base", http_client=client)


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
        fetch_lead("Saturn", http_client=client)


def test_fetch_leads_rejects_oversized_batch():
    # Single-batch contract: caller is responsible for chunking. The API
    # would silently truncate a 21-title query, so we surface the bug
    # loudly here.
    titles = [f"Title{i}" for i in range(21)]
    with pytest.raises(ValueError, match="exceeds API cap"):
        fetch_leads(titles, http_client=FakeHttpClient({}))


def test_fetch_leads_empty_input_returns_empty_dict_no_http_call():
    client = FakeHttpClient({})
    out = fetch_leads([], http_client=client)
    assert out == {}
    assert client.calls == [], "empty input must not trigger an HTTP request"
