#[cfg(test)]
mod db_tests {
    use std::error::Error;
    use tempfile::tempdir;
    use DPM::{Db, DbPackage};

    type TestResult = Result<(), Box<dyn Error>>;

    /// 建立一個跑好 migration 的測試用 Db
    async fn setup_db(dir: &std::path::Path) -> Result<Db, Box<dyn Error>> {
        let db_path = dir.join("test.db");
        let lock_path = dir.join("test.lock");
        let db = Db::new(
            db_path.to_str().ok_or("invalid db path")?,
            lock_path.to_str().ok_or("invalid lock path")?,
        )
        .await?;
        db.run_migrations().await?;
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
        )
    }

    #[tokio::test]
    async fn test_db_new_and_migrations() -> TestResult {
        let dir = tempdir()?;
        let _db = setup_db(dir.path()).await?;
        assert!(dir.path().join("test.db").exists());
        Ok(())
    }

    #[tokio::test]
    async fn test_insert_and_read_all() -> TestResult {
        let dir = tempdir()?;
        let db = setup_db(dir.path()).await?;
        db.insert(sample_pkg("official", "0.1.0")).await?;

        let all = db.read_all().await?;
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
        let db = setup_db(dir.path()).await?;
        let mut pkg = sample_pkg("official", "0.1.0");
        pkg.entry = None;
        db.insert(pkg).await?;

        let found = db.read_one("official", "test_pkg", "0.1.0").await?;
        assert_eq!(found.unwrap().entry, None);
        Ok(())
    }

    #[tokio::test]
    async fn test_read_one_is_scoped_to_source_and_version() -> TestResult {
        let dir = tempdir()?;
        let db = setup_db(dir.path()).await?;
        db.insert(sample_pkg("official", "0.1.0")).await?;

        let found = db.read_one("official", "test_pkg", "0.1.0").await?;
        assert!(found.is_some());
        assert_eq!(found.unwrap().hash, Some("1234567890abcdef".to_string()));

        let wrong_source = db.read_one("other", "test_pkg", "0.1.0").await?;
        assert!(wrong_source.is_none());

        let wrong_version = db.read_one("official", "test_pkg", "9.9.9").await?;
        assert!(wrong_version.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn test_versions_of_returns_every_version_in_that_source() -> TestResult {
        let dir = tempdir()?;
        let db = setup_db(dir.path()).await?;
        db.insert(sample_pkg("official", "0.1.0")).await?;
        db.insert(sample_pkg("official", "0.2.0")).await?;

        let versions = db.versions_of("official", "test_pkg").await?;
        assert_eq!(versions.len(), 2);
        Ok(())
    }

    #[tokio::test]
    async fn test_sources_of_lists_distinct_sources_for_bare_name() -> TestResult {
        let dir = tempdir()?;
        let db = setup_db(dir.path()).await?;
        db.insert(sample_pkg("official", "0.1.0")).await?;
        db.insert(sample_pkg("third-party", "0.1.0")).await?;

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
        let db = setup_db(dir.path()).await?;
        db.insert(sample_pkg("official", "0.1.0")).await?;
        db.insert(sample_pkg("official", "0.2.0")).await?;

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
        let db = setup_db(dir.path()).await?;
        db.insert(sample_pkg("official", "0.1.0")).await?;
        db.insert(sample_pkg("third-party", "0.1.0")).await?;

        db.clear_table_for_source("official").await?;

        assert!(db
            .read_one("official", "test_pkg", "0.1.0")
            .await?
            .is_none());
        assert!(db
            .read_one("third-party", "test_pkg", "0.1.0")
            .await?
            .is_some());
        Ok(())
    }

    #[tokio::test]
    async fn test_delete_is_scoped_to_source_and_version() -> TestResult {
        let dir = tempdir()?;
        let db = setup_db(dir.path()).await?;
        db.insert(sample_pkg("official", "0.1.0")).await?;
        db.insert(sample_pkg("official", "0.2.0")).await?;

        db.delete("official", "test_pkg", "0.1.0").await?;

        assert!(db
            .read_one("official", "test_pkg", "0.1.0")
            .await?
            .is_none());
        assert!(db
            .read_one("official", "test_pkg", "0.2.0")
            .await?
            .is_some());
        Ok(())
    }
}
