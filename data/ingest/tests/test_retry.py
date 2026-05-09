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
