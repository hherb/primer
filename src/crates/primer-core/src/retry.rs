//! Bounded retry helper for transient `InferenceError` conditions.
//!
//! The helper is locale-neutral and HTTP-neutral — it accepts any
//! closure returning `Result<T, InferenceError>` and dispatches on
//! `InferenceError::is_retryable`. Each retry decision emits a
//! `tracing::info!` event on the `primer::retry` target so a future
//! voice-mode change can subscribe and play a bridging message.
//!
//! Defaults live in `primer_core::consts::retry`.

use crate::consts::retry as defaults;
use crate::error::InferenceError;
use std::future::Future;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct RetrySettings {
    /// Total attempts including the first.
    pub max_attempts: u32,
    /// Initial backoff before the second attempt.
    pub base_delay: Duration,
    /// Multiplicative growth factor between attempts.
    pub backoff_factor: u32,
    /// Jitter as a fraction of the computed delay (±jitter_fraction).
    pub jitter_fraction: f32,
    /// Cap on Retry-After we will honor. Longer waits surface immediately.
    pub retry_after_budget: Duration,
}

impl Default for RetrySettings {
    fn default() -> Self {
        Self {
            max_attempts: defaults::DEFAULT_MAX_ATTEMPTS,
            base_delay: defaults::DEFAULT_BASE_DELAY,
            backoff_factor: defaults::DEFAULT_BACKOFF_FACTOR,
            jitter_fraction: defaults::DEFAULT_JITTER_FRACTION,
            retry_after_budget: defaults::DEFAULT_RETRY_AFTER_BUDGET,
        }
    }
}

/// Compute the delay before the (attempt+1)-th try, given the previous
/// attempt index (0-based). Pure — no I/O, no time.
///
/// `delay = base_delay * backoff_factor^attempt * (1 + jitter * uniform(-1, 1))`.
/// `jitter_seed` lets tests pin the random component.
fn compute_delay(
    settings: &RetrySettings,
    attempt: u32,
    jitter_seed: f32, // -1.0..=1.0
) -> Duration {
    let factor = settings.backoff_factor.saturating_pow(attempt) as u128;
    let base_ms = settings.base_delay.as_millis() * factor;
    let jitter_ms = (base_ms as f32) * settings.jitter_fraction * jitter_seed;
    let total = (base_ms as i128 + jitter_ms as i128).max(0) as u64;
    Duration::from_millis(total)
}

/// Cheap deterministic-ish jitter source. Returns a value in `[-1, 1]`
/// derived from the attempt index — sufficient for spreading retries
/// without inviting a `rand` workspace dep for a single helper.
fn jitter_seed_for(attempt: u32) -> f32 {
    // Take a time-derived nibble so different sessions don't lock-step.
    let now_ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let mixed = now_ns.wrapping_mul(1103515245).wrapping_add(attempt);
    let unit = (mixed % 2001) as f32 / 1000.0; // [0, 2]
    unit - 1.0 // [-1, 1]
}

pub async fn retry_with_backoff<T, F, Fut>(
    settings: &RetrySettings,
    mut op: F,
) -> Result<T, InferenceError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, InferenceError>>,
{
    let mut attempt: u32 = 0;
    loop {
        match op().await {
            Ok(t) => return Ok(t),
            Err(e) => {
                let attempts_left = settings.max_attempts.saturating_sub(attempt + 1);
                if !e.is_retryable() || attempts_left == 0 {
                    return Err(e);
                }
                let delay = match &e {
                    InferenceError::RateLimited {
                        retry_after: Some(d),
                    } => {
                        if *d > settings.retry_after_budget {
                            return Err(e);
                        }
                        *d
                    }
                    _ => {
                        let seed = jitter_seed_for(attempt);
                        compute_delay(settings, attempt, seed)
                    }
                };
                tracing::info!(
                    target: "primer::retry",
                    attempt,
                    kind = %e,
                    delay_ms = delay.as_millis() as u64,
                    "retrying inference call",
                );
                tokio::time::sleep(delay).await;
                attempt += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::Duration;

    fn run_async<Fut: Future<Output = T>, T>(fut: Fut) -> T {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .start_paused(true)
            .build()
            .unwrap()
            .block_on(fut)
    }

    fn settings_for_test() -> RetrySettings {
        RetrySettings {
            max_attempts: 3,
            base_delay: Duration::from_millis(100),
            backoff_factor: 2,
            jitter_fraction: 0.0, // deterministic — no jitter in tests
            retry_after_budget: Duration::from_secs(2),
        }
    }

    #[test]
    fn succeeds_on_first_attempt_with_one_call() {
        let calls = Rc::new(RefCell::new(0u32));
        let calls_for_op = calls.clone();
        let settings = settings_for_test();

        let result: Result<&'static str, InferenceError> = run_async(async move {
            retry_with_backoff(&settings, || {
                let calls_for_op = calls_for_op.clone();
                async move {
                    *calls_for_op.borrow_mut() += 1;
                    Ok("ok")
                }
            })
            .await
        });

        assert_eq!(result.unwrap(), "ok");
        assert_eq!(*calls.borrow(), 1);
    }

    #[test]
    fn succeeds_on_third_attempt() {
        let calls = Rc::new(RefCell::new(0u32));
        let calls_for_op = calls.clone();
        let settings = settings_for_test();

        let result: Result<&'static str, InferenceError> = run_async(async move {
            retry_with_backoff(&settings, || {
                let calls_for_op = calls_for_op.clone();
                async move {
                    let mut n = calls_for_op.borrow_mut();
                    *n += 1;
                    if *n < 3 {
                        Err(InferenceError::ServiceUnavailable)
                    } else {
                        Ok("ok")
                    }
                }
            })
            .await
        });

        assert_eq!(result.unwrap(), "ok");
        assert_eq!(*calls.borrow(), 3);
    }

    #[test]
    fn exhausts_attempts_then_surfaces_last_error() {
        let calls = Rc::new(RefCell::new(0u32));
        let calls_for_op = calls.clone();
        let settings = settings_for_test();

        let result: Result<(), InferenceError> = run_async(async move {
            retry_with_backoff(&settings, || {
                let calls_for_op = calls_for_op.clone();
                async move {
                    *calls_for_op.borrow_mut() += 1;
                    Err(InferenceError::ServiceUnavailable)
                }
            })
            .await
        });

        assert!(matches!(result, Err(InferenceError::ServiceUnavailable)));
        assert_eq!(*calls.borrow(), 3); // max_attempts
    }

    #[test]
    fn non_retryable_surfaces_immediately() {
        let calls = Rc::new(RefCell::new(0u32));
        let calls_for_op = calls.clone();
        let settings = settings_for_test();

        let result: Result<(), InferenceError> = run_async(async move {
            retry_with_backoff(&settings, || {
                let calls_for_op = calls_for_op.clone();
                async move {
                    *calls_for_op.borrow_mut() += 1;
                    Err(InferenceError::Auth)
                }
            })
            .await
        });

        assert!(matches!(result, Err(InferenceError::Auth)));
        assert_eq!(*calls.borrow(), 1); // no retry
    }

    #[test]
    fn respects_retry_after_within_budget() {
        let calls = Rc::new(RefCell::new(0u32));
        let calls_for_op = calls.clone();
        let settings = settings_for_test(); // budget = 2 s

        let result: Result<&'static str, InferenceError> = run_async(async move {
            retry_with_backoff(&settings, || {
                let calls_for_op = calls_for_op.clone();
                async move {
                    let mut n = calls_for_op.borrow_mut();
                    *n += 1;
                    if *n == 1 {
                        Err(InferenceError::RateLimited {
                            retry_after: Some(Duration::from_secs(1)),
                        })
                    } else {
                        Ok("ok")
                    }
                }
            })
            .await
        });

        assert_eq!(result.unwrap(), "ok");
        assert_eq!(*calls.borrow(), 2);
    }

    #[test]
    fn gives_up_when_retry_after_exceeds_budget() {
        let calls = Rc::new(RefCell::new(0u32));
        let calls_for_op = calls.clone();
        let settings = settings_for_test(); // budget = 2 s

        let result: Result<(), InferenceError> = run_async(async move {
            retry_with_backoff(&settings, || {
                let calls_for_op = calls_for_op.clone();
                async move {
                    *calls_for_op.borrow_mut() += 1;
                    Err(InferenceError::RateLimited {
                        retry_after: Some(Duration::from_secs(30)),
                    })
                }
            })
            .await
        });

        assert!(matches!(
            result,
            Err(InferenceError::RateLimited { retry_after: Some(d) }) if d == Duration::from_secs(30)
        ));
        assert_eq!(*calls.borrow(), 1); // gave up immediately
    }

    #[test]
    fn settings_default_matches_consts() {
        let s = RetrySettings::default();
        assert_eq!(s.max_attempts, defaults::DEFAULT_MAX_ATTEMPTS);
        assert_eq!(s.base_delay, defaults::DEFAULT_BASE_DELAY);
        assert_eq!(s.backoff_factor, defaults::DEFAULT_BACKOFF_FACTOR);
        assert_eq!(s.jitter_fraction, defaults::DEFAULT_JITTER_FRACTION);
        assert_eq!(s.retry_after_budget, defaults::DEFAULT_RETRY_AFTER_BUDGET);
    }

    #[test]
    fn compute_delay_grows_with_attempt() {
        let s = settings_for_test();
        let d0 = compute_delay(&s, 0, 0.0);
        let d1 = compute_delay(&s, 1, 0.0);
        let d2 = compute_delay(&s, 2, 0.0);
        assert_eq!(d0, Duration::from_millis(100));
        assert_eq!(d1, Duration::from_millis(200));
        assert_eq!(d2, Duration::from_millis(400));
    }
}
