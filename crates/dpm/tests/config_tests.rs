#[cfg(test)]
mod config_tests {
    use DPM::{Context, Scope, Setting, Source, SystemController};

    #[test]
    fn setting_round_trips_through_toml() {
        let setting = Setting {
            sources: vec![Source {
                alias: "official".to_string(),
                repo_url: "crates/dpm-server".to_string(),
                repo_info: "crates/dpm-server/RepoInfo.json".to_string(),
            }],
        };

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        dpm_core::TomlStorage::to_toml(&setting, &path).unwrap();
        let parsed: Setting = dpm_core::TomlStorage::from_toml(&path).unwrap();

        assert_eq!(parsed.sources.len(), 1);
        assert_eq!(parsed.sources[0].alias, "official");
    }

    #[test]
    fn setting_defaults_to_empty_sources_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.toml");
        std::fs::write(&path, "").unwrap();
        let parsed: Setting = dpm_core::TomlStorage::from_toml(&path).unwrap();
        assert!(parsed.sources.is_empty());
    }

    /// `init_first_run()` 本身還是沒有被這個測試整個跑過:第一次執行時,
    /// `init_first_run()` 一律會真的打網路去 seed "official" source
    /// (`ActionInfo::init_update` -> `RepoInfo::fetch_update_repo_info`)——
    /// 這是既有、跟這次改動無關的行為,不該讓單元測試依賴真實網路。
    ///
    /// 這裡驗證的是 `init_first_run()` 真正依賴的持久化機制本身:
    /// `TomlStorage::to_toml` 把一個 `Setting` 寫進真實檔案,再用
    /// `TomlStorage::from_toml` 讀回來,證明這條路徑真的能在真實檔案系統上
    /// 來回一致(不是只在記憶體裡的 `String` 打轉)。
    #[test]
    fn setting_persists_to_disk_and_reloads_via_toml_storage() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        assert!(!config_path.exists());

        let default_setting = Setting {
            sources: vec![Source {
                alias: "official".to_string(),
                repo_url: "crates/dpm-server".to_string(),
                repo_info: "crates/dpm-server/RepoInfo.json".to_string(),
            }],
        };
        dpm_core::TomlStorage::to_toml(&default_setting, &config_path).unwrap();

        assert!(
            config_path.exists(),
            "config.toml must actually exist on disk after TomlStorage::to_toml, \
             not just live in an in-memory struct"
        );
        let reloaded: Setting = dpm_core::TomlStorage::from_toml(&config_path).unwrap();
        assert_eq!(reloaded.sources.len(), 1);
        assert_eq!(reloaded.sources[0].alias, "official");
    }

    /// `Context::for_test` 給每個路徑(包括 `config_dir`)一份隔離的
    /// tempdir,而不是真實的 per-user/`--system` 位置。證明 `Context` 給出
    /// 的 `config_dir` 是真的可寫、真的隔離的,用跟 `init_first_run()`
    /// 內部一樣的 `TomlStorage` 寫/讀循環。
    #[tokio::test]
    async fn context_for_test_gives_an_isolated_writable_config_dir() {
        let root = tempfile::tempdir().unwrap();
        let ctx = Context::for_test(root.path()).await.unwrap();
        assert!(
            ctx.config_dir.starts_with(root.path()),
            "config_dir must live under the caller's tempdir, not a real machine path"
        );

        let config_path = ctx.config_dir.join("config.toml");
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
        dpm_core::TomlStorage::to_toml(&default_setting, &config_path).unwrap();
        assert!(config_path.exists());
        let reloaded: Setting = dpm_core::TomlStorage::from_toml(&config_path).unwrap();
        assert_eq!(reloaded.sources.len(), 1);
    }

    #[test]
    fn system_controllers_with_different_scopes_coexist_in_one_process() {
        let per_user = SystemController::new(Scope::PerUser);
        let system = SystemController::new(Scope::System);

        let bogus_path = std::path::Path::new("/nonexistent/for/this/test");
        assert!(
            per_user.permision_check(bogus_path).is_ok(),
            "PerUser scope must never attempt an ownership change"
        );
        assert!(format!("{system:?}").contains("System"));
    }

    #[tokio::test]
    async fn gen_config_writes_default_setting_when_missing() {
        let root = tempfile::tempdir().unwrap();
        let ctx = Context::for_test(root.path()).await.unwrap();
        let controller = SystemController::new(Scope::PerUser);

        let path = controller.gen_config(&ctx, false).await.unwrap();

        assert!(path.exists());
        let reloaded: Setting = dpm_core::TomlStorage::from_toml(&path).unwrap();
        assert!(
            reloaded.sources.is_empty(),
            "gen-config writes Setting::default(), not the seeded 'official' source"
        );
    }

    #[tokio::test]
    async fn gen_config_refuses_to_overwrite_without_force() {
        let root = tempfile::tempdir().unwrap();
        let ctx = Context::for_test(root.path()).await.unwrap();
        let controller = SystemController::new(Scope::PerUser);

        controller.gen_config(&ctx, false).await.unwrap();
        let err = controller.gen_config(&ctx, false).await.unwrap_err();
        assert!(err.to_string().contains("already exists"));
    }

    #[tokio::test]
    async fn gen_config_overwrites_when_force_is_true() {
        let root = tempfile::tempdir().unwrap();
        let ctx = Context::for_test(root.path()).await.unwrap();
        let controller = SystemController::new(Scope::PerUser);

        let path = controller.gen_config(&ctx, false).await.unwrap();
        dpm_core::TomlStorage::to_toml(
            &Setting {
                sources: vec![Source {
                    alias: "hand-edited".to_string(),
                    repo_url: "https://example.com".to_string(),
                    repo_info: "https://example.com/RepoInfo.json".to_string(),
                }],
            },
            &path,
        )
        .unwrap();

        controller.gen_config(&ctx, true).await.unwrap();

        let reloaded: Setting = dpm_core::TomlStorage::from_toml(&path).unwrap();
        assert!(
            reloaded.sources.is_empty(),
            "--force must overwrite the hand-edited content back to defaults"
        );
    }
}
