//! Streaming voice-asset download.
//!
//! Each file is fetched via `reqwest::get` and streamed to a
//! `<dest>.partial` temp path, then atomically renamed on success.
//! Progress events fire per chunk via the AppHandle so the consent
//! modal can render a progress bar. A killed download leaves only the
//! `.partial` file behind, so the resolver's existence check is never
//! fooled by a half-written file.

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::commands::voice::MissingAsset;

#[derive(Serialize, Clone)]
pub struct DownloadProgressEvent {
    pub asset_id: String,
    pub bytes_done: u64,
    pub bytes_total: Option<u64>,
}

/// Download one [`MissingAsset`] to its target path. Streams the body
/// through `<dest>.partial`, emits a `primer://voice/download_progress`
/// event after each chunk, then atomically renames into place.
///
/// On any error after the partial file has been opened, the partial is
/// removed before returning so a retry starts from a clean slate. A
/// process kill (SIGKILL, OOM, power loss) cannot run the cleanup —
/// the rename-into-place pattern protects against *that* failure mode
/// by ensuring the destination path only ever holds a fully-written
/// file. The explicit cleanup here is for the *graceful*-error path
/// (network error mid-stream, write fault) so the next click doesn't
/// see a stale `.partial` from a previous attempt.
pub async fn download_one<R: tauri::Runtime>(
    app: &AppHandle<R>,
    asset: &MissingAsset,
) -> Result<(), String> {
    let url = asset
        .suggested_url
        .as_ref()
        .ok_or_else(|| format!("no download URL is known for asset kind {:?}", asset.kind))?;
    let dest = &asset.path;
    // Build the partial path by *appending* `.partial` to the dest, not
    // by replacing the trailing extension. `Path::with_extension`
    // chops the last extension, so `foo.onnx.json` would become
    // `foo.onnx.partial` — confusing, and a latent bug for any future
    // multi-extension asset (e.g. `.tar.gz`). Appending preserves the
    // full asset name in the partial filename so concurrent downloads
    // of `foo.onnx` and `foo.onnx.json` cannot collide regardless of
    // their extensions.
    let partial = {
        let mut p = dest.clone().into_os_string();
        p.push(".partial");
        std::path::PathBuf::from(p)
    };

    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }

    let client = reqwest::Client::new();
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("GET {url}: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!(
            "download URL returned {status} for {url}; the model may have been renamed upstream — pick a model manually in Settings → Speech",
        ));
    }
    let total = resp.content_length();

    use futures::StreamExt;
    use tokio::io::AsyncWriteExt;
    let result: Result<(), String> = async {
        let mut file = tokio::fs::File::create(&partial)
            .await
            .map_err(|e| format!("create {}: {e}", partial.display()))?;
        let mut bytes_done: u64 = 0;
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| format!("read chunk: {e}"))?;
            file.write_all(&chunk)
                .await
                .map_err(|e| format!("write: {e}"))?;
            bytes_done += chunk.len() as u64;
            let evt = DownloadProgressEvent {
                asset_id: asset.kind.clone(),
                bytes_done,
                bytes_total: total,
            };
            let _ = app.emit("primer://voice/download_progress", &evt);
        }
        file.flush()
            .await
            .map_err(|e| format!("flush: {e}"))?;
        drop(file);

        // Atomic rename so a killed download leaves no half-files at
        // the destination path. (The `.partial` file is cleaned up by
        // the surrounding error path on failure.)
        tokio::fs::rename(&partial, dest).await.map_err(|e| {
            format!("rename {} -> {}: {e}", partial.display(), dest.display())
        })?;
        Ok(())
    }
    .await;

    if let Err(ref msg) = result {
        // Clean up the partial file on the graceful-error path so the
        // next attempt starts from a clean slate. A failure to remove
        // it is logged but doesn't override the original error — the
        // user needs to see why the *download* failed, not why a stale
        // partial couldn't be deleted.
        if let Err(rm_err) = tokio::fs::remove_file(&partial).await {
            // Suppress NotFound: the partial may never have been created
            // (e.g. mkdir succeeded but `File::create` failed) or a
            // successful rename already moved it.
            if rm_err.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(
                    "download failed ({msg}); also failed to clean up {}: {rm_err}",
                    partial.display(),
                );
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_progress_event_serialises_with_snake_case_fields() {
        let evt = DownloadProgressEvent {
            asset_id: "whisper_model".into(),
            bytes_done: 12_345_678,
            bytes_total: Some(490_000_000),
        };
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["asset_id"], "whisper_model");
        assert_eq!(json["bytes_done"], 12_345_678);
        assert_eq!(json["bytes_total"], 490_000_000);
    }

    #[test]
    fn download_progress_event_omits_unknown_total_as_null() {
        let evt = DownloadProgressEvent {
            asset_id: "piper_onnx".into(),
            bytes_done: 1,
            bytes_total: None,
        };
        let json = serde_json::to_value(&evt).unwrap();
        assert!(
            json["bytes_total"].is_null(),
            "missing content-length must serialise as null, not be skipped: {json}"
        );
    }
}
