// Crate name is intentionally `DPM` (PascalCase) for historical reasons —
// see CLAUDE.md's naming-conventions note. This is the only remaining
// reason for this allow; the PascalCase struct fields that used to need it
// are gone (Candidate 4's clap-derive migration).
#![allow(non_snake_case)]
use std::collections::HashMap;
pub type Hashes = HashMap<String, String>;
use serde::{Deserialize, Serialize};

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

pub async fn entry(ctx: Context, config: Cli) -> ClientResult<()> {
    let system_controller = SystemController::new(ctx.scope);
    let setting_config: Setting = system_controller.init(&ctx).await?;

    match config.command {
        Some(Commands::Install { pn, verbose }) => {
            ActionInfo::new(ctx.clone(), pn, verbose, setting_config)
                .install()
                .await?
        }
        Some(Commands::Update { verbose }) => {
            ActionInfo::new(ctx.clone(), vec![], verbose, setting_config)
                .update()
                .await?
        }
        Some(Commands::Uninstall { pn, verbose }) => {
            ActionInfo::new(ctx.clone(), pn, verbose, setting_config)
                .uninstall()
                .await?
        }
        Some(Commands::Search { pn, verbose }) => {
            ActionInfo::new(ctx.clone(), pn, verbose, setting_config)
                .search()
                .await?
        }
        Some(Commands::List {
            verbose,
            list_sys_installed,
            list_installed,
        }) => {
            let info = ActionInfo::new(ctx.clone(), vec![], verbose, setting_config);
            if list_sys_installed {
                info.list(true).await?;
            }
            if list_installed {
                info.list(false).await?;
            }
        }
        Some(Commands::Upgrade { verbose, pn }) => {
            ActionInfo::new(ctx.clone(), pn, verbose, setting_config)
                .upgrade()
                .await?
        }
        Some(Commands::UpgradeSelf { verbose }) => {
            ActionInfo::new(ctx.clone(), vec![], verbose, setting_config).upgrade_self()
        }
        Some(Commands::Source { action }) => {
            ActionInfo::new(ctx.clone(), vec![], false, setting_config)
                .source(action)
                .await?
        }
        // Only reachable via `--system` alone with no subcommand — `--gen`
        // already exits the process before returning a `Cli` (see
        // `get_args`), and `arg_required_else_help` catches the
        // truly-empty case.
        None => return Err(ClientError::ConfigError("no command given".to_string())),
    }
    system_controller.permision_check(&ctx.main_dir)?;
    Ok(())
}
