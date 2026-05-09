"""Tests for the data/ingest retry helper module.

The helper is HTTP-shaped — it composes around an injected http_client.
Tests use a scripted fake (no real network, no real sleeps).
"""
import pytest

from retry import (
    DEFAULT_BACKOFF_FACTOR,
    DEFAULT_BASE_DELAY_S,
    DEFAULT_JITTER_FRACTION,
    DEFAULT_MAX_ATTEMPTS,
    DEFAULT_RETRY_AFTER_BUDGET_S,
    RetryCapExceeded,
    RetrySettings,
    compute_delay,
    is_retryable_status,
    parse_retry_after,
    retry_http_get,
)


def test_default_settings_mirror_module_consts():
    """Drift guard: changing a const without changing RetrySettings.default()
    (or vice versa) is a bug. Pin both ends here so the discrepancy fails
    loudly on the next test run.
    """
    s = RetrySettings.default()
    assert s.max_attempts == DEFAULT_MAX_ATTEMPTS
    assert s.base_delay_s == DEFAULT_BASE_DELAY_S
    assert s.backoff_factor == DEFAULT_BACKOFF_FACTOR
    assert s.jitter_fraction == DEFAULT_JITTER_FRACTION
    assert s.retry_after_budget_s == DEFAULT_RETRY_AFTER_BUDGET_S


def test_retry_cap_exceeded_carries_diagnostic_fields():
    """RetryCapExceeded is the exception the helper raises when attempts
    exhaust or Retry-After exceeds budget. It must carry the developer-
    facing diagnostic fields (attempts, last_status, retry_after) — those
    are what the developer needs to decide whether to re-run or
    investigate."""
    err = RetryCapExceeded(attempts=3, last_status=503, retry_after=None)
    assert err.attempts == 3
    assert err.last_status == 503
    assert err.retry_after is None
    # Also a RuntimeError so it lands inside the existing exception hierarchy.
    assert isinstance(err, RuntimeError)


@pytest.mark.parametrize(
    "code,expected",
    [
        # Retryable: 429 (rate-limited) and the entire 5xx range.
        (429, True),
        (500, True),
        (502, True),
        (503, True),
        (504, True),
        (599, True),
        # Not retryable: success.
        (200, False),
        (201, False),
        # Not retryable: redirects.
        (301, False),
        (302, False),
        # Not retryable: client errors other than 429. A 401/403/404
        # means re-running won't help — surface to the caller.
        (400, False),
        (401, False),
        (403, False),
        (404, False),
        # Not retryable: out-of-band codes.
        (199, False),
        (600, False),
    ],
)
def test_is_retryable_status(code: int, expected: bool) -> None:
    assert is_retryable_status(code) is expected


@pytest.mark.parametrize(
    "value,expected",
    [
        # Delta-seconds form: integer.
        ("5", 5.0),
        ("0", 0.0),
        ("120", 120.0),
        # Delta-seconds form: float (servers occasionally emit fractional).
        ("3.5", 3.5),
        # Whitespace must be tolerated — some servers add it.
        ("  12  ", 12.0),
        # None header → None (caller falls back to compute_delay).
        (None, None),
        # Empty string → None.
        ("", None),
        # HTTP-date form is silently dropped (carry-forward known issue,
        # documented in the spec). Caller falls back to compute_delay.
        ("Wed, 21 Oct 2015 07:28:00 GMT", None),
        # Malformed values fall back to None.
        ("garbage", None),
        ("--5", None),
        ("3.5seconds", None),
        # Negative values are not delta-seconds; servers emitting these
        # are buggy. Treat as malformed.
        ("-5", None),
    ],
)
def test_parse_retry_after(value: str | None, expected: float | None) -> None:
    assert parse_retry_after(value) == expected


def _settings(
    *,
    base_delay_s: float = 0.5,
    backoff_factor: int = 2,
    jitter_fraction: float = 0.1,
) -> RetrySettings:
    """Builder used by compute_delay tests. The other settings fields
    don't affect compute_delay — pin them to defaults for clarity.
    """
    return RetrySettings(
        max_attempts=3,
        base_delay_s=base_delay_s,
        backoff_factor=backoff_factor,
        jitter_fraction=jitter_fraction,
        retry_after_budget_s=30.0,
    )


def test_compute_delay_attempt_zero_returns_base_delay():
    """At attempt=0 with jitter_seed=0 the delay is exactly base_delay_s."""
    s = _settings(base_delay_s=0.5)
    assert compute_delay(s, attempt=0, jitter_seed=0.0) == 0.5


def test_compute_delay_doubles_with_each_attempt():
    """attempt=1 → base × factor; attempt=2 → base × factor²."""
    s = _settings(base_delay_s=0.5, backoff_factor=2)
    assert compute_delay(s, attempt=1, jitter_seed=0.0) == 1.0
    assert compute_delay(s, attempt=2, jitter_seed=0.0) == 2.0


def test_compute_delay_jitter_plus_one_adds_jitter_fraction():
    """jitter_seed=+1 produces delay × (1 + jitter_fraction)."""
    s = _settings(base_delay_s=1.0, jitter_fraction=0.1)
    assert compute_delay(s, attempt=0, jitter_seed=1.0) == pytest.approx(1.1)


def test_compute_delay_jitter_minus_one_subtracts_jitter_fraction():
    """jitter_seed=-1 produces delay × (1 - jitter_fraction)."""
    s = _settings(base_delay_s=1.0, jitter_fraction=0.1)
    assert compute_delay(s, attempt=0, jitter_seed=-1.0) == pytest.approx(0.9)


def test_compute_delay_never_negative():
    """Pathological jitter_seed must not produce a negative delay
    (sleep would raise ValueError in real code). Clamp at 0.
    """
    # jitter_fraction > 1.0 with jitter_seed=-1.0 would otherwise go
    # negative. Clamp guards against accidentally pathological tunings.
    s = _settings(base_delay_s=1.0, jitter_fraction=1.5)
    assert compute_delay(s, attempt=0, jitter_seed=-1.0) == 0.0


def test_compute_delay_zero_base_stays_zero():
    """A 0 base (used in some tests to bypass real waits) stays 0
    regardless of attempt or jitter."""
    s = _settings(base_delay_s=0.0)
    assert compute_delay(s, attempt=0, jitter_seed=0.0) == 0.0
    assert compute_delay(s, attempt=2, jitter_seed=1.0) == 0.0


# ── Test fakes for retry_http_get ────────────────────────────────────


class _RetryFakeResponse:
    """Test response with controllable status_code, headers, payload.

    Mirrors the shape of ``requests.Response`` for the attributes the
    helper actually reads.
    """

    def __init__(
        self,
        status_code: int,
        headers: dict[str, str] | None = None,
        payload: dict | None = None,
    ) -> None:
        self.status_code = status_code
        self.headers = headers or {}
        self._payload = payload or {}

    def json(self) -> dict:
        return self._payload

    def raise_for_status(self) -> None:
        if self.status_code >= 400:
            raise RuntimeError(f"http {self.status_code}")


class _RetryFakeHttpClient:
    """Test client that returns a scripted sequence of responses.

    Each ``get`` call consumes the next entry from ``script``;
    ``calls`` records the (url, params, timeout) for each invocation.
    """

    def __init__(self, script: list[_RetryFakeResponse]) -> None:
        self.script = list(script)
        self.calls: list[dict] = []

    def get(self, url: str, *, params: dict, timeout: float) -> _RetryFakeResponse:
        self.calls.append({"url": url, "params": params, "timeout": timeout})
        if not self.script:
            raise AssertionError("_RetryFakeHttpClient: script exhausted")
        return self.script.pop(0)


def _no_jitter() -> float:
    """Deterministic jitter source: returns 0.5 every time, which maps
    via ``jitter_fn() * 2 - 1`` to 0.0. Keeps test delays exactly equal
    to ``compute_delay(.., jitter_seed=0.0)``.
    """
    return 0.5


def _record_sleep():
    """Return a (sleep_fn, recorded) pair. ``sleep_fn`` records each
    call's argument and returns immediately."""
    recorded: list[float] = []

    def sleep(seconds: float) -> None:
        recorded.append(seconds)

    return sleep, recorded


def test_retry_http_get_happy_path_no_retry_no_sleep():
    """200 first try → 1 call, 0 sleeps, returns the response."""
    client = _RetryFakeHttpClient([_RetryFakeResponse(200, payload={"ok": True})])
    sleep_fn, sleeps = _record_sleep()
    resp = retry_http_get(
        client,
        "https://example/api",
        params={"q": "x"},
        timeout=10.0,
        settings=RetrySettings.default(),
        sleep=sleep_fn,
        jitter_fn=_no_jitter,
    )
    assert resp.status_code == 200
    assert resp.json() == {"ok": True}
    assert len(client.calls) == 1
    assert sleeps == []
    # Verify the request shape was passed through unchanged.
    assert client.calls[0]["url"] == "https://example/api"
    assert client.calls[0]["params"] == {"q": "x"}
    assert client.calls[0]["timeout"] == 10.0


def test_retry_http_get_returns_4xx_unchanged_no_sleep():
    """A non-retryable 404 is returned to the caller — the strategy's
    own raise_for_status handles 404s. No sleep, 1 call.
    """
    client = _RetryFakeHttpClient([_RetryFakeResponse(404)])
    sleep_fn, sleeps = _record_sleep()
    resp = retry_http_get(
        client,
        "https://example/api",
        params={},
        timeout=10.0,
        settings=RetrySettings.default(),
        sleep=sleep_fn,
        jitter_fn=_no_jitter,
    )
    assert resp.status_code == 404
    assert len(client.calls) == 1
    assert sleeps == []
