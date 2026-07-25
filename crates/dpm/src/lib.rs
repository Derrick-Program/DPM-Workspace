#![allow(non_snake_case)]
use std::collections::HashMap;
use std::path::PathBuf;
pub type Hashes = HashMap<String, String>;
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Source {
    pub alias: String,
    /// Git-clonable remote URL (e.g. `https://github.com/owner/repo`) — this
    /// source's own repo, where `packages/<pkg>/` lives for source-kind
    /// installs. Must be a real clone target, not a human-facing web page.
    pub repo_url: String,
    pub repo_info: String,
}

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct Setting {
    #[serde(default)]
    pub sources: Vec<Source>,
}
mod action;
mod arch;
mod cli_parse;
mod utils;
pub use action::*;
pub use arch::*;
pub use cli_parse::*;
use dpm_core::CoreError::DatabaseError;
pub use utils::*;
static MAIN_DIR: OnceLock<PathBuf> = OnceLock::new();
static BIN_DIR: OnceLock<PathBuf> = OnceLock::new();
static INSTALL_DIR: OnceLock<PathBuf> = OnceLock::new();
static CONFIG: OnceLock<PathBuf> = OnceLock::new();
static VERSION: OnceLock<String> = OnceLock::new();
static BIN: OnceLock<String> = OnceLock::new();
static DB_INSTANCE: OnceLock<Db> = OnceLock::new();
static SCOPE: OnceLock<Scope> = OnceLock::new();

pub async fn entry(config: Cli) -> ClientResult<()> {
    let setting_config: Setting = SystemController.init().await?;
    let pass_info = ActionInfo::new(
        config.PackageName.unwrap_or_default(),
        config.Verbose,
        setting_config,
    );
    match config.Commands.unwrap() {
        CliCommands::Install => pass_info.install().await?,
        CliCommands::List => {
            if let Some(options) = &config.Other {
                if let Some(true) = options.List_sys_installed {
                    pass_info.list(true).await?;
                }
                if let Some(true) = options.List_installed {
                    pass_info.list(false).await?;
                }
            }
        }
        CliCommands::Search => pass_info.search().await?,
        CliCommands::Uninstall => pass_info.uninstall().await?,
        CliCommands::Update => pass_info.update().await?,
        CliCommands::Upgrade => pass_info.upgrade().await?,
        CliCommands::UpgradeSelf => pass_info.upgrade_self(),
        CliCommands::Source(action) => pass_info.source(action).await?,
        CliCommands::None => panic!("No command found"),
    }
    SystemController.permision_check()?;
    // JsonStorage::to_json(&config, &config_path);
    Ok(())
}

pub fn get_db() -> &'static Db {
    DB_INSTANCE
        .get()
        .expect("Database instance not initialized")
}

fn compute_paths(scope: Scope) -> ClientResult<(PathBuf, PathBuf, PathBuf, PathBuf)> {
    match scope {
        Scope::PerUser => {
            let proj_dirs = ProjectDirs::from("com", "duacodie", "dpm").ok_or_else(|| {
                ClientError::SystemError("no valid home directory found".to_string())
            })?;
            let data_dir = proj_dirs.data_dir().to_path_buf();
            Ok((
                data_dir.clone(),
                data_dir.join("bin"),
                data_dir.join("Software"),
                proj_dirs.config_dir().to_path_buf(),
            ))
        }
        Scope::System => {
            let root = PathBuf::from("/opt/com.duacodie/DPM");
            Ok((
                root.clone(),
                root.join("bin"),
                root.join("Software"),
                root.join("Settings"),
            ))
        }
    }
}

/// 設定跟 scope 無關的 CLI metadata(clap 建構 Command 需要),
/// 必須在 get_args() 之前呼叫,因為 scope 要等 get_args() 解析完 --system 才知道。
pub fn init_cli_metadata() {
    VERSION.set(env!("CARGO_PKG_VERSION").to_string()).unwrap();
    BIN.set("dpm".to_string()).unwrap();
}

pub async fn set_globle_var(scope: Scope) -> ClientResult<()> {
    SCOPE.set(scope).unwrap();
    let (main_dir, bin_dir, install_dir, config_dir) = compute_paths(scope)?;
    MAIN_DIR.set(main_dir).unwrap();
    BIN_DIR.set(bin_dir).unwrap();
    INSTALL_DIR.set(install_dir).unwrap();
    CONFIG.set(config_dir).unwrap();
    // 第一次執行時目錄可能不存在,先建立目錄並修正擁有者(system scope 才需要),
    // 否則下面 Db::new 建立 lock 檔會直接 Permission denied
    let main_dir = MAIN_DIR.get().unwrap();
    if !main_dir.exists() {
        SystemController.system_command_runner(
            "mkdir",
            vec!["-p", main_dir.to_str().unwrap()],
            "Can't create DPM main dir",
        )?;
        SystemController.permision_check()?;
    }
    let db_path = MAIN_DIR.get().unwrap().join("LocalRepo.db");
    let lock_path = MAIN_DIR.get().unwrap().join("LocalRepo.lock");
    let db = Db::new(db_path.to_str().unwrap(), lock_path.to_str().unwrap())
        .await
        .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?;
    db.run_migrations().await?;
    DB_INSTANCE
        .set(db)
        .map_err(|_| "Failed to set DB_INSTANCE")
        .unwrap();
    Ok(())
}

#[cfg(test)]
mod scope_tests {
    use super::*;

    #[test]
    fn per_user_and_system_scopes_produce_different_roots() {
        let (per_user_main, _, _, _) = compute_paths(Scope::PerUser).unwrap();
        let (system_main, system_bin, system_install, system_config) =
            compute_paths(Scope::System).unwrap();
        assert_ne!(per_user_main, system_main);
        assert_eq!(system_main, PathBuf::from("/opt/com.duacodie/DPM"));
        assert_eq!(system_bin, PathBuf::from("/opt/com.duacodie/DPM/bin"));
        assert_eq!(
            system_install,
            PathBuf::from("/opt/com.duacodie/DPM/Software")
        );
        assert_eq!(
            system_config,
            PathBuf::from("/opt/com.duacodie/DPM/Settings")
        );
    }
}
