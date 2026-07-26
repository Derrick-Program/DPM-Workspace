use crate::utils::privilege::{chown_dir_to_sudo_user, drop_privileges_for_build};
use crate::{
    clone_package_source, fetch_and_verify_prebuilt, parse_package_spec, place_package,
    resolve_install_set, system::*, unzip_file, ClientError, ClientResult, Context, DbPackage,
    Setting, Source, SourceAction,
};
use colored::Colorize;
use dpm_core::CoreError;
use dpm_core::{Dependency, JsonStorage, PackageKind, RepoInfo};
use std::fs::{remove_dir_all, remove_file};
use std::path::Path;

/// `(source_hint, name, constraint)` — one parsed `[source/]name[@constraint]`
/// CLI argument, as produced by `parse_package_spec` and consumed by
/// `resolve_install_set`. Named to keep `parse_mine`'s return type under
/// clippy's `type_complexity` threshold.
type ParsedInstallSpec = (Option<String>, String, Option<String>);

#[derive(Debug)]
pub struct ActionInfo {
    pub ctx: Context,
    pub pkgs: Vec<String>,
    pub verbose: bool,
    pub setting_config: Setting,
    pub system_controller: SystemController,
    pub system_action: SystemAction,
}
impl ActionInfo {
    pub fn new(
        ctx: Context,
        pkgs: Vec<String>,
        verbose: bool,
        setting_config: Setting,
    ) -> ActionInfo {
        let scope = ctx.scope;
        ActionInfo {
            ctx,
            pkgs,
            verbose,
            setting_config,
            system_action: SystemAction::new(verbose),
            system_controller: SystemController::new(scope),
        }
    }
    /// Splits `self.pkgs` (raw `[source/]name[@constraint]` strings) into
    /// packages known to at least one configured source (`is`, parsed into
    /// `(source_hint, name, constraint)` triples for `resolve_install_set`)
    /// and packages not found locally at all (`isnot`, falls through to the
    /// OS package manager) — same split as before, just keyed off the
    /// parsed bare `name` instead of the raw spec string, since a spec like
    /// `official/foo@^1.0` will never literally equal a DB `name` column.
    fn parse_mine(&self, all_packages: &[DbPackage]) -> (Vec<ParsedInstallSpec>, Vec<String>) {
        let mut is = Vec::new();
        let mut isnot = Vec::new();
        for raw in &self.pkgs {
            let (source, name, constraint) = parse_package_spec(raw);
            if all_packages.iter().any(|p| p.name == name) {
                is.push((
                    source.map(str::to_string),
                    name.to_string(),
                    constraint.map(str::to_string),
                ));
            } else {
                isnot.push(name.to_string());
            }
        }
        (is, isnot)
    }

    /// Reads every locally-known package and splits `self.pkgs` against it —
    /// the same `read_all()` + `parse_mine()` pair every command that acts on
    /// `self.pkgs` needs (`install`/`uninstall`/`search`/`upgrade`), extracted
    /// so the DB error-mapping and the split logic each live in one place
    /// instead of four identical copies.
    async fn parsed_packages(
        &self,
    ) -> ClientResult<(Vec<DbPackage>, Vec<ParsedInstallSpec>, Vec<String>)> {
        let all_packages = self
            .ctx
            .db
            .read_all()
            .await
            .map_err(|e| ClientError::Core(CoreError::DatabaseError(e.to_string())))?;
        let (is, isnot) = self.parse_mine(&all_packages);
        Ok((all_packages, is, isnot))
    }

    /// Resolves every `is` spec (dpm-managed packages, matched by name
    /// against the local index) to a concrete `(source, name, version)` and
    /// fetches/builds + places each one. Shared by `install()` (fresh
    /// installs) and `upgrade()` — installing over an already-installed
    /// `install_path` re-swaps it in via `place_package`'s existing
    /// atomic-upgrade path, so "resolve to the best matching version and
    /// (re)install it" already *is* an upgrade; `upgrade()` used to just
    /// debug-print these names instead of calling this.
    async fn install_resolved(
        &self,
        all_packages: &[DbPackage],
        is: &[ParsedInstallSpec],
    ) -> ClientResult<()> {
        if !is.is_empty() {
            let resolved = resolve_install_set(all_packages, is)?;
            for (source_alias, name, version) in resolved {
                let pkg = name.as_str();
                let repo_package_info = all_packages
                    .iter()
                    .find(|p| p.source == source_alias && p.name == name && p.version == version)
                    .ok_or_else(|| {
                        ClientError::Core(CoreError::PackageNotFound(format!(
                            "{source_alias}/{name}@{version}"
                        )))
                    })?;
                if self.verbose {
                    println!("{}\n\n  {}", pkg.on_green(), "Downloading...".yellow());
                }

                let staging_root_base = self.ctx.main_dir.join(".staging");
                std::fs::create_dir_all(&staging_root_base)
                    .map_err(|e| ClientError::Core(CoreError::IoError(e)))?;
                let staging = tempfile::Builder::new()
                    .prefix(pkg)
                    .tempdir_in(&staging_root_base)
                    .map_err(|e| ClientError::Core(CoreError::IoError(e)))?;

                if matches!(repo_package_info.kind()?, PackageKind::Source { .. }) {
                    self.install_source_package(pkg, &source_alias, repo_package_info, &staging)?;
                    if self.verbose {
                        println!("  {}", "Installed!".green());
                    }
                    continue;
                }

                let filename = repo_package_info.filename.clone().ok_or_else(|| {
                    ClientError::Core(CoreError::InvalidPackage(format!(
                        "{pkg} has no downloadable file (source package kind not yet installable)"
                    )))
                })?;
                let download_path = staging.path().join(&filename);
                let package_info =
                    fetch_and_verify_prebuilt(pkg, repo_package_info, &download_path).await?;
                if self.verbose {
                    println!("  {}", "Download successed!".green());
                    println!("  {}", "Hashes Passed".green());
                    println!("  {}", "Installing ...".yellow());
                }

                let extracted = staging.path().join("extracted");
                unzip_file(&download_path, &extracted)
                    .map_err(|e| ClientError::Core(CoreError::IoError(e)))?;

                place_package(
                    pkg,
                    &extracted,
                    Some(package_info.file_name.as_str()),
                    &self.ctx.install_dir,
                    &self.ctx.bin_dir,
                    staging.path(),
                    &self.system_controller,
                )?;
                if self.verbose {
                    println!("  {}", "Installed!".green());
                    println!("  {}", "Successed Create Link!".green());
                }
                // `staging` (tempfile::TempDir) drop 在這裡發生,連同任何被搬到
                // staging_root/previous 的舊版本一起清掉。
            }
        }
        Ok(())
    }

    pub async fn install(&self) -> ClientResult<()> {
        let (all_packages, is, isnot) = self.parsed_packages().await?;
        self.install_resolved(&all_packages, &is).await?;
        if !isnot.is_empty() {
            for pkg in isnot {
                self.system_action.install_package(&pkg)?;
            }
        }
        Ok(())
    }

    /// 安裝一個 `kind: "source"` 的套件:淺層 clone 它的來源 repo、在 staging
    /// 目錄裡用呼叫者當下的權限(不經過 `system_command_runner`,所以不管
    /// `--system` 與否都不會提權)執行 `build_command`,`$OUT` 指向這次的產出
    /// 目錄,成功後透過既有的 `swap_into_install_dir` 原子換裝。
    fn install_source_package(
        &self,
        pkg: &str,
        source_alias: &str,
        repo_package_info: &DbPackage,
        staging: &tempfile::TempDir,
    ) -> ClientResult<()> {
        if source_alias != "official" {
            println!(
                "{} installing a source package from a third-party source, not vetted by the DPM team",
                "Warning:".yellow()
            );
        }

        let build_command = repo_package_info.build_command.clone().ok_or_else(|| {
            ClientError::Core(CoreError::InvalidPackage(format!(
                "{pkg} is kind=source but has no build command recorded"
            )))
        })?;

        let sources = self.setting_config.sources.clone();
        let source = sources
            .iter()
            .find(|s| s.alias == source_alias)
            .ok_or_else(|| {
                ClientError::ConfigError(format!("source '{source_alias}' is not configured"))
            })?;

        if self.verbose {
            println!("  {}", "Fetching source...".yellow());
        }
        let clone_dir = staging.path().join("clone");
        let package_src = clone_package_source(&source.repo_url, pkg, &clone_dir)?;
        chown_dir_to_sudo_user(&clone_dir)?;

        let out_dir = staging.path().join("out");
        std::fs::create_dir_all(&out_dir).map_err(|e| ClientError::Core(CoreError::IoError(e)))?;
        chown_dir_to_sudo_user(&out_dir)?;

        if self.verbose {
            println!(
                "  {}",
                "Building (running an untrusted build command)...".yellow()
            );
        }
        let mut build_cmd = std::process::Command::new("sh");
        build_cmd
            .arg("-c")
            .arg(&build_command)
            .current_dir(&package_src)
            .env("OUT", &out_dir);
        drop_privileges_for_build(&mut build_cmd)?;
        let status = build_cmd
            .status()
            .map_err(|e| ClientError::SystemError(format!("failed to run build command: {e}")))?;
        if !status.success() {
            return Err(ClientError::SystemError(format!(
                "build command for {pkg} exited with {status}"
            )));
        }

        place_package(
            pkg,
            &out_dir,
            repo_package_info.entry.as_deref(),
            &self.ctx.install_dir,
            &self.ctx.bin_dir,
            staging.path(),
            &self.system_controller,
        )
    }

    /// 抓某一個來源的完整索引,清空該來源在本地 DB 的舊資料,把每個套件的每個
    /// 版本各自插入一列。`update()`(既有來源全部重整)、`init_update()`
    /// (`init()` 第一次執行時的初始灌入)共用這個邏輯——原本兩處各自複製一份
    /// 幾乎相同的程式碼。
    async fn sync_source(ctx: &Context, source: &Source) -> ClientResult<()> {
        let mut remote_repo = RepoInfo::new();
        remote_repo
            .fetch_update_repo_info(&source.repo_info)
            .await?;

        ctx.db.clear_table_for_source(&source.alias).await?;

        for (name, versions) in remote_repo.get_package_handler() {
            for version_info in versions {
                let dependencies: Option<Vec<dpm_core::Dependency>> =
                    version_info.dependencies.as_ref().map(|deps| {
                        deps.iter()
                            .map(|dep| Dependency::new(&dep.name, &dep.version))
                            .collect::<Vec<_>>()
                    });
                let (kind_str, url, hash, filename, build_command) =
                    version_info.kind.to_db_fields();
                ctx.db
                    .insert(DbPackage::new(
                        &source.alias,
                        name,
                        &version_info.version,
                        kind_str,
                        url,
                        hash,
                        filename,
                        build_command,
                        version_info.description.as_deref().unwrap_or(""),
                        version_info.entry.clone(),
                        dependencies,
                        None, // Task 10 會換成 version_info.author.clone()
                        None, // Task 10 會換成 version_info.signature.clone()
                    ))
                    .await?;
            }
        }
        Ok(())
    }

    pub async fn update(&self) -> ClientResult<()> {
        println!("{} Updating...", "==>".blue());
        for source in &self.setting_config.sources {
            println!("{} Updating source '{}'...", "==>".blue(), source.alias);
            Self::sync_source(&self.ctx, source).await?;
        }
        println!("{} Updated!", "==>".green());
        Ok(())
    }

    pub async fn init_update(ctx: &Context, source: &Source) -> ClientResult<()> {
        Self::sync_source(ctx, source).await
    }

    pub async fn source(&self, action: SourceAction) -> ClientResult<()> {
        let config_path = self.ctx.config_path();
        let mut setting: Setting = JsonStorage::from_json(&config_path)?;

        match action {
            SourceAction::Add { url, alias } => {
                if !url.starts_with("https://") {
                    return Err(ClientError::ConfigError(
                        "source url must use https://".to_string(),
                    ));
                }
                let alias = alias.unwrap_or_else(|| {
                    url.trim_start_matches("https://")
                        .split('/')
                        .next()
                        .unwrap_or(&url)
                        .to_string()
                });
                if setting.sources.iter().any(|s| s.alias == alias) {
                    return Err(ClientError::ConfigError(format!(
                        "source alias '{alias}' already exists"
                    )));
                }
                if alias != "official" {
                    println!(
                        "{} third-party source, not vetted by the DPM team",
                        "Warning:".yellow()
                    );
                }
                setting.sources.push(Source {
                    alias,
                    repo_url: url.clone(),
                    repo_info: url,
                });
                JsonStorage::to_json(&setting, &config_path)?;
                println!(
                    "{}",
                    "Source added. Run `dpm update` to fetch its index.".green()
                );
            }
            SourceAction::Remove { alias } => {
                let before = setting.sources.len();
                setting.sources.retain(|s| s.alias != alias);
                if setting.sources.len() == before {
                    return Err(ClientError::ConfigError(format!(
                        "no source with alias '{alias}'"
                    )));
                }
                self.ctx.db.clear_table_for_source(&alias).await?;
                JsonStorage::to_json(&setting, &config_path)?;
                println!("{}", "Source removed.".green());
            }
            SourceAction::List => {
                for source in &setting.sources {
                    println!("{}  {}", source.alias.green(), source.repo_info);
                }
            }
        }
        Ok(())
    }

    pub async fn uninstall(&self) -> ClientResult<()> {
        let (_, is, isnot) = self.parsed_packages().await?;
        if !is.is_empty() {
            for (_, pkg, _) in is {
                let pre_rm_location = self.ctx.install_dir.join(&pkg);
                let pre_rm_ln = self.ctx.bin_dir.join(&pkg);
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
        let (_, is, isnot) = self.parsed_packages().await?;
        if !is.is_empty() {
            println!();
            for (_, pkg, _) in is {
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
            for pkg in installed_package_names(&self.ctx.install_dir)? {
                println!("{}", pkg.green());
            }
        }
        Ok(())
    }

    pub async fn upgrade(&self) -> ClientResult<()> {
        let (all_packages, is, isnot) = self.parsed_packages().await?;
        self.install_resolved(&all_packages, &is).await?;
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

/// 每個安裝好的套件在 `install_dir` 底下都是一個獨立子目錄(見 `place_package`),
/// 所以「已安裝套件」= `install_dir` 的頂層子目錄名稱,不需要遞迴進每個套件內部
/// 的檔案。`install_dir` 在第一次安裝前不存在是正常狀態,回傳空清單而不是錯誤。
fn installed_package_names(install_dir: &Path) -> ClientResult<Vec<String>> {
    if !install_dir.exists() {
        return Ok(vec![]);
    }
    let mut names: Vec<String> = std::fs::read_dir(install_dir)
        .map_err(|e| ClientError::Core(CoreError::IoError(e)))?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    names.sort();
    Ok(names)
}

#[cfg(test)]
mod installed_package_names_tests {
    use super::installed_package_names;
    use tempfile::tempdir;

    #[test]
    fn missing_install_dir_is_an_empty_list_not_an_error() {
        let root = tempdir().unwrap();
        let missing = root.path().join("does-not-exist");

        assert_eq!(
            installed_package_names(&missing).unwrap(),
            Vec::<String>::new()
        );
    }

    #[test]
    fn lists_top_level_package_dirs_sorted_and_skips_files() {
        let root = tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("zeta")).unwrap();
        std::fs::create_dir_all(root.path().join("alpha")).unwrap();
        // A stray file directly under install_dir (not a package) must not
        // show up as an "installed package".
        std::fs::write(root.path().join("README.txt"), b"not a package").unwrap();

        assert_eq!(
            installed_package_names(root.path()).unwrap(),
            vec!["alpha".to_string(), "zeta".to_string()]
        );
    }
}
