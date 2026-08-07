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

    // `gen-config` 必須在「檔案不存在就自動 seed 預設值」那段邏輯之前處理
    // 完畢並直接回傳——不然全新安裝時,第一次呼叫 `gen-config` 會先被
    // 下面的 first-run 邏輯自動寫出預設檔案,`gen_config` 自己再看到「檔案
    // 已存在」而要求 `--force`,對使用者來說是很confusing 的雙重寫入。
    // 用 `&config.command` 借用、不是 `config.command` 移動所有權——下面
    // 第二個 `match config.command { ... }` 之後還要按值 match 同一個
    // 欄位(其他分支要把 `pn: Vec<String>` 這類欄位移進
    // `ActionInfo::new(...)`),這裡先用引用形式只是「偷看一眼是不是
    // GenConfig」,不能把它整個消耗掉,不然下面那個 match 會編譯失敗
    // (use of moved value)。`force` 因此綁定成 `&bool`,呼叫
    // `gen_config` 前用 `*force` 解引用成 `bool`(`bool` 是 `Copy`,解引用
    // 沒有所有權問題)。
    if let Some(Commands::GenConfig { force }) = &config.command {
        let path = system_controller.gen_config(&ctx, *force).await?;
        println!("wrote default config to {}", path.display());
        return Ok(());
    }

    let config_path = ctx.config_path();
    let setting_config = if !config_path.exists() {
        let setting = system_controller.init_first_run(&ctx).await?;
        for source in &setting.sources {
            ActionInfo::init_update(&ctx, source).await?;
        }
        setting
    } else {
        system_controller.init_existing(&ctx).await?
    };

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
        Some(Commands::Info { pn, verbose }) => {
            ActionInfo::new(ctx.clone(), pn, verbose, setting_config)
                .info()
                .await?
        }
        Some(Commands::List {
            verbose,
            sys_mgr,
            outdated,
        }) => {
            let info = ActionInfo::new(ctx.clone(), vec![], verbose, setting_config);
            info.list(sys_mgr, outdated).await?;
        }
        Some(Commands::Upgrade { verbose, pn }) => {
            ActionInfo::new(ctx.clone(), pn, verbose, setting_config)
                .upgrade()
                .await?
        }
        Some(Commands::UpgradeSelf { verbose }) => {
            ActionInfo::new(ctx.clone(), vec![], verbose, setting_config)
                .upgrade_self()
                .await?
        }
        Some(Commands::Pin { pn, verbose }) => {
            ActionInfo::new(ctx.clone(), pn, verbose, setting_config)
                .pin(true)
                .await?
        }
        Some(Commands::Unpin { pn, verbose }) => {
            ActionInfo::new(ctx.clone(), pn, verbose, setting_config)
                .pin(false)
                .await?
        }
        Some(Commands::Autoremove { verbose }) => {
            ActionInfo::new(ctx.clone(), vec![], verbose, setting_config)
                .autoremove()
                .await?
        }
        Some(Commands::Source { action }) => {
            ActionInfo::new(ctx.clone(), vec![], false, setting_config)
                .source(action)
                .await?
        }
        // 已經在函式最前面攔截並提早回傳了,這裡理論上永遠不會執行到。
        Some(Commands::GenConfig { .. }) => unreachable!("GenConfig is handled earlier in entry()"),
        // Only reachable via `--system` alone with no subcommand — `--gen`
        // already exits the process before returning a `Cli` (see
        // `get_args`), and `arg_required_else_help` catches the
        // truly-empty case.
        None => return Err(ClientError::ConfigError("no command given".to_string())),
    }
    system_controller.permision_check(&ctx.main_dir)?;
    Ok(())
}
