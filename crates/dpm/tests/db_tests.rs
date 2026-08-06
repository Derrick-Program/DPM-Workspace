#[cfg(test)]
mod db_tests {
    use std::error::Error;
    use tempfile::tempdir;
    use DPM::{Db, DbPackage};

    type TestResult = Result<(), Box<dyn Error>>;

    /// 建立一個跑好 migration 的測試用 Db
    async fn setup_db(dir: &std::path::Path, is_info: bool) -> Result<Db, Box<dyn Error>> {
        let db_path = dir.join("test.db");
        let lock_path = dir.join("test.lock");
        let db = Db::new(
            db_path.to_str().ok_or("invalid db path")?,
            lock_path.to_str().ok_or("invalid lock path")?,
        )
        .await?;
        db.run_migrations(is_info).await?;
        Ok(db)
    }

    fn sample_pkg(source: &str, version: &str) -> DbPackage {
        DbPackage::new(
            source,
            "test_pkg",
            version,
            "prebuilt",
            Some("http://example.com".to_string()),
            Some("1234567890abcdef".to_string()),
            Some("test_pkg.tar.gz".to_string()),
            None,
            "A test package",
            Some("bin/test_pkg".to_string()),
            None,
            None,
            None,
            true,
        )
    }

    #[tokio::test]
    async fn test_db_new_and_migrations() -> TestResult {
        let dir = tempdir()?;
        let _db = setup_db(dir.path(), false).await?;
        assert!(dir.path().join("test.db").exists());
        Ok(())
    }

    #[tokio::test]
    async fn test_insert_and_read_all() -> TestResult {
        let dir = tempdir()?;
        let db = setup_db(dir.path(), true).await?;
        db.insert_available(sample_pkg("official", "0.1.0")).await?;

        let all = db.read_available().await?;
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "test_pkg");
        assert_eq!(all[0].source, "official");
        assert_eq!(all[0].version, "0.1.0");
        Ok(())
    }

    #[tokio::test]
    async fn test_none_entry_round_trips_as_none_not_empty_string() -> TestResult {
        // Regression guard for the entry column's Option<String> migration:
        // a package with no entry point (e.g. a data-only source package)
        // must come back as `None`, not `""`. `""` used to double as the
        // "no entry" sentinel before the column became nullable.
        let dir = tempdir()?;
        let db = setup_db(dir.path(), true).await?;
        let mut pkg = sample_pkg("official", "0.1.0");
        pkg.entry = None;
        db.insert_available(pkg).await?;

        let found = db
            .read_one_available("official", "test_pkg", "0.1.0")
            .await?;
        assert_eq!(found.unwrap().entry, None);
        Ok(())
    }

    #[tokio::test]
    async fn test_read_one_is_scoped_to_source_and_version() -> TestResult {
        let dir = tempdir()?;
        let db = setup_db(dir.path(), true).await?;
        db.insert_available(sample_pkg("official", "0.1.0")).await?;

        let found = db
            .read_one_available("official", "test_pkg", "0.1.0")
            .await?;
        assert!(found.is_some());
        assert_eq!(found.unwrap().hash, Some("1234567890abcdef".to_string()));

        let wrong_source = db.read_one_available("other", "test_pkg", "0.1.0").await?;
        assert!(wrong_source.is_none());

        let wrong_version = db
            .read_one_available("official", "test_pkg", "9.9.9")
            .await?;
        assert!(wrong_version.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn test_versions_of_returns_every_version_in_that_source() -> TestResult {
        let dir = tempdir()?;
        let db = setup_db(dir.path(), true).await?;
        db.insert_available(sample_pkg("official", "0.1.0")).await?;
        db.insert_available(sample_pkg("official", "0.2.0")).await?;

        let versions = db.versions_of("official", "test_pkg").await?;
        assert_eq!(versions.len(), 2);
        Ok(())
    }

    #[tokio::test]
    async fn test_sources_of_lists_distinct_sources_for_bare_name() -> TestResult {
        let dir = tempdir()?;
        let db = setup_db(dir.path(), true).await?;
        db.insert_available(sample_pkg("official", "0.1.0")).await?;
        db.insert_available(sample_pkg("third-party", "0.1.0"))
            .await?;

        let mut sources = db.sources_of("test_pkg").await?;
        sources.sort();
        assert_eq!(
            sources,
            vec!["official".to_string(), "third-party".to_string()]
        );

        let none = db.sources_of("nonexistent").await?;
        assert!(none.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_latest_version_is_the_most_recently_inserted_row() -> TestResult {
        let dir = tempdir()?;
        let db = setup_db(dir.path(), true).await?;
        db.insert_available(sample_pkg("official", "0.1.0")).await?;
        db.insert_available(sample_pkg("official", "0.2.0")).await?;

        let latest = db
            .latest_version("official", "test_pkg")
            .await?
            .ok_or("expected a latest version")?;
        assert_eq!(latest.version, "0.2.0");
        Ok(())
    }

    #[tokio::test]
    async fn test_clear_table_for_source_only_wipes_that_source() -> TestResult {
        let dir = tempdir()?;
        let db = setup_db(dir.path(), true).await?;
        db.insert_available(sample_pkg("official", "0.1.0")).await?;
        db.insert_available(sample_pkg("third-party", "0.1.0"))
            .await?;

        db.clear_table_for_source("official").await?;

        assert!(db
            .read_one_available("official", "test_pkg", "0.1.0")
            .await?
            .is_none());
        assert!(db
            .read_one_available("third-party", "test_pkg", "0.1.0")
            .await?
            .is_some());
        Ok(())
    }

    #[tokio::test]
    async fn test_delete_is_scoped_to_source_and_version() -> TestResult {
        let dir = tempdir()?;
        let db = setup_db(dir.path(), true).await?;
        db.insert_available(sample_pkg("official", "0.1.0")).await?;
        db.insert_available(sample_pkg("official", "0.2.0")).await?;

        db.delete("official", "test_pkg", "0.1.0").await?;

        assert!(db
            .read_one_available("official", "test_pkg", "0.1.0")
            .await?
            .is_none());
        assert!(db
            .read_one_available("official", "test_pkg", "0.2.0")
            .await?
            .is_some());
        Ok(())
    }

    #[tokio::test]
    async fn test_table_name_validation() -> TestResult {
        assert!(Db::validate_table_name("InstalledPackages").is_ok());
        assert!(Db::validate_table_name("schema_migrations").is_ok());
        assert!(Db::validate_table_name("custom_table_1").is_ok());

        assert!(
            Db::validate_table_name("InstalledPackages; DROP TABLE InstalledPackages;--").is_err()
        );
        assert!(Db::validate_table_name("invalid-table-name").is_err());
        assert!(Db::validate_table_name("123invalid").is_err());
        assert!(Db::validate_table_name("").is_err());
        Ok(())
    }

    #[tokio::test]
    async fn test_clear_and_drop_table() -> TestResult {
        let dir = tempdir()?;
        let db = setup_db(dir.path(), false).await?;
        db.insert(sample_pkg("official", "0.1.0")).await?;

        db.clear_table("InstalledPackages").await?;
        assert!(db.read_all().await?.is_empty());

        assert!(db
            .clear_table("InstalledPackages; DROP TABLE InstalledPackages;--")
            .await
            .is_err());
        assert!(db.drop_table("invalid table").await.is_err());

        db.drop_table("InstalledPackages").await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_explicit_flag_persists_and_defaults_true() -> TestResult {
        let dir = tempdir()?;
        let db = setup_db(dir.path(), false).await?;
        let mut pkg = sample_pkg("official", "0.1.0");
        pkg.explicit = false;
        db.insert(pkg).await?;

        let all = db.read_all().await?;
        assert_eq!(all.len(), 1);
        assert!(!all[0].explicit);
        Ok(())
    }

    #[tokio::test]
    async fn test_reinstall_same_name_upserts_and_sticky_promotes_explicit() -> TestResult {
        let dir = tempdir()?;
        let db = setup_db(dir.path(), false).await?;

        let mut auto_pkg = sample_pkg("official", "0.1.0");
        auto_pkg.explicit = false;
        db.insert(auto_pkg).await?;

        let mut upgraded = sample_pkg("official", "0.2.0");
        upgraded.explicit = false;
        db.insert(upgraded).await?;
        let after_auto_reinstall = db.read_all().await?;
        assert_eq!(
            after_auto_reinstall.len(),
            1,
            "same name must upsert, not duplicate"
        );
        assert_eq!(after_auto_reinstall[0].version, "0.2.0");
        assert!(!after_auto_reinstall[0].explicit);

        let mut explicit_reinstall = sample_pkg("official", "0.2.0");
        explicit_reinstall.explicit = true;
        db.insert(explicit_reinstall).await?;
        let after_explicit_reinstall = db.read_all().await?;
        assert!(
            after_explicit_reinstall[0].explicit,
            "a direct re-install must promote explicit"
        );

        let mut dep_only_reinstall = sample_pkg("official", "0.3.0");
        dep_only_reinstall.explicit = false;
        db.insert(dep_only_reinstall).await?;
        let after_dep_only_reinstall = db.read_all().await?;
        assert!(
            after_dep_only_reinstall[0].explicit,
            "explicit must never auto-demote back to false"
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_run_migrations_is_idempotent_for_the_explicit_column() -> TestResult {
        let dir = tempdir()?;
        let db = setup_db(dir.path(), false).await?;
        // A second run must not error on `ALTER TABLE ... ADD COLUMN explicit`
        // finding the column already there — this is what protects DBs that
        // already had `InstalledPackages` before this column existed.
        db.run_migrations(false).await?;
        db.insert(sample_pkg("official", "0.1.0")).await?;
        assert!(db.read_all().await?[0].explicit);
        Ok(())
    }

    #[tokio::test]
    async fn test_delete_installed_removes_only_the_named_package() -> TestResult {
        let dir = tempdir()?;
        let db = setup_db(dir.path(), false).await?;
        db.insert(sample_pkg("official", "0.1.0")).await?;
        let mut other = sample_pkg("official", "0.1.0");
        other.name = "other_pkg".to_string();
        db.insert(other).await?;

        db.delete_installed("test_pkg").await?;

        let remaining = db.read_all().await?;
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].name, "other_pkg");
        Ok(())
    }

    #[tokio::test]
    async fn test_record_get_and_remove_installed_files() -> TestResult {
        let dir = tempdir()?;
        let db = setup_db(dir.path(), false).await?;

        let files = vec![
            "/opt/dpm/opt/hello".to_string(),
            "/opt/dpm/bin/hello".to_string(),
            "/opt/dpm/share/man/man1/hello.1".to_string(),
        ];

        db.record_installed_files("hello", &files).await?;

        let recorded = db.get_installed_files("hello").await?;
        assert_eq!(recorded.len(), 3);
        assert!(recorded.contains(&"/opt/dpm/bin/hello".to_string()));

        db.remove_installed_files("hello").await?;
        assert!(db.get_installed_files("hello").await?.is_empty());
        Ok(())
    }
}
