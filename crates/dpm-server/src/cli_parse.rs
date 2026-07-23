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
}

#[derive(Args, Debug)]
pub struct Hash {
    /// Project Name
    pub packagename: String,
}

#[derive(Args, Debug)]
pub struct Build {
    /// Project Name
    pub packagename: String,
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
}

#[derive(Args, Debug)]
pub struct Del {
    /// Project Name
    pub project_name: String,
}

pub fn get_styles() -> clap::builder::Styles {
    clap::builder::Styles::styled()
        .usage(
            anstyle::Style::new()
                .bold()
                .underline()
                .fg_color(Some(anstyle::Color::Ansi(anstyle::AnsiColor::Yellow))),
        )
        .header(
            anstyle::Style::new()
                .bold()
                .underline()
                .fg_color(Some(anstyle::Color::Ansi(anstyle::AnsiColor::Yellow))),
        )
        .literal(
            anstyle::Style::new().fg_color(Some(anstyle::Color::Ansi(anstyle::AnsiColor::Green))),
        )
        .invalid(
            anstyle::Style::new()
                .bold()
                .fg_color(Some(anstyle::Color::Ansi(anstyle::AnsiColor::Red))),
        )
        .error(
            anstyle::Style::new()
                .bold()
                .fg_color(Some(anstyle::Color::Ansi(anstyle::AnsiColor::Red))),
        )
        .valid(
            anstyle::Style::new()
                .bold()
                .underline()
                .fg_color(Some(anstyle::Color::Ansi(anstyle::AnsiColor::Green))),
        )
        .placeholder(
            anstyle::Style::new().fg_color(Some(anstyle::Color::Ansi(anstyle::AnsiColor::White))),
        )
}
