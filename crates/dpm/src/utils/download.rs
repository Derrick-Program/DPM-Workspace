use super::{ClientError, ClientResult};
use dpm_core::CoreError::*;
use futures_util::StreamExt;
use std::path::Path;
use tokio::io::AsyncWriteExt;

/// Downloads `url` to `dest_path`, streaming to disk rather than buffering
/// the whole response in memory. This used to live on `Db` — its only
/// relationship to persistence was that its one caller happened to look the
/// URL up from the database first; the download itself never touched DB
/// state, so it didn't belong behind `Db`'s interface.
pub async fn download_file(url: &str, dest_path: &Path) -> ClientResult<()> {
    let req = reqwest::get(url)
        .await
        .map_err(|e| ClientError::Core(NetworkError(e.to_string())))?;
    if !req.status().is_success() {
        return Err(ClientError::Core(NetworkError(format!(
            "Failed to download file: HTTP {}",
            req.status()
        ))));
    }
    let mut file = tokio::fs::File::create(dest_path)
        .await
        .map_err(|e| ClientError::Core(IoError(e)))?;
    let mut stream = req.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk =
            chunk.map_err(|e| ClientError::SystemError(format!("Failed to read chunk: {}", e)))?;
        file.write_all(&chunk)
            .await
            .map_err(|e| ClientError::SystemError(format!("Failed to write chunk: {}", e)))?;
    }
    file.flush()
        .await
        .map_err(|e| ClientError::SystemError(format!("Failed to flush file: {}", e)))?;
    println!("File downloaded to: {}", dest_path.display());
    Ok(())
}
