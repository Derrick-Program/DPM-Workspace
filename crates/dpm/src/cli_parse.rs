use clap::{ColorChoice, CommandFactory, Parser, Subcommand};
use clap_complete::{generate, Generator, Shell};
use dpm_core::get_styles;
use std::io;

/// `dpm`'s CLI, built with `#[derive(Parser)]` — matches `dpm-server`'s
/// derive style instead of hand-building a `clap::Command` and
/// hand-matching every subcommand a second time in a separate function.
/// The two used to be kept in sync by hand on every new subcommand; clap's
/// derive macros generate both from this one definition.
#[derive(Parser, Debug)]
#[command(
    name = "dpm",
    version,
    author = "Derrick Lin",
    about = "Derrick Package Manager (DPM)",
    color = ColorChoice::Always,
    styles = get_styles(),
    propagate_version = true,
    arg_required_else_help = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// Operate on the shared system-wide install (requires root)
    #[arg(short = 'S', long = "system", global = true)]
    pub system: bool,

    /// Generate a shell completion script and exit
    #[arg(short = 'g', long = "gen", visible_aliases = ["gen", "generator", "autocomplete", "complete"])]
    pub generator: Option<Shell>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Install Package
    #[command(visible_aliases = ["i", "add", "inst"], arg_required_else_help = true)]
    Install {
        #[arg(value_name = "Package Name", required = true)]
        pn: Vec<String>,
        #[arg(short, long)]
        verbose: bool,
    },
    /// Update Package
    #[command(visible_aliases = ["ud", "upda", "up"])]
    Update {
        #[arg(short, long)]
        verbose: bool,
    },
    /// Uninstall Package
    #[command(visible_aliases = ["un", "i!", "unin"], arg_required_else_help = true)]
    Uninstall {
        #[arg(value_name = "Package name", required = true)]
        pn: Vec<String>,
        #[arg(short, long)]
        verbose: bool,
    },
    /// Search Package
    #[command(visible_aliases = ["s", "se", "sea"], arg_required_else_help = true)]
    Search {
        #[arg(value_name = "Package name", required = true)]
        pn: Vec<String>,
        #[arg(short, long)]
        verbose: bool,
    },
    /// List can install Package
    #[command(visible_aliases = ["l", "li", "ll"], arg_required_else_help = true)]
    List {
        #[arg(short, long)]
        verbose: bool,
        /// List System installed Package
        #[arg(short = 's', long = "list-sys")]
        list_sys_installed: bool,
        /// List installed Package
        #[arg(short = 'l', long = "list")]
        list_installed: bool,
    },
    /// Upgrade Package
    #[command(visible_aliases = ["U", "UP", "grade"], arg_required_else_help = true)]
    Upgrade {
        #[arg(short, long)]
        verbose: bool,
        #[arg(value_name = "Package name", required = true)]
        pn: Vec<String>,
    },
    /// Upgrade Self
    #[command(visible_aliases = ["US", "UPS", "grades"])]
    UpgradeSelf {
        #[arg(short, long)]
        verbose: bool,
    },
    /// Manage package sources
    #[command(subcommand_required = true, arg_required_else_help = true)]
    Source {
        #[command(subcommand)]
        action: SourceAction,
    },
    /// Generate a default config.toml at the user config layer
    #[command(visible_alias = "gc")]
    GenConfig {
        /// Overwrite an existing user-tier config.toml
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum SourceAction {
    /// Add a package source
    Add {
        #[arg(value_name = "URL")]
        url: String,
        /// Alias for this source (defaults to the URL host)
        #[arg(long = "as", value_name = "ALIAS")]
        alias: Option<String>,
    },
    /// Remove a package source
    Remove {
        #[arg(value_name = "ALIAS")]
        alias: String,
    },
    /// List configured package sources
    List,
}

/// Kept for `clap_complete`'s shell-completion generation (needs a `&mut
/// Command`, derive or builder style makes no difference there) and for
/// `cli_parse_tests.rs`'s direct-`ArgMatches` coverage of the `source`
/// subcommand tree.
pub fn build_cli() -> clap::Command {
    Cli::command()
}

fn print_completions<G: Generator>(gen: G, cmd: &mut clap::Command) {
    generate(gen, cmd, cmd.get_name().to_string(), &mut io::stdout());
    std::process::exit(0);
}

pub fn get_args() -> anyhow::Result<Cli> {
    let cli = Cli::parse();
    if let Some(generator) = cli.generator {
        eprintln!("Generating completion file for {generator}...");
        let mut cmd = Cli::command();
        print_completions(generator, &mut cmd);
    }
    Ok(cli)
}
