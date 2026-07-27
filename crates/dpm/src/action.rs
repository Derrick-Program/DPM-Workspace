use crate::utils::privilege::{chown_dir_to_sudo_user, drop_privileges_for_build};
use crate::{
    clone_package_source, fetch_and_verify_prebuilt, parse_package_spec, place_package,
    resolve_install_set, system::*, unzip_file, ClientError, ClientResult, Context, DbPackage,
    Setting, Source, SourceAction,
};
use colored::Colorize;
use dpm_core::CoreError;
use dpm_core::{Dependency, PackageKind, RepoInfo, VerifyingKey};
use std::collections::HashMap;
use std::fs::{remove_dir_all, remove_file};
use std::path::Path;

/// `(source_hint, name, constraint)` — one parsed `[source/]name[@constraint]`
/// CLI argument, as produced by `parse_package_spec` and consumed by
/// `resolve_install_set`. Named to keep `parse_mine`'s return type under
/// clippy's `type_complexity` threshold.
type ParsedInstallSpec = (Option<String>, String, Option<String>);

/// 驗證下載回來的 `dpm` release archive 簽章用的 ed25519 公鑰(32 bytes)。
/// 對應的私鑰不會出現在這個 repo 裡——它只存在 GitHub Actions secret
/// `ZIPSIGN_SIGNING_KEY_B64`,由 `.github/workflows/release.yml` 用來簽每個
/// release asset。金鑰產生方式見
/// `docs/superpowers/specs/2026-07-26-self-update-design.md`「簽章金鑰」段落。
const RELEASE_SIGNING_PUBLIC_KEY: &[u8; 32] = include_bytes!("../keys/dpm-release-signing.pub");

/// 抓(或重用快取的)`author` 的 ed25519 公鑰(從官方來源的
/// `keys/<author>.pub`),驗證 `signature` 是不是 `hash` 的合法簽章。
/// `sync_source_inner`(同一次 sync,多個套件版本,值得快取)、
/// `install_resolved_with_gate`(單一套件安裝,獨立重新抓一次,不信任
/// `sync_source` 當初看到的任何東西——這就是縱深防禦的重點)共用同一份實作。
async fn verify_official_signature(
    repo_url: &str,
    author: &str,
    hash: &str,
    signature: &str,
    key_cache: &mut HashMap<String, VerifyingKey>,
) -> ClientResult<()> {
    // `author` comes straight from RepoInfo.json (attacker-controlled) and is
    // interpolated into a URL path below — without this check a value like
    // `"../../../mallory/evil-keys/main/keys/mallory"` would redirect the
    // "official" key fetch to an attacker-chosen repo, making the attacker's
    // own signature verify. Same guard as `dpm-server`'s
    // `verify_publish_authorization`.
    dpm_core::validate_author_id(author)?;
    if !key_cache.contains_key(author) {
        let key_url = official_key_url(repo_url, author);
        let response = reqwest::get(&key_url)
            .await
            .map_err(|e| ClientError::Core(CoreError::NetworkError(e.to_string())))?
            .error_for_status()
            .map_err(|e| ClientError::Core(CoreError::NetworkError(e.to_string())))?;
        let bytes = response
            .bytes()
            .await
            .map_err(|e| ClientError::Core(CoreError::NetworkError(e.to_string())))?;
        let verifying_key = dpm_core::verifying_key_from_bytes(bytes.as_ref())?;
        key_cache.insert(author.to_string(), verifying_key);
    }
    let verifying_key = key_cache.get(author).expect("just inserted above");
    dpm_core::verify_hash_signature(verifying_key, hash, signature)?;
    Ok(())
}

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
        self.install_resolved_with_gate(all_packages, is, |repo_url| repo_url == OFFICIAL_REPO_URL)
            .await
    }

    /// `install_resolved` 的實際邏輯,「這個來源是不是官方來源」透過
    /// `is_official` 閉包傳入,而不是在這裡直接比對 `OFFICIAL_REPO_URL`——
    /// 理由跟 `sync_source_inner` 一樣:讓測試能在不打真網路的情況下強制
    /// 走簽章驗證路徑。
    async fn install_resolved_with_gate(
        &self,
        all_packages: &[DbPackage],
        is: &[ParsedInstallSpec],
        is_official: impl Fn(&str) -> bool,
    ) -> ClientResult<()> {
        if !is.is_empty() {
            let resolved = resolve_install_set(all_packages, is)?;
            let mut key_cache: HashMap<String, VerifyingKey> = HashMap::new();
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

                let source = self
                    .setting_config
                    .sources
                    .iter()
                    .find(|s| s.alias == source_alias)
                    .ok_or_else(|| {
                        ClientError::ConfigError(format!(
                            "source '{source_alias}' is not configured"
                        ))
                    })?;
                if is_official(&source.repo_url) {
                    let author = repo_package_info.author.as_deref();
                    let signature = repo_package_info.signature.as_deref();
                    let hash = repo_package_info.hash.as_deref();
                    match (author, signature, hash) {
                        (Some(author), Some(signature), Some(hash)) => {
                            verify_official_signature(
                                &source.repo_url,
                                author,
                                hash,
                                signature,
                                &mut key_cache,
                            )
                            .await
                            .map_err(|e| {
                                ClientError::SystemError(format!(
                                    "INSECURE: {pkg}@{version} failed signature verification (author: {author}): {e}"
                                ))
                            })?;
                        }
                        _ => {
                            return Err(ClientError::SystemError(format!(
                                "INSECURE: {pkg}@{version} is missing author/signature/hash from the official source; refusing to install"
                            )));
                        }
                    }
                }

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
    ///
    /// 已知、刻意延後處理的缺口(見 `dpm_core::PackageKind::Source` 的文件
    /// 註解):下面拿去 `sh -c` 執行的 `repo_package_info.build_command` 是
    /// 直接從(可能被竄改的)`RepoInfo.json` 讀出來的,前面的簽章驗證只檢查
    /// 了 `repo_package_info.hash`,並沒有重算並比對 `build_command`——對
    /// `kind: source` 套件來說,`install_resolved_with_gate` 的簽章閘門目前
    /// 擋不住有 `RepoInfo.json` 寫入權限、但沒有簽名金鑰的攻擊者換掉這裡的
    /// build 指令。
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
    /// 幾乎相同的程式碼。真正的邏輯在 `sync_source_inner`,這裡只是計算
    /// `is_official` 這個安全閘門再委派過去。
    async fn sync_source(ctx: &Context, source: &Source) -> ClientResult<()> {
        let is_official = source.repo_url == OFFICIAL_REPO_URL;
        Self::sync_source_inner(ctx, source, is_official).await
    }

    /// `sync_source` 的實際邏輯,`is_official` 從呼叫端算好傳進來(不是在
    /// 這裡重新比對 `OFFICIAL_REPO_URL`)——這樣測試才能在不打真網路的情況下
    /// 強制走簽章驗證路徑(`OFFICIAL_REPO_URL` 是寫死指向真實 GitHub 的常數,
    /// 測試不可能讓 `source.repo_url` 真的等於它)。
    async fn sync_source_inner(
        ctx: &Context,
        source: &Source,
        is_official: bool,
    ) -> ClientResult<()> {
        let mut remote_repo = RepoInfo::new();
        remote_repo
            .fetch_update_repo_info(&source.repo_info)
            .await?;

        ctx.db.clear_table_for_source(&source.alias).await?;

        let mut key_cache: HashMap<String, VerifyingKey> = HashMap::new();

        for (name, versions) in remote_repo.get_package_handler() {
            for version_info in versions {
                let (kind_str, url, hash, filename, build_command) =
                    version_info.kind.to_db_fields();

                if is_official {
                    let author = version_info.author.as_deref();
                    let signature = version_info.signature.as_deref();
                    let verified = match (author, signature, hash.as_deref()) {
                        (Some(author), Some(signature), Some(hash)) => {
                            verify_official_signature(
                                &source.repo_url,
                                author,
                                hash,
                                signature,
                                &mut key_cache,
                            )
                            .await
                        }
                        _ => Err(ClientError::Core(CoreError::SignatureInvalid(
                            "missing author, signature, or hash".to_string(),
                        ))),
                    };
                    if let Err(e) = verified {
                        // Only an actual signature-validity problem (bad/
                        // missing signature, bad key bytes, missing fields,
                        // a rejected author id) is safe to treat as "this
                        // one package version is untrustworthy, skip it and
                        // keep going." Anything else — a network/HTTP
                        // failure fetching the author's key, an IO error,
                        // etc. — is not evidence the package is malicious;
                        // treating it the same way would silently empty the
                        // local index (the DB for this source was already
                        // cleared above) while `sync_source_inner` still
                        // reports success. Propagate those as a hard error
                        // instead so the sync visibly fails.
                        if !matches!(e, ClientError::Core(CoreError::SignatureInvalid(_))) {
                            return Err(e);
                        }
                        println!(
                            "{} skipping {name}@{} (author: {}) — signature verification failed: {e}",
                            "Warning:".yellow(),
                            version_info.version,
                            author.unwrap_or("<none>"),
                        );
                        continue;
                    }
                }

                let dependencies: Option<Vec<dpm_core::Dependency>> =
                    version_info.dependencies.as_ref().map(|deps| {
                        deps.iter()
                            .map(|dep| Dependency::new(&dep.name, &dep.version))
                            .collect::<Vec<_>>()
                    });
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
                        version_info.author.clone(),
                        version_info.signature.clone(),
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
        let mut setting: Setting = dpm_core::TomlStorage::from_toml(&config_path)?;

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
                dpm_core::TomlStorage::to_toml(&setting, &config_path)?;
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
                dpm_core::TomlStorage::to_toml(&setting, &config_path)?;
                println!("{}", "Source removed.".green());
            }
            SourceAction::List => {
                // The merged (system < user < env) view, not the user-tier-only
                // `setting` that Add/Remove mutate — `list` should show what's
                // actually in effect, including system-tier sources this user
                // can't (and shouldn't) edit.
                for source in &self.setting_config.sources {
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

    pub async fn upgrade_self(&self) -> ClientResult<()> {
        let verbose = self.verbose;
        // `self_update`'s client is a blocking `reqwest::blocking::Client`, which
        // panics if built directly on a thread already inside a tokio runtime
        // (this method is called from `lib.rs::entry()`, which is). Isolate it
        // on a blocking-pool thread instead.
        type UpdateResult = Result<self_update::Status, self_update::errors::Error>;
        let build_result: Result<UpdateResult, ClientError> =
            tokio::task::spawn_blocking(move || {
                self_update::backends::github::Update::configure()
                    .repo_owner("Derrick-Program")
                    .repo_name("DPM-Workspace")
                    .bin_name("dpm")
                    .show_download_progress(verbose)
                    .show_output(verbose)
                    .current_version(env!("CARGO_PKG_VERSION"))
                    .verifying_keys([*RELEASE_SIGNING_PUBLIC_KEY])
                    .build()
                    .map_err(|e| {
                        ClientError::SystemError(format!("failed to configure self-update: {e}"))
                    })
                    .map(|update| update.update())
            })
            .await
            .map_err(|e| ClientError::SystemError(format!("update task failed: {e}")))?;

        match build_result? {
            Ok(self_update::Status::UpToDate(v)) => {
                println!("{} dpm is already up to date (v{v})", "==>".green());
            }
            Ok(self_update::Status::Updated(v)) => {
                println!("{} dpm updated to v{v}", "==>".green());
            }
            Err(e) if is_signature_error(&e) => {
                return Err(ClientError::SystemError(format!(
                    "{}\n{} downloaded update failed signature verification — refusing to install. \
                     This could mean the release was tampered with, or is missing a valid signature. \
                     Not proceeding.",
                    e,
                    "INSECURE:".red().bold()
                )));
            }
            Err(e) if is_permission_denied(&e) => {
                return Err(ClientError::SystemError(format!(
                    "{e}\nhint: dpm's binary isn't writable by the current user — try `sudo dpm upgrade-self`"
                )));
            }
            Err(e) => return Err(ClientError::SystemError(e.to_string())),
        }
        Ok(())
    }
}

/// `self_update::errors::Error::NoSignatures`/`Error::Signature` 都代表下載
/// 回來的 release archive 沒有通過 `RELEASE_SIGNING_PUBLIC_KEY` 的 zipsign
/// 驗證——這是安全相關的失敗,`upgrade_self` 會給它一個獨立的 `INSECURE:`
/// 開頭訊息,跟一般網路/查詢錯誤分開處理。
#[allow(dead_code)]
fn is_signature_error(e: &self_update::errors::Error) -> bool {
    matches!(
        e,
        self_update::errors::Error::NoSignatures(_) | self_update::errors::Error::Signature(_)
    )
}

/// `self_update::errors::Error::Io` 底下包著
/// `io::ErrorKind::PermissionDenied` 代表目前使用者對 `dpm` 執行檔所在位置
/// 沒有寫入權限(例如執行檔是被其他使用者以 system-wide 方式裝的)。
/// `upgrade_self` 會在這個錯誤後面補一句 `sudo dpm upgrade-self` 提示,
/// 而不是直接印出原始的 OS 錯誤訊息。
#[allow(dead_code)]
fn is_permission_denied(e: &self_update::errors::Error) -> bool {
    matches!(
        e,
        self_update::errors::Error::Io(io_err)
            if io_err.kind() == std::io::ErrorKind::PermissionDenied
    )
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

#[cfg(test)]
mod upgrade_self_tests {
    use super::*;
    use self_update::errors::Error;
    use std::io;

    #[test]
    fn permission_denied_io_error_is_detected() {
        let err = Error::Io(io::Error::new(io::ErrorKind::PermissionDenied, "denied"));
        assert!(is_permission_denied(&err));
    }

    #[test]
    fn not_found_io_error_is_not_permission_denied() {
        let err = Error::Io(io::Error::new(io::ErrorKind::NotFound, "missing"));
        assert!(!is_permission_denied(&err));
    }

    #[test]
    fn non_io_error_is_not_permission_denied() {
        let err = Error::Network("boom".to_string());
        assert!(!is_permission_denied(&err));
    }

    #[test]
    fn no_signatures_error_is_a_signature_error() {
        let err = Error::NoSignatures(self_update::ArchiveKind::Tar(None));
        assert!(is_signature_error(&err));
    }

    #[test]
    fn non_signature_error_is_not_a_signature_error() {
        let err = Error::Network("boom".to_string());
        assert!(!is_signature_error(&err));
    }

    // `Error::Signature(zipsign_api::ZipsignError)` isn't covered by its own
    // fixture here: `zipsign_api` is only a transitive dependency (pulled in
    // by self_update's `signatures` feature) and isn't re-exported by
    // self_update, so building one would mean adding zipsign_api as a direct
    // dev-dependency solely to construct a test value. The `matches!` arm in
    // `is_signature_error` covers both `NoSignatures` and `Signature` as one
    // pattern — `no_signatures_error_is_a_signature_error` above exercises
    // that same arm.
}

#[cfg(test)]
mod sync_source_tests {
    use super::*;
    use dpm_core::{PackageKind, PackageVersionInfo};
    use std::collections::HashMap as StdHashMap;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// 跟 `crates/dpm/src/utils/fetcher.rs::serve_once` 同一個手法:接受一次
    /// 連線,回傳固定的 body,忽略實際請求的路徑——`official_key_url`/
    /// `official_repo_info_url` 算出來的路徑不影響這個 mock 的回應內容。
    fn serve_once(body: Vec<u8>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        format!("http://127.0.0.1:{port}")
    }

    /// 跟 `serve_once` 一樣的手法,但回傳一個非 2xx 的 HTTP 狀態——用來模擬
    /// 抓作者公鑰時遇到的暫時性網路問題(GitHub rate-limit、短暫斷線等),
    /// 跟「這個簽章是偽造的」是完全不同的失敗模式。
    fn serve_once_404() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let body = b"not found";
                let response = format!(
                    "HTTP/1.1 404 Not Found\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.write_all(body);
            }
        });
        format!("http://127.0.0.1:{port}")
    }

    /// JSON 形狀跟 `dpm_core::RepoInfo` 的 serde 表示法一致(`{"packages": {...}}`),
    /// 但不透過 `RepoInfo::add_package_version`(那個方法在 `server` feature
    /// 底下,`dpm` 沒有開這個 feature)——直接組出等價的 JSON 內容給
    /// `fetch_update_repo_info` 解析。
    #[derive(serde::Serialize)]
    struct FakeRepoInfo {
        packages: StdHashMap<String, Vec<PackageVersionInfo>>,
    }

    #[tokio::test]
    async fn sync_source_inner_skips_invalid_signature_but_keeps_valid_one_when_official() {
        let signing_key = dpm_core::generate_signing_key().unwrap();
        let pubkey_bytes = signing_key.verifying_key().to_bytes().to_vec();
        let good_hash = "a".repeat(64);
        let good_sig = dpm_core::sign_hash(&signing_key, &good_hash);
        let bad_sig = "0".repeat(128);

        let mut packages = StdHashMap::new();
        packages.insert(
            "good-pkg".to_string(),
            vec![PackageVersionInfo {
                version: "1.0.0".to_string(),
                kind: PackageKind::Prebuilt {
                    url: "https://example.com/good.zip".to_string(),
                    hash: good_hash.clone(),
                    file_name: "good.zip".to_string(),
                },
                dependencies: None,
                entry: None,
                description: None,
                author: Some("alice".to_string()),
                signature: Some(good_sig),
            }],
        );
        packages.insert(
            "bad-pkg".to_string(),
            vec![PackageVersionInfo {
                version: "1.0.0".to_string(),
                kind: PackageKind::Prebuilt {
                    url: "https://example.com/bad.zip".to_string(),
                    hash: good_hash.clone(),
                    file_name: "bad.zip".to_string(),
                },
                dependencies: None,
                entry: None,
                description: None,
                author: Some("alice".to_string()),
                signature: Some(bad_sig),
            }],
        );
        let body = serde_json::to_vec(&FakeRepoInfo { packages }).unwrap();

        let key_url = serve_once(pubkey_bytes);
        let repo_info_url = serve_once(body);

        let source = Source {
            alias: "official".to_string(),
            repo_url: key_url,
            repo_info: repo_info_url,
        };
        let root = tempfile::tempdir().unwrap();
        let ctx = Context::for_test(root.path()).await.unwrap();

        ActionInfo::sync_source_inner(&ctx, &source, true)
            .await
            .unwrap();

        let all = ctx.db.read_all().await.unwrap();
        assert_eq!(all.len(), 1, "the badly-signed package must be skipped");
        assert_eq!(all[0].name, "good-pkg");
    }

    /// Important 1: a transient network/HTTP failure fetching the author's
    /// public key must not be conflated with "this signature is forged."
    /// Regression guard for the bug where ANY error from
    /// `verify_official_signature` (including a rate-limited/erroring key
    /// fetch) was treated as "skip this package," which — since
    /// `clear_table_for_source` already emptied the local DB for this
    /// source — could silently leave the user with an empty official index
    /// while `sync_source_inner` still reported `Ok(())`.
    #[tokio::test]
    async fn sync_source_inner_propagates_hard_error_on_key_fetch_failure() {
        let good_hash = "a".repeat(64);
        // The signature's actual bytes are irrelevant: the key fetch must
        // fail (and propagate) before any signature verification happens.
        let sig = "0".repeat(128);

        let mut packages = StdHashMap::new();
        packages.insert(
            "some-pkg".to_string(),
            vec![PackageVersionInfo {
                version: "1.0.0".to_string(),
                kind: PackageKind::Prebuilt {
                    url: "https://example.com/some.zip".to_string(),
                    hash: good_hash,
                    file_name: "some.zip".to_string(),
                },
                dependencies: None,
                entry: None,
                description: None,
                author: Some("alice".to_string()),
                signature: Some(sig),
            }],
        );
        let body = serde_json::to_vec(&FakeRepoInfo { packages }).unwrap();

        let key_url = serve_once_404();
        let repo_info_url = serve_once(body);

        let source = Source {
            alias: "official".to_string(),
            repo_url: key_url,
            repo_info: repo_info_url,
        };
        let root = tempfile::tempdir().unwrap();
        let ctx = Context::for_test(root.path()).await.unwrap();

        let err = ActionInfo::sync_source_inner(&ctx, &source, true)
            .await
            .unwrap_err();
        assert!(
            matches!(err, ClientError::Core(CoreError::NetworkError(_))),
            "a key-fetch HTTP failure must propagate as a hard error, not be silently skipped: {err}"
        );
    }

    #[tokio::test]
    async fn sync_source_inner_skips_verification_when_not_official() {
        let mut packages = StdHashMap::new();
        packages.insert(
            "unsigned-pkg".to_string(),
            vec![PackageVersionInfo {
                version: "1.0.0".to_string(),
                kind: PackageKind::Prebuilt {
                    url: "https://example.com/unsigned.zip".to_string(),
                    hash: "irrelevant".to_string(),
                    file_name: "unsigned.zip".to_string(),
                },
                dependencies: None,
                entry: None,
                description: None,
                author: None,
                signature: None,
            }],
        );
        let body = serde_json::to_vec(&FakeRepoInfo { packages }).unwrap();
        let repo_info_url = serve_once(body);

        let source = Source {
            alias: "third-party".to_string(),
            repo_url: "https://example.com/some-other-repo".to_string(),
            repo_info: repo_info_url,
        };
        let root = tempfile::tempdir().unwrap();
        let ctx = Context::for_test(root.path()).await.unwrap();

        ActionInfo::sync_source_inner(&ctx, &source, false)
            .await
            .unwrap();

        let all = ctx.db.read_all().await.unwrap();
        assert_eq!(
            all.len(),
            1,
            "unsigned packages from non-official sources are not gated"
        );
    }

    #[tokio::test]
    async fn sync_source_inner_rejects_path_traversal_author() {
        let good_hash = "a".repeat(64);
        // A syntactically well-formed hex signature — irrelevant, since the
        // author id itself must be rejected before any key fetch/verify
        // happens.
        let sig = "0".repeat(128);

        let mut packages = StdHashMap::new();
        packages.insert(
            "evil-pkg".to_string(),
            vec![PackageVersionInfo {
                version: "1.0.0".to_string(),
                kind: PackageKind::Prebuilt {
                    url: "https://example.com/evil.zip".to_string(),
                    hash: good_hash,
                    file_name: "evil.zip".to_string(),
                },
                dependencies: None,
                entry: None,
                description: None,
                author: Some("../../../../mallory/evil-keys/main/keys/mallory".to_string()),
                signature: Some(sig),
            }],
        );
        let body = serde_json::to_vec(&FakeRepoInfo { packages }).unwrap();
        let repo_info_url = serve_once(body);

        let source = Source {
            alias: "official".to_string(),
            // Never actually dialed: the author-id charset check must reject
            // this before any URL gets built or fetched.
            repo_url: "https://example.com/should-not-be-fetched".to_string(),
            repo_info: repo_info_url,
        };
        let root = tempfile::tempdir().unwrap();
        let ctx = Context::for_test(root.path()).await.unwrap();

        ActionInfo::sync_source_inner(&ctx, &source, true)
            .await
            .unwrap();

        let all = ctx.db.read_all().await.unwrap();
        assert_eq!(
            all.len(),
            0,
            "a path-traversal author id must not verify or insert"
        );
    }

    #[tokio::test]
    async fn sync_source_inner_skips_missing_signature_field_when_official() {
        let mut packages = StdHashMap::new();
        packages.insert(
            "no-sig-pkg".to_string(),
            vec![PackageVersionInfo {
                version: "1.0.0".to_string(),
                kind: PackageKind::Prebuilt {
                    url: "https://example.com/no-sig.zip".to_string(),
                    hash: "a".repeat(64),
                    file_name: "no-sig.zip".to_string(),
                },
                dependencies: None,
                entry: None,
                description: None,
                author: Some("alice".to_string()),
                // Present author, but no signature — must be rejected, not
                // silently trusted.
                signature: None,
            }],
        );
        let body = serde_json::to_vec(&FakeRepoInfo { packages }).unwrap();
        let repo_info_url = serve_once(body);

        let source = Source {
            alias: "official".to_string(),
            repo_url: "https://example.com/should-not-be-fetched".to_string(),
            repo_info: repo_info_url,
        };
        let root = tempfile::tempdir().unwrap();
        let ctx = Context::for_test(root.path()).await.unwrap();

        ActionInfo::sync_source_inner(&ctx, &source, true)
            .await
            .unwrap();

        let all = ctx.db.read_all().await.unwrap();
        assert_eq!(
            all.len(),
            0,
            "a package version missing author/signature/hash must be skipped, not inserted"
        );
    }

    #[tokio::test]
    async fn sync_source_inner_skips_signature_signed_over_a_different_hash() {
        let signing_key = dpm_core::generate_signing_key().unwrap();
        let pubkey_bytes = signing_key.verifying_key().to_bytes().to_vec();
        let real_hash = "a".repeat(64);
        let other_hash = "b".repeat(64);
        // A genuinely valid signature — just over the wrong hash. Proves the
        // check binds the signature to *this* package's hash, not merely
        // "any valid signature from this author".
        let sig_over_wrong_hash = dpm_core::sign_hash(&signing_key, &other_hash);

        let mut packages = StdHashMap::new();
        packages.insert(
            "mismatched-hash-pkg".to_string(),
            vec![PackageVersionInfo {
                version: "1.0.0".to_string(),
                kind: PackageKind::Prebuilt {
                    url: "https://example.com/mismatched.zip".to_string(),
                    hash: real_hash,
                    file_name: "mismatched.zip".to_string(),
                },
                dependencies: None,
                entry: None,
                description: None,
                author: Some("alice".to_string()),
                signature: Some(sig_over_wrong_hash),
            }],
        );
        let body = serde_json::to_vec(&FakeRepoInfo { packages }).unwrap();

        let key_url = serve_once(pubkey_bytes);
        let repo_info_url = serve_once(body);

        let source = Source {
            alias: "official".to_string(),
            repo_url: key_url,
            repo_info: repo_info_url,
        };
        let root = tempfile::tempdir().unwrap();
        let ctx = Context::for_test(root.path()).await.unwrap();

        ActionInfo::sync_source_inner(&ctx, &source, true)
            .await
            .unwrap();

        let all = ctx.db.read_all().await.unwrap();
        assert_eq!(
            all.len(),
            0,
            "a valid signature over the wrong hash must not verify"
        );
    }
}

#[cfg(test)]
mod install_resolved_tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    fn serve_once(body: Vec<u8>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        format!("http://127.0.0.1:{port}")
    }

    #[tokio::test]
    async fn install_resolved_rejects_a_signature_that_does_not_match_the_recorded_hash() {
        // 模擬本機 DB 被竄改的情境:提供的公鑰是「真正的作者金鑰」,但
        // DB 裡記錄的 signature 其實是另一把金鑰簽的——代表 DB 裡的
        // author/signature/hash 三者已經兜不起來,驗證必須失敗。
        let real_author_key = dpm_core::generate_signing_key().unwrap();
        let attacker_key = dpm_core::generate_signing_key().unwrap();
        let hash = "b".repeat(64);
        let tampered_signature = dpm_core::sign_hash(&attacker_key, &hash);
        let pubkey_bytes = real_author_key.verifying_key().to_bytes().to_vec();

        let key_url = serve_once(pubkey_bytes);

        let root = tempfile::tempdir().unwrap();
        let ctx = Context::for_test(root.path()).await.unwrap();
        let setting = Setting {
            sources: vec![Source {
                alias: "official".to_string(),
                repo_url: key_url.clone(),
                repo_info: key_url,
            }],
        };
        let action = ActionInfo::new(ctx, vec![], false, setting);

        let pkg = DbPackage::new(
            "official",
            "tampered-pkg",
            "1.0.0",
            "prebuilt",
            Some("https://example.com/tampered.zip".to_string()),
            Some(hash),
            Some("tampered.zip".to_string()),
            None,
            "test",
            None,
            None,
            Some("alice".to_string()),
            Some(tampered_signature),
        );
        let all_packages = vec![pkg];
        let is = vec![(None, "tampered-pkg".to_string(), None)];

        let err = action
            .install_resolved_with_gate(&all_packages, &is, |_| true)
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("INSECURE"),
            "expected an INSECURE-tagged rejection, got: {msg}"
        );
    }

    /// Minor 2: every other test in this module asserts REJECTION. This is
    /// the missing happy-path counterpart — a genuinely valid signature must
    /// be accepted and let the install proceed past the verification gate.
    /// The package's download `url` points at an address nothing is
    /// listening on, so the install can't actually complete; the point is
    /// only that the resulting error is a download/network failure, not the
    /// `"INSECURE"` gate rejection — proving the flow reached past
    /// signature verification instead of being blocked by it.
    #[tokio::test]
    async fn install_resolved_accepts_a_genuinely_valid_signature_and_proceeds_past_the_gate() {
        let signing_key = dpm_core::generate_signing_key().unwrap();
        let pubkey_bytes = signing_key.verifying_key().to_bytes().to_vec();
        let hash = "d".repeat(64);
        let signature = dpm_core::sign_hash(&signing_key, &hash);

        let key_url = serve_once(pubkey_bytes);

        let root = tempfile::tempdir().unwrap();
        let ctx = Context::for_test(root.path()).await.unwrap();
        let setting = Setting {
            sources: vec![Source {
                alias: "official".to_string(),
                repo_url: key_url.clone(),
                repo_info: key_url,
            }],
        };
        let action = ActionInfo::new(ctx, vec![], false, setting);

        let pkg = DbPackage::new(
            "official",
            "valid-pkg",
            "1.0.0",
            "prebuilt",
            // Nothing listens on this port — `download_file` must fail
            // here, well past the signature-verification gate.
            Some("http://127.0.0.1:1/valid.zip".to_string()),
            Some(hash),
            Some("valid.zip".to_string()),
            None,
            "test",
            None,
            None,
            Some("alice".to_string()),
            Some(signature),
        );
        let all_packages = vec![pkg];
        let is = vec![(None, "valid-pkg".to_string(), None)];

        let err = action
            .install_resolved_with_gate(&all_packages, &is, |_| true)
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(
            !msg.contains("INSECURE"),
            "a valid signature must not be rejected by the INSECURE gate, got: {msg}"
        );
    }

    #[tokio::test]
    async fn install_resolved_rejects_a_package_missing_author_or_signature_when_official() {
        let root = tempfile::tempdir().unwrap();
        let ctx = Context::for_test(root.path()).await.unwrap();
        let setting = Setting {
            sources: vec![Source {
                alias: "official".to_string(),
                repo_url: "http://127.0.0.1:1".to_string(),
                repo_info: "http://127.0.0.1:1".to_string(),
            }],
        };
        let action = ActionInfo::new(ctx, vec![], false, setting);

        let pkg = DbPackage::new(
            "official",
            "unsigned-pkg",
            "1.0.0",
            "prebuilt",
            Some("https://example.com/unsigned.zip".to_string()),
            Some("c".repeat(64)),
            Some("unsigned.zip".to_string()),
            None,
            "test",
            None,
            None,
            None,
            None,
        );
        let all_packages = vec![pkg];
        let is = vec![(None, "unsigned-pkg".to_string(), None)];

        let err = action
            .install_resolved_with_gate(&all_packages, &is, |_| true)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("INSECURE"));
    }
}
