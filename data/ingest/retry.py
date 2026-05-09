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

import logging
from dataclasses import dataclass
from typing import Any, Callable

logger = logging.getLogger(__name__)


# ── Defaults (no magic numbers) ──────────────────────────────────────

# Total attempts including the first. 3 × 0.5 s × 2 ≈ ~1.5 s + jitter
# worst case before failure — tight enough to surface persistent
# failures quickly, loose enough to ride out a transient blip.
DEFAULT_MAX_ATTEMPTS = 3

# Initial delay before the second attempt, in seconds.
DEFAULT_BASE_DELAY_S = 0.5

# Multiplicative growth factor between attempts. Float-typed so a
# fractional growth (e.g. 1.5) is a one-line config tweak rather than a
# type-cast surprise.
DEFAULT_BACKOFF_FACTOR = 2.0

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
    backoff_factor: float
    jitter_fraction: float
    retry_after_budget_s: float

    @classmethod
    def default(cls) -> "RetrySettings":
        """Construct a settings instance from the module defaults.

        Pinned by ``tests.test_retry.test_default_settings_mirror_module_consts``
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


# ── Pure helpers ─────────────────────────────────────────────────────


def is_retryable_status(code: int) -> bool:
    """True iff ``code`` is a retryable HTTP status.

    Retryable: 429 (Too Many Requests) and the full 5xx range. 4xx
    errors other than 429 indicate a problem in the request itself
    (auth, missing resource, malformed parameters) and re-running
    won't help.
    """
    return code == 429 or 500 <= code < 600


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
        retry_after_raw = resp.headers.get("Retry-After")
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
        logger.info(
            "retry_http_get: status=%d attempt=%d delay=%.2fs url=%s",
            resp.status_code,
            attempts_made,
            delay,
            url,
        )
        sleep(delay)
        attempt += 1
