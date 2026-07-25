#[cfg(test)]
mod cli_parse_tests {
    use clap::Parser;
    use DPM::{Cli, Commands, SourceAction};

    // Typed assertions against the parsed `Cli`/`Commands`/`SourceAction`
    // enums, not stringly-typed `ArgMatches` lookups — the whole point of
    // moving `dpm`'s CLI parsing to `#[derive(Parser)]` (matching
    // `dpm-server`'s style) is that callers (and tests) get real Rust data
    // straight out of parsing, with no hand-written match-and-extract step
    // in between.

    #[test]
    fn source_add_parses_url_and_alias() {
        let cli = Cli::try_parse_from([
            "dpm",
            "source",
            "add",
            "https://example.com/repo",
            "--as",
            "myrepo",
        ])
        .unwrap();
        match cli.command {
            Some(Commands::Source {
                action: SourceAction::Add { url, alias },
            }) => {
                assert_eq!(url, "https://example.com/repo");
                assert_eq!(alias.as_deref(), Some("myrepo"));
            }
            other => panic!("expected Source(Add), got {other:?}"),
        }
    }

    #[test]
    fn source_add_alias_is_optional() {
        let cli =
            Cli::try_parse_from(["dpm", "source", "add", "https://example.com/repo"]).unwrap();
        match cli.command {
            Some(Commands::Source {
                action: SourceAction::Add { alias, .. },
            }) => assert!(alias.is_none()),
            other => panic!("expected Source(Add), got {other:?}"),
        }
    }

    #[test]
    fn source_remove_requires_alias() {
        let cli = Cli::try_parse_from(["dpm", "source", "remove", "myrepo"]).unwrap();
        match cli.command {
            Some(Commands::Source {
                action: SourceAction::Remove { alias },
            }) => assert_eq!(alias, "myrepo"),
            other => panic!("expected Source(Remove), got {other:?}"),
        }
    }

    #[test]
    fn source_list_takes_no_args() {
        let cli = Cli::try_parse_from(["dpm", "source", "list"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::Source {
                action: SourceAction::List
            })
        ));
    }

    #[test]
    fn source_without_subcommand_is_an_error() {
        let result = Cli::try_parse_from(["dpm", "source"]);
        assert!(result.is_err(), "source requires add/remove/list");
    }
}
