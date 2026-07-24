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

    fn sample_pkg() -> DbPackage {
        DbPackage::new(
            "test_pkg",
            "0.1.0",
            "http://example.com",
            "A test package",
            "test_pkg.tar.gz",
            "1234567890abcdef",
            "bin/test_pkg",
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
        db.insert(sample_pkg()).await?;

        let all = db.read_all().await?;
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "test_pkg");
        assert_eq!(all[0].version, "0.1.0");
        Ok(())
    }

    #[tokio::test]
    async fn test_read_one() -> TestResult {
        let dir = tempdir()?;
        let db = setup_db(dir.path()).await?;
        db.insert(sample_pkg()).await?;

        let found = db.read_one("test_pkg").await?;
        assert!(found.is_some());
        assert_eq!(found.unwrap().hash, "1234567890abcdef");

        let missing = db.read_one("nonexistent").await?;
        assert!(missing.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn test_update_version() -> TestResult {
        let dir = tempdir()?;
        let db = setup_db(dir.path()).await?;
        db.insert(sample_pkg()).await?;

        db.update_version("test_pkg", "0.2.0").await?;
        let found = db.read_one("test_pkg").await?.ok_or("package not found")?;
        assert_eq!(found.version, "0.2.0");
        Ok(())
    }

    #[tokio::test]
    async fn test_delete() -> TestResult {
        let dir = tempdir()?;
        let db = setup_db(dir.path()).await?;
        db.insert(sample_pkg()).await?;

        db.delete("test_pkg").await?;
        assert!(db.read_one("test_pkg").await?.is_none());
        Ok(())
    }
}
