"""End-to-end pipeline test using a fake HTTP client + canned fixtures."""
import json
from pathlib import Path
import pytest
from simple_wikipedia import KLEXIKON, SIMPLE_ENGLISH, main


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
    """Handles both single-title queries (legacy fetch_lead path) and the
    pipe-separated batched titles `main` sends via fetch_leads. For a
    batched query, merges the per-title canned payloads into a single
    response shaped like the real MediaWiki batched response.

    Records each `get` invocation in `self.calls` so tests can assert
    on per-source URL routing without needing to inspect the response
    payloads (which carry the URL but only after the request reaches
    the right endpoint)."""

    def __init__(self, responses: dict[str, dict]):
        self.responses = responses
        self.calls: list[dict] = []

    def get(self, url, params, timeout=None):
        self.calls.append({"url": url, "params": params})
        titles_param = params["titles"]
        if "|" not in titles_param:
            return FakeResponse(self.responses[titles_param])
        # Batched query: merge per-title pages into one response.
        merged_pages: dict[str, dict] = {}
        for title in titles_param.split("|"):
            payload = self.responses[title]
            for pageid, page in payload["query"]["pages"].items():
                merged_pages[pageid] = page
        return FakeResponse({"batchcomplete": "", "query": {"pages": merged_pages}})


def _load(name: str) -> dict:
    return json.loads((FIXTURES / name).read_text(encoding="utf-8"))


def test_pipeline_three_articles_byte_exact(tmp_path: Path):
    whitelist = tmp_path / "wl.txt"
    whitelist.write_text("Photosynthesis\nBlack hole\nGravity\n")
    output = tmp_path / "out.jsonl"

    client = FakeHttpClient({
        "Photosynthesis": _load("photosynthesis.json"),
        "Black hole": _load("black_hole.json"),
        "Gravity": _load("gravity.json"),
    })

    main(
        whitelist_path=whitelist,
        output_path=output,
        http_client=client,
        inter_batch_sleep_s=0.0,
        source=SIMPLE_ENGLISH,
    )

    actual = output.read_text(encoding="utf-8")
    expected = (FIXTURES / "expected_output.jsonl").read_text(encoding="utf-8")
    assert actual == expected, (
        "pipeline output does not match expected_output.jsonl byte-for-byte"
    )


def test_pipeline_output_sorted_by_id(tmp_path: Path):
    # Whitelist order is z, a, m — the output must reorder to a, m, z by id.
    whitelist = tmp_path / "wl.txt"
    # Use the Wikipedia titles so slugs alphabetise as expected.
    # photosynthesis > gravity > black-hole, so output order is b, g, p.
    whitelist.write_text("Photosynthesis\nGravity\nBlack hole\n")
    output = tmp_path / "out.jsonl"

    client = FakeHttpClient({
        "Photosynthesis": _load("photosynthesis.json"),
        "Black hole": _load("black_hole.json"),
        "Gravity": _load("gravity.json"),
    })

    main(
        whitelist_path=whitelist,
        output_path=output,
        http_client=client,
        inter_batch_sleep_s=0.0,
        source=SIMPLE_ENGLISH,
    )
    lines = output.read_text(encoding="utf-8").strip().splitlines()
    ids = [json.loads(line)["id"] for line in lines]
    assert ids == [
        "wiki-simple:en:black-hole",
        "wiki-simple:en:gravity",
        "wiki-simple:en:photosynthesis",
    ]


class KlexikonFakeHttpClient:
    """Fake HTTP client for the Klexikon `action=parse` flow.

    Klexikon's MediaWiki has no TextExtracts, so the pipeline issues
    one `action=parse&page=<title>&prop=wikitext&section=0` request per
    article (no batching). This fake maps the requested `page` param to
    a canned `parse.wikitext.*` payload so the pipeline test can
    exercise the full strip + emit chain without network traffic.
    """

    def __init__(self, responses: dict[str, dict]):
        self.responses = responses
        self.calls: list[dict] = []

    def get(self, url: str, params: dict, timeout=None):
        self.calls.append({"url": url, "params": params})
        page = params.get("page", "")
        if page not in self.responses:
            return FakeResponse({"error": {"code": "missingtitle", "info": page}})
        return FakeResponse(self.responses[page])


def _klexikon_payload(title: str, wikitext: str) -> dict:
    return {
        "parse": {
            "title": title,
            "wikitext": {"*": wikitext},
        },
    }


def test_pipeline_klexikon_source_emits_de_wiki_klexikon_ids(tmp_path: Path):
    """End-to-end: a Klexikon whitelist + the KLEXIKON source preset
    produce `wiki-klexikon:de:<slug>` ids and klexikon.zum.de canonical
    URLs. The fake HTTP client returns canned wikitext for two titles;
    the pipeline strips wiki markup, slugifies, sorts by id, and writes
    JSONL identical in shape to the existing English wiki layer."""
    whitelist = tmp_path / "wl.txt"
    whitelist.write_text("Klima\nWetter\n")
    output = tmp_path / "out.jsonl"

    client = KlexikonFakeHttpClient({
        "Klima": _klexikon_payload(
            "Klima",
            (
                "Wenn man vom Klima spricht, ist gemeint, dass es "
                "irgendwo normalerweise warm oder kalt ist. Das "
                "[[Wetter]] ist etwas Ähnliches, aber vom Wetter "
                "spricht man, wenn man an einen Tag oder wenige "
                "[[Woche]]n denkt. Es geht also beim Wetter um einen "
                "kurzen Zeitraum."
            ),
        ),
        "Wetter": _klexikon_payload(
            "Wetter",
            (
                "Das Wetter ist die Beschaffenheit der Luft, also "
                "[[Sonne]] und [[Regen]], [[Wind]] und [[Wolke]]n. "
                "Wir sehen das Wetter draußen jeden Tag und es kann "
                "sich sehr schnell ändern."
            ),
        ),
    })

    main(
        whitelist_path=whitelist,
        output_path=output,
        http_client=client,
        inter_batch_sleep_s=0.0,
        source=KLEXIKON,
    )

    lines = output.read_text(encoding="utf-8").strip().splitlines()
    records = [json.loads(line) for line in lines]
    ids = [r["id"] for r in records]
    # Sorted by id ascending.
    assert ids == ["wiki-klexikon:de:klima", "wiki-klexikon:de:wetter"]

    # Spot-check Klexikon-specific shape.
    by_id = {r["id"]: r for r in records}
    klima = by_id["wiki-klexikon:de:klima"]
    assert klima["source_url"] == "https://klexikon.zum.de/wiki/Klima"
    assert klima["license"] == "CC-BY-SA-4.0"
    assert "Klexikon" in klima["attribution"]
    assert "Klima" in klima["attribution"]
    assert klima["topics"] == ["wikipedia", "klexikon", "klima"]
    # Wikitext markup must be entirely gone.
    assert "[[" not in klima["text"]
    assert "]]" not in klima["text"]


def test_pipeline_klexikon_request_routed_to_klexikon_zum_de(tmp_path: Path):
    """Every HTTP request the pipeline makes for `source=KLEXIKON`
    must hit klexikon.zum.de. A misrouted request (e.g. to
    de.wikipedia.org) would silently produce regular German Wikipedia
    content under a Klexikon id — the wrong vocabulary level."""
    whitelist = tmp_path / "wl.txt"
    whitelist.write_text("Klima\n")
    output = tmp_path / "out.jsonl"

    client = KlexikonFakeHttpClient({
        "Klima": _klexikon_payload(
            "Klima",
            (
                "Klima ist die Beschaffenheit der Luft über lange Zeit. "
                "Wenn man vom Klima spricht, denkt man an Jahre. "
                "Das Wetter ist eher kurz und ändert sich schnell."
            ),
        ),
    })

    main(
        whitelist_path=whitelist,
        output_path=output,
        http_client=client,
        inter_batch_sleep_s=0.0,
        source=KLEXIKON,
    )

    assert client.calls, "pipeline must have made at least one HTTP call"
    for call in client.calls:
        assert "klexikon.zum.de" in call["url"], (
            f"Klexikon source must route to klexikon.zum.de; got {call['url']!r}"
        )
        assert "wikipedia.org" not in call["url"], (
            f"Klexikon source leaked to wikipedia.org: {call['url']!r}"
        )
        # Klexikon strategy uses the `parse` action, not `query`.
        assert call["params"]["action"] == "parse"
