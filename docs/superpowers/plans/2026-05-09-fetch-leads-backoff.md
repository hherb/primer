# `fetch_leads` Backoff Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add HTTP 429 / 5xx retry-with-exponential-backoff to `data/ingest/simple_wikipedia.py` via a new pure-helper module `data/ingest/retry.py`. Issue #38.

**Architecture:** Pure-function helpers (`is_retryable_status`, `parse_retry_after`, `compute_delay`) plus a single composing function (`retry_http_get`) that the three strategy fetchers call. `sleep` and `jitter_fn` are kwarg-injected for fast deterministic tests. Network errors propagate unchanged (out of scope per issue #38). HTTP-date `Retry-After` silently drops to computed exponential delay (matches the Rust carry-forward).

**Tech Stack:** Python 3.12, pytest, `requests` (already a dep). Test seam: `http_client` + `monkeypatch`. No new third-party deps.

**Spec:** [docs/superpowers/specs/2026-05-09-fetch-leads-backoff-design.md](../specs/2026-05-09-fetch-leads-backoff-design.md)

---

## File map

| File | Status | Responsibility |
| --- | --- | --- |
| `data/ingest/retry.py` | **CREATE** | Constants, `RetrySettings`, `RetryCapExceeded`, three pure helpers, `retry_http_get` |
| `data/ingest/tests/test_retry.py` | **CREATE** | All pure-helper unit tests + `retry_http_get` integration tests against a scripted fake client |
| `data/ingest/simple_wikipedia.py` | **MODIFY** | Three strategy fetchers replace `http_client.get(...)` with `retry_http_get(http_client, ..., params=..., timeout=...)` |
| `data/ingest/tests/test_fetch_lead.py` | **MODIFY** | `FakeResponse.__init__` gains `headers={}` default; one new integration test |
| `CLAUDE.md` | **MODIFY** | One-line invariant bullet under the data/ingest section |

---

## Working environment

Every Python command runs from `data/ingest/` and uses the existing venv:

```bash
cd /Users/hherb/src/primer/data/ingest
.venv/bin/pytest tests/ -v
```

Branch: `feature/issue-38-fetch-leads-backoff` (already created and currently checked out).

---

### Task 1: Bootstrap `retry.py` skeleton with constants, `RetrySettings`, `RetryCapExceeded`

**Files:**
- Create: `data/ingest/retry.py`
- Create: `data/ingest/tests/test_retry.py`

- [ ] **Step 1: Write the failing test**

Create `data/ingest/tests/test_retry.py`:

```python
"""Tests for the data/ingest retry helper module.

The helper is HTTP-shaped — it composes around an injected http_client.
Tests use a scripted fake (no real network, no real sleeps).
"""
from retry import (
    DEFAULT_BACKOFF_FACTOR,
    DEFAULT_BASE_DELAY_S,
    DEFAULT_JITTER_FRACTION,
    DEFAULT_MAX_ATTEMPTS,
    DEFAULT_RETRY_AFTER_BUDGET_S,
    RetryCapExceeded,
    RetrySettings,
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
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cd /Users/hherb/src/primer/data/ingest
.venv/bin/pytest tests/test_retry.py -v
```

Expected: `ModuleNotFoundError: No module named 'retry'`.

- [ ] **Step 3: Write minimal implementation**

Create `data/ingest/retry.py`:

```python
"""HTTP retry helper for the data-ingestion pipeline.

Adds 429/5xx retry-with-exponential-backoff around an injected
``http_client.get`` call. The helper is HTTP-shaped (status-code
dispatch + ``Retry-After`` header) and lives separately from the
Rust ``primer_core::retry`` (which is ``InferenceError``-shaped).

Pure helpers — ``is_retryable_status``, ``parse_retry_after``,
``compute_delay`` — carry no I/O and are easy to unit-test
exhaustively. The composing function ``retry_http_get`` accepts
``sleep`` and ``jitter_fn`` as kwargs so tests inject deterministic
no-op equivalents.

Out of scope here: network-error retry, HTTP-date ``Retry-After``,
failed-batch persistence. See
``docs/superpowers/specs/2026-05-09-fetch-leads-backoff-design.md``.
"""
from __future__ import annotations

from dataclasses import dataclass


# ── Defaults (no magic numbers) ──────────────────────────────────────

# Total attempts including the first. 3 × 0.5 s × 2 ≈ ~1.5 s + jitter
# worst case before failure — tight enough to surface persistent
# failures quickly, loose enough to ride out a transient blip.
DEFAULT_MAX_ATTEMPTS = 3

# Initial delay before the second attempt, in seconds.
DEFAULT_BASE_DELAY_S = 0.5

# Multiplicative growth factor between attempts.
DEFAULT_BACKOFF_FACTOR = 2

# Jitter as a fraction of the computed delay. ±10% noise so concurrent
# runs (rare but possible during dev) don't lock-step their backoffs.
DEFAULT_JITTER_FRACTION = 0.1

# Cap on Retry-After we will honour, in seconds. Looser than the Rust
# 5 s budget because this is a one-shot dev tool — no live conversation
# latency to protect. If a server says "wait 30 s", wait. If 60 s,
# surface the failure so the developer can re-run later.
DEFAULT_RETRY_AFTER_BUDGET_S = 30.0


@dataclass(frozen=True)
class RetrySettings:
    """Configuration for ``retry_http_get``.

    Frozen so the same instance can be safely shared across calls.
    Use :meth:`default` for production wiring.
    """

    max_attempts: int
    base_delay_s: float
    backoff_factor: int
    jitter_fraction: float
    retry_after_budget_s: float

    @classmethod
    def default(cls) -> "RetrySettings":
        """Construct a settings instance from the module defaults.

        Pinned by :func:`tests.test_retry.test_default_settings_mirror_module_consts`
        so a drift between consts and this builder fails loudly.
        """
        return cls(
            max_attempts=DEFAULT_MAX_ATTEMPTS,
            base_delay_s=DEFAULT_BASE_DELAY_S,
            backoff_factor=DEFAULT_BACKOFF_FACTOR,
            jitter_fraction=DEFAULT_JITTER_FRACTION,
            retry_after_budget_s=DEFAULT_RETRY_AFTER_BUDGET_S,
        )


class RetryCapExceeded(RuntimeError):
    """Raised when ``retry_http_get`` exhausts its attempt budget OR
    when a ``Retry-After`` header asks for a wait that exceeds
    ``RetrySettings.retry_after_budget_s``.

    Subclasses :class:`RuntimeError` so the existing exception
    hierarchy in ``simple_wikipedia.py`` (every other failure raises
    ``RuntimeError``) keeps working.

    Carries the diagnostic fields a developer needs to decide whether
    to re-run or investigate: how many attempts were made, the final
    HTTP status, and the raw ``Retry-After`` header value (if any).
    """

    def __init__(
        self,
        *,
        attempts: int,
        last_status: int,
        retry_after: str | None,
    ) -> None:
        self.attempts = attempts
        self.last_status = last_status
        self.retry_after = retry_after
        super().__init__(
            f"retry cap exceeded after {attempts} attempt(s); "
            f"last_status={last_status}, retry_after={retry_after!r}"
        )
```

- [ ] **Step 4: Run test to verify it passes**

```bash
.venv/bin/pytest tests/test_retry.py -v
```

Expected: 2 passed.

- [ ] **Step 5: Commit**

```bash
cd /Users/hherb/src/primer
git add data/ingest/retry.py data/ingest/tests/test_retry.py
git commit -m "feat(ingest): add retry.py skeleton + consts + RetrySettings + RetryCapExceeded (#38)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: `is_retryable_status` pure helper

**Files:**
- Modify: `data/ingest/retry.py` (append)
- Modify: `data/ingest/tests/test_retry.py` (append)

- [ ] **Step 1: Write the failing tests**

Append to `data/ingest/tests/test_retry.py`:

```python
import pytest

from retry import is_retryable_status


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
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
.venv/bin/pytest tests/test_retry.py -v
```

Expected: `ImportError: cannot import name 'is_retryable_status'`.

- [ ] **Step 3: Write minimal implementation**

Append to `data/ingest/retry.py`:

```python
# ── Pure helpers ─────────────────────────────────────────────────────


def is_retryable_status(code: int) -> bool:
    """True iff ``code`` is a retryable HTTP status.

    Retryable: 429 (Too Many Requests) and the full 5xx range. 4xx
    errors other than 429 indicate a problem in the request itself
    (auth, missing resource, malformed parameters) and re-running
    won't help.
    """
    return code == 429 or 500 <= code < 600
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
.venv/bin/pytest tests/test_retry.py -v
```

Expected: 17 passed (15 parameter cases + 2 from Task 1).

- [ ] **Step 5: Commit**

```bash
git add data/ingest/retry.py data/ingest/tests/test_retry.py
git commit -m "feat(ingest): add is_retryable_status pure helper (#38)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: `parse_retry_after` pure helper

**Files:**
- Modify: `data/ingest/retry.py` (append)
- Modify: `data/ingest/tests/test_retry.py` (append)

- [ ] **Step 1: Write the failing tests**

Append to `data/ingest/tests/test_retry.py`:

```python
from retry import parse_retry_after


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
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
.venv/bin/pytest tests/test_retry.py -v
```

Expected: `ImportError: cannot import name 'parse_retry_after'`.

- [ ] **Step 3: Write minimal implementation**

Append to `data/ingest/retry.py`:

```python
def parse_retry_after(value: str | None) -> float | None:
    """Parse a ``Retry-After`` header value as delta-seconds.

    Returns ``None`` for ``None``, empty, malformed, or HTTP-date
    form (the carry-forward known limitation matching
    ``primer_core::retry``). The caller's intent on ``None`` is to
    fall back to the computed exponential delay.

    Negative values are treated as malformed; the spec defines
    Retry-After as a non-negative duration.
    """
    if value is None:
        return None
    stripped = value.strip()
    if not stripped:
        return None
    try:
        seconds = float(stripped)
    except ValueError:
        return None
    if seconds < 0:
        return None
    return seconds
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
.venv/bin/pytest tests/test_retry.py -v
```

Expected: 28 passed (11 new parameter cases + 17 prior).

- [ ] **Step 5: Commit**

```bash
git add data/ingest/retry.py data/ingest/tests/test_retry.py
git commit -m "feat(ingest): add parse_retry_after pure helper (#38)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: `compute_delay` pure helper

**Files:**
- Modify: `data/ingest/retry.py` (append)
- Modify: `data/ingest/tests/test_retry.py` (append)

- [ ] **Step 1: Write the failing tests**

Append to `data/ingest/tests/test_retry.py`:

```python
from retry import compute_delay


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
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
.venv/bin/pytest tests/test_retry.py -v
```

Expected: `ImportError: cannot import name 'compute_delay'`.

- [ ] **Step 3: Write minimal implementation**

Append to `data/ingest/retry.py`:

```python
def compute_delay(
    settings: RetrySettings,
    *,
    attempt: int,
    jitter_seed: float,
) -> float:
    """Compute the delay before the next attempt.

    ``delay = base_delay_s * backoff_factor ** attempt * (1 + jitter_fraction * jitter_seed)``

    ``jitter_seed`` is in ``[-1.0, 1.0]``; the caller is responsible
    for the mapping (in production, ``random.random() * 2 - 1``;
    in tests, a constant). The result is clamped at 0 so a
    pathological tuning can't produce a negative delay.

    Pure: no I/O, no time read.
    """
    raw = (
        settings.base_delay_s
        * (settings.backoff_factor ** attempt)
        * (1.0 + settings.jitter_fraction * jitter_seed)
    )
    return max(0.0, raw)
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
.venv/bin/pytest tests/test_retry.py -v
```

Expected: 34 passed (6 new + 28 prior).

- [ ] **Step 5: Commit**

```bash
git add data/ingest/retry.py data/ingest/tests/test_retry.py
git commit -m "feat(ingest): add compute_delay pure helper (#38)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: `retry_http_get` happy-path (no retry needed)

**Files:**
- Modify: `data/ingest/retry.py` (append)
- Modify: `data/ingest/tests/test_retry.py` (append)

- [ ] **Step 1: Write the failing test**

Append to `data/ingest/tests/test_retry.py`:

```python
from retry import retry_http_get


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

    Note: retry_http_get does the *2 - 1 mapping internally, so this
    deterministic 0.5 cleanly produces a 0.0 jitter_seed.
    """
    return 0.5


def _record_sleep() -> tuple[callable, list[float]]:
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
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
.venv/bin/pytest tests/test_retry.py -v
```

Expected: `ImportError: cannot import name 'retry_http_get'`.

- [ ] **Step 3: Write minimal implementation**

Append to `data/ingest/retry.py`:

```python
# Type imports for retry_http_get's annotation. We import lazily to
# avoid a top-of-file Protocol import for what's a simple internal
# helper — the duck-typed shape is documented in the docstring.
from typing import Any, Callable


def retry_http_get(
    http_client: Any,
    url: str,
    *,
    params: dict,
    timeout: float,
    settings: RetrySettings,
    sleep: Callable[[float], None],
    jitter_fn: Callable[[], float],
) -> Any:
    """Call ``http_client.get(url, params=..., timeout=...)`` with
    retry-on-429-or-5xx.

    Network errors (``requests.exceptions.*``) propagate unchanged —
    out of scope for issue #38.

    The ``sleep`` and ``jitter_fn`` parameters are kwarg-injected so
    tests can pass deterministic no-op equivalents (``lambda _: None``,
    ``lambda: 0.5``). Production callers pass ``time.sleep`` and
    ``random.random``.

    ``jitter_fn`` returns a value in ``[0.0, 1.0)`` (matching
    ``random.random``); we map to ``[-1.0, 1.0)`` internally before
    feeding ``compute_delay``.

    Returns the final ``Response`` (which may have a non-2xx but
    non-retryable status — the caller's ``raise_for_status`` handles
    that). Raises :class:`RetryCapExceeded` only when retries are
    exhausted OR when ``Retry-After`` exceeds the budget.
    """
    attempt = 0
    while True:
        resp = http_client.get(url, params=params, timeout=timeout)
        if not is_retryable_status(resp.status_code):
            return resp
        # Retryable. Decide if we have any attempts left.
        attempts_made = attempt + 1
        attempts_left = settings.max_attempts - attempts_made
        retry_after_raw = getattr(resp, "headers", {}).get("Retry-After")
        if attempts_left <= 0:
            raise RetryCapExceeded(
                attempts=attempts_made,
                last_status=resp.status_code,
                retry_after=retry_after_raw,
            )
        parsed = parse_retry_after(retry_after_raw)
        if parsed is not None:
            if parsed > settings.retry_after_budget_s:
                # Don't sleep for longer than we're willing to wait.
                # Surface the failure so the developer can re-run later.
                raise RetryCapExceeded(
                    attempts=attempts_made,
                    last_status=resp.status_code,
                    retry_after=retry_after_raw,
                )
            delay = parsed
        else:
            jitter_seed = jitter_fn() * 2.0 - 1.0
            delay = compute_delay(settings, attempt=attempt, jitter_seed=jitter_seed)
        sleep(delay)
        attempt += 1
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
.venv/bin/pytest tests/test_retry.py -v
```

Expected: 36 passed (2 new + 34 prior).

- [ ] **Step 5: Commit**

```bash
git add data/ingest/retry.py data/ingest/tests/test_retry.py
git commit -m "feat(ingest): add retry_http_get happy-path (#38)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 6: `retry_http_get` retries on 503

**Files:**
- Modify: `data/ingest/tests/test_retry.py` (append)

- [ ] **Step 1: Write the failing test**

Append to `data/ingest/tests/test_retry.py`:

```python
def test_retry_http_get_succeeds_on_third_attempt_after_two_503s():
    """Two 503 then 200: 3 calls, 2 sleeps with delays equal to the
    pure-helper output for attempts 0 and 1."""
    client = _RetryFakeHttpClient(
        [
            _RetryFakeResponse(503),
            _RetryFakeResponse(503),
            _RetryFakeResponse(200, payload={"ok": True}),
        ]
    )
    sleep_fn, sleeps = _record_sleep()
    settings = RetrySettings.default()
    resp = retry_http_get(
        client,
        "https://example/api",
        params={},
        timeout=10.0,
        settings=settings,
        sleep=sleep_fn,
        jitter_fn=_no_jitter,
    )
    assert resp.status_code == 200
    assert len(client.calls) == 3
    # Two sleeps, with delays exactly matching compute_delay for
    # attempts 0 and 1 under jitter_seed=0.0 (since _no_jitter → 0.5
    # → mapped to 0.0).
    assert len(sleeps) == 2
    assert sleeps[0] == compute_delay(settings, attempt=0, jitter_seed=0.0)
    assert sleeps[1] == compute_delay(settings, attempt=1, jitter_seed=0.0)
```

- [ ] **Step 2: Run test to verify it passes (existing implementation already handles this)**

```bash
.venv/bin/pytest tests/test_retry.py -v
```

Expected: 37 passed. The Task 5 implementation already handles this — Task 6 is a behaviour-pinning test that catches a future regression where someone simplifies the loop.

- [ ] **Step 3: Commit**

```bash
git add data/ingest/tests/test_retry.py
git commit -m "test(ingest): pin retry_http_get retries-on-503 behaviour (#38)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 7: `retry_http_get` exhausts attempts

**Files:**
- Modify: `data/ingest/tests/test_retry.py` (append)

- [ ] **Step 1: Write the failing test**

Append to `data/ingest/tests/test_retry.py`:

```python
def test_retry_http_get_exhausts_attempts_then_raises_with_diagnostics():
    """Three 503s: 3 calls, 2 sleeps (no sleep after the final
    failure), RetryCapExceeded raised with attempts=3 and
    last_status=503."""
    client = _RetryFakeHttpClient(
        [
            _RetryFakeResponse(503),
            _RetryFakeResponse(503),
            _RetryFakeResponse(503),
        ]
    )
    sleep_fn, sleeps = _record_sleep()
    with pytest.raises(RetryCapExceeded) as exc_info:
        retry_http_get(
            client,
            "https://example/api",
            params={},
            timeout=10.0,
            settings=RetrySettings.default(),
            sleep=sleep_fn,
            jitter_fn=_no_jitter,
        )
    assert exc_info.value.attempts == 3
    assert exc_info.value.last_status == 503
    assert exc_info.value.retry_after is None
    assert len(client.calls) == 3
    assert len(sleeps) == 2  # No sleep after the final failure.
```

- [ ] **Step 2: Run test to verify it passes**

```bash
.venv/bin/pytest tests/test_retry.py -v
```

Expected: 38 passed.

- [ ] **Step 3: Commit**

```bash
git add data/ingest/tests/test_retry.py
git commit -m "test(ingest): pin retry_http_get cap-exceeded diagnostics (#38)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 8: `retry_http_get` honours Retry-After within budget

**Files:**
- Modify: `data/ingest/tests/test_retry.py` (append)

- [ ] **Step 1: Write the failing test**

Append to `data/ingest/tests/test_retry.py`:

```python
def test_retry_http_get_honours_retry_after_within_budget():
    """A 429 with Retry-After=1 (within the 30 s budget) sleeps
    exactly 1.0 — not the computed exponential delay. compute_delay
    for attempt=0 with default settings would be 0.5; this test pins
    that the explicit Retry-After wins."""
    client = _RetryFakeHttpClient(
        [
            _RetryFakeResponse(429, headers={"Retry-After": "1"}),
            _RetryFakeResponse(200, payload={"ok": True}),
        ]
    )
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
    assert resp.status_code == 200
    assert len(client.calls) == 2
    assert sleeps == [1.0]
```

- [ ] **Step 2: Run test to verify it passes**

```bash
.venv/bin/pytest tests/test_retry.py -v
```

Expected: 39 passed.

- [ ] **Step 3: Commit**

```bash
git add data/ingest/tests/test_retry.py
git commit -m "test(ingest): pin retry_http_get Retry-After-within-budget path (#38)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 9: `retry_http_get` Retry-After exceeds budget

**Files:**
- Modify: `data/ingest/tests/test_retry.py` (append)

- [ ] **Step 1: Write the failing test**

Append to `data/ingest/tests/test_retry.py`:

```python
def test_retry_http_get_retry_after_exceeds_budget_raises_immediately():
    """A 429 with Retry-After=60 (exceeds 30 s default budget):
    immediate RetryCapExceeded, attempts=1, no sleep, no further
    HTTP calls. Long waits surface immediately so the developer can
    re-run later."""
    client = _RetryFakeHttpClient(
        [_RetryFakeResponse(429, headers={"Retry-After": "60"})]
    )
    sleep_fn, sleeps = _record_sleep()
    with pytest.raises(RetryCapExceeded) as exc_info:
        retry_http_get(
            client,
            "https://example/api",
            params={},
            timeout=10.0,
            settings=RetrySettings.default(),
            sleep=sleep_fn,
            jitter_fn=_no_jitter,
        )
    assert exc_info.value.attempts == 1
    assert exc_info.value.last_status == 429
    assert exc_info.value.retry_after == "60"
    assert len(client.calls) == 1
    assert sleeps == []  # Immediate surface — no sleep.
```

- [ ] **Step 2: Run test to verify it passes**

```bash
.venv/bin/pytest tests/test_retry.py -v
```

Expected: 40 passed.

- [ ] **Step 3: Commit**

```bash
git add data/ingest/tests/test_retry.py
git commit -m "test(ingest): pin retry_http_get Retry-After-exceeds-budget path (#38)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 10: Add `headers={}` default to existing `FakeResponse`

**Files:**
- Modify: `data/ingest/tests/test_fetch_lead.py:18-28`

- [ ] **Step 1: Update the fixture**

Replace the existing `FakeResponse` class at lines 18-28 of `data/ingest/tests/test_fetch_lead.py`:

```python
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
```

- [ ] **Step 2: Run tests to verify nothing breaks**

```bash
.venv/bin/pytest tests/ -v
```

Expected: all existing tests still pass (87 + 40 from `test_retry.py` = 127 passed). No test changes — this is a back-compat-additive change.

- [ ] **Step 3: Commit**

```bash
git add data/ingest/tests/test_fetch_lead.py
git commit -m "test(ingest): add headers default to FakeResponse for retry path (#38)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 11: Wire `retry_http_get` into the three strategy fetchers

**Files:**
- Modify: `data/ingest/simple_wikipedia.py:526-527, 573-574, 643-644`
- Modify: `data/ingest/tests/test_fetch_lead.py` (append integration test)

- [ ] **Step 1: Write the failing integration test**

Append to `data/ingest/tests/test_fetch_lead.py`:

```python
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
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
.venv/bin/pytest tests/test_fetch_lead.py::test_fetch_lead_retries_on_429_then_succeeds -v
```

Expected: FAIL with `RuntimeError: fake http error 429` — the strategy fetcher's `resp.raise_for_status()` fires on the first 429 because `retry_http_get` isn't wired in yet.

- [ ] **Step 3: Wire the three strategy fetchers**

Replace the existing call site at `data/ingest/simple_wikipedia.py:526-527` (in `_fetch_lead_via_text_extracts`):

```python
    resp = retry_http_get(
        http_client,
        source.api_url,
        params=params,
        timeout=30.0,
        settings=_RETRY_SETTINGS,
        sleep=time.sleep,
        jitter_fn=random.random,
    )
    resp.raise_for_status()
```

Replace the call site at `data/ingest/simple_wikipedia.py:573-574` (in `_fetch_leads_via_text_extracts`):

```python
    resp = retry_http_get(
        http_client,
        source.api_url,
        params=params,
        timeout=60.0,
        settings=_RETRY_SETTINGS,
        sleep=time.sleep,
        jitter_fn=random.random,
    )
    resp.raise_for_status()
```

Replace the call site at `data/ingest/simple_wikipedia.py:643-644` (in `_fetch_lead_via_klexikon`):

```python
    resp = retry_http_get(
        http_client,
        source.api_url,
        params=params,
        timeout=30.0,
        settings=_RETRY_SETTINGS,
        sleep=time.sleep,
        jitter_fn=random.random,
    )
    resp.raise_for_status()
```

Add `random` to the imports at the top of `simple_wikipedia.py` (after the existing `time` import, alphabetical):

```python
import random
import re
import time
```

(`time` is already imported; `random` is the only new one. Verify with `grep "^import" data/ingest/simple_wikipedia.py` before adding.)

Add the `retry` module import after the existing imports (find the existing `from typing import ...` block or the first project-internal import; place this above the constants region):

```python
from retry import RetrySettings, retry_http_get
```

Add a module-level retry settings instance near the existing `_DEFAULT_USER_AGENT` constant (around line 763 — pick a natural spot in the constants region, e.g. just before `_SHORT_LEAD_WORD_THRESHOLD`):

```python
# Retry settings used by every strategy fetcher's HTTP-call wrapper.
# Single source of truth so tuning is one constant edit, not three.
_RETRY_SETTINGS = RetrySettings.default()
```

- [ ] **Step 4: Run the integration test to verify it passes**

```bash
.venv/bin/pytest tests/test_fetch_lead.py::test_fetch_lead_retries_on_429_then_succeeds -v
```

Expected: PASS.

- [ ] **Step 5: Run the full Python test suite**

```bash
.venv/bin/pytest tests/ -v
```

Expected: 88 + 40 = 128 passed (87 existing + 1 new in test_fetch_lead.py + 40 from test_retry.py).

- [ ] **Step 6: Commit**

```bash
git add data/ingest/simple_wikipedia.py data/ingest/tests/test_fetch_lead.py
git commit -m "feat(ingest): wire retry_http_get into all 3 strategy fetchers (#38)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 12: Update CLAUDE.md with the retry-helper invariant

**Files:**
- Modify: `CLAUDE.md` (one new bullet near the existing data/ingest gotchas)

- [ ] **Step 1: Find the right insertion point**

```bash
cd /Users/hherb/src/primer
grep -n "Wikipedia-shaped ingestion" CLAUDE.md
```

Expected: a line number near the existing data/ingest bullet (around line 130-135 today; verify with the grep output).

- [ ] **Step 2: Insert the new bullet immediately before the closing pedagogical-principles section**

Use the Edit tool to add this bullet right after the existing "Klexikon was chosen over regular `de.wikipedia.org`" bullet (the last data/ingest-related bullet in the file). Match exact surrounding text first to find the unique anchor:

```markdown
- **`retry_http_get` is the single HTTP-call retry boundary in `data/ingest/`.** Every strategy fetcher wraps its `http_client.get(...)` through `retry.retry_http_get`, retrying on HTTP 429 / 5xx with exponential backoff (3 attempts × 0.5 s × 2 + ±10% jitter ≈ ~1.5 s worst case before failure). `Retry-After` is honoured for delta-seconds form (HTTP-date silently drops to `compute_delay`); waits beyond the 30 s budget surface immediately as `RetryCapExceeded`. Network errors (`requests.exceptions.*`) propagate unchanged — out of scope for issue #38. Constants live in `data/ingest/retry.py` (`DEFAULT_*` module level + `RetrySettings.default()`); a drift-guard test in `tests/test_retry.py` pins the alignment.
```

- [ ] **Step 3: Sanity-check the bullet's placement**

```bash
grep -A 2 "retry_http_get is the single" CLAUDE.md
```

Expected: the bullet plus the next bullet of the file (the pedagogical-principles header or a related neighbour).

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md
git commit -m "docs(claude.md): record retry_http_get invariant for data/ingest (#38)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 13: Final verification + cargo lints

**Files:** none.

- [ ] **Step 1: Run the full Python test suite one more time**

```bash
cd /Users/hherb/src/primer/data/ingest
.venv/bin/pytest tests/ -v
```

Expected: 128 passed.

- [ ] **Step 2: Run the full Rust test suite (defensive — should be untouched)**

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo test --workspace
```

Expected: 605 passed, 0 failed, 2 ignored.

- [ ] **Step 3: Rust lints (defensive)**

```bash
~/.cargo/bin/cargo clippy --workspace --all-targets
~/.cargo/bin/cargo fmt --all -- --check
```

Expected: clean.

- [ ] **Step 4: Push the branch and open the PR**

```bash
git push -u origin feature/issue-38-fetch-leads-backoff
gh pr create --title "fix(ingest): add 429/5xx backoff to fetch_leads (#38)" --body "$(cat <<'EOF'
## Summary

Closes #38.

- New `data/ingest/retry.py` module: pure helpers (`is_retryable_status`, `parse_retry_after`, `compute_delay`) + composing `retry_http_get`. Constants and `RetrySettings.default()` live in the same module; drift-guard test pins the alignment.
- Three strategy fetchers in `simple_wikipedia.py` route their `http_client.get(...)` through `retry_http_get`. Single source of truth via module-level `_RETRY_SETTINGS = RetrySettings.default()`.
- Defaults: 3 attempts × 0.5 s × 2 + ±10% jitter ≈ ~1.5 s worst case before failure. `Retry-After` honoured up to 30 s.
- Out of scope (deferred): failed-batch persistence, network-error retry, HTTP-date `Retry-After` parsing. Spec at [docs/superpowers/specs/2026-05-09-fetch-leads-backoff-design.md](docs/superpowers/specs/2026-05-09-fetch-leads-backoff-design.md).

## Test plan

- [x] `pytest tests/` from `data/ingest/` — 128 passed (87 prior + 40 in `test_retry.py` + 1 new integration test in `test_fetch_lead.py`)
- [x] `cargo test --workspace` from `src/` — 605 passed, 0 failed, 2 ignored (Rust untouched)
- [x] `cargo clippy --workspace --all-targets` clean
- [x] `cargo fmt --all -- --check` clean
- [x] TDD: every behavioural change watched RED before GREEN

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

Expected: PR URL printed.

---

## Self-review summary

- **Spec coverage:** Every spec section maps to at least one task. Architecture (Task 1, 11). Algorithm (Task 5–9). Pure helpers (Task 2–4). Test surface (Task 1–10). Constants (Task 1). Documentation (Task 12). Out-of-scope items are not in any task — intentional.
- **No placeholders:** every code block is complete; every command is exact; every expected outcome is named.
- **Type consistency:** `RetrySettings` field names (`max_attempts`, `base_delay_s`, `backoff_factor`, `jitter_fraction`, `retry_after_budget_s`) and `RetryCapExceeded` constructor kwargs (`attempts`, `last_status`, `retry_after`) are uniform across every task that references them.
- **Granularity:** each step is a single editor action. Most tasks have 5 steps (test → fail → impl → pass → commit). Task 6 is unusual (no impl step because Task 5's impl already covers it; the new test is purely a regression pin).
- **TDD discipline:** every behavioural task starts with a failing test. The exception is Task 10 (additive fixture change — runs the existing tests) and Task 12 (CLAUDE.md doc edit — no test surface).
