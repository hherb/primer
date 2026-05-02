//! Read sample-rate from a Piper voice config JSON file.
//!
//! Piper voice configs declare their own `audio.sample_rate` (typically
//! 16_000, 22_050, or 24_000 depending on the voice). This helper is
//! a pure function over a JSON path so it can be unit-tested without
//! constructing a real `piper_rs::Piper`.

use std::fs;
use std::path::Path;

use primer_core::error::{PrimerError, Result};

/// JSON path into a Piper voice config: `audio.sample_rate`. Used in
/// error messages so a misconfigured voice file is easy to diagnose.
const SAMPLE_RATE_KEY: &str = "audio.sample_rate";

/// Read `audio.sample_rate` from the voice config JSON at `path`.
///
/// Errors with [`PrimerError::Speech`] if the file is missing,
/// unparseable, or doesn't contain a non-zero integer at
/// `audio.sample_rate`.
pub fn read_sample_rate(path: impl AsRef<Path>) -> Result<u32> {
    let path = path.as_ref();
    let raw = fs::read_to_string(path)
        .map_err(|e| PrimerError::Speech(format!("read piper config {path:?}: {e}")))?;
    let json: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| PrimerError::Speech(format!("parse piper config {path:?}: {e}")))?;
    let rate = json
        .get("audio")
        .and_then(|a| a.get("sample_rate"))
        .and_then(|v| v.as_u64())
        .ok_or_else(|| {
            PrimerError::Speech(format!(
                "piper config {path:?} missing {SAMPLE_RATE_KEY} or wrong type"
            ))
        })?;
    if rate == 0 || rate > u32::MAX as u64 {
        return Err(PrimerError::Speech(format!(
            "piper config {path:?} has implausible sample_rate {rate}"
        )));
    }
    Ok(rate as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_temp_json(content: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().expect("temp file");
        f.write_all(content.as_bytes()).expect("write");
        f
    }

    #[test]
    fn reads_sample_rate_from_minimal_config() {
        let f = write_temp_json(r#"{"audio":{"sample_rate":22050}}"#);
        assert_eq!(read_sample_rate(f.path()).unwrap(), 22_050);
    }

    #[test]
    fn errors_when_audio_section_missing() {
        let f = write_temp_json(r#"{"phonemes":{}}"#);
        let err = read_sample_rate(f.path()).unwrap_err();
        assert!(format!("{err}").contains("audio.sample_rate"));
    }

    #[test]
    fn errors_when_sample_rate_zero() {
        let f = write_temp_json(r#"{"audio":{"sample_rate":0}}"#);
        let err = read_sample_rate(f.path()).unwrap_err();
        assert!(format!("{err}").contains("implausible"));
    }

    #[test]
    fn errors_when_file_missing() {
        let err = read_sample_rate("/tmp/__primer_no_such_file__.json").unwrap_err();
        assert!(format!("{err}").contains("read piper config"));
    }
}
