use crate::{download_file, read_file_from_zip, ClientError, ClientResult, DbPackage, Hashes};
use dpm_core::{CoreError, JsonStorage, PackageInfo};
use std::path::Path;

/// Downloads a `kind: "prebuilt"` package's archive to `download_path` and
/// verifies it two ways: the archive's own blake3 hash against what the
/// local index recorded for it, and the `packageInfo.json` inside it
/// against `hashes.json`'s own record of that same file (both computed by
/// `dpm-server` at publish time). Returns the parsed `PackageInfo` — the
/// caller still needs its `file_name` to build the entry path, and unzips
/// `download_path` separately since placement needs the extracted
/// directory, not the archive itself.
///
/// This is the "Fetcher" half of `install()`'s previously-fused six
/// concerns (spec-parse/resolve, staging, download, verify, place, link):
/// paired with `place_package` (in `placer.rs`), together they replace one
/// ~140-line method with two independently-testable pieces.
pub async fn fetch_and_verify_prebuilt(
    pkg: &str,
    repo_package_info: &DbPackage,
    download_path: &Path,
) -> ClientResult<PackageInfo> {
    let url = repo_package_info
        .url
        .as_deref()
        .ok_or_else(|| ClientError::Core(CoreError::InvalidPackage(format!("{pkg} has no url"))))?;
    download_file(url, download_path).await?;

    let package_info_str = read_file_from_zip(download_path, "packageInfo.json")
        .map_err(|e| ClientError::Core(CoreError::IoError(e)))?;
    let package_info: PackageInfo = JsonStorage::from_str_to(&package_info_str)?;
    let hashes_str = read_file_from_zip(download_path, "hashes.json")
        .map_err(|e| ClientError::Core(CoreError::IoError(e)))?;
    let package_hash_info: Hashes = JsonStorage::from_str_to(&hashes_str)?;

    let hash = dpm_core::hash_file(download_path)?;
    let expected_hash = repo_package_info.hash.clone().ok_or_else(|| {
        ClientError::Core(CoreError::InvalidPackage(format!(
            "{pkg} has no hash recorded"
        )))
    })?;
    if expected_hash != hash {
        return Err(ClientError::Core(CoreError::HashMismatch {
            expected: expected_hash,
            actual: hash,
        }));
    }

    let recorded_package_info_hash = package_hash_info.get("hashes.json").ok_or_else(|| {
        ClientError::Core(CoreError::InvalidPackage(format!(
            "{pkg}'s hashes.json has no entry for itself"
        )))
    })?;
    if &package_info.hash != recorded_package_info_hash {
        return Err(ClientError::Core(CoreError::HashMismatch {
            expected: package_info.hash.clone(),
            actual: recorded_package_info_hash.clone(),
        }));
    }

    Ok(package_info)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dpm_core::PackageInfo;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// Serves `body` as the entire HTTP response body for the next
    /// connection accepted, once, on a background thread. A raw TCP
    /// responder rather than a mock-server crate: one fixed round trip
    /// doesn't need a new dependency.
    fn serve_once(body: Vec<u8>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf); // drain the request, ignore it
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        format!("http://127.0.0.1:{port}/pkg.zip")
    }

    /// Builds a real zip containing `packageInfo.json` + `hashes.json`,
    /// mirroring exactly what `dpm-server hash`/`build` produce at publish
    /// time (`packageInfo.json.hash` == blake3 of `hashes.json`). Returns
    /// the zip's bytes and its own blake3 hash (what a real index would
    /// record as `DbPackage.hash`).
    fn build_fixture_zip(dir: &std::path::Path) -> (Vec<u8>, String) {
        use std::collections::HashMap;

        let src = dir.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("main"), b"#!/bin/sh\necho hi\n").unwrap();
        let main_hash = dpm_core::hash_file(&src.join("main")).unwrap();

        // Mirrors dpm-server's real `hash` action exactly: write hashes.json
        // with every *other* file's hash first, hash that file itself, then
        // rewrite hashes.json a second time with a "hashes.json" -> self-hash
        // entry added. `packageInfo.json.hash` records the *first-pass*
        // hash (before the self-entry existed) — the same value
        // fetch_and_verify_prebuilt checks package_hash_info["hashes.json"]
        // against.
        let hashes_path = src.join("hashes.json");
        let mut hashes: HashMap<String, String> = HashMap::new();
        hashes.insert("main".to_string(), main_hash);
        dpm_core::JsonStorage::to_json(&hashes, &hashes_path).unwrap();
        let hashes_json_hash = dpm_core::hash_file(&hashes_path).unwrap();
        hashes.insert("hashes.json".to_string(), hashes_json_hash.clone());
        dpm_core::JsonStorage::to_json(&hashes, &hashes_path).unwrap();

        let package_info = PackageInfo::new(
            "fixture".to_string(),
            "main".to_string(),
            "1.0.0".to_string(),
            "test fixture".to_string(),
            hashes_json_hash,
            None,
        );
        dpm_core::JsonStorage::to_json(&package_info, &src.join("packageInfo.json")).unwrap();

        let zip_path = dir.join("pkg.zip");
        crate::zip_folder(&src, &zip_path).unwrap();
        let zip_hash = dpm_core::hash_file(&zip_path).unwrap();
        let bytes = std::fs::read(&zip_path).unwrap();
        (bytes, zip_hash)
    }

    fn fixture_db_row(url: String, hash: String) -> DbPackage {
        DbPackage::new(
            "official",
            "fixture",
            "1.0.0",
            "prebuilt",
            Some(url),
            Some(hash),
            Some("pkg.zip".to_string()),
            None,
            "test fixture",
            Some("main".to_string()),
            None,
        )
    }

    #[tokio::test]
    async fn fetches_and_verifies_a_real_matching_archive() {
        let dir = tempfile::tempdir().unwrap();
        let (zip_bytes, zip_hash) = build_fixture_zip(dir.path());
        let url = serve_once(zip_bytes);
        let repo_package_info = fixture_db_row(url, zip_hash);
        let download_path = dir.path().join("downloaded.zip");

        let package_info = fetch_and_verify_prebuilt("fixture", &repo_package_info, &download_path)
            .await
            .unwrap();

        assert_eq!(package_info.file_name, "main");
        assert_eq!(package_info.version, "1.0.0");
        assert!(
            download_path.exists(),
            "the archive must be left on disk for the caller to unzip"
        );
    }

    #[tokio::test]
    async fn rejects_an_archive_that_does_not_match_the_recorded_hash() {
        let dir = tempfile::tempdir().unwrap();
        let (zip_bytes, _correct_hash) = build_fixture_zip(dir.path());
        let url = serve_once(zip_bytes);
        // A hash that doesn't match what was actually served — simulates a
        // tampered-in-transit or wrong-index-entry archive.
        let repo_package_info = fixture_db_row(url, "0".repeat(64));
        let download_path = dir.path().join("downloaded.zip");

        let err = fetch_and_verify_prebuilt("fixture", &repo_package_info, &download_path)
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            ClientError::Core(CoreError::HashMismatch { .. })
        ));
    }
}
