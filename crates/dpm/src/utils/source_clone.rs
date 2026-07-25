use crate::{ClientError, ClientResult};
use dpm_core::CoreError;
use std::path::{Path, PathBuf};

/// 把 `repo_url` 淺層(depth=1)clone 進 `clone_into`,回傳 clone 出來的樹裡
/// `packages/<package_name>/` 的絕對路徑。不做真的 sparse-checkout——整個
/// repo 的內容都會被抓下來,只是抓的是最新一次 commit,沒有歷史。
pub fn clone_package_source(
    repo_url: &str,
    package_name: &str,
    clone_into: &Path,
) -> ClientResult<PathBuf> {
    let mut fetch_opts = git2::FetchOptions::new();
    fetch_opts.depth(1);
    let mut builder = git2::build::RepoBuilder::new();
    builder.fetch_options(fetch_opts);
    builder
        .clone(repo_url, clone_into)
        .map_err(|e| ClientError::SystemError(format!("git clone of {repo_url} failed: {e}")))?;

    let package_src = clone_into.join("packages").join(package_name);
    if !package_src.is_dir() {
        return Err(ClientError::Core(CoreError::PackageNotFound(format!(
            "{package_name} (no packages/{package_name}/ directory in {repo_url})"
        ))));
    }
    Ok(package_src)
}

#[cfg(test)]
mod tests {
    use super::clone_package_source;
    use git2::{Repository, Signature};
    use std::fs;
    use std::path::Path;
    use tempfile::tempdir;

    /// 建一個本機 git repo,裡面有一個 `packages/<name>/` 目錄跟一個檔案,
    /// commit 好回傳 repo 的路徑——用來當 `clone_package_source` 的來源,
    /// 完全不需要對外網路連線。
    fn make_source_repo(package_name: &str, file_contents: &str) -> tempfile::TempDir {
        let dir = tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();

        let pkg_dir = dir.path().join("packages").join(package_name);
        fs::create_dir_all(&pkg_dir).unwrap();
        fs::write(pkg_dir.join("packageInfo.json"), file_contents).unwrap();

        let mut index = repo.index().unwrap();
        index
            .add_path(
                Path::new("packages")
                    .join(package_name)
                    .join("packageInfo.json")
                    .as_path(),
            )
            .unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = Signature::now("test", "test@example.com").unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "initial commit", &tree, &[])
            .unwrap();

        dir
    }

    #[test]
    fn clones_and_finds_the_package_subdirectory() {
        let source_repo = make_source_repo("demo-pkg", r#"{"version":"1.0.0"}"#);
        let dest = tempdir().unwrap();

        let result = clone_package_source(
            source_repo.path().to_str().unwrap(),
            "demo-pkg",
            &dest.path().join("clone"),
        )
        .unwrap();

        assert!(result.ends_with("packages/demo-pkg"));
        assert!(result.join("packageInfo.json").exists());
        let contents = std::fs::read_to_string(result.join("packageInfo.json")).unwrap();
        assert_eq!(contents, r#"{"version":"1.0.0"}"#);
    }

    #[test]
    fn missing_package_subdirectory_is_an_error() {
        let source_repo = make_source_repo("other-pkg", "{}");
        let dest = tempdir().unwrap();

        let result = clone_package_source(
            source_repo.path().to_str().unwrap(),
            "demo-pkg",
            &dest.path().join("clone"),
        );

        assert!(
            result.is_err(),
            "demo-pkg was never added to the source repo"
        );
    }
}
