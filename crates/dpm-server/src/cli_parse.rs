use clap::{Args, Subcommand};
#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Hash File or all in Project File
    Hash(Hash),
    /// Fix Packages.json
    Fix(Fix),
    /// Build Package
    Build(Build),
    ///Create Project
    Init(Init),
    /// Generate an ed25519 signing key pair for a package author
    Keygen(Keygen),
    /// Sign a package's packageInfo.json hash with its author's private key
    Sign(Sign),
    /// Generate a default config.toml at the user config layer
    #[command(visible_alias = "gc")]
    GenConfig(GenConfig),
}

#[derive(Args, Debug)]
pub struct Hash {
    /// Project Name
    pub package_name: String,
    /// Build command for a `kind: source` package — when given, computes a
    /// signable hash from this command + the current git HEAD commit
    /// instead of walking `packages/<name>/`'s files.
    #[arg(long)]
    pub build: Option<String>,
}

#[derive(Args, Debug)]
pub struct Build {
    /// Project Name
    pub package_name: String,
}
#[derive(Args, Debug)]
pub struct Init {
    /// Project Name
    pub name: String,
    ///Project Entry
    pub entry: String,
    #[arg(long, short = 'v', default_value = "0.1.0")]
    ///Project Version
    pub ver: String,
    #[arg(long, short = 'd', default_value = "description")]
    ///Project Description
    pub description: String,
    /// Author id this package's key belongs to (see `dpm-server keygen`)
    #[arg(long)]
    pub author: String,
}

#[derive(Args, Debug)]
pub struct Fix {
    #[command(subcommand)]
    pub command: FixAction,
}

#[derive(Subcommand, Debug)]
pub enum FixAction {
    /// add Package to Packages.json
    Add(Add),
    /// delete Package from Packages.json
    Del(Del),
}

#[derive(Args, Debug)]
pub struct Add {
    /// Project Name
    pub project_name: String,
    #[command(subcommand)]
    pub kind: AddKind,
}

/// Mirrors `dpm_core::PackageKind`'s two variants at the CLI layer: exactly
/// one of "publish from a URL" or "publish a source build" must be chosen.
/// Previously `url`/`build` were two independently-optional `Add` fields
/// with `conflicts_with` wiring them together, which only rejects the
/// illegal "both" state at parse time — `fix_add` still had to defend
/// against it (`unreachable!()`) and against "neither" with a runtime
/// `ValidationError`. A subcommand makes "exactly one" a parse-time
/// guarantee: there is no `AddKind` value that isn't a legal `PackageKind`.
#[derive(Subcommand, Debug)]
pub enum AddKind {
    /// Publish a prebuilt package hosted at a URL
    Url {
        /// External URL hosting the prebuilt package archive. dpm-server
        /// downloads it once to compute its blake3 hash — it does not keep
        /// a copy. Must be https://.
        url: String,
        /// Override the file name recorded in RepoInfo.json (defaults to
        /// the URL's last path segment)
        #[arg(long)]
        file_name: Option<String>,
    },
    /// Publish a source package clients build locally
    Build {
        /// Shell command clients run locally to build this package from
        /// source. $OUT will point at the install destination when clients
        /// actually run it (Phase 4 client-side work).
        build: String,
    },
}

// `disable_version_flag`: the derived positional `version` field's arg id collides
// with clap's auto `-V`/`--version` flag (from `propagate_version = true` on the
// top-level `Cli`), which trips a debug_assert panic at runtime otherwise.
#[derive(Args, Debug)]
#[command(disable_version_flag = true)]
pub struct Del {
    /// Project Name
    pub project_name: String,
    /// Version to remove (required if the package has more than one published version)
    pub version: Option<String>,
}

#[derive(Args, Debug)]
pub struct Keygen {
    /// Author id (e.g. a GitHub username) this key belongs to
    pub author_id: String,
    /// Overwrite an existing key pair for this author
    #[arg(long)]
    pub force: bool,
}

#[derive(Args, Debug)]
pub struct Sign {
    /// Project Name
    pub name: String,
}

#[derive(Args, Debug)]
pub struct GenConfig {
    /// Overwrite an existing user-tier config.toml
    #[arg(long)]
    pub force: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    /// The real top-level `Cli` lives in `main.rs` (a binary crate root), so
    /// it isn't reachable from here. This wrapper exercises exactly the same
    /// `Commands` subcommand parsing without needing it.
    #[derive(Parser, Debug)]
    struct TestCli {
        #[command(subcommand)]
        command: Commands,
    }

    #[test]
    fn gen_config_parses_force_flag() {
        let cli = TestCli::try_parse_from(["dpm-server", "gen-config", "--force"]).unwrap();
        match cli.command {
            Commands::GenConfig(obj) => assert!(obj.force),
            other => panic!("expected Commands::GenConfig, got {other:?}"),
        }
    }

    #[test]
    fn gen_config_gc_alias_parses() {
        let cli = TestCli::try_parse_from(["dpm-server", "gc"]).unwrap();
        match cli.command {
            Commands::GenConfig(obj) => assert!(!obj.force),
            other => panic!("expected Commands::GenConfig, got {other:?}"),
        }
    }
}
