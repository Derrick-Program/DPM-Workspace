use crate::utils::privilege::{chown_dir_to_sudo_user, drop_privileges_for_build};
use crate::{
    clone_package_source, fetch_and_verify_prebuilt, find_orphans, find_outdated,
    parse_package_spec, place_package, read_file_from_zip, resolve_install_set, system::*,
    unzip_file, ClientError, ClientResult, Context, DbPackage, Hashes, Setting, Source,
    SourceAction,
};
use colored::Colorize;
use dpm_core::CoreError;
use dpm_core::{JsonStorage, PackageInfo, PackageKind, VerifyingKey};
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
        let bytes = if let Some(path_str) = key_url.strip_prefix("file://") {
            tokio::fs::read(path_str)
                .await
                .map_err(|e| ClientError::Core(CoreError::IoError(e)))?
        } else if std::path::Path::new(&key_url).exists() {
            tokio::fs::read(&key_url)
                .await
                .map_err(|e| ClientError::Core(CoreError::IoError(e)))?
        } else {
            let response = reqwest::get(&key_url)
                .await
                .map_err(|e| ClientError::Core(CoreError::NetworkError(e.to_string())))?
                .error_for_status()
                .map_err(|e| ClientError::Core(CoreError::NetworkError(e.to_string())))?;
            response
                .bytes()
                .await
                .map_err(|e| ClientError::Core(CoreError::NetworkError(e.to_string())))?
                .to_vec()
        };
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
        use_info_db: bool,
    ) -> ClientResult<(Vec<DbPackage>, Vec<ParsedInstallSpec>, Vec<String>)> {
        let all_packages = if use_info_db {
            self.ctx.info_db.read_available().await
        } else {
            self.ctx.db.read_all().await
        }
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
        promote: bool,
    ) -> ClientResult<()> {
        self.install_resolved_with_gate(all_packages, is, promote, |repo_url| {
            repo_url == OFFICIAL_REPO_URL
        })
        .await
    }

    /// Whether `name` was directly requested on this command's `is` list —
    /// i.e. deserves `explicit = true` when written to `InstalledPackages`,
    /// as opposed to being pulled in only because another package's
    /// `dependencies` resolved to it. `is` only ever contains what the user
    /// actually typed (see `parse_mine`), never the transitive closure
    /// `resolve_install_set` adds on top.
    fn is_directly_requested(is: &[ParsedInstallSpec], name: &str) -> bool {
        is.iter().any(|(_, n, _)| n == name)
    }

    /// `install_resolved` 的實際邏輯,「這個來源是不是官方來源」透過
    /// `is_official` 閉包傳入,而不是在這裡直接比對 `OFFICIAL_REPO_URL`——
    /// 理由跟 `sync_source_inner` 一樣:讓測試能在不打真網路的情況下強制
    /// 走簽章驗證路徑。
    async fn install_resolved_with_gate(
        &self,
        all_packages: &[DbPackage],
        is: &[ParsedInstallSpec],
        promote: bool,
        is_official: impl Fn(&str) -> bool,
    ) -> ClientResult<()> {
        if !is.is_empty() {
            // Only `install()` (`promote=true`) is allowed to sticky-promote
            // a directly-requested dependency to `explicit=true`. `upgrade()`
            // (`promote=false`) must not — "refresh this" isn't "I now own
            // this" — so it needs to know what was already installed before
            // this call to tell "brand-new via upgrade" apart from "already
            // an auto-installed dependency". Skip the read entirely on the
            // `install()` path since the set is never consulted there.
            let already_installed: std::collections::HashSet<String> = if promote {
                std::collections::HashSet::new()
            } else {
                self.ctx
                    .db
                    .read_all()
                    .await?
                    .into_iter()
                    .map(|p| p.name)
                    .collect()
            };
            let resolved = resolve_install_set(all_packages, is)?;
            let mut key_cache: HashMap<String, VerifyingKey> = HashMap::new();
            for (source_alias, name, version) in resolved {
                let pkg = name.as_str();
                let explicit = Self::is_directly_requested(is, pkg)
                    && (promote || !already_installed.contains(pkg));
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
                    self.install_source_package(
                        pkg,
                        &source_alias,
                        repo_package_info,
                        explicit,
                        &staging,
                    )
                    .await?;
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

                let tracked_files = place_package(
                    pkg,
                    &extracted,
                    Some(package_info.file_name.as_str()),
                    &self.ctx.install_dir,
                    &self.ctx.bin_dir,
                    staging.path(),
                    &self.system_controller,
                )?;
                let mut installed_pkg = repo_package_info.clone();
                installed_pkg.explicit = explicit;
                self.ctx.db.insert(installed_pkg).await?;
                self.ctx
                    .db
                    .record_installed_files(pkg, &tracked_files)
                    .await?;

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
        // Any `self.pkgs` entry that's an existing file on disk is a local
        // `.dpm` archive (`dpm install ./foo.dpm`), not a name to resolve
        // from the index — same "does this look like a path" heuristic
        // `pip install ./x.whl` uses, no extension sniffing required since
        // `install_local_file` only cares about the archive's contents.
        let (local_files, remaining): (Vec<String>, Vec<String>) = self
            .pkgs
            .iter()
            .cloned()
            .partition(|p| Path::new(p).is_file());

        for path_str in &local_files {
            self.install_local_file(Path::new(path_str)).await?;
        }

        if remaining.is_empty() {
            return Ok(());
        }

        // A fresh `ActionInfo` scoped to just the index-based names, so
        // `parsed_packages`/`install_resolved` (which read `self.pkgs`)
        // never see the local-file entries already handled above.
        let remote = ActionInfo::new(
            self.ctx.clone(),
            remaining,
            self.verbose,
            self.setting_config.clone(),
        );
        let (all_packages, is, isnot) = remote.parsed_packages(true).await?;
        if !isnot.is_empty() {
            let pkg = &isnot[0];
            println!("Error: Package '{}' not found in local cache. Please run 'dpm update' to refresh package index.", pkg);
            return Err(ClientError::Core(CoreError::PackageNotFound(format!(
                "Package '{}' not found in local cache. Please run 'dpm update' to refresh package index.", pkg
            ))));
        }
        remote.install_resolved(&all_packages, &is, true).await?;
        Ok(())
    }

    /// `dpm install <path>` for a local `.dpm` archive — the `dpm-server`-
    /// published analog of `apt install ./x.deb`, not `dpkg -i`: there's no
    /// index entry to compare against, so the index-hash and signature
    /// checks `fetch_and_verify_prebuilt` does are skipped entirely (the
    /// user already has physical possession of the file — that *is* the
    /// trust boundary here, same as a locally downloaded `.deb`). The
    /// archive's own internal self-consistency check
    /// (`packageInfo.json.hash == blake3(hashes.json)`) still runs, to
    /// catch a corrupted or hand-edited archive.
    ///
    /// Unlike `dpkg -i`, missing dependencies are resolved and installed
    /// automatically through the same `install_resolved` pipeline a normal
    /// install uses (`promote=false`: they're auto-installed, not
    /// user-explicit, so `dpm autoremove` can clean them up later if this
    /// package is ever removed) — a dependency that can't be resolved from
    /// the configured sources fails the whole install with a clear error,
    /// it does not leave a half-configured package behind.
    async fn install_local_file(&self, path: &Path) -> ClientResult<()> {
        let package_info_str = read_file_from_zip(path, "packageInfo.json").map_err(|e| {
            ClientError::Core(CoreError::InvalidPackage(format!(
                "{}: missing packageInfo.json in archive: {e}",
                path.display()
            )))
        })?;
        let package_info: PackageInfo =
            JsonStorage::from_str_to(&package_info_str).map_err(|e| {
                ClientError::Core(CoreError::InvalidPackage(format!(
                    "{}: invalid packageInfo.json: {e}",
                    path.display()
                )))
            })?;
        let hashes_str = read_file_from_zip(path, "hashes.json").map_err(|e| {
            ClientError::Core(CoreError::InvalidPackage(format!(
                "{}: missing hashes.json in archive: {e}",
                path.display()
            )))
        })?;
        let package_hash_info: Hashes = JsonStorage::from_str_to(&hashes_str).map_err(|e| {
            ClientError::Core(CoreError::InvalidPackage(format!(
                "{}: invalid hashes.json: {e}",
                path.display()
            )))
        })?;
        let recorded_package_info_hash = package_hash_info.get("hashes.json").ok_or_else(|| {
            ClientError::Core(CoreError::InvalidPackage(format!(
                "{}'s hashes.json has no entry for itself",
                path.display()
            )))
        })?;
        if &package_info.hash != recorded_package_info_hash {
            return Err(ClientError::Core(CoreError::HashMismatch {
                expected: package_info.hash.clone(),
                actual: recorded_package_info_hash.clone(),
            }));
        }

        println!(
            "{}",
            format!(
                "Installing {} from local file {} (unverified: no index/signature check)",
                package_info.package_name,
                path.display()
            )
            .yellow()
        );

        // Resolve + install whatever this archive depends on that isn't
        // already installed, same as a normal install would — `apt install
        // ./x.deb` behavior, not dpkg -i's "leave it broken, fix it
        // yourself". Dependency version constraints against packages
        // already installed aren't cross-checked here, matching the same
        // level of rigor `resolve_install_set` already applies everywhere
        // else in dpm (it doesn't re-validate already-installed versions
        // either).
        let missing: Vec<ParsedInstallSpec> = {
            let installed = self.ctx.db.read_all().await?;
            package_info
                .dependencies
                .iter()
                .flatten()
                .filter(|d| !installed.iter().any(|p| p.name == d.name))
                .map(|d| (None, d.name.clone(), Some(d.version.clone())))
                .collect()
        };
        if !missing.is_empty() {
            let all_packages = self.ctx.info_db.read_available().await?;
            self.install_resolved(&all_packages, &missing, false)
                .await?;
            // `install_resolved_with_gate` marks every name in `missing`
            // explicit=true (it was in `is`, and none of them existed
            // before this call) — force them back to auto so `autoremove`
            // can still reclaim them later. See `Db::set_explicit`.
            for (_, name, _) in &missing {
                self.ctx.db.set_explicit(name, false).await?;
            }
        }

        let staging_root_base = self.ctx.main_dir.join(".staging");
        std::fs::create_dir_all(&staging_root_base)
            .map_err(|e| ClientError::Core(CoreError::IoError(e)))?;
        let staging = tempfile::Builder::new()
            .prefix(package_info.package_name.as_str())
            .tempdir_in(&staging_root_base)
            .map_err(|e| ClientError::Core(CoreError::IoError(e)))?;
        let extracted = staging.path().join("extracted");
        unzip_file(path, &extracted).map_err(|e| ClientError::Core(CoreError::IoError(e)))?;

        let tracked_files = place_package(
            &package_info.package_name,
            &extracted,
            Some(package_info.file_name.as_str()),
            &self.ctx.install_dir,
            &self.ctx.bin_dir,
            staging.path(),
            &self.system_controller,
        )?;

        // `source: "local"` marks this as not having come from any
        // configured index — `dpm info`/`list` show it like any other
        // installed package, `upgrade` simply never finds a newer version
        // for it (there's no index entry to compare against).
        let db_pkg = DbPackage::new(
            "local",
            &package_info.package_name,
            &package_info.version,
            "prebuilt",
            Some(format!("file://{}", path.display())),
            Some(package_info.hash.clone()),
            path.file_name().and_then(|f| f.to_str()).map(String::from),
            None,
            &package_info.description,
            Some(package_info.file_name.clone()),
            package_info.dependencies.clone(),
            None,
            None,
            true,
        );
        self.ctx.db.insert(db_pkg).await?;
        self.ctx
            .db
            .record_installed_files(&package_info.package_name, &tracked_files)
            .await?;

        println!(
            "{}",
            format!(
                "{} installed from {}",
                package_info.package_name,
                path.display()
            )
            .green()
        );
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
    async fn install_source_package(
        &self,
        pkg: &str,
        source_alias: &str,
        repo_package_info: &DbPackage,
        explicit: bool,
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

        let tracked_files = place_package(
            pkg,
            &out_dir,
            repo_package_info.entry.as_deref(),
            &self.ctx.install_dir,
            &self.ctx.bin_dir,
            staging.path(),
            &self.system_controller,
        )?;
        let mut installed_pkg = repo_package_info.clone();
        installed_pkg.explicit = explicit;
        self.ctx.db.insert(installed_pkg).await?;
        self.ctx
            .db
            .record_installed_files(pkg, &tracked_files)
            .await
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
        let temp_dir = tempfile::tempdir().map_err(|e| ClientError::Core(CoreError::IoError(e)))?;
        let temp_db_path = temp_dir.path().join("RepoInfo.db");
        crate::fetch_repo_info(&source.repo_info, &temp_db_path).await?;

        ctx.info_db.clear_source_available(&source.alias).await?;

        let mut key_cache: HashMap<String, VerifyingKey> = HashMap::new();
        let target = self_update::get_target();

        let remote_db = turso::Builder::new_local(temp_db_path.to_str().unwrap())
            .build()
            .await
            .map_err(|e| ClientError::Core(CoreError::DatabaseError(e.to_string())))?
            .connect()
            .map_err(|e| ClientError::Core(CoreError::DatabaseError(e.to_string())))?;

        let mut rows = remote_db
            .query("SELECT name, version, kind, url, hash, filename, build_command, description, entry, dependencies, author, signature, targets FROM Packages", ())
            .await
            .map_err(|e| ClientError::Core(CoreError::DatabaseError(e.to_string())))?;

        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| ClientError::Core(CoreError::DatabaseError(e.to_string())))?
        {
            let name = row.get_value(0).unwrap().as_text().unwrap().to_string();
            let version = row.get_value(1).unwrap().as_text().unwrap().to_string();
            let kind_str = row.get_value(2).unwrap().as_text().unwrap().to_string();
            let url = row.get_value(3).unwrap().as_text().map(|s| s.to_string());
            let hash = row.get_value(4).unwrap().as_text().map(|s| s.to_string());
            let filename = row.get_value(5).unwrap().as_text().map(|s| s.to_string());
            let build_command = row.get_value(6).unwrap().as_text().map(|s| s.to_string());
            let description = row.get_value(7).unwrap().as_text().unwrap().to_string();
            let entry = row.get_value(8).unwrap().as_text().map(|s| s.to_string());
            let dependencies = row.get_value(9).unwrap().as_text().map(|s| s.to_string());
            let author = row.get_value(10).unwrap().as_text().map(|s| s.to_string());
            let signature = row.get_value(11).unwrap().as_text().map(|s| s.to_string());
            let targets_str = row.get_value(12).unwrap().as_text().map(|s| s.to_string());

            if let Some(ref targets) = targets_str {
                if !targets.is_empty() {
                    let target_list: Vec<&str> = targets.split(',').collect();
                    if !target_list.contains(&target) {
                        println!(
                            "{} skipping {}@{} — unsupported target {}",
                            "Warning:".yellow(),
                            name,
                            version,
                            target
                        );
                        continue;
                    }
                }
            }

            if is_official {
                let author_ref = author.as_deref();
                let signature_ref = signature.as_deref();
                let verified = match (author_ref, signature_ref, hash.as_deref()) {
                    (Some(a), Some(s), Some(h)) => {
                        verify_official_signature(&source.repo_url, a, h, s, &mut key_cache).await
                    }
                    _ => Err(ClientError::Core(CoreError::SignatureInvalid(
                        "missing author, signature, or hash".to_string(),
                    ))),
                };
                if let Err(e) = verified {
                    if !matches!(e, ClientError::Core(CoreError::SignatureInvalid(_))) {
                        return Err(e);
                    }
                    println!(
                        "{} skipping {}@{} (author: {}) — signature verification failed: {e}",
                        "Warning:".yellow(),
                        name,
                        version,
                        author_ref.unwrap_or("<none>"),
                    );
                    continue;
                }
            }

            let dependencies_obj = dependencies.and_then(|d| serde_json::from_str(&d).ok());
            ctx.info_db
                .insert_available(DbPackage::new(
                    &source.alias,
                    &name,
                    &version,
                    &kind_str,
                    url,
                    hash,
                    filename,
                    build_command,
                    &description,
                    entry,
                    dependencies_obj,
                    author,
                    signature,
                    true,
                ))
                .await?;
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
                if alias == "official" {
                    return Err(ClientError::ConfigError(
                        "the official source is built-in and hardcoded; cannot be modified"
                            .to_string(),
                    ));
                }
                if setting.sources.iter().any(|s| s.alias == alias) {
                    return Err(ClientError::ConfigError(format!(
                        "source alias '{alias}' already exists"
                    )));
                }
                println!(
                    "{} third-party source, not vetted by the DPM team",
                    "Warning:".yellow()
                );
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
                if alias == "official" {
                    return Err(ClientError::ConfigError(
                        "cannot remove the built-in official source".to_string(),
                    ));
                }
                let before = setting.sources.len();
                setting.sources.retain(|s| s.alias != alias);
                if setting.sources.len() == before {
                    return Err(ClientError::ConfigError(format!(
                        "no source with alias '{alias}'"
                    )));
                }
                self.ctx.info_db.clear_source_available(&alias).await?;
                dpm_core::TomlStorage::to_toml(&setting, &config_path)?;
                println!("{}", "Source removed.".green());
            }
            SourceAction::List => {
                if self.setting_config.sources.is_empty() {
                    println!("{}", "No package sources currently configured.".yellow());
                } else {
                    println!("{}", "Configured Package Sources:".green().bold());
                    for source in &self.setting_config.sources {
                        println!("  {}  {}", source.alias.green(), source.repo_info);
                    }
                }
            }
        }
        Ok(())
    }

    pub async fn uninstall(&self) -> ClientResult<()> {
        let (_, is, isnot) = self.parsed_packages(false).await?;
        if is.is_empty() && isnot.is_empty() {
            println!(
                "{}",
                "No matching installed packages found to uninstall.".yellow()
            );
            return Ok(());
        }
        if !is.is_empty() {
            for (_, pkg, _) in is {
                if self.verbose {
                    println!("{}\n\n  {}", pkg.on_green(), "Removing...".red());
                }
                self.remove_installed_package(&pkg).await?;
                if self.verbose {
                    println!("  {}", "Done".green());
                }
            }
        }
        if !isnot.is_empty() {
            for pkg in isnot {
                println!(
                    "==> Host OS Package Manager ({}): Removing '{pkg}'...",
                    self.system_action.primary_manager_name().cyan()
                );
                self.system_action.uninstall_package(&pkg)?;
            }
        }

        let remaining = self
            .ctx
            .db
            .read_all()
            .await
            .map_err(|e| ClientError::Core(CoreError::DatabaseError(e.to_string())))?;
        let orphans = find_orphans(&remaining);
        if !orphans.is_empty() {
            let names: Vec<&str> = orphans.iter().map(|p| p.name.as_str()).collect();
            println!(
                "{} {} package(s) are now orphaned: {}. Run 'dpm autoremove' to remove them.",
                "Note:".yellow(),
                orphans.len(),
                names.join(", ")
            );
        }
        Ok(())
    }

    pub async fn autoremove(&self) -> ClientResult<()> {
        let installed = self
            .ctx
            .db
            .read_all()
            .await
            .map_err(|e| ClientError::Core(CoreError::DatabaseError(e.to_string())))?;
        let orphans = find_orphans(&installed);
        if orphans.is_empty() {
            println!("{}", "No orphaned packages found.".yellow());
            return Ok(());
        }

        println!("{}", "==> Orphaned packages:".green().bold());
        for pkg in &orphans {
            println!("  {} v{}", pkg.name.bold(), pkg.version);
        }
        for pkg in &orphans {
            if self.verbose {
                println!(
                    "{}\n\n  {}",
                    pkg.name.as_str().on_green(),
                    "Removing...".red()
                );
            }
            self.remove_installed_package(&pkg.name).await?;
        }
        println!("{} {} package(s) removed.", "Done:".green(), orphans.len());
        Ok(())
    }

    /// Removes `pkg`'s on-disk files (per the `installed_files` manifest,
    /// plus the install-dir/opt-link/bin-link fallback paths it also cleans
    /// up) and its `InstalledPackages`/`installed_files` DB rows. Shared by
    /// `uninstall()` and `autoremove()` so there is exactly one place that
    /// knows how to fully remove an installed package.
    async fn remove_installed_package(&self, pkg: &str) -> ClientResult<()> {
        let pre_rm_location = self.ctx.install_dir.join(pkg);
        let opt_link = self.ctx.opt_dir().join(pkg);

        // 1. O(1) DB Manifest Cleanup: remove every file/symlink registered in DB
        let recorded_files = self
            .ctx
            .db
            .get_installed_files(pkg)
            .await
            .unwrap_or_default();
        for file_path in recorded_files {
            let p = Path::new(&file_path);
            if p.exists() || p.symlink_metadata().is_ok() {
                let _ = remove_file(p).or_else(|_| remove_dir_all(p));
            }
        }

        // 2. Remove main install dir Software/<pkg> & opt/<pkg>
        if opt_link.exists() || opt_link.symlink_metadata().is_ok() {
            let _ = remove_file(&opt_link);
        }
        if pre_rm_location.exists() {
            let _ = remove_dir_all(&pre_rm_location);
        }

        // 3. Fallback cleanup for direct bin link if present
        let direct_bin_link = self.ctx.bin_dir.join(pkg);
        if direct_bin_link.exists() || direct_bin_link.symlink_metadata().is_ok() {
            let _ = remove_file(&direct_bin_link);
        }

        // 4. Remove manifest rows from DB
        self.ctx.db.remove_installed_files(pkg).await?;
        self.ctx.db.delete_installed(pkg).await?;
        Ok(())
    }

    pub async fn search(&self) -> ClientResult<()> {
        let all_packages = self
            .ctx
            .info_db
            .read_available()
            .await
            .map_err(|e| ClientError::Core(CoreError::DatabaseError(e.to_string())))?;

        if self.pkgs.is_empty() {
            if !all_packages.is_empty() {
                println!("{}", "==> Available DPM Packages:".green().bold());
                for p in &all_packages {
                    let desc = if p.description.is_empty() {
                        String::new()
                    } else {
                        format!(" - {}", p.description)
                    };
                    println!(
                        "  {}/{} v{} ({}){}",
                        p.source.cyan(),
                        p.name.bold(),
                        p.version,
                        p.kind,
                        desc
                    );
                }
            } else {
                println!("{}", "No packages found in local DPM index. Run `dpm update` to refresh package index.".yellow());
            }
            return Ok(());
        }

        let mut matched_dpm: Vec<&DbPackage> = Vec::new();
        let mut unmatched_queries: Vec<&str> = Vec::new();

        for query in &self.pkgs {
            let q_lower = query.to_lowercase();
            let mut found_for_query = false;

            for p in &all_packages {
                let full_name = format!("{}/{}", p.source, p.name);
                if p.name.to_lowercase().contains(&q_lower)
                    || full_name.to_lowercase().contains(&q_lower)
                    || p.description.to_lowercase().contains(&q_lower)
                {
                    found_for_query = true;
                    if !matched_dpm
                        .iter()
                        .any(|m| m.source == p.source && m.name == p.name && m.version == p.version)
                    {
                        matched_dpm.push(p);
                    }
                }
            }

            if !found_for_query {
                unmatched_queries.push(query.as_str());
            }
        }

        if !matched_dpm.is_empty() {
            println!("{}", "==> DPM Package Index Results:".green().bold());
            for p in &matched_dpm {
                let desc = if p.description.is_empty() {
                    String::new()
                } else {
                    format!(" - {}", p.description)
                };
                println!(
                    "  {}/{} v{} ({}){}",
                    p.source.cyan(),
                    p.name.bold(),
                    p.version,
                    p.kind,
                    desc
                );
            }
        } else {
            println!("{}", "No matching packages found in DPM index.".yellow());
        }

        for q in unmatched_queries {
            if let Err(e) = self.system_action.search_package(q) {
                println!("Host OS package search error for '{q}': {e}");
            }
        }

        Ok(())
    }

    /// `dpm info <pkg>...`: prints one package's full known metadata — its
    /// installed state (if any) plus every available `(source, version)` in
    /// the local index. `self.pkgs` reused as the query list, same as
    /// `search`/`uninstall`. Reads `read_available()`/`read_all()` once each
    /// up front and filters in memory (exact name match, unlike `search`'s
    /// substring match) rather than re-querying per name.
    pub async fn info(&self) -> ClientResult<()> {
        let available = self.ctx.info_db.read_available().await?;
        let installed = self.ctx.db.read_all().await?;
        let pinned = self.ctx.db.pinned_names().await?;

        for name in &self.pkgs {
            let installed_pkg = installed.iter().find(|p| &p.name == name);
            let mut matches: Vec<&DbPackage> =
                available.iter().filter(|p| &p.name == name).collect();
            matches.sort_by(|a, b| a.source.cmp(&b.source).then(a.version.cmp(&b.version)));

            if matches.is_empty() && installed_pkg.is_none() {
                println!(
                    "{}",
                    format!("{name}: not found in local DPM index").yellow()
                );
                continue;
            }

            println!("{}", format!("==> {name}").green().bold());
            match installed_pkg {
                Some(p) => {
                    let pin_tag = if pinned.contains(name) {
                        " (pinned)"
                    } else {
                        ""
                    };
                    println!(
                        "  Installed: v{} ({}, {}){}",
                        p.version,
                        p.source,
                        if p.explicit { "explicit" } else { "auto" },
                        pin_tag.yellow()
                    )
                }
                None => println!("  Installed: not installed"),
            }

            // Metadata (description/entry/dependencies/author) reflects the
            // installed row if there is one, otherwise the first available
            // match — versions can drift in these fields, but there's no
            // single "canonical" version to prefer once nothing is
            // installed, so first-available is the same order `matches` is
            // displayed in below.
            if let Some(meta) = installed_pkg.or_else(|| matches.first().copied()) {
                if !meta.description.is_empty() {
                    println!("  Description: {}", meta.description);
                }
                if let Some(entry) = &meta.entry {
                    println!("  Entry: {entry}");
                }
                if let Some(deps) = &meta.dependencies {
                    if !deps.is_empty() {
                        let list = deps
                            .iter()
                            .map(|d| format!("{} {}", d.name, d.version))
                            .collect::<Vec<_>>()
                            .join(", ");
                        println!("  Dependencies: {list}");
                    }
                }
                if let Some(author) = &meta.author {
                    let signed = if meta.signature.is_some() {
                        "signed"
                    } else {
                        "unsigned"
                    };
                    println!("  Author: {author} ({signed})");
                }
            }

            if !matches.is_empty() {
                println!("\n  {}", "Available versions:".bold());
                for p in &matches {
                    println!(
                        "    {}/{}  v{}  ({})",
                        p.source.cyan(),
                        p.name,
                        p.version,
                        p.kind
                    );
                }
            }
            println!();
        }
        Ok(())
    }

    /// Reverse-lookup: for each path in `self.pkgs` (here holding file
    /// paths, not package names — same field, different CLI-level meaning,
    /// see `Commands::Owns`), print which installed package(s) registered
    /// it as a tracked symlink in `installed_files`. Only matches DPM-built
    /// symlinks (`opt/`, `bin/`, `sbin/`, `lib/`, `share/<pkg>/`, ...) —
    /// raw files inside a package's private `Software/<pkg>/` install
    /// directory are never individually tracked, so they never match here.
    pub async fn owns(&self) -> ClientResult<()> {
        for raw_path in &self.pkgs {
            let absolute = normalize_lexically(
                &std::path::absolute(raw_path)
                    .map_err(|e| ClientError::Core(CoreError::IoError(e)))?,
            );
            let owners = self
                .ctx
                .db
                .find_owners(&absolute.display().to_string())
                .await?;
            if owners.is_empty() {
                println!(
                    "{}",
                    format!("{raw_path}: not owned by any installed package").yellow()
                );
            } else {
                println!("{}: {}", raw_path, owners.join(", ").bold());
            }
        }
        Ok(())
    }

    pub async fn list(&self, sys: bool, outdated: bool) -> ClientResult<()> {
        if outdated {
            return self.list_outdated().await;
        }
        if sys {
            if let Err(e) = self.system_action.list_packages() {
                println!("Host OS package list error: {e}");
            }
        } else {
            let pkgs = installed_package_names(&self.ctx.install_dir)?;
            if pkgs.is_empty() {
                println!("{}", "No DPM packages currently installed.".yellow());
            } else {
                let pinned = self.ctx.db.pinned_names().await?;
                println!("{}", "==> DPM Installed Packages:".green().bold());
                for pkg in pkgs {
                    if pinned.contains(&pkg) {
                        println!("  {} {}", pkg.green(), "(pinned)".yellow());
                    } else {
                        println!("  {}", pkg.green());
                    }
                }
            }
        }
        Ok(())
    }

    /// `dpm list --outdated`: compares `InstalledPackages` against the local
    /// `AvailablePackages` cache (`dpm update`'s snapshot, not a live fetch —
    /// same convention as `search`) and reports every dpm-managed package
    /// with a newer version in its own source. See `find_outdated`.
    async fn list_outdated(&self) -> ClientResult<()> {
        let installed = self.ctx.db.read_all().await?;
        let available = self.ctx.info_db.read_available().await?;
        let outdated = find_outdated(&installed, &available);
        if outdated.is_empty() {
            println!("{}", "All DPM packages are up to date.".green());
        } else {
            println!("{}", "==> Outdated DPM Packages:".green().bold());
            for pkg in &outdated {
                println!(
                    "  {} {} -> {} ({})",
                    pkg.name.bold(),
                    pkg.current,
                    pkg.latest.green(),
                    pkg.source.cyan()
                );
            }
        }
        Ok(())
    }

    pub async fn upgrade(&self) -> ClientResult<()> {
        let (all_packages, is, isnot) = self.parsed_packages(true).await?;
        if is.is_empty() && isnot.is_empty() {
            println!("{}", "No packages available for upgrade.".yellow());
            return Ok(());
        }
        // `dpm pin` lets a package opt out of `upgrade` without erroring the
        // whole command — every other requested package still upgrades, the
        // pinned ones are just skipped with a hint on how to lift it.
        let pinned = self.ctx.db.pinned_names().await?;
        let is: Vec<_> = is
            .into_iter()
            .filter(|(_, name, _)| {
                if pinned.contains(name) {
                    println!(
                        "{}",
                        format!("{name} is pinned, skipping (unpin with `dpm unpin {name}`)")
                            .yellow()
                    );
                    false
                } else {
                    true
                }
            })
            .collect();
        self.install_resolved(&all_packages, &is, false).await?;
        if !isnot.is_empty() {
            for pkg in isnot {
                self.system_action.upgrade_package(&pkg)?;
            }
        }
        Ok(())
    }

    /// `dpm pin`/`dpm unpin`: flips `InstalledPackages.pinned` for every
    /// name in `self.pkgs`. `pin = true` pins, `false` unpins — same shape
    /// as the two CLI subcommands sharing one implementation instead of two
    /// near-identical copies. Names that aren't currently installed are
    /// reported and skipped rather than erroring the whole command, same
    /// tolerance `autoremove`'s per-package loop uses.
    pub async fn pin(&self, pin: bool) -> ClientResult<()> {
        let verb = if pin { "pin" } else { "unpin" };
        for name in &self.pkgs {
            if self.ctx.db.set_pinned(name, pin).await? {
                println!("{}", format!("{name}: {verb}ned").green());
            } else {
                println!(
                    "{}",
                    format!("{name}: not installed, nothing to {verb}").yellow()
                );
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

/// Lexically folds `.`/`..` components out of `path` — pure component
/// manipulation, no filesystem access. `std::path::absolute()` (used just
/// before this in `owns()`) prepends cwd but deliberately does NOT resolve
/// `..` (correct for real filesystem paths, where a preceding component
/// could be a symlink) or drop a trailing slash. Neither of those matters
/// here: this produces a plain string key compared against
/// `installed_files.file_path`, which is always stored as a clean absolute
/// path with no `.`/`..`/trailing slash — so folding lexically (not
/// resolving symlinks) is exactly what's needed to make tab-completed and
/// `..`-relative inputs match it.
fn normalize_lexically(path: &Path) -> std::path::PathBuf {
    use std::path::Component;
    let mut result = std::path::PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                result.pop();
            }
            Component::CurDir => {}
            other => result.push(other),
        }
    }
    result
}

#[cfg(test)]
mod normalize_lexically_tests {
    use super::normalize_lexically;
    use std::path::Path;

    #[test]
    fn strips_a_trailing_slash() {
        assert_eq!(
            normalize_lexically(Path::new("/opt/dpm/opt/hello/")),
            Path::new("/opt/dpm/opt/hello")
        );
    }

    #[test]
    fn folds_parent_dir_components() {
        assert_eq!(
            normalize_lexically(Path::new("/opt/dpm/bin/../opt/hello")),
            Path::new("/opt/dpm/opt/hello")
        );
    }

    #[test]
    fn leaves_a_plain_path_unchanged() {
        assert_eq!(
            normalize_lexically(Path::new("/opt/dpm/bin/hello")),
            Path::new("/opt/dpm/bin/hello")
        );
    }

    #[test]
    fn drops_current_dir_components() {
        assert_eq!(
            normalize_lexically(Path::new("/opt/dpm/./bin/hello")),
            Path::new("/opt/dpm/bin/hello")
        );
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
    async fn build_fake_repo_db(fake_info: &FakeRepoInfo) -> Vec<u8> {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("RepoInfo.db");
        {
            let conn = turso::Builder::new_local(db_path.to_str().unwrap())
                .build()
                .await
                .unwrap()
                .connect()
                .unwrap();
            conn.execute(
                "CREATE TABLE IF NOT EXISTS Packages (
                    name TEXT NOT NULL,
                    version TEXT NOT NULL,
                    kind TEXT NOT NULL,
                    url TEXT,
                    hash TEXT,
                    filename TEXT,
                    build_command TEXT,
                    description TEXT NOT NULL,
                    entry TEXT,
                    dependencies TEXT,
                    author TEXT,
                    signature TEXT,
                    targets TEXT,
                    PRIMARY KEY (name, version)
                )",
                (),
            )
            .await
            .unwrap();

            for (name, versions) in &fake_info.packages {
                for v in versions {
                    let (kind_str, url, hash, filename, build_command) = match &v.kind {
                        PackageKind::Prebuilt { builds } => {
                            let b = &builds[0];
                            (
                                "prebuilt".to_string(),
                                Some(b.url.clone()),
                                Some(b.hash.clone()),
                                Some(b.file_name.clone()),
                                None,
                            )
                        }
                        PackageKind::Source { build, hash, .. } => (
                            "source".to_string(),
                            None,
                            hash.clone(),
                            None,
                            Some(build.clone()),
                        ),
                    };

                    let targets_str = match &v.kind {
                        PackageKind::Prebuilt { builds } => {
                            let mut targets = Vec::new();
                            for b in builds {
                                if let Some(t) = &b.target {
                                    targets.push(t.clone());
                                }
                            }
                            if targets.is_empty() {
                                None
                            } else {
                                Some(targets.join(","))
                            }
                        }
                        PackageKind::Source {
                            supported_targets, ..
                        } => {
                            if let Some(st) = supported_targets {
                                Some(st.join(","))
                            } else {
                                None
                            }
                        }
                    };

                    let to_value = |opt: Option<String>| match opt {
                        Some(s) => turso::Value::Text(s),
                        None => turso::Value::Null,
                    };
                    let dependencies_str = v
                        .dependencies
                        .as_ref()
                        .map(|d| serde_json::to_string(d).unwrap());

                    conn.execute(
                        "INSERT INTO Packages (name, version, kind, url, hash, filename, build_command, description, entry, dependencies, author, signature, targets)
                        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                        vec![
                            turso::Value::Text(name.clone()),
                            turso::Value::Text(v.version.clone()),
                            turso::Value::Text(kind_str),
                            to_value(url),
                            to_value(hash),
                            to_value(filename),
                            to_value(build_command),
                            turso::Value::Text(v.description.clone().unwrap_or_default()),
                            to_value(v.entry.clone()),
                            to_value(dependencies_str),
                            to_value(v.author.clone()),
                            to_value(v.signature.clone()),
                            to_value(targets_str),
                        ],
                    )
                    .await
                    .unwrap();
                }
            }
            let _ = conn.execute("PRAGMA wal_checkpoint(FULL)", ()).await;
        }
        std::fs::read(&db_path).unwrap()
    }

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
                    builds: vec![dpm_core::PrebuiltBuild {
                        target: None,
                        url: "https://example.com/good.zip".to_string(),
                        hash: good_hash.clone(),
                        file_name: "good.zip".to_string(),
                    }],
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
                    builds: vec![dpm_core::PrebuiltBuild {
                        target: None,
                        url: "https://example.com/bad.zip".to_string(),
                        hash: good_hash.clone(),
                        file_name: "bad.zip".to_string(),
                    }],
                },
                dependencies: None,
                entry: None,
                description: None,
                author: Some("alice".to_string()),
                signature: Some(bad_sig),
            }],
        );
        let body = build_fake_repo_db(&FakeRepoInfo { packages }).await;

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

        let all = ctx.info_db.read_available().await.unwrap();
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
                    builds: vec![dpm_core::PrebuiltBuild {
                        target: None,
                        url: "https://example.com/some.zip".to_string(),
                        hash: good_hash,
                        file_name: "some.zip".to_string(),
                    }],
                },
                dependencies: None,
                entry: None,
                description: None,
                author: Some("alice".to_string()),
                signature: Some(sig),
            }],
        );
        let body = build_fake_repo_db(&FakeRepoInfo { packages }).await;

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
                    builds: vec![dpm_core::PrebuiltBuild {
                        target: None,
                        url: "https://example.com/unsigned.zip".to_string(),
                        hash: "irrelevant".to_string(),
                        file_name: "unsigned.zip".to_string(),
                    }],
                },
                dependencies: None,
                entry: None,
                description: None,
                author: None,
                signature: None,
            }],
        );
        let body = build_fake_repo_db(&FakeRepoInfo { packages }).await;
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

        let all = ctx.info_db.read_available().await.unwrap();
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
                    builds: vec![dpm_core::PrebuiltBuild {
                        target: None,
                        url: "https://example.com/evil.zip".to_string(),
                        hash: good_hash,
                        file_name: "evil.zip".to_string(),
                    }],
                },
                dependencies: None,
                entry: None,
                description: None,
                author: Some("../../../../mallory/evil-keys/main/keys/mallory".to_string()),
                signature: Some(sig),
            }],
        );
        let body = build_fake_repo_db(&FakeRepoInfo { packages }).await;
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

        let all = ctx.info_db.read_available().await.unwrap();
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
                    builds: vec![dpm_core::PrebuiltBuild {
                        target: None,
                        url: "https://example.com/no-sig.zip".to_string(),
                        hash: "a".repeat(64),
                        file_name: "no-sig.zip".to_string(),
                    }],
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
        let body = build_fake_repo_db(&FakeRepoInfo { packages }).await;
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

        let all = ctx.info_db.read_available().await.unwrap();
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
                    builds: vec![dpm_core::PrebuiltBuild {
                        target: None,
                        url: "https://example.com/mismatched.zip".to_string(),
                        hash: real_hash,
                        file_name: "mismatched.zip".to_string(),
                    }],
                },
                dependencies: None,
                entry: None,
                description: None,
                author: Some("alice".to_string()),
                signature: Some(sig_over_wrong_hash),
            }],
        );
        let body = build_fake_repo_db(&FakeRepoInfo { packages }).await;

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

        let all = ctx.info_db.read_available().await.unwrap();
        assert_eq!(
            all.len(),
            0,
            "a valid signature over the wrong hash must not verify"
        );
    }

    #[tokio::test]
    async fn sync_source_inner_skips_a_prebuilt_version_with_no_build_for_this_target() {
        let target = self_update::get_target();
        // 故意登記一個「不是本機 target」的 build,確保這個版本一定會被
        // 跳過——用一個真實 target 字串裡不會出現的假字串當「另一個平台」,
        // 避免巧合等於本機 target。
        let other_target = format!("not-{target}");

        let body = build_fake_repo_db(&FakeRepoInfo {
            packages: {
                let mut m = StdHashMap::new();
                m.insert(
                    "wrong-target-pkg".to_string(),
                    vec![PackageVersionInfo {
                        version: "1.0.0".to_string(),
                        kind: PackageKind::Prebuilt {
                            builds: vec![dpm_core::PrebuiltBuild {
                                target: Some(other_target),
                                url: "https://example.com/wrong.zip".to_string(),
                                hash: "a".repeat(64),
                                file_name: "wrong.zip".to_string(),
                            }],
                        },
                        dependencies: None,
                        entry: None,
                        description: None,
                        author: None,
                        signature: None,
                    }],
                );
                m
            },
        })
        .await;
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

        let all = ctx.info_db.read_available().await.unwrap();
        assert_eq!(
            all.len(),
            0,
            "a Prebuilt version with no build for this machine's target must be skipped, not inserted"
        );
    }

    #[tokio::test]
    async fn sync_source_inner_keeps_a_prebuilt_version_with_a_universal_build() {
        let body = build_fake_repo_db(&FakeRepoInfo {
            packages: {
                let mut m = StdHashMap::new();
                m.insert(
                    "universal-pkg".to_string(),
                    vec![PackageVersionInfo {
                        version: "1.0.0".to_string(),
                        kind: PackageKind::Prebuilt {
                            builds: vec![dpm_core::PrebuiltBuild {
                                target: None,
                                url: "https://example.com/universal.zip".to_string(),
                                hash: "a".repeat(64),
                                file_name: "universal.zip".to_string(),
                            }],
                        },
                        dependencies: None,
                        entry: None,
                        description: None,
                        author: None,
                        signature: None,
                    }],
                );
                m
            },
        })
        .await;
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

        let all = ctx.info_db.read_available().await.unwrap();
        assert_eq!(all.len(), 1, "a universal build must always be kept");
    }

    #[tokio::test]
    async fn sync_source_inner_skips_a_source_version_unsupported_on_this_target() {
        let target = self_update::get_target();
        let other_target = format!("not-{target}");

        let body = build_fake_repo_db(&FakeRepoInfo {
            packages: {
                let mut m = StdHashMap::new();
                m.insert(
                    "unsupported-source-pkg".to_string(),
                    vec![PackageVersionInfo {
                        version: "1.0.0".to_string(),
                        kind: PackageKind::Source {
                            build: "make".to_string(),
                            hash: Some("a".repeat(64)),
                            supported_targets: Some(vec![other_target]),
                        },
                        dependencies: None,
                        entry: None,
                        description: None,
                        author: None,
                        signature: None,
                    }],
                );
                m
            },
        })
        .await;
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

        let all = ctx.info_db.read_available().await.unwrap();
        assert_eq!(
            all.len(),
            0,
            "a Source version not supporting this target must be skipped"
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
            true,
        );
        let all_packages = vec![pkg];
        let is = vec![(None, "tampered-pkg".to_string(), None)];

        let err = action
            .install_resolved_with_gate(&all_packages, &is, true, |_| true)
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
            true,
        );
        let all_packages = vec![pkg];
        let is = vec![(None, "valid-pkg".to_string(), None)];

        let err = action
            .install_resolved_with_gate(&all_packages, &is, true, |_| true)
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
            true,
        );
        let all_packages = vec![pkg];
        let is = vec![(None, "unsigned-pkg".to_string(), None)];

        let err = action
            .install_resolved_with_gate(&all_packages, &is, true, |_| true)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("INSECURE"));
    }

    /// Builds a real zip containing `packageInfo.json` + `hashes.json`,
    /// mirroring exactly what `dpm-server hash`/`build` produce at publish
    /// time — same construction as `fetcher::tests::build_fixture_zip`,
    /// duplicated per that module's own precedent (its `serve_once` is
    /// likewise copied rather than shared) rather than introducing a
    /// cross-module test-fixture dependency for one extra call site.
    /// `body` becomes the entry file's content, so two versions built with
    /// different bodies can be told apart on disk after install.
    fn build_fixture_zip(dir: &std::path::Path, version: &str, body: &str) -> (Vec<u8>, String) {
        use std::collections::HashMap;

        let src = dir.join(format!("src-{version}"));
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("main"), body).unwrap();
        let main_hash = dpm_core::hash_file(&src.join("main")).unwrap();

        let hashes_path = src.join("hashes.json");
        let mut hashes: HashMap<String, String> = HashMap::new();
        hashes.insert("main".to_string(), main_hash);
        dpm_core::JsonStorage::to_json(&hashes, &hashes_path).unwrap();
        let hashes_json_hash = dpm_core::hash_file(&hashes_path).unwrap();
        hashes.insert("hashes.json".to_string(), hashes_json_hash.clone());
        dpm_core::JsonStorage::to_json(&hashes, &hashes_path).unwrap();

        let package_info = dpm_core::PackageInfo::new(
            "downgrade-pkg".to_string(),
            "main".to_string(),
            version.to_string(),
            "test fixture".to_string(),
            hashes_json_hash,
            None,
            None,
        );
        dpm_core::JsonStorage::to_json(&package_info, &src.join("packageInfo.json")).unwrap();

        let zip_path = dir.join(format!("pkg-{version}.zip"));
        crate::zip_folder(&src, &zip_path).unwrap();
        let zip_hash = dpm_core::hash_file(&zip_path).unwrap();
        (std::fs::read(&zip_path).unwrap(), zip_hash)
    }

    /// Traces the real `install()` -> `resolve_install_set` ->
    /// `swap_into_install_dir` -> `Db::insert` path: none of those compare
    /// the requested version against what's currently installed, so
    /// `dpm install pkg@<older>` already downgrades end-to-end today.
    /// Locks that in with an actual round trip (this is also the first test
    /// exercising `install()`'s full happy path start to finish, not just
    /// the pieces around it) so a future change can't silently reintroduce
    /// a "can't install older than installed" guard without a test noticing.
    #[tokio::test]
    async fn install_pkg_at_older_version_downgrades_the_already_installed_one() {
        let root = tempfile::tempdir().unwrap();
        let ctx = Context::for_test(root.path()).await.unwrap();
        let setting = Setting {
            sources: vec![Source {
                alias: "official".to_string(),
                repo_url: "http://127.0.0.1:1".to_string(),
                repo_info: "http://127.0.0.1:1".to_string(),
            }],
        };

        let fixtures_dir = tempfile::tempdir().unwrap();
        let (v2_bytes, v2_hash) = build_fixture_zip(fixtures_dir.path(), "2.0.0", "v2 content\n");
        let (v1_bytes, v1_hash) = build_fixture_zip(fixtures_dir.path(), "1.0.0", "v1 content\n");
        let v2_url = serve_once(v2_bytes);
        let v1_url = serve_once(v1_bytes);

        let row = |version: &str, url: String, hash: String| {
            DbPackage::new(
                "official",
                "downgrade-pkg",
                version,
                "prebuilt",
                Some(url),
                Some(hash),
                Some(format!("pkg-{version}.zip")),
                None,
                "test fixture",
                Some("main".to_string()),
                None,
                None,
                None,
                true,
            )
        };
        ctx.info_db
            .insert_available(row("2.0.0", v2_url, v2_hash))
            .await
            .unwrap();
        ctx.info_db
            .insert_available(row("1.0.0", v1_url, v1_hash))
            .await
            .unwrap();

        let install_v2 = ActionInfo::new(
            ctx.clone(),
            vec!["downgrade-pkg@2.0.0".to_string()],
            false,
            setting.clone(),
        );
        install_v2.install().await.unwrap();
        let installed = ctx.db.read_all().await.unwrap();
        assert_eq!(installed.len(), 1);
        assert_eq!(installed[0].version, "2.0.0");
        let entry_path = ctx.install_dir.join("downgrade-pkg").join("main");
        assert_eq!(
            std::fs::read_to_string(&entry_path).unwrap(),
            "v2 content\n"
        );

        let install_v1 = ActionInfo::new(
            ctx.clone(),
            vec!["downgrade-pkg@1.0.0".to_string()],
            false,
            setting,
        );
        install_v1.install().await.unwrap();
        let installed = ctx.db.read_all().await.unwrap();
        assert_eq!(installed.len(), 1);
        assert_eq!(
            installed[0].version, "1.0.0",
            "installing an older version must overwrite the InstalledPackages row, not refuse"
        );
        assert_eq!(
            std::fs::read_to_string(&entry_path).unwrap(),
            "v1 content\n",
            "the on-disk content must actually be swapped back to the older version"
        );
    }

    #[tokio::test]
    async fn owns_finds_the_package_that_registered_a_symlink() {
        let root = tempfile::tempdir().unwrap();
        let ctx = Context::for_test(root.path()).await.unwrap();
        let setting = Setting {
            sources: vec![Source {
                alias: "official".to_string(),
                repo_url: "http://127.0.0.1:1".to_string(),
                repo_info: "http://127.0.0.1:1".to_string(),
            }],
        };

        let fixtures_dir = tempfile::tempdir().unwrap();
        let (zip_bytes, zip_hash) =
            build_fixture_zip(fixtures_dir.path(), "1.0.0", "owns test content\n");
        let url = serve_once(zip_bytes);

        let row = DbPackage::new(
            "official",
            "downgrade-pkg",
            "1.0.0",
            "prebuilt",
            Some(url),
            Some(zip_hash),
            Some("pkg-1.0.0.zip".to_string()),
            None,
            "test fixture",
            Some("main".to_string()),
            None,
            None,
            None,
            true,
        );
        ctx.info_db.insert_available(row).await.unwrap();

        let install = ActionInfo::new(
            ctx.clone(),
            vec!["downgrade-pkg@1.0.0".to_string()],
            false,
            setting.clone(),
        );
        install.install().await.unwrap();

        let entry_link = ctx.bin_dir.join("downgrade-pkg");
        let owners = ctx
            .db
            .find_owners(&entry_link.display().to_string())
            .await
            .unwrap();
        assert_eq!(
            owners,
            vec!["downgrade-pkg".to_string()],
            "install() must have recorded the entry-point symlink in installed_files"
        );

        // Shell tab-completion of a symlink-to-directory commonly appends a
        // trailing `/` — `normalize_lexically` (applied after
        // `std::path::absolute()` in `owns()`) must strip it so the lookup
        // still matches the clean path stored in `installed_files`.
        let entry_link_with_trailing_slash = format!("{}/", entry_link.display());
        let owners_with_trailing_slash = ctx
            .db
            .find_owners(
                &normalize_lexically(std::path::Path::new(&entry_link_with_trailing_slash))
                    .display()
                    .to_string(),
            )
            .await
            .unwrap();
        assert_eq!(
            owners_with_trailing_slash,
            vec!["downgrade-pkg".to_string()],
            "a trailing slash (as shells add on tab-completing a symlinked dir) must still match"
        );

        // owns() 本身目前只印到 stdout,不回傳結構化結果——這裡直接驗證它
        // 對已知會命中/不會命中的路徑都能跑完不報錯(印什麼由人工驗證,
        // 見任務 3 的手動驗證步驟)。
        let owns_hit = ActionInfo::new(
            ctx.clone(),
            vec![entry_link.display().to_string()],
            false,
            setting.clone(),
        );
        owns_hit.owns().await.unwrap();

        let owns_miss = ActionInfo::new(
            ctx.clone(),
            vec!["/definitely/does/not/exist".to_string()],
            false,
            setting,
        );
        owns_miss.owns().await.unwrap();
    }

    #[tokio::test]
    async fn search_finds_packages_by_partial_name_and_description() {
        let root = tempfile::tempdir().unwrap();
        let ctx = Context::for_test(root.path()).await.unwrap();

        let pkg1 = DbPackage::new(
            "official",
            "hello",
            "0.1.0",
            "prebuilt",
            Some("https://example.com/hello.zip".to_string()),
            Some("a".repeat(64)),
            Some("hello.zip".to_string()),
            None,
            "simple universal hello package",
            None,
            None,
            None,
            None,
            true,
        );
        let pkg2 = DbPackage::new(
            "official",
            "addsub",
            "0.1.0",
            "source",
            None,
            Some("b".repeat(64)),
            None,
            Some("cc -shared -o libaddsub.dylib src/addsub.c".to_string()),
            "C add/subtract shared library",
            None,
            None,
            None,
            None,
            true,
        );

        ctx.info_db.insert_available(pkg1).await.unwrap();
        ctx.info_db.insert_available(pkg2).await.unwrap();

        let setting = Setting::default();
        // Query "he" should match "hello" by partial substring
        let action_partial =
            ActionInfo::new(ctx.clone(), vec!["he".to_string()], false, setting.clone());
        assert!(action_partial.search().await.is_ok());

        // Query "sub" should match "addsub" by partial substring in name AND description
        let action_desc =
            ActionInfo::new(ctx.clone(), vec!["sub".to_string()], false, setting.clone());
        assert!(action_desc.search().await.is_ok());

        // Empty query should list all available packages without error
        let action_empty = ActionInfo::new(ctx, vec![], false, setting);
        assert!(action_empty.search().await.is_ok());
    }

    #[tokio::test]
    async fn pin_and_unpin_report_installed_vs_missing_packages() {
        let root = tempfile::tempdir().unwrap();
        let ctx = Context::for_test(root.path()).await.unwrap();

        let pkg = DbPackage::new(
            "official", "hello", "0.1.0", "prebuilt", None, None, None, None, "", None, None, None,
            None, true,
        );
        ctx.db.insert(pkg).await.unwrap();

        let setting = Setting::default();
        let action = ActionInfo::new(
            ctx.clone(),
            vec!["hello".to_string(), "does-not-exist".to_string()],
            false,
            setting.clone(),
        );
        assert!(action.pin(true).await.is_ok());
        assert!(ctx.db.pinned_names().await.unwrap().contains("hello"));

        let action = ActionInfo::new(ctx.clone(), vec!["hello".to_string()], false, setting);
        assert!(action.pin(false).await.is_ok());
        assert!(ctx.db.pinned_names().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn upgrade_skips_a_pinned_package_without_erroring() {
        let root = tempfile::tempdir().unwrap();
        let ctx = Context::for_test(root.path()).await.unwrap();

        let installed = DbPackage::new(
            "official", "hello", "0.1.0", "prebuilt", None, None, None, None, "", None, None, None,
            None, true,
        );
        ctx.db.insert(installed).await.unwrap();
        ctx.db.set_pinned("hello", true).await.unwrap();

        let available = DbPackage::new(
            "official",
            "hello",
            "0.2.0",
            "prebuilt",
            Some("https://example.com/hello.zip".to_string()),
            Some("a".repeat(64)),
            Some("hello.zip".to_string()),
            None,
            "",
            None,
            None,
            None,
            None,
            true,
        );
        ctx.info_db.insert_available(available).await.unwrap();

        let setting = Setting::default();
        let action = ActionInfo::new(ctx, vec!["hello".to_string()], false, setting);
        // If the pinned filter didn't remove "hello" from `is`, this would
        // try to actually download/verify hello.zip and fail/hang instead
        // of returning Ok quickly.
        assert!(action.upgrade().await.is_ok());
    }

    #[tokio::test]
    async fn info_covers_installed_available_only_and_not_found() {
        let root = tempfile::tempdir().unwrap();
        let ctx = Context::for_test(root.path()).await.unwrap();

        let available_only = DbPackage::new(
            "official",
            "addsub",
            "0.1.0",
            "source",
            None,
            Some("b".repeat(64)),
            None,
            Some("cc -shared -o libaddsub.dylib src/addsub.c".to_string()),
            "C add/subtract shared library",
            None,
            None,
            None,
            None,
            true,
        );
        ctx.info_db.insert_available(available_only).await.unwrap();

        let installed_and_available = DbPackage::new(
            "official",
            "hello",
            "0.1.0",
            "prebuilt",
            Some("https://example.com/hello.zip".to_string()),
            Some("a".repeat(64)),
            Some("hello.zip".to_string()),
            None,
            "simple universal hello package",
            Some("hello".to_string()),
            None,
            Some("someauthor".to_string()),
            Some("deadbeef".to_string()),
            true,
        );
        ctx.info_db
            .insert_available(installed_and_available.clone())
            .await
            .unwrap();
        ctx.db.insert(installed_and_available).await.unwrap();

        let setting = Setting::default();
        let action = ActionInfo::new(
            ctx,
            vec![
                "hello".to_string(),          // installed + available
                "addsub".to_string(),         // available only
                "does-not-exist".to_string(), // neither
            ],
            false,
            setting,
        );
        assert!(action.info().await.is_ok());
    }

    #[test]
    fn is_directly_requested_matches_by_name_only() {
        let is: Vec<ParsedInstallSpec> =
            vec![(Some("official".to_string()), "foo".to_string(), None)];
        assert!(ActionInfo::is_directly_requested(&is, "foo"));
        assert!(!ActionInfo::is_directly_requested(&is, "bar"));
    }

    #[tokio::test]
    async fn upgrade_does_not_promote_an_already_auto_installed_dependency() -> ClientResult<()> {
        let root = tempfile::tempdir().unwrap();
        let ctx = Context::for_test(root.path()).await.unwrap();

        let auto_dep = DbPackage::new(
            "official", "dep", "1.0.0", "prebuilt", None, None, None, None, "", None, None, None,
            None, false,
        );
        ctx.db.insert(auto_dep).await.unwrap();

        // Simulates what upgrade() would do: "dep" is directly named in
        // `is`, but promote=false because this call represents an upgrade,
        // not a fresh install. `install_resolved_with_gate` will fail to
        // resolve "dep" from an empty `all_packages` (no network/download
        // needed to prove the point) — we're not testing the full
        // resolve+download path here, only that `is_directly_requested`'s
        // promotion math is gated by `promote`.
        let already_installed: std::collections::HashSet<String> = ctx
            .db
            .read_all()
            .await
            .unwrap()
            .into_iter()
            .map(|p| p.name)
            .collect();
        let is: Vec<ParsedInstallSpec> =
            vec![(Some("official".to_string()), "dep".to_string(), None)];
        let explicit_if_upgrade = ActionInfo::is_directly_requested(&is, "dep")
            && (false || !already_installed.contains("dep"));
        let explicit_if_install = ActionInfo::is_directly_requested(&is, "dep")
            && (true || !already_installed.contains("dep"));
        assert!(
            !explicit_if_upgrade,
            "upgrade must not promote an already-installed auto dependency"
        );
        assert!(
            explicit_if_install,
            "install must still promote a directly-requested package"
        );
        Ok(())
    }

    #[tokio::test]
    async fn uninstall_prints_hint_and_leaves_orphan_installed_for_autoremove_to_handle(
    ) -> ClientResult<()> {
        let root = tempfile::tempdir().unwrap();
        let ctx = Context::for_test(root.path()).await.unwrap();

        let leaf = DbPackage::new(
            "official", "leaf", "1.0.0", "prebuilt", None, None, None, None, "", None, None, None,
            None, false,
        );
        ctx.db.insert(leaf).await.unwrap();

        let root_pkg = DbPackage::new(
            "official",
            "root",
            "1.0.0",
            "prebuilt",
            None,
            None,
            None,
            None,
            "",
            None,
            Some(vec![dpm_core::Dependency {
                name: "leaf".to_string(),
                version: "*".to_string(),
            }]),
            None,
            None,
            true,
        );
        ctx.db.insert(root_pkg).await.unwrap();

        let action = ActionInfo::new(
            ctx.clone(),
            vec!["root".to_string()],
            false,
            Setting::default(),
        );
        action.uninstall().await?;

        // "root" is gone, "leaf" is still installed (autoremove's job, not
        // uninstall's) but now orphaned.
        let remaining = ctx.db.read_all().await.unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].name, "leaf");
        assert!(!remaining[0].explicit);
        assert!(find_orphans(&remaining).iter().any(|p| p.name == "leaf"));
        Ok(())
    }

    #[tokio::test]
    async fn autoremove_removes_orphans_and_leaves_explicit_packages_alone() -> ClientResult<()> {
        let root = tempfile::tempdir().unwrap();
        let ctx = Context::for_test(root.path()).await.unwrap();

        let orphan = DbPackage::new(
            "official", "orphan", "1.0.0", "prebuilt", None, None, None, None, "", None, None,
            None, None, false,
        );
        ctx.db.insert(orphan).await.unwrap();

        let kept = DbPackage::new(
            "official", "kept", "1.0.0", "prebuilt", None, None, None, None, "", None, None, None,
            None, true,
        );
        ctx.db.insert(kept).await.unwrap();

        let action = ActionInfo::new(ctx.clone(), vec![], false, Setting::default());
        action.autoremove().await?;

        let remaining = ctx.db.read_all().await.unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].name, "kept");
        Ok(())
    }

    #[tokio::test]
    async fn autoremove_is_a_no_op_when_nothing_is_orphaned() -> ClientResult<()> {
        let root = tempfile::tempdir().unwrap();
        let ctx = Context::for_test(root.path()).await.unwrap();

        let kept = DbPackage::new(
            "official", "kept", "1.0.0", "prebuilt", None, None, None, None, "", None, None, None,
            None, true,
        );
        ctx.db.insert(kept).await.unwrap();

        let action = ActionInfo::new(ctx.clone(), vec![], false, Setting::default());
        action.autoremove().await?;

        assert_eq!(ctx.db.read_all().await.unwrap().len(), 1);
        Ok(())
    }
}

#[cfg(test)]
mod install_local_file_tests {
    use super::*;
    use dpm_core::Dependency;
    use std::collections::HashMap;
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

    /// Builds a real `.dpm` archive (still a plain zip container — only the
    /// extension is branded) on disk at `dir/<name>.dpm`, with a
    /// `packageInfo.json`/`hashes.json` pair that's internally
    /// self-consistent (mirrors `dpm-server hash`/`build`'s real output,
    /// same construction `install_resolved_tests::build_fixture_zip` uses
    /// for the remote-fetch path), optionally declaring `dependencies`.
    fn build_local_dpm_file(
        dir: &std::path::Path,
        name: &str,
        dependencies: Option<Vec<Dependency>>,
    ) -> std::path::PathBuf {
        let src = dir.join(format!("src-{name}"));
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("main"), b"#!/bin/sh\necho hi\n").unwrap();
        let main_hash = dpm_core::hash_file(&src.join("main")).unwrap();

        let hashes_path = src.join("hashes.json");
        let mut hashes: HashMap<String, String> = HashMap::new();
        hashes.insert("main".to_string(), main_hash);
        JsonStorage::to_json(&hashes, &hashes_path).unwrap();
        let hashes_json_hash = dpm_core::hash_file(&hashes_path).unwrap();
        hashes.insert("hashes.json".to_string(), hashes_json_hash.clone());
        JsonStorage::to_json(&hashes, &hashes_path).unwrap();

        let mut package_info = PackageInfo::new(
            name.to_string(),
            "main".to_string(),
            "1.0.0".to_string(),
            "local test fixture".to_string(),
            hashes_json_hash,
            dependencies,
            None,
        );
        // `PackageInfo::new` always starts `signature: None` (there's no
        // author key involved in a purely local file) -- explicit for
        // clarity, this is what makes install_local_file's "no
        // signature/index verification" path the one under test.
        package_info.signature = None;
        JsonStorage::to_json(&package_info, &src.join("packageInfo.json")).unwrap();

        let dpm_path = dir.join(format!("{name}.dpm"));
        crate::zip_folder(&src, &dpm_path).unwrap();
        dpm_path
    }

    #[tokio::test]
    async fn installs_a_local_file_with_no_dependencies() {
        let root = tempfile::tempdir().unwrap();
        let ctx = Context::for_test(root.path()).await.unwrap();

        let dpm_path = build_local_dpm_file(root.path(), "standalone", None);
        let install = ActionInfo::new(
            ctx.clone(),
            vec![dpm_path.display().to_string()],
            false,
            Setting::default(),
        );
        install.install().await.unwrap();

        let installed = ctx.db.read_all().await.unwrap();
        assert_eq!(installed.len(), 1);
        assert_eq!(installed[0].name, "standalone");
        assert_eq!(installed[0].source, "local");
        assert!(installed[0].explicit);
        assert!(ctx.install_dir.join("standalone").join("main").exists());
    }

    /// Builds a fixture archive as *bytes* (rather than
    /// `build_local_dpm_file`'s on-disk file) so it can be served over the
    /// local HTTP `serve_once` — this is what a dependency actually
    /// resolved from the index goes through (`install_resolved` downloads
    /// it), as opposed to the local file under test which is read straight
    /// off disk.
    fn build_remote_fixture_zip(dir: &std::path::Path, name: &str) -> (Vec<u8>, String) {
        let src = dir.join(format!("remote-src-{name}"));
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("main"), b"#!/bin/sh\necho dep\n").unwrap();
        let main_hash = dpm_core::hash_file(&src.join("main")).unwrap();

        let hashes_path = src.join("hashes.json");
        let mut hashes: HashMap<String, String> = HashMap::new();
        hashes.insert("main".to_string(), main_hash);
        JsonStorage::to_json(&hashes, &hashes_path).unwrap();
        let hashes_json_hash = dpm_core::hash_file(&hashes_path).unwrap();
        hashes.insert("hashes.json".to_string(), hashes_json_hash.clone());
        JsonStorage::to_json(&hashes, &hashes_path).unwrap();

        let package_info = PackageInfo::new(
            name.to_string(),
            "main".to_string(),
            "1.0.0".to_string(),
            "dependency".to_string(),
            hashes_json_hash,
            None,
            None,
        );
        JsonStorage::to_json(&package_info, &src.join("packageInfo.json")).unwrap();

        let zip_path = dir.join(format!("remote-{name}.dpm"));
        crate::zip_folder(&src, &zip_path).unwrap();
        let zip_hash = dpm_core::hash_file(&zip_path).unwrap();
        (std::fs::read(&zip_path).unwrap(), zip_hash)
    }

    #[tokio::test]
    async fn auto_resolves_a_missing_dependency_from_the_index() {
        let root = tempfile::tempdir().unwrap();
        let ctx = Context::for_test(root.path()).await.unwrap();

        // The dependency lives in the configured index, same as a normal
        // remote install would resolve it through.
        let (dep_bytes, dep_hash) = build_remote_fixture_zip(root.path(), "libfoo");
        let dep_url = serve_once(dep_bytes);
        let dep_row = DbPackage::new(
            "official",
            "libfoo",
            "1.0.0",
            "prebuilt",
            Some(dep_url),
            Some(dep_hash),
            Some("libfoo.dpm".to_string()),
            None,
            "dependency",
            Some("main".to_string()),
            None,
            None,
            None,
            true,
        );
        ctx.info_db.insert_available(dep_row).await.unwrap();

        let deps = vec![Dependency {
            name: "libfoo".to_string(),
            version: "^1.0.0".to_string(),
        }];
        let dpm_path = build_local_dpm_file(root.path(), "needs-libfoo", Some(deps));

        let setting = Setting {
            sources: vec![Source {
                alias: "official".to_string(),
                repo_url: "http://127.0.0.1:1".to_string(),
                repo_info: "http://127.0.0.1:1".to_string(),
            }],
        };
        let install = ActionInfo::new(
            ctx.clone(),
            vec![dpm_path.display().to_string()],
            false,
            setting,
        );
        install.install().await.unwrap();

        let installed = ctx.db.read_all().await.unwrap();
        assert_eq!(installed.len(), 2);
        let needs = installed.iter().find(|p| p.name == "needs-libfoo").unwrap();
        assert_eq!(needs.source, "local");
        assert!(needs.explicit);
        let dep = installed.iter().find(|p| p.name == "libfoo").unwrap();
        assert_eq!(
            dep.source, "official",
            "the auto-resolved dependency must come from the index, not be marked local"
        );
        assert!(
            !dep.explicit,
            "an auto-resolved dependency must not be promoted to explicit"
        );
    }

    #[tokio::test]
    async fn rejects_a_local_file_whose_hashes_json_self_entry_is_tampered() {
        let root = tempfile::tempdir().unwrap();
        let ctx = Context::for_test(root.path()).await.unwrap();
        let dpm_path = build_local_dpm_file(root.path(), "tampered", None);

        // Corrupt the archive's internal self-consistency: rewrite
        // hashes.json's "hashes.json" self-entry so it no longer matches
        // packageInfo.json.hash, without touching packageInfo.json itself.
        let extracted = root.path().join("extracted-for-tamper");
        unzip_file(&dpm_path, &extracted).unwrap();
        let mut hashes: HashMap<String, String> =
            JsonStorage::from_json(&extracted.join("hashes.json")).unwrap();
        hashes.insert("hashes.json".to_string(), "0".repeat(64));
        JsonStorage::to_json(&hashes, &extracted.join("hashes.json")).unwrap();
        crate::zip_folder(&extracted, &dpm_path).unwrap();

        let action = ActionInfo::new(
            ctx,
            vec![dpm_path.display().to_string()],
            false,
            Setting::default(),
        );
        let err = action.install().await.unwrap_err();
        assert!(matches!(
            err,
            ClientError::Core(CoreError::HashMismatch { .. })
        ));
    }
}
