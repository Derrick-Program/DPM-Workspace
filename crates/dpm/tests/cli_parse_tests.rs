#[cfg(test)]
mod cli_parse_tests {
    use std::sync::Once;
    use DPM::build_cli;

    // build_cli() reads the BIN/VERSION OnceLocks that init_cli_metadata()
    // populates (normally done once by main() before any CLI parsing runs).
    // Tests share a process, so guard the one-time init with Once rather
    // than letting the second test's call panic on an already-set OnceLock.
    static INIT: Once = Once::new();
    fn setup() {
        INIT.call_once(|| {
            DPM::init_cli_metadata();
        });
    }

    #[test]
    fn source_add_parses_url_and_alias() {
        setup();
        let cli = build_cli();
        let matches = cli
            .try_get_matches_from([
                "dpm",
                "source",
                "add",
                "https://example.com/repo",
                "--as",
                "myrepo",
            ])
            .unwrap();
        let (name, sub) = matches.subcommand().unwrap();
        assert_eq!(name, "source");
        let (inner_name, inner) = sub.subcommand().unwrap();
        assert_eq!(inner_name, "add");
        assert_eq!(
            inner.get_one::<String>("URL").unwrap(),
            "https://example.com/repo"
        );
        assert_eq!(inner.get_one::<String>("as").unwrap(), "myrepo");
    }

    #[test]
    fn source_add_alias_is_optional() {
        setup();
        let cli = build_cli();
        let matches = cli
            .try_get_matches_from(["dpm", "source", "add", "https://example.com/repo"])
            .unwrap();
        let (_, sub) = matches.subcommand().unwrap();
        let (_, inner) = sub.subcommand().unwrap();
        assert!(inner.get_one::<String>("as").is_none());
    }

    #[test]
    fn source_remove_requires_alias() {
        setup();
        let cli = build_cli();
        let matches = cli
            .try_get_matches_from(["dpm", "source", "remove", "myrepo"])
            .unwrap();
        let (_, sub) = matches.subcommand().unwrap();
        let (inner_name, inner) = sub.subcommand().unwrap();
        assert_eq!(inner_name, "remove");
        assert_eq!(inner.get_one::<String>("ALIAS").unwrap(), "myrepo");
    }

    #[test]
    fn source_list_takes_no_args() {
        setup();
        let cli = build_cli();
        let matches = cli.try_get_matches_from(["dpm", "source", "list"]).unwrap();
        let (_, sub) = matches.subcommand().unwrap();
        assert_eq!(sub.subcommand().unwrap().0, "list");
    }

    #[test]
    fn source_without_subcommand_is_an_error() {
        setup();
        let cli = build_cli();
        let result = cli.try_get_matches_from(["dpm", "source"]);
        assert!(result.is_err(), "source requires add/remove/list");
    }
}
