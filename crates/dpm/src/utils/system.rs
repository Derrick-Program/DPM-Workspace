use super::ClientError;
use super::ClientResult;
use crate::{Context, Scope, Setting, Source};
use libc::{getpwuid, getuid};
use std::{
    ffi::CStr,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
/// 「官方」套件來源的預設 git repo 位址——見上方模組文件註解。這次改成
/// `pub(crate)` 是因為簽章驗證(`action.rs` 的 `sync_source`/`install_resolved`)
/// 要拿它跟 `Source.repo_url` 比對,判斷是否該套用簽章驗證這個安全閘門
/// (刻意比對這個寫死的常數,不是使用者本機 `config.toml` 可以自己編輯的
/// `alias` 字串)。
pub(crate) const OFFICIAL_REPO_URL: &str = "https://github.com/Derrick-Program/DPM-Workspace";

/// 把 `https://github.com/<owner>/<repo>` 轉成該 repo 在 `main` 分支上某個
/// 檔案路徑的 raw content URL。`official_repo_info_url`/`official_key_url`
/// 共用這個轉換,只差要抓的路徑。
fn raw_content_url(repo_url: &str, path: &str) -> String {
    if let Some(base_path) = repo_url.strip_prefix("file://") {
        let full = Path::new(base_path).join(path);
        format!("file://{}", full.display())
    } else if Path::new(repo_url).exists() {
        Path::new(repo_url).join(path).display().to_string()
    } else {
        let raw_base = repo_url.replacen(
            "https://github.com/",
            "https://raw.githubusercontent.com/",
            1,
        );
        if repo_url.contains("DPM-Workspace") && !path.starts_with("crates/dpm-server/") {
            format!("{raw_base}/main/crates/dpm-server/{path}")
        } else {
            format!("{raw_base}/main/{path}")
        }
    }
}

/// 把 `https://github.com/<owner>/<repo>` 轉成該 repo 在 `main` 分支上
/// `RepoInfo.db` 的 raw content URL。
fn official_repo_info_url(repo_url: &str) -> String {
    raw_content_url(repo_url, "RepoInfo.db")
}

/// 把 `https://github.com/<owner>/<repo>` 轉成該 repo 在 `main` 分支上
/// `keys/<author_id>.pub` 的 raw content URL——跟 `official_repo_info_url`
/// 同一個 host 轉換規則,只差路徑。
pub(crate) fn official_key_url(repo_url: &str, author_id: &str) -> String {
    raw_content_url(repo_url, &format!("keys/{author_id}.pub"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PackageManager {
    Apt,
    Dnf,
    Yum,
    Pacman,
    Zypper,
    Brew,
    Unknown,
}

/// One of the six actions `SystemAction` performs through the detected OS
/// package manager. Kept as a value (not six near-identical methods each
/// re-deriving their own match) so `PackageManager::command_for` is the one
/// place that knows every manager's verbs.
#[derive(Debug, Clone, Copy)]
enum Verb {
    Install,
    Uninstall,
    Search,
    Upgrade,
    UpdateIndex,
    List,
}

impl Verb {
    /// Short verb used in "Failed to {verb} package: {name}"-style messages.
    fn description(self) -> &'static str {
        match self {
            Verb::Install => "install",
            Verb::Uninstall => "remove",
            Verb::Search => "search",
            Verb::Upgrade => "upgrade",
            Verb::UpdateIndex => "update package index",
            Verb::List => "list packages",
        }
    }
}

impl PackageManager {
    /// The (command, base-args) pair for one verb under this package
    /// manager. Replaces what used to be six separate `match self.package_manager`
    /// blocks (one per `SystemAction` method), each ending in its own
    /// `Unknown => panic!("Unsupported package manager.")` — now a single
    /// table, and `Unknown` returns an error instead of panicking.
    ///
    /// Yum previously got silently grouped with Dnf (running the `dnf`
    /// binary) for uninstall/upgrade/update-index while install/search/list
    /// correctly used `yum` — an inconsistency only visible once every verb
    /// sat in one table side by side. Fixed here: Yum always uses `yum`.
    fn command_for(self, verb: Verb) -> ClientResult<(&'static str, Vec<&'static str>)> {
        use PackageManager::*;
        use Verb::*;
        let result: (&'static str, Vec<&'static str>) = match (self, verb) {
            (Apt, Install) => ("apt-get", vec!["install", "-y"]),
            (Apt, Uninstall) => ("apt-get", vec!["remove", "-y"]),
            (Apt, Search) => ("apt-cache", vec!["search"]),
            (Apt, Upgrade) => ("apt-get", vec!["install", "--only-upgrade", "-y"]),
            (Apt, UpdateIndex) => ("apt-get", vec!["update"]),
            (Apt, List) => ("apt", vec!["list", "--installed"]),

            (Dnf, Install) => ("dnf", vec!["install", "-y"]),
            (Dnf, Uninstall) => ("dnf", vec!["remove", "-y"]),
            (Dnf, Search) => ("dnf", vec!["search"]),
            (Dnf, Upgrade) => ("dnf", vec!["upgrade", "-y"]),
            (Dnf, UpdateIndex) => ("dnf", vec!["makecache"]),
            (Dnf, List) => ("dnf", vec!["list", "installed"]),

            (Yum, Install) => ("yum", vec!["install", "-y"]),
            (Yum, Uninstall) => ("yum", vec!["remove", "-y"]),
            (Yum, Search) => ("yum", vec!["search"]),
            (Yum, Upgrade) => ("yum", vec!["upgrade", "-y"]),
            (Yum, UpdateIndex) => ("yum", vec!["makecache"]),
            (Yum, List) => ("yum", vec!["list", "installed"]),

            (Pacman, Install) => ("pacman", vec!["-S", "--noconfirm"]),
            (Pacman, Uninstall) => ("pacman", vec!["-R"]),
            (Pacman, Search) => ("pacman", vec!["-Ss"]),
            (Pacman, Upgrade) => ("pacman", vec!["-Syu"]),
            (Pacman, UpdateIndex) => ("pacman", vec!["-Sy"]),
            (Pacman, List) => ("pacman", vec!["-Q"]),

            (Zypper, Install) => ("zypper", vec!["install", "-y"]),
            (Zypper, Uninstall) => ("zypper", vec!["remove", "-y"]),
            (Zypper, Search) => ("zypper", vec!["search"]),
            (Zypper, Upgrade) => ("zypper", vec!["update", "-y"]),
            (Zypper, UpdateIndex) => ("zypper", vec!["refresh"]),
            (Zypper, List) => ("zypper", vec!["search", "--installed-only"]),

            (Brew, Install) => ("brew", vec!["install"]),
            (Brew, Uninstall) => ("brew", vec!["uninstall"]),
            (Brew, Search) => ("brew", vec!["search"]),
            (Brew, Upgrade) => ("brew", vec!["upgrade"]),
            (Brew, UpdateIndex) => ("brew", vec!["update"]),
            (Brew, List) => ("brew", vec!["list"]),

            (Unknown, verb) => {
                let action = match verb {
                    Install => "install packages",
                    Uninstall => "remove packages",
                    Search => "search for packages",
                    Upgrade => "upgrade packages",
                    UpdateIndex => "update the package index",
                    List => "list installed packages",
                };
                return Err(ClientError::SystemError(format!(
                    "cannot {action}: unrecognized or unsupported package manager"
                )));
            }
        };
        Ok(result)
    }
}
#[derive(Debug)]
pub struct SystemAction {
    package_manager: PackageManager,
    _verbose: bool,
}
/// Holds the `Scope` its caller resolved once (from `--system`/no flag) —
/// replaces reading a process-wide `SCOPE: OnceLock<Scope>` global from
/// deep inside `permision_check`/`system_command_runner`. Two calls with
/// different `Scope`s (e.g. one real, one from a test) now genuinely
/// behave differently, instead of both racing to set the same `OnceLock`.
#[derive(Debug, Clone, Copy)]
pub struct SystemController {
    scope: Scope,
}

impl SystemController {
    pub fn new(scope: Scope) -> Self {
        Self { scope }
    }

    fn get_current_username(&self) -> Option<String> {
        unsafe {
            let uid = getuid();
            let pwd = getpwuid(uid);
            if !pwd.is_null() {
                let c_str = CStr::from_ptr((*pwd).pw_name);
                c_str.to_str().ok().map(|s| s.to_owned())
            } else {
                None
            }
        }
    }
    /// Sweeps ownership of `main_dir` back to root (Linux) or the invoking
    /// user (macOS) under `--system`. Takes the directory explicitly rather
    /// than reading a global: this runs once *before* `Context` exists yet
    /// (`Context::for_scope`'s own `main_dir` bootstrap) and again *after*
    /// (`entry()`'s post-command sweep), so a `Context` isn't always
    /// available at the call site — a bare path is the smaller, always-valid
    /// interface.
    pub fn permision_check(&self, main_dir: &Path) -> ClientResult<()> {
        if self.scope != Scope::System {
            return Ok(());
        }
        // 若透過 sudo 執行,getuid() 會拿到 root;優先用 SUDO_USER 取得原始使用者
        let username = std::env::var("SUDO_USER")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| self.get_current_username())
            .ok_or_else(|| {
                ClientError::SystemError("Could not get current username".to_string())
            })?;
        let main_dir_str = main_dir.to_str().ok_or_else(|| {
            ClientError::SystemError("Invalid UTF-8 in main_dir path".to_string())
        })?;
        if cfg!(target_os = "linux") {
            self.system_command_runner(
                "chown",
                vec!["-R", "root:root", main_dir_str],
                "Can't run chown",
            )?;
        } else if cfg!(target_os = "macos") {
            self.system_command_runner(
                "chown",
                vec!["-R", format!("{}:admin", username).as_str(), main_dir_str],
                "Can't run chown",
            )?;
        }
        Ok(())
    }
    pub fn system_command_runner(
        &self,
        command: &str,
        args: Vec<&str>,
        err_message: &str,
    ) -> ClientResult<()> {
        if !(cfg!(target_os = "linux") || cfg!(target_os = "macos")) {
            return Err(ClientError::SystemError(
                "unsupported OS: dpm only supports Linux and macOS".to_string(),
            ));
        }
        let mut cmd = if self.scope == Scope::System {
            let mut c = Command::new("sudo");
            c.arg(command);
            c
        } else {
            Command::new(command)
        };
        cmd.args(&args);
        let status = cmd
            .status()
            .map_err(|e| ClientError::SystemError(e.to_string()))?;
        if !status.success() {
            let err = err_message.to_string();
            return Err(ClientError::SystemError(err));
        }
        Ok(())
    }
    /// `chown_dir_to_sudo_user` for the shared per-user `config_dir`, but
    /// tolerant of "this process is root and `SUDO_UID` isn't set".
    ///
    /// The helper itself treats that state as a hard error, and rightly so at
    /// its original call site (`utils::privilege`'s build-directory prep,
    /// where the whole point is refusing to let an untrusted build command run
    /// as root). Here the goal is only to hand the config directory back to
    /// whoever invoked `sudo`. A genuine root login — a Docker container, a
    /// root-only server, `--system` where `sudo::escalate_if_needed()` found
    /// itself already root — has no such user, and root-owned is then the
    /// correct outcome, not a failure. Without this guard `dpm` would refuse
    /// to bootstrap at all for those users.
    ///
    /// Gating on `SUDO_UID`'s presence (not on `getuid()`) keeps the Linux
    /// `--system`-via-sudo path — the one this whole fix exists for — going
    /// through the real chown untouched: `sudo` always sets it.
    fn chown_config_dir_to_invoking_user(ctx: &Context) -> ClientResult<()> {
        if std::env::var_os("SUDO_UID").is_none() {
            return Ok(());
        }
        crate::utils::privilege::chown_dir_to_sudo_user(&ctx.config_dir)
    }

    /// Creates the scope's three directories (idempotent) and syncs their
    /// ownership. Shared by `init_first_run`/`init_existing` — bootstrap
    /// happens either way, only what's done with `config.toml` differs.
    fn bootstrap_dirs(&self, ctx: &Context) -> ClientResult<()> {
        let env_dirs = [
            &ctx.install_dir,
            &ctx.bin_dir,
            &ctx.sbin_dir(),
            &ctx.lib_dir(),
            &ctx.include_dir(),
            &ctx.share_dir(),
            &ctx.completions_dir(),
            &ctx.docs_dir(),
            &ctx.opt_dir(),
            &ctx.etc_dir(),
            &ctx.var_dir(),
        ];
        for dir in env_dirs {
            let dir_str = dir.to_str().ok_or_else(|| {
                ClientError::SystemError(format!(
                    "Invalid UTF-8 in directory path: {}",
                    dir.display()
                ))
            })?;
            self.system_command_runner("mkdir", vec!["-p", dir_str], "Can't create directory")?;
        }
        std::fs::create_dir_all(&ctx.config_dir)
            .map_err(|e| ClientError::SystemError(format!("Can't create config dir: {e}")))?;
        Self::chown_config_dir_to_invoking_user(ctx)?;
        self.permision_check(&ctx.main_dir)
    }

    /// OS bootstrap for the first-ever run on this machine/scope — no
    /// longer reaches into `ActionInfo` to seed the first-run source index
    /// (that used to make this "system utility" module the sole
    /// orchestrator of first-run business logic, while `ActionInfo` itself
    /// constructs a `SystemController`: a two-way dependency between the
    /// OS-utility seam and the command-orchestration seam).
    ///
    /// Replaces the old `init() -> (Setting, bool)`, whose `bool` told the
    /// caller whether `config.toml` was just created so it could decide
    /// whether to seed sources. `entry()` now checks `config.toml`'s
    /// existence itself and calls this function only on that branch, so
    /// "was this the first run" and "should I seed sources" collapse into
    /// the same call — there's no bool to let those two facts drift apart.
    pub async fn init_first_run(&self, ctx: &Context) -> ClientResult<Setting> {
        self.bootstrap_dirs(ctx)?;
        let config_path = ctx.config_path();
        let default_setting = Setting {
            sources: vec![Source {
                alias: "official".to_string(),
                repo_url: OFFICIAL_REPO_URL.to_string(),
                repo_info: official_repo_info_url(OFFICIAL_REPO_URL),
            }],
        };
        dpm_core::TomlStorage::to_toml(&default_setting, &config_path)?;
        // The file we just wrote is brand new, created by this process — which
        // on Linux `--system` is root (see bootstrap_dirs). Re-sweep the whole
        // config dir so the new file lands on the invoking user too.
        Self::chown_config_dir_to_invoking_user(ctx)?;
        self.permision_check(&ctx.main_dir)?;
        Ok(dpm_core::load_layered(
            &Context::system_config_path(),
            &config_path,
            "DPM",
        )?)
    }

    /// OS bootstrap for every run after the first: the user-tier
    /// `config.toml` already exists, so this only (re-)creates the scope's
    /// directories and re-reads the effective (system < user < env) config.
    pub async fn init_existing(&self, ctx: &Context) -> ClientResult<Setting> {
        self.bootstrap_dirs(ctx)?;
        let config_path = ctx.config_path();
        let mut setting: Setting =
            dpm_core::load_layered(&Context::system_config_path(), &config_path, "DPM")?;

        // Enforce that official source is strictly hardcoded to OFFICIAL_REPO_URL
        // and cannot be modified or omitted via config files or env vars.
        let mut found_official = false;
        for source in &mut setting.sources {
            if source.alias == "official" {
                source.repo_url = OFFICIAL_REPO_URL.to_string();
                source.repo_info = official_repo_info_url(OFFICIAL_REPO_URL);
                found_official = true;
            }
        }
        if !found_official {
            setting.sources.insert(
                0,
                Source {
                    alias: "official".to_string(),
                    repo_url: OFFICIAL_REPO_URL.to_string(),
                    repo_info: official_repo_info_url(OFFICIAL_REPO_URL),
                },
            );
        }

        Ok(setting)
    }

    /// `gen-config` subcommand:在使用者層建立一個「空的」`config.toml`
    /// (零個 key,不是序列化後的 `Setting::default()`——理由見函式內註解)。使用者層
    /// 已存在且沒帶 `force` 就拒絕——那個檔案可能已經被手動改過,不能悄悄
    /// 蓋掉。永遠不會碰系統層(`Context::system_config_path()`)。
    pub async fn gen_config(&self, ctx: &Context, force: bool) -> ClientResult<PathBuf> {
        self.bootstrap_dirs(ctx)?;
        let config_path = ctx.config_path();
        if config_path.exists() && !force {
            return Err(ClientError::ConfigError(format!(
                "{} already exists — pass --force to overwrite",
                config_path.display()
            )));
        }
        // An empty file, not a serialized Setting::default() — a present-but-
        // empty `sources = []` key still wins over the system tier in
        // config-crate's key-presence-based merge (whole-key replace, not deep
        // merge), silently making the system tier unreachable for anyone who
        // runs gen-config. An empty file contributes no keys, so the system
        // tier (and, failing that, Setting::default() itself) can still supply
        // `sources`.
        std::fs::write(&config_path, "")
            .map_err(|e| ClientError::SystemError(format!("Can't write config file: {e}")))?;
        Self::chown_config_dir_to_invoking_user(ctx)?;
        Ok(config_path)
    }
}

impl SystemAction {
    pub fn new(verbose: bool) -> Self {
        Self {
            package_manager: Self::detect_package_manager(),
            _verbose: verbose,
        }
    }

    fn detect_package_manager() -> PackageManager {
        let managers = vec![
            ("apt-get", PackageManager::Apt),
            ("dnf", PackageManager::Dnf),
            ("yum", PackageManager::Yum),
            ("pacman", PackageManager::Pacman),
            ("zypper", PackageManager::Zypper),
            ("brew", PackageManager::Brew),
        ];
        for (command, manager) in managers {
            if Command::new(command).arg("--version").output().is_ok() {
                return manager;
            }
        }
        PackageManager::Unknown
    }
    /// Single seam every `SystemAction` method now funnels through, in
    /// place of six copy-pasted matches on `self.package_manager`. Building
    /// the command/args table (`PackageManager::command_for`) and running it
    /// are the same two steps regardless of verb; only the verb and whether
    /// it takes a package name differ.
    fn run_verb(&self, verb: Verb, package_name: Option<&str>) -> ClientResult<()> {
        let (command, mut args) = self.package_manager.command_for(verb)?;
        if let Some(name) = package_name {
            args.push(name);
        }
        let err = match package_name {
            Some(name) => format!("Failed to {} package: {name}", verb.description()),
            None => format!("Failed to {}", verb.description()),
        };
        self.command_runner(command, args, &err)
    }

    pub fn install_package(&self, package_name: &str) -> ClientResult<()> {
        self.run_verb(Verb::Install, Some(package_name))
    }
    pub fn update_package_index(&self) -> ClientResult<()> {
        self.run_verb(Verb::UpdateIndex, None)
    }
    pub fn uninstall_package(&self, package_name: &str) -> ClientResult<()> {
        self.run_verb(Verb::Uninstall, Some(package_name))
    }
    pub fn search_package(&self, package_name: &str) -> ClientResult<()> {
        self.run_verb(Verb::Search, Some(package_name))
    }
    pub fn upgrade_package(&self, package_name: &str) -> ClientResult<()> {
        self.run_verb(Verb::Upgrade, Some(package_name))
    }
    pub fn list_packages(&self) -> ClientResult<()> {
        self.run_verb(Verb::List, None)
    }

    fn command_runner(
        &self,
        command: &str,
        args: Vec<&str>,
        err_message: &str,
    ) -> ClientResult<()> {
        let mut cmd = if cfg!(target_os = "linux") {
            let mut c = Command::new("sudo");
            c.arg(command);
            c
        } else {
            Command::new(command)
        };

        cmd.stdout(Stdio::inherit());
        cmd.stderr(Stdio::inherit());

        cmd.args(&args);

        let status = cmd
            .status()
            .map_err(|e| ClientError::SystemError(e.to_string()))?;
        if !status.success() {
            let err = err_message.to_string();
            return Err(ClientError::SystemError(err));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_manager_errors_instead_of_panicking() {
        for verb in [
            Verb::Install,
            Verb::Uninstall,
            Verb::Search,
            Verb::Upgrade,
            Verb::UpdateIndex,
            Verb::List,
        ] {
            assert!(PackageManager::Unknown.command_for(verb).is_err());
        }
    }

    #[test]
    fn yum_always_uses_the_yum_binary() {
        // Regression test: uninstall/upgrade/update-index used to be grouped
        // with Dnf and silently ran the `dnf` binary for a Yum-detected
        // system. Every verb must use "yum" now.
        for verb in [
            Verb::Install,
            Verb::Uninstall,
            Verb::Search,
            Verb::Upgrade,
            Verb::UpdateIndex,
            Verb::List,
        ] {
            let (command, _) = PackageManager::Yum.command_for(verb).unwrap();
            assert_eq!(command, "yum", "Yum + {verb:?} used '{command}' instead");
        }
    }

    #[test]
    fn every_known_manager_has_all_six_verbs() {
        for manager in [
            PackageManager::Apt,
            PackageManager::Dnf,
            PackageManager::Yum,
            PackageManager::Pacman,
            PackageManager::Zypper,
            PackageManager::Brew,
        ] {
            for verb in [
                Verb::Install,
                Verb::Uninstall,
                Verb::Search,
                Verb::Upgrade,
                Verb::UpdateIndex,
                Verb::List,
            ] {
                assert!(
                    manager.command_for(verb).is_ok(),
                    "{manager:?} is missing a mapping for {verb:?}"
                );
            }
        }
    }

    #[test]
    fn official_repo_info_url_derives_raw_content_url_from_repo_url() {
        assert_eq!(
            official_repo_info_url("https://github.com/Derrick-Program/DPM-Workspace"),
            "https://raw.githubusercontent.com/Derrick-Program/DPM-Workspace/main/crates/dpm-server/RepoInfo.db"
        );
    }

    #[test]
    fn official_key_url_derives_raw_content_url_for_an_author() {
        assert_eq!(
            official_key_url("https://github.com/Derrick-Program/DPM-Workspace", "alice"),
            "https://raw.githubusercontent.com/Derrick-Program/DPM-Workspace/main/crates/dpm-server/keys/alice.pub"
        );
    }

    #[tokio::test]
    async fn init_first_run_persists_repo_url_and_repo_info_to_config() {
        let root = tempfile::tempdir().unwrap();
        let ctx = Context::for_test(root.path()).await.unwrap();
        let controller = SystemController::new(ctx.scope);

        let setting = controller.init_first_run(&ctx).await.unwrap();
        assert_eq!(setting.sources.len(), 1);
        assert_eq!(setting.sources[0].repo_url, OFFICIAL_REPO_URL);
        assert_eq!(
            setting.sources[0].repo_info,
            official_repo_info_url(OFFICIAL_REPO_URL)
        );

        let reloaded = controller.init_existing(&ctx).await.unwrap();
        assert_eq!(reloaded.sources.len(), 1);
        assert_eq!(reloaded.sources[0].repo_url, OFFICIAL_REPO_URL);
        assert_eq!(
            reloaded.sources[0].repo_info,
            official_repo_info_url(OFFICIAL_REPO_URL)
        );
    }

    #[tokio::test]
    async fn init_existing_automatically_migrates_deprecated_dpm_server_url() {
        let root = tempfile::tempdir().unwrap();
        let ctx = Context::for_test(root.path()).await.unwrap();
        let controller = SystemController::new(ctx.scope);

        controller.bootstrap_dirs(&ctx).unwrap();
        let legacy_setting = Setting {
            sources: vec![Source {
                alias: "official".to_string(),
                repo_url: "https://github.com/Derrick-Program/DPM-Server".to_string(),
                repo_info: "https://raw.githubusercontent.com/Derrick-Program/DPM-Server/main/RepoInfo.json?cb=1".to_string(),
            }],
        };
        dpm_core::TomlStorage::to_toml(&legacy_setting, &ctx.config_path()).unwrap();

        let migrated = controller.init_existing(&ctx).await.unwrap();
        assert_eq!(migrated.sources[0].repo_url, OFFICIAL_REPO_URL);
        assert_eq!(
            migrated.sources[0].repo_info,
            official_repo_info_url(OFFICIAL_REPO_URL)
        );
    }

    #[test]
    fn system_action_unknown_manager_methods_return_error() {
        let action = SystemAction {
            package_manager: PackageManager::Unknown,
            _verbose: false,
        };
        assert!(action.install_package("foo").is_err());
        assert!(action.update_package_index().is_err());
        assert!(action.uninstall_package("foo").is_err());
        assert!(action.search_package("foo").is_err());
        assert!(action.upgrade_package("foo").is_err());
        assert!(action.list_packages().is_err());
    }
}
