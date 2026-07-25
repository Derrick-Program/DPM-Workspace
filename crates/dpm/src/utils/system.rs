use super::ClientError;
use super::ClientResult;
use crate::{Context, Scope, Setting, Source};
use dpm_core::JsonStorage;
use libc::{getpwuid, getuid};
use std::{
    ffi::CStr,
    path::Path,
    process::{Command, Stdio},
};
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
    verbose: bool,
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
        if cfg!(target_os = "linux") {
            self.system_command_runner(
                "chown",
                vec!["-R", "root:root", main_dir.to_str().unwrap()],
                "Can't run chown",
            )?;
        } else if cfg!(target_os = "macos") {
            self.system_command_runner(
                "chown",
                vec![
                    "-R",
                    format!("{}:admin", username).as_str(),
                    main_dir.to_str().unwrap(),
                ],
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
    /// OS bootstrap only — no longer reaches into `ActionInfo` to seed the
    /// first-run source index (that used to make this "system utility"
    /// module the sole orchestrator of first-run business logic, while
    /// `ActionInfo` itself constructs a `SystemController`: a two-way
    /// dependency between the OS-utility seam and the command-orchestration
    /// seam). The `bool` tells the caller whether `config.json` was just
    /// created, so `entry()` — which already owns command dispatch — can
    /// decide whether to seed sources, instead of that decision living here.
    pub async fn init(&self, ctx: &Context) -> ClientResult<(Setting, bool)> {
        self.system_command_runner(
            "mkdir",
            vec!["-p", ctx.install_dir.to_str().unwrap()],
            "Can't create Software dir",
        )?;
        self.system_command_runner(
            "mkdir",
            vec!["-p", ctx.config_dir.to_str().unwrap()],
            "Can't create Settings dir",
        )?;
        self.system_command_runner(
            "mkdir",
            vec!["-p", ctx.bin_dir.to_str().unwrap()],
            "Can't create bin dir",
        )?;
        self.permision_check(&ctx.main_dir)?;
        let config_path = ctx.config_dir.join("config.json");
        let is_first_run = !config_path.exists();
        if is_first_run {
            let default_setting = Setting {
                sources: vec![Source {
                    alias: "official".to_string(),
                    repo_url: "https://github.com/Derrick-Program/DPM-Server".to_string(),
                    repo_info:
                        "https://raw.githubusercontent.com/Derrick-Program/DPM-Server/main/RepoInfo.json"
                            .to_string(),
                }],
            };
            JsonStorage::to_json(&default_setting, &config_path)?;
        }
        self.permision_check(&ctx.main_dir)?;
        let config: Setting = JsonStorage::from_json(&config_path)?;
        Ok((config, is_first_run))
    }
}

impl SystemAction {
    pub fn new(verbose: bool) -> Self {
        Self {
            package_manager: Self::detect_package_manager(),
            verbose,
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

        if self.verbose {
            cmd.stdout(Stdio::inherit());
            cmd.stderr(Stdio::inherit());
        } else {
            cmd.stdout(Stdio::null());
            cmd.stderr(Stdio::null());
        }

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
}
