#![allow(warnings)]
use crate::{Cli, CliCommands, Option_set, SourceAction, BIN, VERSION};
use anyhow::*;
use clap::{value_parser, Arg, ArgAction, ArgGroup, ColorChoice, Command, ValueHint};
use clap_complete::{generate, Generator, Shell};
use std::io;

pub fn build_cli() -> Command {
    Command::new(
        BIN.get()
            .unwrap_or_else(|| panic!("BIN is not initialized"))
            .as_str(),
    )
    .version(VERSION.get().unwrap().as_str())
    .color(ColorChoice::Always)
    .styles(get_styles())
    .author("Derrick Lin")
    .about("Derrick Package Manager (DPM)")
    .propagate_version(true)
    .arg_required_else_help(true)
    .subcommands([
        Command::new("install")
            .about("Install Package")
            .visible_aliases(["i", "add", "inst"])
            .arg_required_else_help(true)
            .arg(
                Arg::new("PN")
                    .value_name("Package Name")
                    .required(true)
                    .action(ArgAction::Append),
            )
            .arg(
                Arg::new("verbose")
                    .help("Verbose")
                    .short('v')
                    .long("verbose")
                    .action(ArgAction::SetTrue),
            ),
        Command::new("update")
            .about("Update Package")
            .visible_aliases(["ud", "upda", "up"])
            // .arg_required_else_help(true)
            // .arg(
            //     Arg::new("PN")
            //         .value_name("Package name")
            //         .required(true)
            //         .action(ArgAction::Append),
            // )
            .arg(
                Arg::new("verbose")
                    .help("Verbose")
                    .short('v')
                    .long("verbose")
                    .action(ArgAction::SetTrue),
            ),
        Command::new("uninstall")
            .about("Uninstall Package")
            .arg_required_else_help(true)
            .visible_aliases(["un", "i!", "unin"])
            .arg(
                Arg::new("PN")
                    .value_name("Package name")
                    .required(true)
                    .action(ArgAction::Append),
            )
            .arg(
                Arg::new("verbose")
                    .help("Verbose")
                    .short('v')
                    .long("verbose")
                    .action(ArgAction::SetTrue),
            ),
        Command::new("search")
            .about("Search Package")
            .arg_required_else_help(true)
            .visible_aliases(["s", "se", "sea"])
            .arg(
                Arg::new("PN")
                    .value_name("Package name")
                    .required(true)
                    .action(ArgAction::Append),
            )
            .arg(
                Arg::new("verbose")
                    .help("Verbose")
                    .short('v')
                    .long("verbose")
                    .action(ArgAction::SetTrue),
            ),
        Command::new("list")
            .about("List can install Package")
            .visible_aliases(["l", "li", "ll"])
            .arg_required_else_help(true)
            .arg(
                Arg::new("verbose")
                    .help("Verbose")
                    .short('v')
                    .long("verbose")
                    .action(ArgAction::SetTrue),
            )
            .arg(
                Arg::new("list-sys-installed")
                    .help("List System installed Package")
                    .short('s')
                    .long("list-sys")
                    .action(ArgAction::SetTrue),
            )
            .arg(
                Arg::new("list-installed")
                    .help("List installed Package")
                    .short('l')
                    .long("list")
                    .action(ArgAction::SetTrue),
            ),
        Command::new("upgrade")
            .about("Upgrade Package")
            .arg_required_else_help(true)
            .visible_aliases(["U", "UP", "grade"])
            .arg(
                Arg::new("verbose")
                    .help("Verbose")
                    .short('v')
                    .long("verbose")
                    .action(ArgAction::SetTrue),
            )
            .arg(
                Arg::new("PN")
                    .value_name("Package name")
                    .required(true)
                    .action(ArgAction::Append),
            ),
        Command::new("upgradeSelf")
            .about("Upgrade Self")
            .visible_aliases(["US", "UPS", "grades"])
            .arg(
                Arg::new("verbose")
                    .help("Verbose")
                    .short('v')
                    .long("verbose")
                    .action(ArgAction::SetTrue),
            ),
        Command::new("source")
            .about("Manage package sources")
            .subcommand_required(true)
            .arg_required_else_help(true)
            .subcommand(
                Command::new("add")
                    .about("Add a package source")
                    .arg(Arg::new("URL").value_name("URL").required(true))
                    .arg(
                        Arg::new("as")
                            .long("as")
                            .value_name("ALIAS")
                            .help("Alias for this source (defaults to the URL host)"),
                    ),
            )
            .subcommand(
                Command::new("remove")
                    .about("Remove a package source")
                    .arg(Arg::new("ALIAS").value_name("ALIAS").required(true)),
            )
            .subcommand(Command::new("list").about("List configured package sources")),
    ])
    .arg(
        Arg::new("generator")
            .short('g')
            .long("gen")
            .action(ArgAction::Set)
            .aliases(["gen", "generator", "autocomplete", "complete"])
            .value_parser(value_parser!(Shell)),
    )
    .arg(
        Arg::new("system")
            .short('S')
            .long("system")
            .help("Operate on the shared system-wide install (requires root)")
            .action(ArgAction::SetTrue),
    )
}

fn print_completions<G: Generator>(gen: G, cmd: &mut Command) {
    generate(gen, cmd, cmd.get_name().to_string(), &mut io::stdout());
    std::process::exit(0);
}

fn get_styles() -> clap::builder::Styles {
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
pub fn get_args() -> Result<Cli> {
    let matches = build_cli().get_matches();
    if let Some(generator) = matches.get_one::<Shell>("generator").copied() {
        let mut cmd = build_cli();
        eprintln!("Generating completion file for {generator}...");
        print_completions(generator, &mut cmd);
    }
    let system = matches.get_flag("system");
    let mut Commands: Option<CliCommands> = Option::<CliCommands>::None;
    let mut Verbose = false;
    let mut PN = vec![];
    let mut Other = Option_set::default();

    let config = match matches.subcommand() {
        Some(("install", sub_command)) => {
            Commands = Some(CliCommands::Install);
            Verbose = sub_command.get_flag("verbose");
            PN = sub_command
                .get_many::<String>("PN")
                .unwrap_or_default()
                .map(|v| v.to_string())
                .collect::<Vec<String>>();
        }
        Some(("update", sub_command)) => {
            Commands = Some(CliCommands::Update);
            Verbose = sub_command.get_flag("verbose");
            // PN = sub_command
            //     .get_many::<String>("PN")
            //     .unwrap_or_default()
            //     .map(|v| v.to_string())
            //     .collect::<Vec<String>>();
        }
        Some(("uninstall", sub_command)) => {
            Commands = Some(CliCommands::Uninstall);
            Verbose = sub_command.get_flag("verbose");
            PN = sub_command
                .get_many::<String>("PN")
                .unwrap_or_default()
                .map(|v| v.to_string())
                .collect::<Vec<String>>();
        }
        Some(("search", sub_command)) => {
            Commands = Some(CliCommands::Search);
            Verbose = sub_command.get_flag("verbose");
            PN = sub_command
                .get_many::<String>("PN")
                .unwrap_or_default()
                .map(|v| v.to_string())
                .collect::<Vec<String>>();
        }
        Some(("list", sub_command)) => {
            Commands = Some(CliCommands::List);
            Verbose = sub_command.get_flag("verbose");
            Other.List_installed = Some(sub_command.get_flag("list-installed"));
            Other.List_sys_installed = Some(sub_command.get_flag("list-sys-installed"));
        }
        Some(("upgrade", sub_command)) => {
            Commands = Some(CliCommands::Upgrade);
            Verbose = sub_command.get_flag("verbose");
            PN = sub_command
                .get_many::<String>("PN")
                .unwrap_or_default()
                .map(|v| v.to_string())
                .collect::<Vec<String>>();
        }
        Some(("upgradeSelf", sub_command)) => {
            Commands = Some(CliCommands::UpgradeSelf);
            Verbose = sub_command.get_flag("verbose");
        }
        Some(("source", sub_command)) => match sub_command.subcommand() {
            Some(("add", add_args)) => {
                Commands = Some(CliCommands::Source(SourceAction::Add {
                    url: add_args.get_one::<String>("URL").unwrap().to_string(),
                    alias: add_args.get_one::<String>("as").map(|s| s.to_string()),
                }));
            }
            Some(("remove", remove_args)) => {
                Commands = Some(CliCommands::Source(SourceAction::Remove {
                    alias: remove_args.get_one::<String>("ALIAS").unwrap().to_string(),
                }));
            }
            Some(("list", _)) => {
                Commands = Some(CliCommands::Source(SourceAction::List));
            }
            _ => unreachable!("clap enforces subcommand_required(true) on `source`"),
        },
        _ => return Err(anyhow!("Unrecognized command")),
    };
    let PackageName = if PN.is_empty() { None } else { Some(PN) };
    Ok(Cli {
        Commands,
        PackageName,
        Verbose,
        Other: Some(Other),
        System: system,
    })
}
