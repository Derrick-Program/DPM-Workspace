use std::{
    fs::{self, remove_dir_all, remove_file, File, Permissions},
    io::Read,
    os::unix::fs::PermissionsExt,
    path::Path,
};

use crate::{
    get_db, read_file_from_zip, system::*, unzip_file, ClientError, ClientResult, DbPackage,
    Hashes, Setting, BIN_DIR, INSTALL_DIR,
};
use colored::Colorize;
use dpm_core::CoreError;
use dpm_core::{Dependency, JsonStorage, PackageInfo, RepoInfo};
use sha2::{Digest, Sha256};
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
    fn parse_mine(&self) -> (Vec<String>, Vec<String>) {
        let mut is: Vec<String> = Vec::new();
        let mut isnot: Vec<String> = Vec::new();
        let all_packages = get_db().read_all().unwrap_or_else(|_| Vec::new());
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
        let (is, isnot) = self.parse_mine();
        if !is.is_empty() {
            for pkg in is {
                let pkg = pkg.as_str();
                let repo_package_info = get_db()
                    .read_one(pkg)
                    .map_err(|e| ClientError::Core(CoreError::DatabaseError(e.to_string())))?
                    .ok_or_else(|| {
                        ClientError::Core(CoreError::PackageNotFound(pkg.to_string()))
                    })?;
                if self.verbose {
                    println!("{}\n\n  {}", pkg.on_green(), "Downloading...".yellow());
                }
                get_db()
                    .download_file(pkg)
                    .await
                    .map_err(|e| ClientError::Core(CoreError::NetworkError(e.to_string())))?;
                if self.verbose {
                    println!("  {}", "Download successed!".green());
                }
                let ori_path = Path::new("/tmp").join(repo_package_info.filename);
                let package_info_test: String =
                    read_file_from_zip(&ori_path, "packageInfo.json").unwrap();
                let package_info: PackageInfo =
                    JsonStorage::from_str_to(package_info_test.as_str()).unwrap();
                let package_hash_info: Hashes = JsonStorage::from_str_to(
                    read_file_from_zip(&ori_path, "hashes.json")
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
                let hash = Self::hasher(&ori_path)?;
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

                let install_path = INSTALL_DIR.get().unwrap().join(pkg);
                unzip_file(&ori_path, &install_path)
                    .map_err(|e| ClientError::Core(CoreError::IoError(e)))?;
                if self.verbose {
                    println!("  {}", "Installed!".green());
                    println!("  {}", "Removing tmp file ...".blue());
                }
                remove_file(ori_path).map_err(|e| ClientError::Core(CoreError::IoError(e)))?;
                if self.verbose {
                    println!("  {}", "Removed Success ...".green());
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

        db.clear_table("LocalRepo")?;

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
            get_db().insert(DbPackage::new(
                name,
                repo_info.version.as_str(),
                repo_info.url.as_str(),
                package_info.description.as_str(),
                repo_info.file_name.as_str(),
                repo_info.hash.as_str(),
                package_info.file_name.as_str(),
                dependencies1,
            ))?;
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
            get_db().insert(DbPackage::new(
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
            ))?;
        }
        Ok(())
    }

    pub fn uninstall(&self) -> ClientResult<()> {
        let (is, isnot) = self.parse_mine();
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

    pub fn search(&self) -> ClientResult<()> {
        let (is, isnot) = self.parse_mine();
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

    pub fn list(&self, sys: bool) -> ClientResult<()> {
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

    pub fn upgrade(&self) -> ClientResult<()> {
        let (is, isnot) = self.parse_mine();
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

    fn hasher(file_path: &Path) -> ClientResult<String> {
        let mut hasher = Sha256::new();
        let mut file =
            File::open(file_path).map_err(|e| ClientError::Core(CoreError::IoError(e)))?;
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer)
            .map_err(|e| ClientError::Core(CoreError::IoError(e)))?;
        hasher.update(&buffer);
        let result = hasher.finalize();
        Ok(hex::encode(result))
    }
}
