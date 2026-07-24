#[cfg(test)]
mod config_tests {
    use DPM::{Setting, Source};

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

    /// `init()` itself can't be driven from a unit test in isolation: the
    /// paths it writes to (`CONFIG`/`INSTALL_DIR`/`BIN_DIR`) are process-wide
    /// `OnceLock<PathBuf>`s only populated by `set_globle_var(scope)`, whose
    /// `compute_paths` resolves either the real per-user `ProjectDirs`
    /// directory or `/opt/com.duacodie/DPM` — there's no seam to point it at
    /// a throwaway tempdir, and `OnceLock::set` can only succeed once per
    /// process, so a test calling it would either mutate the real machine's
    /// config dir or collide with other tests/processes.
    ///
    /// What *is* independently testable is the exact persistence mechanism
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
}
