//! Thermal sampling helpers for the QNN benchmark harness.
//!
//! On the target device (RedMagic 11 Pro / Snapdragon 8 Elite Gen 5) the
//! kernel exposes per-zone temperatures under
//! `/sys/class/thermal/thermal_zone*/temp` as an integer count of
//! **millidegrees Celsius** (e.g. `48000` ⇒ 48.0 °C). The benchmark
//! example reads those files on a timer; everything that turns the raw
//! bytes into a number, a CSV row, or a peak reading lives here as a pure
//! function so it can be unit-tested on any host — the device-only part is
//! the file read in [`super`]'s example, not the maths.

/// Divisor turning a `/sys/class/thermal` millidegree reading into degrees
/// Celsius. The sysfs `temp` node reports thousandths of a degree.
pub const MILLIDEGREES_PER_DEGREE: f64 = 1000.0;

/// One temperature reading captured during a benchmark run.
///
/// `elapsed_secs` is measured from the start of the benchmark (not wall
/// clock) so the CSV is self-contained and trivially plottable without a
/// timezone. `zone` is the sysfs zone label (e.g. `"thermal_zone0"`) so a
/// multi-zone capture stays disambiguated in a single file.
#[derive(Debug, Clone, PartialEq)]
pub struct ThermalSample {
    /// Seconds since the benchmark started.
    pub elapsed_secs: f64,
    /// Sysfs thermal-zone label the reading came from.
    pub zone: String,
    /// Temperature in degrees Celsius.
    pub temp_celsius: f64,
}

/// Parse a raw `/sys/class/thermal/thermal_zoneN/temp` reading (a
/// millidegree integer, possibly with a trailing newline) into degrees
/// Celsius.
///
/// Returns `None` when the trimmed contents are not a valid integer — a
/// non-numeric or empty node is treated as "no reading" rather than a hard
/// error so a single flaky zone never aborts a 15-minute benchmark.
///
/// ```
/// # use primer_inference::qnn::bench::thermal::parse_thermal_millidegrees;
/// assert_eq!(parse_thermal_millidegrees("48000\n"), Some(48.0));
/// assert_eq!(parse_thermal_millidegrees("  garbage "), None);
/// ```
pub fn parse_thermal_millidegrees(raw: &str) -> Option<f64> {
    raw.trim()
        .parse::<i64>()
        .ok()
        .map(|millidegrees| millidegrees as f64 / MILLIDEGREES_PER_DEGREE)
}

/// CSV header emitted by [`thermal_csv`]. Kept as a constant so the
/// example and any downstream parser agree on the column order.
pub const THERMAL_CSV_HEADER: &str = "elapsed_secs,zone,temp_celsius";

/// Render a slice of [`ThermalSample`]s as CSV text (header + one row per
/// sample, newline-terminated). Pure: no I/O, so the example owns the
/// actual file write.
///
/// `elapsed_secs` and `temp_celsius` are formatted to three decimal places
/// — enough to distinguish 2-second sample cadence and sub-degree thermal
/// drift without flooding the file with float noise.
pub fn thermal_csv(samples: &[ThermalSample]) -> String {
    let mut out = String::with_capacity(THERMAL_CSV_HEADER.len() + samples.len() * 24);
    out.push_str(THERMAL_CSV_HEADER);
    out.push('\n');
    for s in samples {
        out.push_str(&format!(
            "{:.3},{},{:.3}\n",
            s.elapsed_secs, s.zone, s.temp_celsius
        ));
    }
    out
}

/// Peak (maximum) temperature across all samples, or `None` when the slice
/// is empty. NaN samples are ignored by the `partial_cmp` fold so a single
/// bad reading can't poison the peak.
pub fn peak_temp_celsius(samples: &[ThermalSample]) -> Option<f64> {
    samples
        .iter()
        .map(|s| s.temp_celsius)
        .filter(|t| !t.is_nan())
        .fold(None, |acc, t| match acc {
            Some(max) if max >= t => Some(max),
            _ => Some(t),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_millidegrees_with_trailing_newline() {
        assert_eq!(parse_thermal_millidegrees("48000\n"), Some(48.0));
    }

    #[test]
    fn parses_millidegrees_without_newline() {
        assert_eq!(parse_thermal_millidegrees("69500"), Some(69.5));
    }

    #[test]
    fn parses_with_surrounding_whitespace() {
        assert_eq!(parse_thermal_millidegrees("  52000  "), Some(52.0));
    }

    #[test]
    fn rejects_non_numeric() {
        assert_eq!(parse_thermal_millidegrees("N/A"), None);
        assert_eq!(parse_thermal_millidegrees(""), None);
        assert_eq!(parse_thermal_millidegrees("48.0"), None); // already degrees, not the sysfs shape
    }

    #[test]
    fn csv_has_header_and_one_row_per_sample() {
        let samples = vec![
            ThermalSample {
                elapsed_secs: 0.0,
                zone: "thermal_zone0".to_string(),
                temp_celsius: 40.0,
            },
            ThermalSample {
                elapsed_secs: 2.0,
                zone: "thermal_zone0".to_string(),
                temp_celsius: 41.25,
            },
        ];
        let csv = thermal_csv(&samples);
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines[0], THERMAL_CSV_HEADER);
        assert_eq!(lines[1], "0.000,thermal_zone0,40.000");
        assert_eq!(lines[2], "2.000,thermal_zone0,41.250");
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn csv_of_empty_slice_is_header_only() {
        let csv = thermal_csv(&[]);
        assert_eq!(csv, format!("{THERMAL_CSV_HEADER}\n"));
    }

    #[test]
    fn peak_is_none_for_empty() {
        assert_eq!(peak_temp_celsius(&[]), None);
    }

    #[test]
    fn peak_is_max_temperature() {
        let samples = vec![
            ThermalSample {
                elapsed_secs: 0.0,
                zone: "z0".to_string(),
                temp_celsius: 40.0,
            },
            ThermalSample {
                elapsed_secs: 2.0,
                zone: "z1".to_string(),
                temp_celsius: 68.5,
            },
            ThermalSample {
                elapsed_secs: 4.0,
                zone: "z0".to_string(),
                temp_celsius: 55.0,
            },
        ];
        assert_eq!(peak_temp_celsius(&samples), Some(68.5));
    }

    #[test]
    fn peak_ignores_nan_readings() {
        let samples = vec![
            ThermalSample {
                elapsed_secs: 0.0,
                zone: "z0".to_string(),
                temp_celsius: f64::NAN,
            },
            ThermalSample {
                elapsed_secs: 2.0,
                zone: "z0".to_string(),
                temp_celsius: 50.0,
            },
        ];
        assert_eq!(peak_temp_celsius(&samples), Some(50.0));
    }
}
