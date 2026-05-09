# Backoff for `fetch_leads` (issue #38)

**Status:** Approved 2026-05-09.
**Author:** Horst Herb + Claude Code (paired brainstorming).
**Phase:** 0.3 — robustness pass on the data-ingestion pipeline.
**Predecessor:** [2026-05-06-wikipedia-ingestion-design.md](2026-05-06-wikipedia-ingestion-design.md), Klexikon ingest (PR #54).

## Goal

Add exponential-backoff retry to `data/ingest/simple_wikipedia.py`'s HTTP path so a transient HTTP 429 (rate-limit) or 5xx (server error) doesn't abort a multi-title ingest run at title 87 of 100. Required behaviour: cap retries (default 3 attempts), honour `Retry-After` when present, surface the original failure if the cap is exceeded. Test seam (`http_client` injection) stays intact.

## Non-goals

- **Failed-batch persistence.** Issue #38 lists this as optional; we explicitly defer it. The current ingest is one-shot per dev session; if scheduled ingest ever ships, file a follow-up.
- **Network-error retry.** `requests.exceptions.ConnectionError`, `Timeout`, etc. propagate immediately. Issue #38 scope is HTTP 429 / 5xx only. Worth re-evaluating if scheduled ingest lands.
- **HTTP-date `Retry-After` parsing.** Delta-seconds form only; HTTP-date silently drops to the computed exponential delay. Carries the same trade-off as `primer_core::retry` (already a known carry-forward risk).
- **Throttle compliance against MediaWiki's documented per-IP rate limits.** Those kick in at thousands-of-edits-per-minute scale; this design is defensive against transient blips, not a polite-crawler implementation.
- **Bumping `simple_wikipedia.py` past 500 lines as part of this PR.** It's at 907 lines today, already over the guideline, but split candidates are listed in NEXT_SESSION.md and deferred until a third source ships.

## Architecture

A new pure helper module `data/ingest/retry.py` exposes backoff arithmetic + a single composing function the strategy fetchers call. The module mirrors the conceptual shape of `src/crates/primer-core/src/retry.rs` but stays HTTP-shaped (the Rust helper is `InferenceError`-shaped).

```
data/ingest/
├── retry.py                                   # NEW: pure helpers + retry_http_get
├── simple_wikipedia.py                        # 3 call sites change: http_client.get(...) → retry_http_get(...)
└── tests/
    ├── test_retry.py                          # NEW: pure-helper unit tests (no HTTP)
    └── test_fetch_lead.py                     # MODIFIED: add 1 integration-shaped test that drives the retry path
```

`retry.py` exports:

| Symbol | Kind | Purpose |
| --- | --- | --- |
| `DEFAULT_MAX_ATTEMPTS = 3` | const | Total attempts including the first |
| `DEFAULT_BASE_DELAY_S = 0.5` | const | Initial delay before the second attempt |
| `DEFAULT_BACKOFF_FACTOR = 2` | const | Multiplicative growth factor |
| `DEFAULT_JITTER_FRACTION = 0.1` | const | ±10% noise on computed delays |
| `DEFAULT_RETRY_AFTER_BUDGET_S = 30.0` | const | Cap on `Retry-After` we will honour |
| `RetrySettings` | `@dataclass(frozen=True)` | Carrier; `.default()` classmethod returns the consts above |
| `RetryCapExceeded(RuntimeError)` | exception | Raised when attempts exhaust or `Retry-After` exceeds budget |
| `is_retryable_status(code: int) -> bool` | pure fn | True iff `code == 429 or 500 <= code < 600` |
| `parse_retry_after(value: str \| None) -> float \| None` | pure fn | Delta-seconds form only; HTTP-date / malformed → None |
| `compute_delay(settings, attempt, jitter_seed) -> float` | pure fn | `base * factor**attempt * (1 + jitter_fraction * jitter_seed)`; clamp ≥ 0 |
| `retry_http_get(http_client, url, *, params, timeout, settings, sleep, jitter_fn) -> Response` | composing fn | The single function the strategy fetchers call |

`retry_http_get` injects `sleep=time.sleep` and `jitter_fn=random.random` as kwargs so tests can pass `sleep=lambda _: None` (no real sleeps in unit tests) and `jitter_fn=lambda: 0.5` (deterministic).

## Algorithm

`retry_http_get(http_client, url, *, params, timeout, settings, sleep, jitter_fn)`:

1. `attempt = 0`. Loop:
   1. Call `resp = http_client.get(url, params=params, timeout=timeout)`.
      - Network errors (`requests.exceptions.*`) propagate unchanged.
   2. If `not is_retryable_status(resp.status_code)` → return `resp` (existing strategy code calls `resp.raise_for_status()` and continues).
   3. Else (retryable):
      - `attempts_left = settings.max_attempts - (attempt + 1)`.
      - If `attempts_left == 0` → raise `RetryCapExceeded(attempts=attempt+1, last_status=resp.status_code, retry_after=...)`.
      - Read `getattr(resp, "headers", {}).get("Retry-After")`.
      - `parsed = parse_retry_after(header_value)`.
      - If `parsed is not None`:
        - If `parsed > settings.retry_after_budget_s` → raise `RetryCapExceeded(...)` immediately.
        - `delay = parsed`.
      - Else: `delay = compute_delay(settings, attempt, jitter_fn() * 2 - 1)` (map [0, 1) to [-1, 1)).
      - `sleep(delay)`.
      - `attempt += 1`.

Three strategy call sites change one line each:

```python
# before
resp = http_client.get(source.api_url, params=params, timeout=30.0)
resp.raise_for_status()

# after
resp = retry_http_get(http_client, source.api_url, params=params, timeout=30.0)
resp.raise_for_status()
```

`retry_http_get` accepts an optional `settings` kwarg; default-constructs `RetrySettings.default()` if omitted. The strategy fetchers do **not** pass `settings`. A single source-of-truth means tuning is one constant edit, not three.

## Failure modes

| Condition | Behaviour |
| --- | --- |
| 200 / 3xx / 4xx (non-429) | Returned to caller unchanged; existing `raise_for_status` handles 4xx |
| 429 / 5xx, attempt 1 of 3 | Sleep, retry |
| 429 / 5xx, attempt 3 of 3 | Raise `RetryCapExceeded(attempts=3, last_status, retry_after=str|None)` |
| 429 with `Retry-After: 5` (within budget) | Sleep 5 s, retry |
| 429 with `Retry-After: 60` (exceeds 30 s budget) | Raise `RetryCapExceeded` immediately, attempts=1 |
| 429 with `Retry-After: <HTTP-date>` | Falls back to `compute_delay`; HTTP-date dropped silently |
| 429 with `Retry-After: garbage` | Falls back to `compute_delay` |
| `requests.exceptions.ConnectionError` | Propagates unchanged (out of scope) |

## Test surface

### `tests/test_retry.py` (new, ~12 tests)

Pure-helper unit tests. No real HTTP, no real sleeping.

- `is_retryable_status`: 429 → True; 500 → True; 502 → True; 503 → True; 599 → True; 200 → False; 301 → False; 404 → False; 401 → False.
- `parse_retry_after`: `"5"` → 5.0; `" 12 "` → 12.0; `"3.5"` → 3.5; `None` → None; `"Wed, 21 Oct 2015 07:28:00 GMT"` (HTTP-date) → None; `"garbage"` → None; `""` → None.
- `compute_delay`: attempt=0 jitter=0 → base; attempt=1 jitter=0 → base × factor; attempt=2 jitter=0 → base × factor²; jitter=+1 produces `delay × (1 + jitter_fraction)`; jitter=-1 produces `delay × (1 - jitter_fraction)`; never negative.
- `RetrySettings.default` mirrors module consts (drift guard).
- `retry_http_get` happy-path: 200 first try → 1 call, no sleep.
- `retry_http_get` succeeds on attempt 3 after two 503s → 3 calls, 2 sleeps with delays equal to `compute_delay(.., attempt=0)` and `compute_delay(.., attempt=1)`.
- `retry_http_get` exhausts attempts → `RetryCapExceeded` raised, calls=3, sleeps=2 (no sleep after the final failure).
- `retry_http_get` honours Retry-After=`"1"` within budget → sleep called with 1.0.
- `retry_http_get` Retry-After=`"60"` exceeds 30 s budget → immediate `RetryCapExceeded`, no sleep.
- `retry_http_get` non-retryable 404 returned unchanged → 1 call, no sleep.

A test fake `RetryFakeHttpClient` returns a scripted sequence of `(status_code, payload, headers)` per call. Tiny, lives in `test_retry.py`. Distinct from `FakeHttpClient` / `KlexikonFakeHttpClient` because retry tests need to drive multiple calls per logical fetch.

### `tests/test_fetch_lead.py` (modified, +1 integration-shaped test)

Existing `FakeResponse` gains a `headers={}` default in `__init__` so existing tests keep working unchanged. Existing tests are not edited.

Add one test:

- `test_fetch_lead_retries_on_429_then_succeeds` — `FakeHttpClient` configured to return a 429 once (with no `Retry-After` header so the path goes through `compute_delay`), then the real Photosynthesis payload. Asserts `fetch_lead` returns the lead AND `len(client.calls) == 2`. Uses pytest's `monkeypatch` fixture to stub `time.sleep` to a no-op for the duration of the test — that's the standard pytest pattern for "this code calls a real-world side effect we don't want in unit tests" and avoids plumbing a `settings` kwarg through `fetch_lead`'s public signature for a single test.

## Constants

All five numerical defaults live in `retry.py`:

```python
DEFAULT_MAX_ATTEMPTS = 3
DEFAULT_BASE_DELAY_S = 0.5
DEFAULT_BACKOFF_FACTOR = 2
DEFAULT_JITTER_FRACTION = 0.1
DEFAULT_RETRY_AFTER_BUDGET_S = 30.0
```

Rationale:
- 3 attempts × 0.5 s × 2 ≈ ~1.5 s + jitter worst case before failure. Tight enough that an exhausted retry surfaces quickly; loose enough that one transient blip recovers.
- 30 s `Retry-After` budget is much looser than the Rust 5 s because this is a one-shot dev tool — no live conversation latency to protect. If a server says "wait 30 s", we wait. If it says 60 s, we surface the failure so the developer can re-run later.
- 10% jitter is small but non-zero so concurrent runs (rare, but possible during dev) don't lock-step.

A drift-guard test pins `RetrySettings.default()` to these constants, mirroring the Rust pattern.

## Documentation

- New CLAUDE.md bullet under the "data/ingest" section: one-line invariant about `retry_http_get` being the single HTTP-call retry boundary, what it does and doesn't retry on, and where the constants live.
- README.md: no change needed (this is a dev-tool internals improvement; no user-facing surface).
- ROADMAP.md: no change needed (issue #38 is in the smaller-scope list, not a phase milestone).

## Risks

- **`FakeResponse.headers` default may conflict with future test fixtures that explicitly set headers.** Mitigation: dataclass-style optional kwarg with `headers: dict | None = None` → `self.headers = headers or {}` so explicit `{...}` still wins.
- **`retry_http_get` swallows the original `Response` on cap-exceed.** Mitigation: `RetryCapExceeded` carries `last_status` and `retry_after` (the values most useful for the developer); the response body is rarely useful in this failure mode and storing it would extend the exception's lifetime unnecessarily.
- **The integration test in `test_fetch_lead.py` is the only place we exercise the full strategy → `retry_http_get` → fake-client → strategy path.** Mitigation: that single test is enough because every strategy fetcher uses `retry_http_get` the same way; one example proves the wiring. Pure-helper coverage in `test_retry.py` is what defends behaviour at scale.

## Out of scope

Already covered above; restated here for the implementation plan:

- Failed-batch persistence sidecar (issue #38 optional bullet).
- Network-error retry.
- HTTP-date `Retry-After` parsing.
- Splitting `simple_wikipedia.py` into smaller modules.
- Adding retry to the new Klexikon `parse&prop=wikitext` path beyond what `_fetch_lead_via_klexikon` already routes through (the per-title loop already uses `retry_http_get`).
