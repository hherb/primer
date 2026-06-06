//! Run-loop helpers shared by every backend's benchmark example.
//!
//! [`measure_prompt`] drives one `generate_stream` call and times it;
//! [`format_report`] renders an aggregate report as text. Both are generic
//! over [`InferenceBackend`] / plain data so the device examples
//! (`examples/qnn_bench.rs`, `examples/llamacpp_bench.rs`) carry only argument
//! parsing + backend construction, and the timing/format logic is unit-tested
//! here on the default `cargo test`.

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use futures::StreamExt;
use primer_core::error::Result;
use primer_core::inference::{GenerationParams, InferenceBackend};

use super::metrics::{BenchReport, BenchTargets, PromptMeasurement, Verdict};
use super::prompts::BenchPrompt;

/// Measure one prompt: TTFT (issue → first non-empty chunk) and steady-state
/// decode rate (first token → last token). Generic over the backend. A
/// mid-stream error propagates.
pub async fn measure_prompt(
    backend: &dyn InferenceBackend,
    prompt: &BenchPrompt,
    params: &GenerationParams,
) -> Result<PromptMeasurement> {
    let issued = Instant::now();
    let mut stream = backend.generate_stream(&prompt.prompt, params).await?;

    let mut ttft: Option<Duration> = None;
    let mut first_token_at: Option<Instant> = None;
    let mut tokens_after_first = 0usize;
    let mut last_token_at = issued;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        let now = Instant::now();
        if !chunk.text.is_empty() {
            if ttft.is_none() {
                ttft = Some(now.duration_since(issued));
                first_token_at = Some(now);
            } else {
                tokens_after_first += 1;
            }
            last_token_at = now;
        }
        if chunk.done {
            break;
        }
    }

    let ttft = ttft.unwrap_or_else(|| issued.elapsed());
    let decode_duration = match first_token_at {
        Some(first) => last_token_at.duration_since(first),
        None => Duration::ZERO,
    };

    Ok(PromptMeasurement {
        label: prompt.label.clone(),
        ttft,
        decode_tokens: tokens_after_first,
        decode_duration,
    })
}

/// Render an aggregate report as multi-line text. Pure (returns a `String` the
/// caller prints) so it is testable without capturing stdout. When
/// `targets.is_measurement_only()` the verdict lines are replaced by a
/// "measurement only" note.
pub fn format_report(
    title: &str,
    report: &BenchReport,
    targets: &BenchTargets,
    verdict: &Verdict,
) -> String {
    let pf = |ok: bool| if ok { "PASS" } else { "FAIL" };
    let mut out = String::new();

    let _ = writeln!(
        out,
        "=== {title} report ({} runs, {} degenerate) ===",
        report.runs, report.degenerate_runs
    );

    let _ = write!(
        out,
        "TTFT  p50={:.0}ms  p95={:.0}ms",
        report.ttft_p50.as_secs_f64() * 1000.0,
        report.ttft_p95.as_secs_f64() * 1000.0,
    );
    match targets.max_ttft {
        Some(t) => {
            let _ = writeln!(
                out,
                "   (target p95 < {:.0}ms)  [{}]",
                t.as_secs_f64() * 1000.0,
                pf(verdict.ttft_pass)
            );
        }
        None => {
            let _ = writeln!(out);
        }
    }

    let _ = write!(
        out,
        "decode mean={:.2} tok/s  min={:.2} tok/s",
        report.decode_mean_tokens_per_sec, report.decode_min_tokens_per_sec,
    );
    match targets.min_decode_tokens_per_sec {
        Some(t) => {
            let _ = writeln!(
                out,
                "   (target min >= {t:.2})  [{}]",
                pf(verdict.decode_pass)
            );
        }
        None => {
            let _ = writeln!(out);
        }
    }

    match report.peak_temp_celsius {
        Some(peak) => {
            let _ = write!(out, "peak temp={peak:.1}°C");
            match targets.max_peak_temp_celsius {
                Some(t) => {
                    let _ = writeln!(
                        out,
                        "   (target <= {t:.1}°C)  [{}]",
                        pf(verdict.thermal_pass)
                    );
                }
                None => {
                    let _ = writeln!(out);
                }
            }
        }
        None => {
            let _ = writeln!(out, "peak temp=n/a (no thermal samples)");
        }
    }

    if targets.is_measurement_only() {
        let _ = writeln!(out, "measurement only (no acceptance gate)");
    } else {
        let _ = writeln!(out, "overall: [{}]", pf(verdict.all_pass()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::metrics::evaluate;
    use super::*;
    use async_trait::async_trait;
    use futures::channel::mpsc;
    use primer_core::inference::{Prompt, TokenChunk, TokenStream};

    /// A backend that emits `chunks` as non-empty token chunks, sleeping
    /// `gap` before each chunk after the first, then a final empty done chunk.
    struct TimedMock {
        chunks: Vec<String>,
        gap: Duration,
    }

    #[async_trait]
    impl InferenceBackend for TimedMock {
        fn name(&self) -> &str {
            "timed-mock"
        }
        async fn is_available(&self) -> bool {
            true
        }
        async fn generate_stream(
            &self,
            _prompt: &Prompt,
            _params: &GenerationParams,
        ) -> Result<TokenStream> {
            let chunks = self.chunks.clone();
            let gap = self.gap;
            let (tx, rx) = mpsc::unbounded::<Result<TokenChunk>>();
            tokio::spawn(async move {
                for (i, c) in chunks.iter().enumerate() {
                    if i > 0 {
                        tokio::time::sleep(gap).await;
                    }
                    let _ = tx.unbounded_send(Ok(TokenChunk {
                        text: c.clone(),
                        done: false,
                    }));
                }
                let _ = tx.unbounded_send(Ok(TokenChunk {
                    text: String::new(),
                    done: true,
                }));
            });
            Ok(Box::pin(rx))
        }
    }

    fn bench_prompt() -> BenchPrompt {
        BenchPrompt {
            label: "probe".to_string(),
            prompt: Prompt {
                system: "be socratic".to_string(),
                messages: vec![],
            },
        }
    }

    #[tokio::test]
    async fn measure_prompt_times_multi_chunk_stream() {
        let mock = TimedMock {
            chunks: vec!["a".into(), "b".into(), "c".into(), "d".into()],
            gap: Duration::from_millis(20),
        };
        let m = measure_prompt(&mock, &bench_prompt(), &GenerationParams::default())
            .await
            .unwrap();
        assert_eq!(m.label, "probe");
        // 4 chunks → 3 after the first.
        assert_eq!(m.decode_tokens, 3);
        // Decode window spans 3 gaps (~60ms); allow loose floor for scheduling.
        assert!(
            m.decode_duration >= Duration::from_millis(40),
            "decode_duration too short: {:?}",
            m.decode_duration
        );
        // First chunk is immediate, so TTFT is well under the decode window.
        assert!(m.ttft < m.decode_duration);
    }

    #[tokio::test]
    async fn measure_prompt_single_chunk_is_degenerate() {
        let mock = TimedMock {
            chunks: vec!["only".into()],
            gap: Duration::from_millis(20),
        };
        let m = measure_prompt(&mock, &bench_prompt(), &GenerationParams::default())
            .await
            .unwrap();
        assert_eq!(m.decode_tokens, 0);
        assert_eq!(m.decode_duration, Duration::ZERO);
        assert_eq!(m.decode_tokens_per_sec(), 0.0);
    }

    fn report() -> BenchReport {
        BenchReport {
            runs: 2,
            degenerate_runs: 0,
            ttft_p50: Duration::from_millis(800),
            ttft_p95: Duration::from_millis(1200),
            decode_mean_tokens_per_sec: 12.0,
            decode_min_tokens_per_sec: 10.0,
            peak_temp_celsius: Some(55.0),
        }
    }

    #[test]
    fn format_report_gated_shows_pass_fail() {
        let targets = BenchTargets {
            min_decode_tokens_per_sec: Some(15.0),
            max_ttft: Some(Duration::from_secs(3)),
            max_peak_temp_celsius: Some(70.0),
        };
        let verdict = evaluate(&report(), &targets);
        let text = format_report("llama.cpp benchmark", &report(), &targets, &verdict);
        assert!(text.contains("llama.cpp benchmark"));
        // decode min 10 < floor 15 → FAIL line present; overall present.
        assert!(text.contains("FAIL"));
        assert!(text.contains("overall:"));
        assert!(!text.contains("measurement only"));
    }

    #[test]
    fn format_report_measurement_only_has_note() {
        let targets = BenchTargets::none();
        let verdict = evaluate(&report(), &targets);
        let text = format_report("llama.cpp benchmark", &report(), &targets, &verdict);
        assert!(text.contains("measurement only"));
        assert!(!text.contains("overall:"));
        assert!(!text.contains("PASS"));
        assert!(!text.contains("FAIL"));
    }
}
