use std::{
    fs::{self, remove_dir_all, remove_file, Permissions},
    os::unix::fs::PermissionsExt,
    path::{Component, Path},
};

use crate::{
    clone_package_source, download_file, parse_package_spec, read_file_from_zip,
    resolve_install_set, system::*, unzip_file, ClientError, ClientResult, Context, DbPackage,
    Hashes, Setting, Source, SourceAction,
};
use colored::Colorize;
use dpm_core::CoreError;
use dpm_core::{Dependency, JsonStorage, PackageInfo, PackageKind, RepoInfo};
use walkdir::WalkDir;

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
    pub async fn install(&self) -> ClientResult<()> {
        let all_packages = self
            .ctx
            .db
            .read_all()
            .await
            .map_err(|e| ClientError::Core(CoreError::DatabaseError(e.to_string())))?;
        let (is, isnot) = self.parse_mine(&all_packages);
        if !is.is_empty() {
            let resolved = resolve_install_set(&all_packages, &is)?;
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

                if repo_package_info.kind == "source" {
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
                let url = repo_package_info.url.as_deref().ok_or_else(|| {
                    ClientError::Core(CoreError::InvalidPackage(format!("{pkg} has no url")))
                })?;
                download_file(url, &download_path).await?;
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

                let install_path = self.ctx.install_dir.join(pkg);
                swap_into_install_dir(&extracted, &install_path, staging.path())?;
                if self.verbose {
                    println!("  {}", "Installed!".green());
                    println!("  {}", "Create Links ...".yellow());
                }
                if !entry_is_safe(&package_info.file_name) {
                    return Err(ClientError::Core(CoreError::InvalidPackage(format!(
                        "{pkg} has an unsafe entry path: {}",
                        package_info.file_name
                    ))));
                }
                let main_file = install_path.join(&package_info.file_name);
                entry_resolves_inside_install_dir(pkg, &main_file, &install_path)?;
                let ln_path = self.ctx.bin_dir.join(pkg);
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

        let install_path = self.ctx.install_dir.join(pkg);
        swap_into_install_dir(&out_dir, &install_path, staging.path())?;

        if !repo_package_info.entry.is_empty() {
            if !entry_is_safe(&repo_package_info.entry) {
                return Err(ClientError::Core(CoreError::InvalidPackage(format!(
                    "{pkg} has an unsafe entry path: {}",
                    repo_package_info.entry
                ))));
            }
            let main_file = install_path.join(&repo_package_info.entry);
            entry_resolves_inside_install_dir(pkg, &main_file, &install_path)?;
            let ln_path = self.ctx.bin_dir.join(pkg);
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
        }
        Ok(())
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
                let (kind_str, url, hash, filename, build_command) = match &version_info.kind {
                    PackageKind::Prebuilt {
                        url,
                        hash,
                        file_name,
                    } => (
                        "prebuilt".to_string(),
                        Some(url.clone()),
                        Some(hash.clone()),
                        Some(file_name.clone()),
                        None,
                    ),
                    PackageKind::Source { build } => {
                        ("source".to_string(), None, None, None, Some(build.clone()))
                    }
                };
                ctx.db
                    .insert(DbPackage::new(
                        &source.alias,
                        name,
                        &version_info.version,
                        &kind_str,
                        url,
                        hash,
                        filename,
                        build_command,
                        version_info.description.as_deref().unwrap_or(""),
                        version_info.entry.as_deref().unwrap_or(""),
                        dependencies,
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
        let config_path = self.ctx.config_dir.join("config.json");
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
        let all_packages = self
            .ctx
            .db
            .read_all()
            .await
            .map_err(|e| ClientError::Core(CoreError::DatabaseError(e.to_string())))?;
        let (is, isnot) = self.parse_mine(&all_packages);
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
        let all_packages = self
            .ctx
            .db
            .read_all()
            .await
            .map_err(|e| ClientError::Core(CoreError::DatabaseError(e.to_string())))?;
        let (is, isnot) = self.parse_mine(&all_packages);
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
            let path = &self.ctx.install_dir;
            for entry in WalkDir::new(path) {
                let entry = entry.map_err(|e| ClientError::Core(CoreError::IoError(e.into())))?;
                let _path = entry.path();
            }
        }
        Ok(())
    }

    pub async fn upgrade(&self) -> ClientResult<()> {
        let all_packages = self
            .ctx
            .db
            .read_all()
            .await
            .map_err(|e| ClientError::Core(CoreError::DatabaseError(e.to_string())))?;
        let (is, isnot) = self.parse_mine(&all_packages);
        if !is.is_empty() {
            for (_, pkg, _) in is {
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

/// 判斷一個從(可能未受信任的)套件索引拿到的 entry/file_name 相對路徑,
/// 是否會在跟 `install_path` join 之後逃逸出 install_path 之外。
/// `PathBuf::join` 遇到絕對路徑會直接整個取代 base(這是文件化行為,不是
/// bug),所以只要 entry 是絕對路徑,或含有 `..` component,就一律視為不安全
/// ——不需要等到檔案系統上真的存在才能判斷,也不需要 canonicalize。
fn entry_is_safe(entry: &str) -> bool {
    let path = Path::new(entry);
    !path.is_absolute() && !path.components().any(|c| matches!(c, Component::ParentDir))
}

/// `entry_is_safe` 只擋得住路徑「字串」裡的絕對路徑/`..`,擋不住
/// `install_path` 底下真的有個 symlink 指到外面——`out_dir`(source 安裝)或
/// 解壓出來的內容(prebuilt 安裝)都是 build_command/發佈者可控的,可以放一個
/// `entry` 指向的 symlink 指到任意路徑(例如 `/etc/shadow`)。呼叫這個函式時
/// `swap_into_install_dir` 一定已經跑完,`install_path` 已經在檔案系統上存在,
/// 所以在把 `main_file` 交給 `fs::set_permissions`(等同 `chmod`,會跟隨
/// symlink)之前,canonicalize 兩邊、確認解完符號連結後 `main_file` 仍然落在
/// `install_path` 底下——這是額外的一層檢查,不是取代 `entry_is_safe`。
fn entry_resolves_inside_install_dir(
    pkg: &str,
    main_file: &Path,
    install_path: &Path,
) -> ClientResult<()> {
    let canonical_install = std::fs::canonicalize(install_path)
        .map_err(|e| ClientError::Core(CoreError::IoError(e)))?;
    let canonical_main_file =
        std::fs::canonicalize(main_file).map_err(|e| ClientError::Core(CoreError::IoError(e)))?;
    if !canonical_main_file.starts_with(&canonical_install) {
        return Err(ClientError::Core(CoreError::InvalidPackage(format!(
            "{pkg}'s entry resolves outside the install directory"
        ))));
    }
    Ok(())
}

/// 把即將執行 build_command 的子行程,在 fork 之後、exec 之前從 root drop
/// 回原本呼叫 `sudo`(Linux `--system`)的那個使用者,避免「整個 process 已經
/// 是 root,build_command 就跟著是 root」——`system_command_runner` 沒被
/// 呼叫只能防住額外的 `sudo` 前綴,防不了本來就是 root 的父行程。
/// 只有 Linux 需要:macOS 的 `--system` 是逐指令 sudo,呼叫這個函式的
/// process 本身從來不會是 root。
#[cfg(target_os = "linux")]
fn drop_privileges_for_build(cmd: &mut std::process::Command) -> ClientResult<()> {
    use std::os::unix::process::CommandExt;

    if unsafe { libc::getuid() } != 0 {
        // 不是 root(PerUser scope,或非 sudo 直接以一般使用者執行),
        // build_command 本來就不會以 root 執行,不需要 drop。
        return Ok(());
    }

    let uid: libc::uid_t = std::env::var("SUDO_UID")
        .ok()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| {
            ClientError::SystemError(
                "refusing to run an untrusted build command as root: process is running as \
                 root but SUDO_UID is not set/parseable, so there is no non-root user to drop \
                 privileges to"
                    .to_string(),
            )
        })?;
    let gid: libc::gid_t = std::env::var("SUDO_GID")
        .ok()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| {
            ClientError::SystemError(
                "refusing to run an untrusted build command as root: process is running as \
                 root but SUDO_GID is not set/parseable, so there is no non-root group to drop \
                 privileges to"
                    .to_string(),
            )
        })?;

    // Safety: the closure only calls async-signal-safe libc functions
    // (setgroups/setgid/setuid) and returns before doing anything else; this
    // matches the documented safety contract of `pre_exec`.
    unsafe {
        cmd.pre_exec(move || {
            // Clear root's supplementary groups (typically includes gid 0)
            // FIRST — otherwise the "de-privileged" child stays a member of
            // any group-0-writable file on the system even after
            // setgid/setuid change the primary/effective/saved ids. Must run
            // before setgid/setuid: dropping the primary uid/gid first can
            // remove the privilege needed to call setgroups at all.
            if libc::setgroups(0, std::ptr::null()) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            // GID before UID: dropping UID first can leave us unprivileged to
            // change GID afterward (standard Unix privilege-drop ordering).
            if libc::setgid(gid) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::setuid(uid) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn drop_privileges_for_build(_cmd: &mut std::process::Command) -> ClientResult<()> {
    // ponytail: macOS `--system` is per-command sudo, never whole-process
    // elevation, so the parent (and thus this build child) is never root
    // here — nothing to drop. Revisit only if macOS ever gains a
    // whole-process elevation mode.
    Ok(())
}

/// 在 Linux `--system` 下,`main.rs` 已經把整個 `dpm` process 提權成 root,
/// 所以呼叫這個函式的當下(parent process)建立出來的 `clone_dir`/`out_dir`
/// 都是 root 所有、mode ~0755。`drop_privileges_for_build` 之後只會把「build
/// 子行程」降回 `SUDO_UID`/`SUDO_GID`,降權後的子行程沒辦法寫進 root 專屬的
/// 目錄——build 只要寫自己的 working tree 或寫 `$OUT` 就會直接 `EACCES`。這裡
/// 把該目錄整棵樹 `chown` 給同一個 `SUDO_UID`/`SUDO_GID`,讓降權後的 build
/// 子行程能真的寫得進去;不需要之後再 chown 回 root——`swap_into_install_dir`
/// 的 `rename` 呼叫方(parent process)本身還是 root,不受來源內容 ownership
/// 影響,任何殘留的非 root ownership 會被既有的 `permision_check()`(下次
/// `dpm` 呼叫時對整個 `MAIN_DIR` 做 `chown -R root:root`)自動收斂。
/// 只有 Linux root 需要:條件跟 `drop_privileges_for_build` 一致——per-user
/// scope 與 macOS 下,建立這些目錄的行程本來就不是 root,不需要 chown。
#[cfg(target_os = "linux")]
fn chown_dir_to_sudo_user(dir: &Path) -> ClientResult<()> {
    if unsafe { libc::getuid() } != 0 {
        return Ok(());
    }

    let uid: libc::uid_t = std::env::var("SUDO_UID")
        .ok()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| {
            ClientError::SystemError(
                "couldn't prepare build directory for the target user: process is running as \
                 root but SUDO_UID is not set/parseable"
                    .to_string(),
            )
        })?;
    let gid: libc::gid_t = std::env::var("SUDO_GID")
        .ok()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| {
            ClientError::SystemError(
                "couldn't prepare build directory for the target user: process is running as \
                 root but SUDO_GID is not set/parseable"
                    .to_string(),
            )
        })?;

    let status = std::process::Command::new("chown")
        .arg("-R")
        .arg(format!("{uid}:{gid}"))
        .arg(dir)
        .status()
        .map_err(|e| {
            ClientError::SystemError(format!(
                "couldn't prepare build directory for the target user: failed to run chown: {e}"
            ))
        })?;
    if !status.success() {
        return Err(ClientError::SystemError(format!(
            "couldn't prepare build directory for the target user: chown exited with {status}"
        )));
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn chown_dir_to_sudo_user(_dir: &Path) -> ClientResult<()> {
    // ponytail: macOS `--system` is per-command sudo, never whole-process
    // elevation, so the parent creating these dirs is never root here —
    // there's no ownership mismatch to fix. Mirrors drop_privileges_for_build's
    // own OS gating.
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
    ///
    /// Timing note: the watcher/main-thread synchronization here is real OS
    /// thread scheduling (spin-poll on `backup.exists()`), not a guaranteed
    /// ordering — there is no deterministic seam in `swap_into_install_dir`
    /// to hook into, and adding one just for this test would be more
    /// production-code surface than this test-only race is worth. On a
    /// busy/throttled/virtualized CI runner the watcher can lose the race
    /// (chmod lands after both renames already completed). If that happens,
    /// this test does **not** pass vacuously — the `!install_path.exists()`
    /// / `backup.exists()` assertions below would fail loudly, since the
    /// rollback would have succeeded normally. So a lost race shows up as an
    /// intermittent, non-actionable CI failure rather than a silent false
    /// pass. If this test is ever observed flaking in CI, quarantine it with
    /// `#[ignore]` rather than trying to chase the scheduling further.
    #[test]
    #[cfg(unix)]
    fn rollback_failure_still_returns_err_without_panicking() {
        use std::os::unix::fs::PermissionsExt;

        // chmod 0o555 below only blocks the rollback rename for a
        // non-root user; root can write through it regardless, which would
        // make the rollback always succeed and this test deterministically
        // fail for a reason unrelated to the code under test. Skip
        // gracefully when running as root (e.g. some containerized CI).
        if unsafe { libc::geteuid() } == 0 {
            eprintln!(
                "skipping rollback_failure_still_returns_err_without_panicking: \
                 running as root, chmod 0o555 does not block root's renames"
            );
            return;
        }

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

#[cfg(test)]
mod entry_is_safe_tests {
    use super::entry_is_safe;

    #[test]
    fn rejects_absolute_path() {
        assert!(!entry_is_safe("/etc/cron.d/x"));
    }

    #[test]
    fn rejects_parent_dir_traversal() {
        assert!(!entry_is_safe("../../../etc/passwd"));
        assert!(!entry_is_safe("bin/../../escape"));
    }

    #[test]
    fn accepts_normal_relative_filename() {
        assert!(entry_is_safe("bin/main"));
        assert!(entry_is_safe("main"));
        assert!(entry_is_safe("./main"));
    }

    #[test]
    fn rejects_empty_string_is_still_safe_but_caller_checks_empty_separately() {
        // entry_is_safe itself doesn't special-case "": the `.is_empty()`
        // check happens at the call sites before this is even invoked.
        // Document that here so nobody "fixes" this later.
        assert!(entry_is_safe(""));
    }
}

#[cfg(test)]
mod entry_resolves_inside_install_dir_tests {
    use super::entry_resolves_inside_install_dir;
    use std::os::unix::fs::symlink;
    use tempfile::tempdir;

    #[test]
    fn accepts_normal_file_inside_install_dir() {
        let install_dir = tempdir().unwrap();
        let main_file = install_dir.path().join("main");
        std::fs::write(&main_file, b"bin").unwrap();

        assert!(entry_resolves_inside_install_dir("pkg", &main_file, install_dir.path()).is_ok());
    }

    #[test]
    fn rejects_symlink_escaping_install_dir() {
        let install_dir = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let secret = outside.path().join("secret");
        std::fs::write(&secret, b"root-owned").unwrap();

        // `out_dir`/extracted content is build_command/publisher-controlled;
        // this simulates `entry = "evil"` resolving to a symlink planted
        // there that points outside install_path.
        let evil = install_dir.path().join("evil");
        symlink(&secret, &evil).unwrap();

        let result = entry_resolves_inside_install_dir("pkg", &evil, install_dir.path());
        assert!(
            result.is_err(),
            "symlink escaping install_dir must be rejected"
        );
    }
}
