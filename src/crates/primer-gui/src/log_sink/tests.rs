use super::*;
use std::fs;
use tracing_subscriber::EnvFilter;

#[test]
fn desktop_log_path_lives_under_primer_home_logs() {
    let home = Path::new("/home/someone");
    assert_eq!(
        desktop_log_path(home),
        home.join(".primer").join("logs").join("gui.log")
    );
}

#[test]
fn rotate_moves_an_oversized_log_aside() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("gui.log");
    fs::write(&log, b"0123456789").unwrap();

    rotate_if_oversized(&log, 10).unwrap();

    assert!(!log.exists(), "oversized log must be moved aside");
    assert_eq!(fs::read(rotated_path(&log)).unwrap(), b"0123456789");
}

#[test]
fn rotate_keeps_a_log_under_the_cap() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("gui.log");
    fs::write(&log, b"short").unwrap();

    rotate_if_oversized(&log, 10).unwrap();

    assert_eq!(fs::read(&log).unwrap(), b"short");
    assert!(!rotated_path(&log).exists());
}

#[test]
fn rotate_replaces_the_previous_rotated_log() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("gui.log");
    fs::write(rotated_path(&log), b"ancient").unwrap();
    fs::write(&log, b"0123456789").unwrap();

    rotate_if_oversized(&log, 10).unwrap();

    // Only one generation is kept, so disk use stays at most 2x the cap.
    assert_eq!(fs::read(rotated_path(&log)).unwrap(), b"0123456789");
}

#[test]
fn rotate_tolerates_a_missing_log() {
    let dir = tempfile::tempdir().unwrap();
    rotate_if_oversized(&dir.path().join("gui.log"), 10).unwrap();
}

#[test]
fn open_log_file_creates_the_logs_dir_and_appends() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join(".primer").join("logs").join("gui.log");

    {
        use std::io::Write;
        open_log_file(&log, GUI_LOG_MAX_BYTES)
            .unwrap()
            .write_all(b"first\n")
            .unwrap();
        open_log_file(&log, GUI_LOG_MAX_BYTES)
            .unwrap()
            .write_all(b"second\n")
            .unwrap();
    }

    assert_eq!(fs::read_to_string(&log).unwrap(), "first\nsecond\n");
}

/// The issue #355 failure: a Dock launch discards stdout, so a `warn!` must
/// also land in the file — as plain text, since nothing renders ANSI there.
#[test]
fn desktop_subscriber_tees_events_into_the_log_file() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("gui.log");
    let file = open_log_file(&log, GUI_LOG_MAX_BYTES).unwrap();

    let subscriber = desktop_subscriber(EnvFilter::new("info"), Some(file));
    tracing::subscriber::with_default(subscriber, || {
        tracing::warn!("falling back to BM25-only retrieval");
        tracing::debug!("filtered out at info");
    });

    let written = fs::read_to_string(&log).unwrap();
    assert!(written.contains("WARN"), "got: {written}");
    assert!(written.contains("falling back to BM25-only retrieval"));
    assert!(!written.contains("filtered out at info"));
    assert!(!written.contains('\u{1b}'), "no ANSI escapes in the file");
}
