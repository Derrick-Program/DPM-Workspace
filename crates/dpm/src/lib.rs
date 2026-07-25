#![allow(non_snake_case)]
use std::collections::HashMap;
pub type Hashes = HashMap<String, String>;
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
mod context;
mod utils;
pub use action::*;
pub use arch::*;
pub use cli_parse::*;
pub use context::*;
pub use utils::*;
static VERSION: OnceLock<String> = OnceLock::new();
static BIN: OnceLock<String> = OnceLock::new();

pub async fn entry(ctx: Context, config: Cli) -> ClientResult<()> {
    let system_controller = SystemController::new(ctx.scope);
    let setting_config: Setting = system_controller.init(&ctx).await?;
    let pass_info = ActionInfo::new(
        ctx.clone(),
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
    system_controller.permision_check(&ctx.main_dir)?;
    Ok(())
}

/// 設定跟 scope 無關的 CLI metadata(clap 建構 Command 需要),
/// 必須在 get_args() 之前呼叫,因為 scope 要等 get_args() 解析完 --system 才知道。
pub fn init_cli_metadata() {
    VERSION.set(env!("CARGO_PKG_VERSION").to_string()).unwrap();
    BIN.set("dpm".to_string()).unwrap();
}
