"""End-to-end pipeline test using a fake HTTP client + canned fixtures."""
import json
from pathlib import Path
import pytest
from simple_wikipedia import main


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
    response shaped like the real MediaWiki batched response."""

    def __init__(self, responses: dict[str, dict]):
        self.responses = responses

    def get(self, url, params, timeout=None):
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

    main(whitelist_path=whitelist, output_path=output, http_client=client, inter_batch_sleep_s=0.0)

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

    main(whitelist_path=whitelist, output_path=output, http_client=client, inter_batch_sleep_s=0.0)
    lines = output.read_text(encoding="utf-8").strip().splitlines()
    ids = [json.loads(line)["id"] for line in lines]
    assert ids == [
        "wiki-simple:en:black-hole",
        "wiki-simple:en:gravity",
        "wiki-simple:en:photosynthesis",
    ]
