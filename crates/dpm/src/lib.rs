#![allow(non_snake_case)]
use std::collections::HashMap;
use std::path::PathBuf;
pub type Setting = HashMap<String, String>;
pub type Hashes = HashMap<String, String>;
use std::sync::OnceLock;
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

#[tokio::main]
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
                    pass_info.list(true)?;
                }
                if let Some(true) = options.List_installed {
                    pass_info.list(false)?;
                }
            }
        }
        CliCommands::Search => pass_info.search()?,
        CliCommands::Uninstall => pass_info.uninstall()?,
        CliCommands::Update => pass_info.update().await?,
        CliCommands::Upgrade => pass_info.upgrade()?,
        CliCommands::UpgradeSelf => pass_info.upgrade_self(),
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
pub fn set_globle_var() -> ClientResult<()> {
    MAIN_DIR.set(PathBuf::from("/opt/DPM")).unwrap();
    BIN_DIR.set(PathBuf::from("/opt/DPM/bin")).unwrap();
    INSTALL_DIR.set(PathBuf::from("/opt/DPM/Software")).unwrap();
    CONFIG.set(PathBuf::from("/opt/DPM/Settings")).unwrap();
    VERSION.set(env!("CARGO_PKG_VERSION").to_string()).unwrap();
    BIN.set("dpm".to_string()).unwrap();
    // 第一次執行時 /opt/DPM 可能不存在,先建立目錄並修正擁有者,
    // 否則下面 Db::new 建立 lock 檔會直接 Permission denied
    let main_dir = MAIN_DIR.get().unwrap();
    if !main_dir.exists() {
        SystemController.system_command_runner(
            "mkdir",
            vec!["-p", main_dir.to_str().unwrap()],
            "Can't create /opt/DPM dir",
        )?;
        SystemController.permision_check()?;
    }
    let db_path = MAIN_DIR.get().unwrap().join("LocalRepo.db");
    let mut db = Db::new(db_path.to_str().unwrap(), "/opt/DPM/LocalRepo.lock")
        .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?;
    db.run_migrations()?;
    DB_INSTANCE
        .set(db)
        .map_err(|_| "Failed to set DB_INSTANCE")
        .unwrap();
    Ok(())
}
