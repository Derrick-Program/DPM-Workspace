mod action;
mod cli_parse;
// mod json_parse;
mod zip_file;
pub use action::*;
use anyhow::Result;
use clap::Parser;
pub use cli_parse::*;
use dpm_core::*;

// pub use json_parse::*;
use std::sync::OnceLock;
use std::{env::current_dir, fs::create_dir_all, path::PathBuf};
pub use zip_file::*;
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
static PROJECT_SRC: OnceLock<PathBuf> = OnceLock::new();
fn main() -> Result<()> {
    let cli = Cli::parse();
    let repo_src = current_dir()?.join("Repo/src");
    PROJECT_SRC.set(repo_src.clone()).unwrap();
    let software_repo_info = current_dir()?.join("RepoInfo.json");
    create_dir_all(repo_src)?;
    let mut repo_info: RepoInfo;
    if !software_repo_info.exists() {
        println!("RepoInfo.json not found. Initializing a new one.");
        repo_info = RepoInfo::new();
        repo_init(&mut repo_info)?;
    } else {
        println!("Loading RepoInfo.json...");
        repo_info = JsonStorage::from_json(&software_repo_info).unwrap_or_else(|_| {
            println!("Failed to parse RepoInfo.json. Initializing as empty.");
            RepoInfo::new()
        });
    }
    match &cli.command {
        Commands::Hash(obj) => hash(obj)?,
        Commands::Fix(obj) => fix(obj, &mut repo_info)?,
        Commands::Build(obj) => build(obj)?,
        Commands::Init(obj) => init(obj)?,
    }
    JsonStorage::to_json(&repo_info, &software_repo_info)?;
    Ok(())
}
