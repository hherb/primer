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
pub async fn download_one<R: tauri::Runtime>(
    app: &AppHandle<R>,
    asset: &MissingAsset,
) -> Result<(), String> {
    let url = asset
        .suggested_url
        .as_ref()
        .ok_or_else(|| format!("no download URL is known for asset kind {:?}", asset.kind))?;
    let dest = &asset.path;
    // `with_extension` replaces the trailing extension; a path like
    // `foo.onnx.json` becomes `foo.onnx.partial`, which is unique
    // enough for our two-file-per-locale set. (No collision risk for
    // distinct asset_kinds because they live in distinct subdirs.)
    let partial = dest.with_extension("partial");

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

    // Atomic rename so a killed download leaves no half-files.
    tokio::fs::rename(&partial, dest).await.map_err(|e| {
        format!(
            "rename {} -> {}: {e}",
            partial.display(),
            dest.display()
        )
    })?;
    Ok(())
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
