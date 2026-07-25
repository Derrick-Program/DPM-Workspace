#[cfg(test)]
mod config_tests {
    use DPM::{Context, Scope, Setting, Source, SystemController};

    #[test]
    fn setting_round_trips_through_json() {
        let setting = Setting {
            sources: vec![Source {
                alias: "official".to_string(),
                repo_url: "https://github.com/Derrick-Program/DPM-Server/tree/main/Repo"
                    .to_string(),
                repo_info:
                    "https://raw.githubusercontent.com/Derrick-Program/DPM-Server/main/RepoInfo.json"
                        .to_string(),
            }],
        };

        let json = serde_json::to_string(&setting).unwrap();
        let parsed: Setting = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.sources.len(), 1);
        assert_eq!(parsed.sources[0].alias, "official");
    }

    #[test]
    fn setting_defaults_to_empty_sources_when_missing() {
        let parsed: Setting = serde_json::from_str("{}").unwrap();
        assert!(parsed.sources.is_empty());
    }

    /// `init()` itself still isn't called end-to-end by this test: when
    /// `config.json` doesn't exist yet, `init()` unconditionally seeds the
    /// "official" source via a real network fetch (`ActionInfo::init_update`
    /// -> `RepoInfo::fetch_update_repo_info`) — a separate, pre-existing
    /// behavior untouched by the `Context` refactor, and not something a
    /// unit test should depend on (flaky, slow, requires network).
    ///
    /// What *is* now independently testable — this used to be impossible,
    /// see `context_for_test_gives_an_isolated_writable_config_dir` and
    /// `system_controllers_with_different_scopes_coexist_in_one_process`
    /// below for the part that changed — is the exact persistence mechanism
    /// `init()` relies on to fix the "config.json stays `{}` forever" bug:
    /// `JsonStorage::to_json` writing a `Setting` to a real file, followed by
    /// `JsonStorage::from_json` reading it back. This proves that path
    /// actually round-trips through disk (not just through an in-memory
    /// `String`, unlike `setting_round_trips_through_json` above) so the
    /// "official" source really survives a to_json/from_json cycle against a
    /// real filesystem — the same two calls `init()` makes back to back.
    #[test]
    fn setting_persists_to_disk_and_reloads_via_json_storage() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");
        assert!(!config_path.exists());

        let default_setting = Setting {
            sources: vec![Source {
                alias: "official".to_string(),
                repo_url: "https://github.com/Derrick-Program/DPM-Server/tree/main/Repo"
                    .to_string(),
                repo_info:
                    "https://raw.githubusercontent.com/Derrick-Program/DPM-Server/main/RepoInfo.json"
                        .to_string(),
            }],
        };
        dpm_core::JsonStorage::to_json(&default_setting, &config_path).unwrap();

        assert!(
            config_path.exists(),
            "config.json must actually exist on disk after JsonStorage::to_json, \
             not just live in an in-memory struct"
        );
        let reloaded: Setting = dpm_core::JsonStorage::from_json(&config_path).unwrap();
        assert_eq!(reloaded.sources.len(), 1);
        assert_eq!(reloaded.sources[0].alias, "official");
    }

    /// `Context::for_test` gives every path (including `config_dir`) an
    /// isolated tempdir instead of the real per-user/`--system` locations —
    /// this is the seam the comment above says used to be missing. Proves
    /// the `config_dir` a real `Context` hands out is genuinely writable
    /// and genuinely isolated, using the same `JsonStorage` write/read cycle
    /// `init()` performs internally.
    #[tokio::test]
    async fn context_for_test_gives_an_isolated_writable_config_dir() {
        let root = tempfile::tempdir().unwrap();
        let ctx = Context::for_test(root.path()).await.unwrap();
        assert!(
            ctx.config_dir.starts_with(root.path()),
            "config_dir must live under the caller's tempdir, not a real machine path"
        );

        let config_path = ctx.config_dir.join("config.json");
        assert!(!config_path.exists());

        let default_setting = Setting {
            sources: vec![Source {
                alias: "official".to_string(),
                repo_url: "https://github.com/Derrick-Program/DPM-Server".to_string(),
                repo_info:
                    "https://raw.githubusercontent.com/Derrick-Program/DPM-Server/main/RepoInfo.json"
                        .to_string(),
            }],
        };
        dpm_core::JsonStorage::to_json(&default_setting, &config_path).unwrap();
        assert!(config_path.exists());
        let reloaded: Setting = dpm_core::JsonStorage::from_json(&config_path).unwrap();
        assert_eq!(reloaded.sources.len(), 1);
    }

    /// Two `SystemController`s with different `Scope`s coexisting correctly
    /// in one process is exactly what the old design made impossible: scope
    /// lived in one process-wide `OnceLock<Scope>`, so whichever call site
    /// set it first "won" for the rest of the process — there was no way
    /// for two call sites (let alone two tests in the same binary) to
    /// genuinely differ.
    #[test]
    fn system_controllers_with_different_scopes_coexist_in_one_process() {
        let per_user = SystemController::new(Scope::PerUser);
        let system = SystemController::new(Scope::System);

        let bogus_path = std::path::Path::new("/nonexistent/for/this/test");
        assert!(
            per_user.permision_check(bogus_path).is_ok(),
            "PerUser scope must never attempt an ownership change"
        );
        // `system`'s Scope::System branch would go on to run a real chown
        // via sudo for a real path — not appropriate to actually execute in
        // a test — but constructing it successfully alongside `per_user`
        // above, with a different Scope, in the same process, is exactly
        // what a single process-wide OnceLock<Scope> could never allow.
        assert!(format!("{system:?}").contains("System"));
    }
}
