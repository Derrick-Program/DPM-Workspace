mod action;
mod cli_parse;
mod config;
mod error;
pub use action::*;
use anyhow::Result;
use clap::Parser;
pub use cli_parse::*;
pub use config::*;
use dpm_core::*;
pub use error::*;

use std::{env::current_dir, fs::create_dir_all};
// pub type Repos = HashMap<String, RepoInfo>;
#[derive(Parser)]
#[command(propagate_version = true)]
#[command(
    version,
    about,
    long_about = "Derrick Package Manager Server (DPM-Server)",
    styles = get_styles(),
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}
fn main() -> Result<()> {
    let cli = Cli::parse();

    if let Commands::GenConfig(obj) = &cli.command {
        let path = user_config_path()?;
        gen_config(&path, obj.force)?;
        println!("wrote default config to {}", path.display());
        return Ok(());
    }

    let cfg = load_or_init(&system_config_path(), &user_config_path()?, "DPM_SERVER")?;
    let project_src = current_dir()?.join(&cfg.project_src);
    let repo_dir = current_dir()?.join(&cfg.repo_dir);
    let keys_dir = current_dir()?.join(&cfg.keys_dir);
    let software_repo_info = current_dir()?.join(&cfg.repo_info);
    create_dir_all(&project_src)?;
    create_dir_all(&repo_dir)?;
    create_dir_all(&keys_dir)?;
    let conn = rusqlite::Connection::open(&software_repo_info)
        .map_err(|e| anyhow::anyhow!("Failed to open RepoInfo.db: {}", e))?;
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
        [],
    )
    .map_err(|e| anyhow::anyhow!("Failed to initialize RepoInfo.db table: {}", e))?;

    match &cli.command {
        Commands::Hash(obj) => hash(obj, &project_src, &repo_dir)?,
        Commands::Fix(obj) => fix(obj, &conn, &project_src, &keys_dir)?,
        Commands::Build(obj) => build(obj, &project_src, &repo_dir)?,
        Commands::Init(obj) => init(obj, &project_src, &keys_dir)?,
        Commands::Keygen(obj) => keygen(obj, &keys_dir)?,
        Commands::Sign(obj) => sign(obj, &project_src, &keys_dir)?,
        // 已經在函式最前面攔截並提早回傳了,這裡理論上永遠不會執行到。
        Commands::GenConfig(_) => unreachable!("GenConfig is handled earlier in main()"),
    }
    Ok(())
}
