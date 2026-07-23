use super::ClientError;
use super::ClientResult;
use crate::{ActionInfo, Setting, BIN_DIR, CONFIG, INSTALL_DIR, MAIN_DIR};
use dpm_core::CoreError::*;
use dpm_core::JsonStorage;
use libc::{getpwuid, getuid};
use std::{
    collections::HashMap,
    ffi::CStr,
    fs::File,
    io::Write,
    process::{Command, Stdio},
};
#[derive(Debug)]
enum PackageManager {
    Apt,
    Dnf,
    Yum,
    Pacman,
    Zypper,
    Brew,
    Unknown,
}
#[derive(Debug)]
pub struct SystemAction {
    package_manager: PackageManager,
    verbose: bool,
}
#[derive(Debug)]
pub struct SystemController;

impl SystemController {
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
    pub fn permision_check(&self) -> ClientResult<()> {
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
                vec!["-R", "root:root", MAIN_DIR.get().unwrap().to_str().unwrap()],
                "Can't run chown",
            )?;
        } else if cfg!(target_os = "macos") {
            self.system_command_runner(
                "chown",
                vec![
                    "-R",
                    format!("{}:admin", username).as_str(),
                    MAIN_DIR.get().unwrap().to_str().unwrap(),
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
            panic!("Unsupported OS");
        }
        let mut cmd = Command::new("sudo");
        cmd.arg(command);
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
    pub async fn init(&self) -> ClientResult<Setting> {
        self.system_command_runner(
            "mkdir",
            vec!["-p", INSTALL_DIR.get().unwrap().to_str().unwrap()],
            "Can't '/opt/DPM/Software' dir",
        )?;
        self.system_command_runner(
            "mkdir",
            vec!["-p", CONFIG.get().unwrap().to_str().unwrap()],
            "Can't /opt/DPM/Setting dir",
        )?;
        self.system_command_runner(
            "mkdir",
            vec!["-p", BIN_DIR.get().unwrap().to_str().unwrap()],
            "Can't /opt/DPM/bin dir",
        )?;
        self.permision_check()?;
        let config_path = CONFIG.get().unwrap().join("config.json");
        if !config_path.exists() {
            let mut file = File::create(&config_path).map_err(|e| ClientError::Core(IoError(e)))?;
            file.write_all(b"{}")
                .map_err(|e| ClientError::Core(IoError(e)))?;
            let mut config: Setting =
                JsonStorage::from_json(&config_path).unwrap_or_else(|_| HashMap::new());
            config.insert(
                "repo_url".to_string(),
                "https://github.com/Derrick-Program/DPM-Server/tree/main/Repo".to_string(),
            );
            config.insert(
                "repo_info".to_string(),
                "https://raw.githubusercontent.com/Derrick-Program/DPM-Server/main/RepoInfo.json"
                    .to_string(),
            );

            ActionInfo::init_update(config.get("repo_info").unwrap()).await?;
        }
        self.permision_check()?;
        let config: Setting = JsonStorage::from_json(&config_path)?;
        Ok(config)
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
    pub fn install_package(&self, package_name: &str) -> ClientResult<()> {
        let err = format!("Failed to install package: {}", package_name);
        let err = err.as_str();
        let (command, args) = match self.package_manager {
            PackageManager::Apt => ("apt-get", vec!["install", "-y", package_name]),
            PackageManager::Dnf => ("dnf", vec!["install", "-y", package_name]),
            PackageManager::Yum => ("yum", vec!["install", "-y", package_name]),
            PackageManager::Pacman => ("pacman", vec!["-S", "--noconfirm", package_name]),
            PackageManager::Zypper => ("zypper", vec!["install", "-y", package_name]),
            PackageManager::Brew => ("brew", vec!["install", package_name]),
            PackageManager::Unknown => panic!("Unsupported package manager."),
        };
        self.command_runner(command, args, err)?;
        Ok(())
    }
    pub fn update_package_index(&self) -> ClientResult<()> {
        let err = "Failed to update package index";
        let (command, args) = match self.package_manager {
            PackageManager::Apt => ("apt-get", vec!["update"]),
            PackageManager::Dnf | PackageManager::Yum => ("dnf", vec!["makecache"]),
            PackageManager::Pacman => ("pacman", vec!["-Sy"]),
            PackageManager::Zypper => ("zypper", vec!["refresh"]),
            PackageManager::Brew => ("brew", vec!["update"]),
            PackageManager::Unknown => panic!("Unsupported package manager."),
        };
        self.command_runner(command, args, err)?;
        Ok(())
    }
    pub fn uninstall_package(&self, package_name: &str) -> ClientResult<()> {
        let err = format!("Failed to remove package: {}", package_name);
        let err = err.as_str();
        let (command, args) = match self.package_manager {
            PackageManager::Apt => ("apt-get", vec!["remove", "-y", package_name]),
            PackageManager::Dnf | PackageManager::Yum => {
                ("dnf", vec!["remove", "-y", package_name])
            }
            PackageManager::Pacman => ("pacman", vec!["-R", package_name]),
            PackageManager::Zypper => ("zypper", vec!["remove", "-y", package_name]),
            PackageManager::Brew => ("brew", vec!["uninstall", package_name]),
            PackageManager::Unknown => panic!("Unsupported package manager."),
        };
        self.command_runner(command, args, err)?;
        Ok(())
    }
    pub fn search_package(&self, package_name: &str) -> ClientResult<()> {
        let err = format!("Failed to search package: {}", package_name);
        let err = err.as_str();
        let (command, args) = match self.package_manager {
            PackageManager::Apt => ("apt-cache", vec!["search", package_name]),
            PackageManager::Dnf => ("dnf", vec!["search", package_name]),
            PackageManager::Yum => ("yum", vec!["search", package_name]),
            PackageManager::Pacman => ("pacman", vec!["-Ss", package_name]),
            PackageManager::Zypper => ("zypper", vec!["search", package_name]),
            PackageManager::Brew => ("brew", vec!["search", package_name]),
            PackageManager::Unknown => panic!("Unsupported package manager."),
        };
        self.command_runner(command, args, err)?;
        Ok(())
    }
    pub fn upgrade_package(&self, package_name: &str) -> ClientResult<()> {
        let err = format!("Failed to upgrade package: {}", package_name);
        let err = err.as_str();
        let (command, args) = match self.package_manager {
            PackageManager::Apt => (
                "apt-get",
                vec!["install", "--only-upgrade", "-y", package_name],
            ),
            PackageManager::Dnf | PackageManager::Yum => {
                ("dnf", vec!["upgrade", "-y", package_name])
            }
            PackageManager::Pacman => ("pacman", vec!["-Syu", package_name]),
            PackageManager::Zypper => ("zypper", vec!["update", "-y", package_name]),
            PackageManager::Brew => ("brew", vec!["upgrade", package_name]),
            PackageManager::Unknown => panic!("Unsupported package manager."),
        };
        self.command_runner(command, args, err)?;
        Ok(())
    }

    pub fn list_packages(&self) -> ClientResult<()> {
        let err = "Failed to list packages";
        let (command, args) = match self.package_manager {
            PackageManager::Apt => ("apt", vec!["list", "--installed"]),
            PackageManager::Dnf => ("dnf", vec!["list", "installed"]),
            PackageManager::Yum => ("yum", vec!["list", "installed"]),
            PackageManager::Pacman => ("pacman", vec!["-Q"]),
            PackageManager::Zypper => ("zypper", vec!["search", "--installed-only"]),
            PackageManager::Brew => ("brew", vec!["list"]),
            PackageManager::Unknown => panic!("Unsupported package manager."),
        };

        self.command_runner(command, args, err)?;
        Ok(())
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
