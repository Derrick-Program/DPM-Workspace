mod action;
mod cli_parse;
mod error;
pub use action::*;
use anyhow::Result;
use clap::Parser;
pub use cli_parse::*;
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
    let project_src = current_dir()?.join("packages");
    let repo_dir = current_dir()?.join("Repo");
    let keys_dir = current_dir()?.join("keys");
    let software_repo_info = current_dir()?.join("RepoInfo.json");
    create_dir_all(&project_src)?;
    create_dir_all(&repo_dir)?;
    create_dir_all(&keys_dir)?;
    let mut repo_info: RepoInfo;
    if !software_repo_info.exists() {
        println!("RepoInfo.json not found. Initializing an empty one.");
        repo_info = RepoInfo::new();
    } else {
        println!("Loading RepoInfo.json...");
        repo_info = JsonStorage::from_json(&software_repo_info).unwrap_or_else(|_| {
            eprintln!("Warning: failed to parse RepoInfo.json. Initializing as empty — saving after this run will overwrite the unparseable file.");
            RepoInfo::new()
        });
    }
    match &cli.command {
        Commands::Hash(obj) => hash(obj, &project_src)?,
        Commands::Fix(obj) => fix(obj, &mut repo_info, &project_src)?,
        Commands::Build(obj) => build(obj, &project_src, &repo_dir)?,
        Commands::Init(obj) => init(obj, &project_src)?,
        Commands::Keygen(obj) => keygen(obj, &keys_dir)?,
    }
    JsonStorage::to_json(&repo_info, &software_repo_info)?;
    Ok(())
}
