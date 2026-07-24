use std::{
    fs::{self, remove_dir_all, remove_file, Permissions},
    os::unix::fs::PermissionsExt,
    path::Path,
};

use crate::{
    get_db, read_file_from_zip, system::*, unzip_file, ClientError, ClientResult, DbPackage,
    Hashes, Setting, BIN_DIR, INSTALL_DIR, MAIN_DIR,
};
use colored::Colorize;
use dpm_core::CoreError;
use dpm_core::{Dependency, JsonStorage, PackageInfo, RepoInfo};
use walkdir::WalkDir;
#[derive(Debug)]
pub struct ActionInfo {
    pub pkgs: Vec<String>,
    pub verbose: bool,
    pub setting_config: Setting,
    pub system_controller: SystemController,
    pub system_action: SystemAction,
}
impl ActionInfo {
    pub fn new(pkgs: Vec<String>, verbose: bool, setting_config: Setting) -> ActionInfo {
        ActionInfo {
            pkgs,
            verbose,
            setting_config,
            system_action: SystemAction::new(verbose),
            system_controller: SystemController,
        }
    }
    async fn parse_mine(&self) -> (Vec<String>, Vec<String>) {
        let mut is: Vec<String> = Vec::new();
        let mut isnot: Vec<String> = Vec::new();
        let all_packages = get_db().read_all().await.unwrap_or_else(|_| Vec::new());
        let package_names: Vec<String> = all_packages.into_iter().map(|pkg| pkg.name).collect();
        for pkg in &self.pkgs {
            if package_names.contains(pkg) {
                is.push(pkg.clone());
            } else {
                isnot.push(pkg.clone());
            }
        }
        (is, isnot)
    }
    pub async fn install(&self) -> ClientResult<()> {
        let (is, isnot) = self.parse_mine().await;
        if !is.is_empty() {
            for pkg in is {
                let pkg = pkg.as_str();
                let repo_package_info = get_db()
                    .read_one(pkg)
                    .await
                    .map_err(|e| ClientError::Core(CoreError::DatabaseError(e.to_string())))?
                    .ok_or_else(|| {
                        ClientError::Core(CoreError::PackageNotFound(pkg.to_string()))
                    })?;
                if self.verbose {
                    println!("{}\n\n  {}", pkg.on_green(), "Downloading...".yellow());
                }

                let staging_root_base = MAIN_DIR.get().unwrap().join(".staging");
                std::fs::create_dir_all(&staging_root_base)
                    .map_err(|e| ClientError::Core(CoreError::IoError(e)))?;
                let staging = tempfile::Builder::new()
                    .prefix(pkg)
                    .tempdir_in(&staging_root_base)
                    .map_err(|e| ClientError::Core(CoreError::IoError(e)))?;

                let download_path = staging.path().join(&repo_package_info.filename);
                get_db()
                    .download_file(pkg, &download_path)
                    .await
                    .map_err(|e| ClientError::Core(CoreError::NetworkError(e.to_string())))?;
                if self.verbose {
                    println!("  {}", "Download successed!".green());
                }
                let package_info_test: String =
                    read_file_from_zip(&download_path, "packageInfo.json").unwrap();
                let package_info: PackageInfo =
                    JsonStorage::from_str_to(package_info_test.as_str()).unwrap();
                let package_hash_info: Hashes = JsonStorage::from_str_to(
                    read_file_from_zip(&download_path, "hashes.json")
                        .unwrap()
                        .as_str(),
                )
                .unwrap();
                if self.verbose {
                    println!(
                        "  {}",
                        "Checking Package Hash ...(May take a while)".yellow()
                    );
                }
                let hash = dpm_core::hash_file(&download_path)?;
                if repo_package_info.hash != hash {
                    return Err(ClientError::Core(CoreError::HashMismatch {
                        expected: repo_package_info.hash,
                        actual: hash,
                    }));
                }
                if &package_info.hash != package_hash_info.get("hashes.json").unwrap() {
                    return Err(ClientError::Core(CoreError::HashMismatch {
                        expected: package_info.hash.clone(),
                        actual: package_hash_info.get("hashes.json").unwrap().clone(),
                    }));
                }

                if self.verbose {
                    println!("  {}", "Hashes Passed".green());
                    println!("  {}", "Installing ...".yellow());
                }

                let extracted = staging.path().join("extracted");
                unzip_file(&download_path, &extracted)
                    .map_err(|e| ClientError::Core(CoreError::IoError(e)))?;

                let install_path = INSTALL_DIR.get().unwrap().join(pkg);
                swap_into_install_dir(&extracted, &install_path, staging.path())?;
                if self.verbose {
                    println!("  {}", "Installed!".green());
                    println!("  {}", "Create Links ...".yellow());
                }
                let main_file = install_path.join(&package_info.file_name);
                let ln_path = BIN_DIR.get().unwrap().join(pkg);
                fs::set_permissions(&main_file, Permissions::from_mode(0o755))
                    .map_err(|e| ClientError::SystemError(e.to_string()))?;
                self.system_controller.system_command_runner(
                    "ln",
                    vec![
                        "-s",
                        main_file.display().to_string().as_str(),
                        ln_path.display().to_string().as_str(),
                    ],
                    "Can't create link",
                )?;
                if self.verbose {
                    println!("  {}", "Successed Create Link!".green());
                }
                // `staging` (tempfile::TempDir) drop 在這裡發生,連同任何被搬到
                // staging_root/previous 的舊版本一起清掉。
            }
        }
        if !isnot.is_empty() {
            for pkg in isnot {
                self.system_action.install_package(&pkg)?;
            }
        }
        Ok(())
    }

    pub async fn update(&self) -> ClientResult<()> {
        println!("{} Updating...", "==>".blue());
        let mut remote_repo = RepoInfo::new();

        let repo_info_url = self.setting_config.get("repo_info").ok_or_else(|| {
            ClientError::ConfigError("Missing 'repo_info' in settings".to_string())
        })?;

        // 獲取更新的遠程資料
        remote_repo.fetch_update_repo_info(repo_info_url).await?;
        let db = get_db();

        db.clear_table("LocalRepo").await?;

        let repo_handler = remote_repo.get_package_handler();

        for (name, repo_info) in repo_handler {
            let dependencies1: Option<Vec<dpm_core::Dependency>> =
                repo_info.dependencies.as_ref().map(|deps| {
                    deps.iter()
                        .map(|dep| Dependency::new(&dep.name, &dep.version))
                        .collect::<Vec<_>>()
                });
            let package_info = remote_repo.get_single_package_info(name).await?;
            println!("{} Updating...", name.green());
            get_db()
                .insert(DbPackage::new(
                    name,
                    repo_info.version.as_str(),
                    repo_info.url.as_str(),
                    package_info.description.as_str(),
                    repo_info.file_name.as_str(),
                    repo_info.hash.as_str(),
                    package_info.file_name.as_str(),
                    dependencies1,
                ))
                .await?;
        }
        // update_package_index(self.verbose);
        println!("{} Updated!", "==>".green());
        Ok(())
    }
    pub async fn init_update(url_json: &str) -> ClientResult<()> {
        let mut remote_repo = RepoInfo::new();
        remote_repo.fetch_update_repo_info(url_json).await?;
        for (name, repo_info) in remote_repo.get_package_handler() {
            let dependencies1: Option<Vec<dpm_core::Dependency>> =
                repo_info.dependencies.as_ref().map(|deps| {
                    deps.iter()
                        .map(|dep| Dependency::new(&dep.name, &dep.version))
                        .collect::<Vec<_>>()
                });
            get_db()
                .insert(DbPackage::new(
                    name,
                    repo_info.version.as_str(),
                    repo_info.url.as_str(),
                    repo_info
                        .description
                        .as_ref()
                        .unwrap_or(&String::new())
                        .as_str(),
                    repo_info.file_name.as_str(),
                    repo_info.hash.as_str(),
                    repo_info.entry.as_ref().unwrap_or(&String::new()).as_str(),
                    dependencies1,
                ))
                .await?;
        }
        Ok(())
    }

    pub async fn uninstall(&self) -> ClientResult<()> {
        let (is, isnot) = self.parse_mine().await;
        if !is.is_empty() {
            for pkg in is {
                let pre_rm_location = INSTALL_DIR.get().unwrap().join(&pkg);
                let pre_rm_ln = BIN_DIR.get().unwrap().join(&pkg);
                if self.verbose {
                    println!("{}\n\n  {}", pkg.on_green(), "Removing...".red());
                }
                remove_dir_all(pre_rm_location)
                    .map_err(|e| ClientError::Core(CoreError::IoError(e)))?;
                if self.verbose {
                    println!("  {}", "Removed!".green());
                    println!("  {}", "UnLinking...".red());
                }
                remove_file(pre_rm_ln).map_err(|e| ClientError::Core(CoreError::IoError(e)))?;
                if self.verbose {
                    println!("  {}", "Done".green());
                }
            }
        }
        if !isnot.is_empty() {
            for pkg in isnot {
                self.system_action.uninstall_package(&pkg)?;
            }
        }
        Ok(())
    }

    pub async fn search(&self) -> ClientResult<()> {
        let (is, isnot) = self.parse_mine().await;
        if !is.is_empty() {
            println!();
            for pkg in is {
                println!("{} {}", pkg, "Found!!".green());
            }
        }
        if !isnot.is_empty() {
            for pkg in &self.pkgs {
                self.system_action.search_package(pkg.as_str())?;
            }
        }
        Ok(())
    }

    pub async fn list(&self, sys: bool) -> ClientResult<()> {
        if sys {
            self.system_action.list_packages()?;
        } else {
            let path = INSTALL_DIR.get().unwrap();
            for entry in WalkDir::new(path) {
                let entry = entry.map_err(|e| ClientError::Core(CoreError::IoError(e.into())))?;
                let _path = entry.path();
            }
        }
        Ok(())
    }

    pub async fn upgrade(&self) -> ClientResult<()> {
        let (is, isnot) = self.parse_mine().await;
        if !is.is_empty() {
            for pkg in is {
                println!("{:#?}", pkg);
            }
        }
        if !isnot.is_empty() {
            for pkg in isnot {
                self.system_action.upgrade_package(&pkg)?;
            }
        }
        Ok(())
    }

    pub fn upgrade_self(&self) {
        println!("{} Upgrading self", "==>".blue());
    }
}

/// 把 staging 目錄裡已經驗證好的內容原子性換裝進最終安裝路徑。
/// 若 install_path 已存在(升級情境),先把舊的搬進 staging_root/previous
/// (同檔案系統 rename,不是複製),新內容才搬進最終路徑——任何一步失敗,
/// install_path 都維持在「舊版本完整存在」或「還沒開始換裝」其中一種完好
/// 狀態,不會出現半殘目錄。呼叫端的 staging TempDir drop 時會把搬出來的
/// 舊版本一併清掉。
fn swap_into_install_dir(
    new_dir: &Path,
    install_path: &Path,
    staging_root: &Path,
) -> ClientResult<()> {
    if install_path.exists() {
        let backup = staging_root.join("previous");
        std::fs::rename(install_path, &backup)
            .map_err(|e| ClientError::Core(CoreError::IoError(e)))?;
        if let Err(e) = std::fs::rename(new_dir, install_path) {
            // Roll back: put the old version back so install_path is never
            // left missing/half-installed if the final rename fails. If the
            // rollback itself fails (double fault), the old install may be
            // gone for good once the caller's staging TempDir is dropped, so
            // this must never fail silently.
            if let Err(rollback_err) = std::fs::rename(&backup, install_path) {
                eprintln!(
                    "CRITICAL: failed to restore backup after failed install \
                     (install error: {e}; rollback error: {rollback_err}); \
                     the previous install at {} may be lost",
                    install_path.display()
                );
            }
            return Err(ClientError::Core(CoreError::IoError(e)));
        }
    } else {
        std::fs::rename(new_dir, install_path)
            .map_err(|e| ClientError::Core(CoreError::IoError(e)))?;
    }
    Ok(())
}

#[cfg(test)]
mod atomic_install_tests {
    use super::swap_into_install_dir;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn fresh_install_moves_new_dir_into_place() {
        let root = tempdir().unwrap();
        let new_dir = root.path().join("new");
        let install_path = root.path().join("install");
        fs::create_dir_all(&new_dir).unwrap();
        fs::write(new_dir.join("marker.txt"), b"v2").unwrap();

        swap_into_install_dir(&new_dir, &install_path, root.path()).unwrap();

        assert!(install_path.join("marker.txt").exists());
        assert_eq!(
            fs::read_to_string(install_path.join("marker.txt")).unwrap(),
            "v2"
        );
        assert!(
            !new_dir.exists(),
            "new_dir should have been moved, not copied"
        );
    }

    #[test]
    fn upgrade_replaces_old_content_with_new() {
        let root = tempdir().unwrap();
        let new_dir = root.path().join("new");
        let install_path = root.path().join("install");
        fs::create_dir_all(&new_dir).unwrap();
        fs::write(new_dir.join("marker.txt"), b"v2").unwrap();
        fs::create_dir_all(&install_path).unwrap();
        fs::write(install_path.join("marker.txt"), b"v1").unwrap();

        swap_into_install_dir(&new_dir, &install_path, root.path()).unwrap();

        assert_eq!(
            fs::read_to_string(install_path.join("marker.txt")).unwrap(),
            "v2",
            "install_path must contain the new version's content after swap"
        );
    }

    #[test]
    fn old_install_survives_if_new_dir_is_missing() {
        let root = tempdir().unwrap();
        let missing_new_dir = root.path().join("does-not-exist");
        let install_path = root.path().join("install");
        fs::create_dir_all(&install_path).unwrap();
        fs::write(install_path.join("marker.txt"), b"v1").unwrap();

        let result = swap_into_install_dir(&missing_new_dir, &install_path, root.path());

        assert!(result.is_err(), "swap must fail if new_dir doesn't exist");
        assert_eq!(
            fs::read_to_string(install_path.join("marker.txt")).unwrap(),
            "v1",
            "old install must be untouched when the swap fails before completion"
        );
    }

    /// Forces the double-fault path: the backup rename succeeds, the
    /// new-content rename fails (missing `new_dir`), and then the rollback
    /// rename is *also* made to fail by stripping write permission from
    /// `root` the instant the backup lands. A background thread races the
    /// call to `swap_into_install_dir` to flip that permission bit between
    /// the first and second renames — both operations share `root` as their
    /// parent directory, so there is no way to set this up with a static
    /// permission change alone.
    #[test]
    #[cfg(unix)]
    fn rollback_failure_still_returns_err_without_panicking() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempdir().unwrap();
        let missing_new_dir = root.path().join("does-not-exist");
        let install_path = root.path().join("install");
        fs::create_dir_all(&install_path).unwrap();
        fs::write(install_path.join("marker.txt"), b"v1").unwrap();

        let backup = root.path().join("previous");
        let root_path = root.path().to_path_buf();

        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let watcher_root = root_path.clone();
        let watcher_backup = backup.clone();
        let watcher = std::thread::spawn(move || {
            ready_tx.send(()).unwrap();
            while !watcher_backup.exists() {
                std::hint::spin_loop();
            }
            // The backup just landed: make root read-only so neither the
            // new-content rename nor the rollback rename can (re)create the
            // "install" entry inside it.
            let mut perms = fs::metadata(&watcher_root).unwrap().permissions();
            perms.set_mode(0o555);
            fs::set_permissions(&watcher_root, perms).unwrap();
        });
        // Make sure the watcher is already spinning before we start the race.
        ready_rx.recv().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));

        let result = swap_into_install_dir(&missing_new_dir, &install_path, root.path());
        watcher.join().unwrap();

        // Restore permissions so the TempDir can clean itself up on drop.
        let mut perms = fs::metadata(&root_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&root_path, perms).unwrap();

        assert!(
            result.is_err(),
            "swap must still return Err (not panic, not silently succeed) \
             when the rollback rename also fails"
        );
        assert!(
            !install_path.exists(),
            "when the rollback also fails, install_path must not silently end up \
             restored or half-installed"
        );
        assert!(
            backup.exists(),
            "the backed-up old install must still be sitting in staging_root/previous \
             (unrestored) since the rollback rename failed"
        );
    }
}
