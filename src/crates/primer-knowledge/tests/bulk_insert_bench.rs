//! Bulk-insert micro-benchmark for the Phase 0.2 corpus-bootstrap path.
//!
//! Issue #22: `SqliteKnowledgeBase` insert previously rebuilt its SQL with
//! `format!` and re-`prepare()`d a fresh statement on every row. At
//! conversation-turn rates that is irrelevant (LLM latency dominates), but
//! the corpus-bootstrap path inserts the entire Wikipedia/encyclopedia
//! corpus in one go — potentially millions of rows — and there the per-row
//! re-parse is real cost. The fix switched the hot paths to
//! `Connection::prepare_cached`, so the per-row cost is a statement-cache
//! hash lookup rather than an FTS5/SQL re-parse.
//!
//! This benchmark times two paths inserting the same number of rows into
//! equivalent in-memory schemas:
//!
//! * **baseline** — a raw `rusqlite` loop doing `conn.execute(&format!(..))`
//!   per row (the pre-#22 behaviour), and
//! * **cached** — the crate's public `insert_passage`, which now goes
//!   through `prepare_cached`.
//!
//! In-memory DBs (`:memory:`) are used on both sides so disk `fsync` does
//! not mask the CPU cost the change actually targets. The test asserts
//! correctness (both paths insert every row) and prints the wall-clock
//! delta; the printed ratio is the "is the win real?" evidence the issue
//! asks for. It is `#[ignore]`'d so it never runs in the default suite —
//! run it explicitly:
//!
//! ```text
//! ~/.cargo/bin/cargo test -p primer-knowledge --test bulk_insert_bench \
//!     -- --ignored --nocapture
//! ```

use std::path::Path;
use std::time::Instant;

use primer_core::i18n::Locale;
use primer_knowledge::SqliteKnowledgeBase;
use rusqlite::Connection;

/// Row count for the benchmark. Large enough that the cumulative per-row
/// parse cost in the baseline path is well above timer noise, small enough
/// that the whole test runs in a second or two on a laptop.
const BENCH_ROWS: usize = 50_000;

/// One synthetic passage, deterministic in `i` so both paths insert
/// byte-identical data and the comparison is apples-to-apples.
fn row(i: usize) -> (String, String, String) {
    (
        format!("bench-{i}"),
        "bench:source".to_string(),
        format!("passage number {i} about photosynthesis and rayleigh scattering"),
    )
}

/// Baseline: hand-rolled legacy insert — re-`format!` + `execute` (which
/// re-`prepare`s) per row, exactly what the crate did before issue #22.
fn time_baseline_format_execute() -> std::time::Duration {
    let conn = Connection::open_in_memory().expect("open in-memory baseline DB");
    conn.execute_batch(
        "CREATE VIRTUAL TABLE passages_en USING fts5(
            id, source, text,
            content='passages_en_content', content_rowid='rowid'
        );
        CREATE TABLE passages_en_content(
            rowid INTEGER PRIMARY KEY,
            id TEXT NOT NULL,
            source TEXT NOT NULL,
            text TEXT NOT NULL
        );",
    )
    .expect("create baseline schema");

    // Mirror the pre-#22 code: the table names were interpolated into the
    // SQL via `format!` on every call, re-allocating and re-parsing per row.
    let content_table = "passages_en_content";
    let passages_table = "passages_en";
    let start = Instant::now();
    for i in 0..BENCH_ROWS {
        let (id, source, text) = row(i);
        conn.execute(
            &format!("INSERT INTO {content_table}(id, source, text) VALUES (?1, ?2, ?3)"),
            rusqlite::params![id, source, text],
        )
        .expect("baseline content insert");
        let rowid = conn.last_insert_rowid();
        conn.execute(
            &format!(
                "INSERT INTO {passages_table}(rowid, id, source, text) VALUES (?4, ?1, ?2, ?3)"
            ),
            rusqlite::params![id, source, text, rowid],
        )
        .expect("baseline fts insert");
    }
    let elapsed = start.elapsed();

    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM passages_en_content", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n as usize, BENCH_ROWS, "baseline must insert every row");
    elapsed
}

/// Cached: the crate's public `insert_passage`, which routes through
/// `prepare_cached` after the #22 change.
fn time_cached_prepare_cached() -> std::time::Duration {
    let kb = SqliteKnowledgeBase::open_for_locale(Path::new(":memory:"), Locale::English)
        .expect("open in-memory cached KB");
    let start = Instant::now();
    for i in 0..BENCH_ROWS {
        let (id, source, text) = row(i);
        kb.insert_passage(&id, &source, &text)
            .expect("cached insert");
    }
    let elapsed = start.elapsed();
    assert_eq!(
        kb.passage_count().unwrap() as usize,
        BENCH_ROWS,
        "cached path must insert every row"
    );
    elapsed
}

#[test]
#[ignore = "perf benchmark; run explicitly with --ignored --nocapture"]
fn bulk_insert_prepare_cached_beats_format_execute() {
    // Warm both paths once so allocator / page-cache effects don't bias
    // whichever runs first, then take the timed measurement.
    let _ = time_cached_prepare_cached();
    let _ = time_baseline_format_execute();

    let baseline = time_baseline_format_execute();
    let cached = time_cached_prepare_cached();

    let speedup = baseline.as_secs_f64() / cached.as_secs_f64();
    println!("bulk insert of {BENCH_ROWS} rows (in-memory):");
    println!("  baseline (format! + execute, re-parse per row): {baseline:?}");
    println!("  cached   (prepare_cached):                       {cached:?}");
    println!("  speedup: {speedup:.2}x");

    // The printed ratio is the evidence the issue asks for. We do not
    // assert on wall-clock — that would flake under CI contention — only
    // that both paths actually inserted every row (checked above), and
    // that the cached path is not pathologically slower (a loose guard
    // that only fires on a genuine regression, never on timer noise).
    assert!(
        cached < baseline * 3,
        "prepare_cached path unexpectedly slower than the re-parse baseline \
         (cached {cached:?} vs baseline {baseline:?})"
    );
}
