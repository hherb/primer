use super::*;
use futures::StreamExt;
use futures::channel::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[test]
fn metrics_path_from_value_disabled_when_unset_or_empty() {
    assert_eq!(metrics_path_from_value(None), None);
    assert_eq!(metrics_path_from_value(Some(OsString::new())), None);
}

#[test]
fn metrics_path_from_value_returns_path_when_set() {
    assert_eq!(
        metrics_path_from_value(Some(OsString::from("/data/x/qnn_metrics.jsonl"))),
        Some(PathBuf::from("/data/x/qnn_metrics.jsonl"))
    );
}

#[test]
fn format_metric_line_emits_expected_json_fields() {
    let timing = StreamTiming {
        ttft: Duration::from_millis(1234),
        decode_tokens: 30,
        decode_duration: Duration::from_millis(2000),
    };
    let line = format_metric_line(1_750_000_000_000, &timing);
    let v: serde_json::Value = serde_json::from_str(&line).expect("valid JSON line");
    assert_eq!(v["ts_unix_ms"], 1_750_000_000_000_u64);
    assert_eq!(v["ttft_ms"], 1234.0);
    assert_eq!(v["decode_tokens"], 30);
    assert_eq!(v["decode_ms"], 2000.0);
    // 30 tokens / 2.0s = 15.0 tok/s.
    assert_eq!(v["tok_per_s"], 15.0);
}

#[test]
fn append_metric_line_creates_and_appends() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("qnn_metrics.jsonl");
    append_metric_line(&path, r#"{"a":1}"#);
    append_metric_line(&path, r#"{"a":2}"#);
    let contents = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = contents.lines().collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0], r#"{"a":1}"#);
    assert_eq!(lines[1], r#"{"a":2}"#);
}

#[test]
fn should_rotate_is_inclusive_at_the_cap() {
    // `>=`: at-or-over the cap rotates; strictly under does not. The
    // boundary matters — the cap is the largest the live file may be
    // *before* an append, so it can exceed the cap by at most one record.
    assert!(!should_rotate(99, 100));
    assert!(should_rotate(100, 100));
    assert!(should_rotate(101, 100));
}

#[test]
fn rotated_metrics_path_appends_suffix_to_file_name() {
    assert_eq!(
        rotated_metrics_path(Path::new("/data/x/qnn_metrics.jsonl")),
        PathBuf::from("/data/x/qnn_metrics.jsonl.1")
    );
}

#[test]
fn append_rotates_when_file_reaches_cap() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("qnn_metrics.jsonl");
    let rotated = rotated_metrics_path(&path);
    let cap = 50u64;

    // A line longer than the cap so the file is over-cap after one write.
    let big = format!("first-{}", "a".repeat(60));
    // First write: the file does not exist yet (len 0 < cap) ⇒ no rotation,
    // just appended.
    append_metric_line_capped(&path, &big, cap);
    assert!(
        !rotated.exists(),
        "no rotation before the file reaches the cap"
    );

    // Second write: the existing file is now over the cap ⇒ rotate first,
    // then append into a fresh live file.
    append_metric_line_capped(&path, "second", cap);
    assert!(rotated.exists(), "rotated backup must be created");

    // The live file holds only the post-rotation line.
    let live = std::fs::read_to_string(&path).unwrap();
    assert_eq!(live.lines().collect::<Vec<_>>(), vec!["second"]);
    // The backup holds the pre-rotation content.
    let backup = std::fs::read_to_string(&rotated).unwrap();
    assert!(
        backup.contains("first-"),
        "backup should hold the first line"
    );
}

#[test]
fn append_rotation_replaces_an_existing_backup() {
    // Single-backup rotation: a later rotation overwrites the prior `.1`
    // so the footprint stays bounded at ~2× cap rather than accumulating
    // .1/.2/.3 forever. Each line here is itself over the cap, so every
    // append after the first rotates the previous (single-line) live file.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("qnn_metrics.jsonl");
    let rotated = rotated_metrics_path(&path);
    let cap = 30u64;
    let big = |tag: &str| format!("{tag}-{}", "z".repeat(40));

    append_metric_line_capped(&path, &big("one"), cap); // file created
    append_metric_line_capped(&path, &big("two"), cap); // rotates "one" out
    append_metric_line_capped(&path, &big("three"), cap); // rotates "two" out
    append_metric_line_capped(&path, &big("four"), cap); // rotates "three" out

    // The single backup holds only the most-recent rotation ("three"),
    // never the older ones — proving the `.1` is replaced, not chained.
    let backup = std::fs::read_to_string(&rotated).unwrap();
    assert!(
        backup.contains("three-"),
        "backup must hold the most recent rotated content, got: {backup:?}"
    );
    assert!(
        !backup.contains("one-") && !backup.contains("two-"),
        "older rotations must be gone, got: {backup:?}"
    );
    let live = std::fs::read_to_string(&path).unwrap();
    assert_eq!(live.lines().count(), 1);
    assert!(live.contains("four-"));
}

/// Build a `TokenStream` that emits `texts` as non-empty chunks then a
/// trailing empty `done` sentinel (the QNN backend's shape).
fn stream_of(texts: &[&str]) -> TokenStream {
    let (tx, rx) = mpsc::unbounded::<Result<TokenChunk>>();
    for t in texts {
        let _ = tx.unbounded_send(Ok(TokenChunk {
            text: (*t).to_string(),
            done: false,
            ..Default::default()
        }));
    }
    let _ = tx.unbounded_send(Ok(TokenChunk {
        text: String::new(),
        done: true,
        ..Default::default()
    }));
    drop(tx);
    Box::pin(rx)
}

#[tokio::test]
async fn metered_stream_passes_chunks_through_and_records_once() {
    let captured: Arc<Mutex<Vec<StreamTiming>>> = Arc::new(Mutex::new(Vec::new()));
    let sink_buf = Arc::clone(&captured);
    let metered = MeteredStream::new(
        stream_of(&["a", "b", "c", "d"]),
        Instant::now(),
        Box::new(move |timing| sink_buf.lock().unwrap().push(timing)),
    );

    // Consumer sees every chunk verbatim, including the done sentinel.
    let chunks: Vec<TokenChunk> = metered.map(|c| c.unwrap()).collect().await;
    assert_eq!(chunks.len(), 5);
    assert_eq!(chunks[0].text, "a");
    assert!(chunks[4].done);

    // Recorded exactly once: 4 non-empty chunks ⇒ 3 decode tokens.
    let recorded = captured.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].decode_tokens, 3);
}

#[tokio::test]
async fn metered_stream_records_when_consumer_breaks_on_done() {
    // The real chat consumer (dialogue manager) breaks out of its loop on
    // the first `done` chunk and never polls again, so the wrapped stream
    // is dropped before yielding `None`. The metric must still be recorded
    // from the `done` chunk passing through — this is the regression that
    // produced zero metric lines despite working turns on-device.
    let captured: Arc<Mutex<Vec<StreamTiming>>> = Arc::new(Mutex::new(Vec::new()));
    let sink_buf = Arc::clone(&captured);
    let mut metered = MeteredStream::new(
        stream_of(&["a", "b", "c"]),
        Instant::now(),
        Box::new(move |timing| sink_buf.lock().unwrap().push(timing)),
    );

    // Drive exactly like turn.rs: pull chunks, break on the first `done`,
    // then drop the stream without polling for `None`.
    while let Some(item) = metered.next().await {
        let chunk = item.unwrap();
        if chunk.done {
            break;
        }
    }
    drop(metered);

    let recorded = captured.lock().unwrap();
    assert_eq!(
        recorded.len(),
        1,
        "metric must be recorded on the done chunk"
    );
    assert_eq!(recorded[0].decode_tokens, 2); // 3 body chunks ⇒ 2 after first
}

#[tokio::test]
async fn metered_stream_finalizes_on_error() {
    let captured: Arc<Mutex<Vec<StreamTiming>>> = Arc::new(Mutex::new(Vec::new()));
    let sink_buf = Arc::clone(&captured);
    let (tx, rx) = mpsc::unbounded::<Result<TokenChunk>>();
    let _ = tx.unbounded_send(Ok(TokenChunk {
        text: "partial".to_string(),
        done: false,
        ..Default::default()
    }));
    let _ = tx.unbounded_send(Err(primer_core::error::PrimerError::Inference(
        "boom".into(),
    )));
    drop(tx);

    let metered = MeteredStream::new(
        Box::pin(rx),
        Instant::now(),
        Box::new(move |timing| sink_buf.lock().unwrap().push(timing)),
    );
    let results: Vec<Result<TokenChunk>> = metered.collect().await;
    // One ok chunk, one error.
    assert_eq!(results.len(), 2);
    assert!(results[1].is_err());
    // Still recorded exactly once despite the error.
    assert_eq!(captured.lock().unwrap().len(), 1);
}
