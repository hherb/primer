//! Pure helpers for rewriting a Genie dialog-config JSON before handing it
//! to `GenieDialogConfig_createFromJson`.
//!
//! QAIRT 2.45's `GenieDialogConfig_createFromJson` consumes the JSON
//! *content* (not a path), and the AI-Hub-published `genie_config.json`
//! names its per-shard context binaries, tokenizer, and HTP-extensions file
//! *relative* to the bundle directory. Handed to Genie as-is, those
//! relative paths resolve against the process working directory and fail to
//! load. These functions rewrite the three path-bearing fields to absolute
//! paths resolved against the bundle directory. No FFI, no I/O — every
//! function here is unit-testable on the host.

use std::path::Path;

/// Rewrite the relative file paths inside a Genie dialog-config JSON to
/// absolute paths resolved against `bundle_dir`, returning the rewritten
/// JSON as a string.
///
/// Mirrors chatapp_android's `LoadModelConfig`, which rewrites exactly
/// these three fields:
///
/// - `dialog.tokenizer.path` (string)
/// - `dialog.engine.backend.extensions` (string)
/// - `dialog.engine.model.binary.ctx-bins[]` (array of strings)
///
/// A path that is already absolute is left untouched (so a pre-absolutised
/// config round-trips unchanged). `ctx-bins` is required — a config missing
/// it is a broken bundle and yields an `Err`. The optional `tokenizer.path`
/// and `extensions` fields are rewritten only when present, so a config
/// that inlines them (or omits them) is tolerated.
///
/// Pure function: no FFI, no I/O. Errors are returned as human-readable
/// strings for the caller to wrap in
/// [`super::GenieCallError::BadConfigPath`].
pub(super) fn absolutize_genie_config(raw_json: &str, bundle_dir: &Path) -> Result<String, String> {
    use serde_json::Value;

    let mut config: Value = serde_json::from_str(raw_json)
        .map_err(|e| format!("genie config is not valid JSON: {e}"))?;

    let dialog = config
        .get_mut("dialog")
        .ok_or_else(|| "genie config has no `dialog` object".to_string())?;

    // tokenizer.path — optional, rewrite if present.
    if let Some(path) = dialog
        .get_mut("tokenizer")
        .and_then(|t| t.get_mut("path"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_owned())
    {
        let abs = absolutize_one(&path, bundle_dir);
        dialog["tokenizer"]["path"] = Value::String(abs);
    }

    // engine.backend.extensions — optional, rewrite if present.
    if let Some(ext) = dialog
        .get_mut("engine")
        .and_then(|e| e.get_mut("backend"))
        .and_then(|b| b.get_mut("extensions"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_owned())
    {
        let abs = absolutize_one(&ext, bundle_dir);
        dialog["engine"]["backend"]["extensions"] = Value::String(abs);
    }

    // engine.model.binary.ctx-bins[] — REQUIRED; the model can't load
    // without its context binaries.
    let ctx_bins = dialog
        .get_mut("engine")
        .and_then(|e| e.get_mut("model"))
        .and_then(|m| m.get_mut("binary"))
        .and_then(|b| b.get_mut("ctx-bins"))
        .and_then(Value::as_array_mut)
        .ok_or_else(|| {
            "genie config has no `dialog.engine.model.binary.ctx-bins` array".to_string()
        })?;
    if ctx_bins.is_empty() {
        return Err("genie config `ctx-bins` array is empty".to_string());
    }
    for bin in ctx_bins.iter_mut() {
        let rel = bin
            .as_str()
            .ok_or_else(|| "a `ctx-bins` entry is not a string".to_string())?;
        *bin = Value::String(absolutize_one(rel, bundle_dir));
    }

    Ok(config.to_string())
}

/// Resolve a single config path against `bundle_dir`: absolute paths pass
/// through unchanged; relative paths are joined onto `bundle_dir`. The
/// result is rendered with `Path::display`, which is lossless for the
/// UTF-8 paths a genie config carries.
fn absolutize_one(path: &str, bundle_dir: &Path) -> String {
    let p = Path::new(path);
    if p.is_absolute() {
        path.to_owned()
    } else {
        bundle_dir.join(p).display().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn sample_config() -> String {
        json!({
            "dialog": {
                "tokenizer": { "path": "tokenizer.json" },
                "engine": {
                    "backend": { "type": "QnnHtp", "extensions": "htp_backend_ext_config.json" },
                    "model": {
                        "binary": {
                            "ctx-bins": [
                                "model_part_1_of_2.bin",
                                "model_part_2_of_2.bin"
                            ]
                        }
                    }
                }
            }
        })
        .to_string()
    }

    #[test]
    fn absolutizes_relative_paths_against_bundle_dir() {
        let out =
            absolutize_genie_config(&sample_config(), Path::new("/data/local/tmp/bundle")).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            v["dialog"]["tokenizer"]["path"],
            json!("/data/local/tmp/bundle/tokenizer.json")
        );
        assert_eq!(
            v["dialog"]["engine"]["backend"]["extensions"],
            json!("/data/local/tmp/bundle/htp_backend_ext_config.json")
        );
        assert_eq!(
            v["dialog"]["engine"]["model"]["binary"]["ctx-bins"],
            json!([
                "/data/local/tmp/bundle/model_part_1_of_2.bin",
                "/data/local/tmp/bundle/model_part_2_of_2.bin"
            ])
        );
    }

    #[test]
    fn leaves_absolute_paths_unchanged() {
        let cfg = json!({
            "dialog": {
                "tokenizer": { "path": "/abs/tokenizer.json" },
                "engine": {
                    "backend": { "extensions": "/abs/htp.json" },
                    "model": { "binary": { "ctx-bins": ["/abs/part1.bin"] } }
                }
            }
        })
        .to_string();
        let out = absolutize_genie_config(&cfg, Path::new("/data/local/tmp/bundle")).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            v["dialog"]["tokenizer"]["path"],
            json!("/abs/tokenizer.json")
        );
        assert_eq!(
            v["dialog"]["engine"]["backend"]["extensions"],
            json!("/abs/htp.json")
        );
        assert_eq!(
            v["dialog"]["engine"]["model"]["binary"]["ctx-bins"],
            json!(["/abs/part1.bin"])
        );
    }

    #[test]
    fn preserves_other_config_fields() {
        // Non-path fields (context size, sampler, etc.) must survive the
        // rewrite untouched — the rewrite only changes the three known
        // path-bearing fields.
        let cfg = json!({
            "dialog": {
                "context": { "size": 4096, "n-vocab": 151936 },
                "tokenizer": { "path": "tokenizer.json" },
                "engine": {
                    "n-threads": 3,
                    "backend": { "type": "QnnHtp", "extensions": "htp.json", "poll": true },
                    "model": { "binary": { "ctx-bins": ["a.bin"] } }
                }
            }
        })
        .to_string();
        let out = absolutize_genie_config(&cfg, Path::new("/b")).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["dialog"]["context"]["size"], json!(4096));
        assert_eq!(v["dialog"]["context"]["n-vocab"], json!(151936));
        assert_eq!(v["dialog"]["engine"]["n-threads"], json!(3));
        assert_eq!(v["dialog"]["engine"]["backend"]["poll"], json!(true));
        assert_eq!(v["dialog"]["engine"]["backend"]["type"], json!("QnnHtp"));
    }

    #[test]
    fn tolerates_missing_optional_fields() {
        // A config with ctx-bins but no tokenizer/extensions is still
        // rewritten successfully (only the required ctx-bins matter).
        let cfg = json!({
            "dialog": {
                "engine": { "model": { "binary": { "ctx-bins": ["a.bin"] } } }
            }
        })
        .to_string();
        let out = absolutize_genie_config(&cfg, Path::new("/b")).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            v["dialog"]["engine"]["model"]["binary"]["ctx-bins"],
            json!(["/b/a.bin"])
        );
    }

    #[test]
    fn errors_on_invalid_json() {
        let err = absolutize_genie_config("{not json", Path::new("/b")).unwrap_err();
        assert!(err.contains("not valid JSON"), "got: {err}");
    }

    #[test]
    fn errors_on_missing_ctx_bins() {
        let cfg = json!({ "dialog": { "tokenizer": { "path": "t.json" } } }).to_string();
        let err = absolutize_genie_config(&cfg, Path::new("/b")).unwrap_err();
        assert!(err.contains("ctx-bins"), "got: {err}");
    }

    #[test]
    fn errors_on_empty_ctx_bins() {
        let cfg = json!({ "dialog": { "engine": { "model": { "binary": { "ctx-bins": [] } } } } })
            .to_string();
        let err = absolutize_genie_config(&cfg, Path::new("/b")).unwrap_err();
        assert!(err.contains("empty"), "got: {err}");
    }
}
