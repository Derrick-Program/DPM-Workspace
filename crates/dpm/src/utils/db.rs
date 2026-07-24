use super::{ClientError, ClientResult, DbPackage};
use dpm_core::CoreError::*;
use fs2::FileExt;
use futures_util::StreamExt;
use std::{fs::File, path::Path};
use tokio::io::AsyncWriteExt;

pub struct Db {
    db_path: String,
    _lock_file: File,
}

impl Db {
    pub async fn new(database_path: &str, lock_file_path: &str) -> ClientResult<Self> {
        let lock_file = File::create(lock_file_path)
            .map_err(|e| ClientError::LockError(format!("Failed to create lock file: {}", e)))?;
        lock_file
            .lock_exclusive()
            .map_err(|e| ClientError::LockError(format!("Failed to lock file: {}", e)))?;
        // ponytail: 這裡刻意不 eager 開 turso::Database。實測發現 turso 會在
        // 第一次開檔時把 schema 快取住,如果檔案這時還沒跑過 migration(空的
        // LocalRepo 都不存在),之後就算重開一個全新的 Database 物件指向同一
        // 個路徑也還是讀不到 geni(用 libsql)後來寫進去的 table,報
        // "no such table"。所以在這裡只驗證 lock 檔拿得到,真正的
        // turso::Database 留到 connect() 才開,並保證呼叫端一定是
        // run_migrations() 跑完之後才會呼叫任何一個會 connect() 的方法。
        Ok(Db {
            db_path: database_path.to_string(),
            _lock_file: lock_file,
        })
    }

    async fn connect(&self) -> ClientResult<turso::Connection> {
        let db = turso::Builder::new_local(&self.db_path)
            .build()
            .await
            .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?;
        db.connect()
            .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))
    }

    /// 把編進 binary 的 migration SQL 攤開到 DB 檔案同層的 migrations/ 資料夾,
    /// 再交給 geni 執行(geni 只吃真實存在的資料夾路徑,不吃 embedded 內容)。
    pub async fn run_migrations(&self) -> ClientResult<()> {
        let migrations_dir = Path::new(&self.db_path)
            .parent()
            .ok_or_else(|| ClientError::SystemError("invalid database path".to_string()))?
            .join("migrations");
        std::fs::create_dir_all(&migrations_dir).map_err(|e| ClientError::Core(IoError(e)))?;
        std::fs::write(
            migrations_dir.join("0001_init.up.sql"),
            include_str!("../../migrations/0001_init.up.sql"),
        )
        .map_err(|e| ClientError::Core(IoError(e)))?;
        std::fs::write(
            migrations_dir.join("0001_init.down.sql"),
            include_str!("../../migrations/0001_init.down.sql"),
        )
        .map_err(|e| ClientError::Core(IoError(e)))?;

        geni::migrate_database(
            format!("sqlite://{}", self.db_path),
            None,
            "schema_migrations".to_string(),
            migrations_dir.to_string_lossy().to_string(),
            migrations_dir
                .join("schema.sql")
                .to_string_lossy()
                .to_string(),
            Some(30),
            false,
        )
        .await
        .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?;
        Ok(())
    }

    fn row_to_package(row: turso::Row) -> ClientResult<DbPackage> {
        let get_text = |idx: usize| -> ClientResult<String> {
            row.get_value(idx)
                .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?
                .as_text()
                .cloned()
                .ok_or_else(|| {
                    ClientError::Core(DatabaseError(format!("column {idx} is not text")))
                })
        };
        let dependencies_json = row
            .get_value(7)
            .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?
            .as_text()
            .cloned();
        Ok(DbPackage {
            name: get_text(0)?,
            version: get_text(1)?,
            url: get_text(2)?,
            description: get_text(3)?,
            filename: get_text(4)?,
            hash: get_text(5)?,
            entry: get_text(6)?,
            dependencies: dependencies_json.and_then(|json| serde_json::from_str(&json).ok()),
        })
    }

    pub async fn execute_query(&self, query: &str) -> ClientResult<()> {
        let conn = self.connect().await?;
        conn.execute(query, ())
            .await
            .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?;
        Ok(())
    }

    pub async fn insert(&self, pkg: DbPackage) -> ClientResult<()> {
        let dependencies_json = pkg
            .dependencies
            .as_ref()
            .map(|deps| serde_json::to_string(deps).unwrap_or_else(|_| "[]".to_string()));
        let conn = self.connect().await?;
        let params: Vec<turso::Value> = vec![
            turso::Value::Text(pkg.name),
            turso::Value::Text(pkg.version),
            turso::Value::Text(pkg.url),
            turso::Value::Text(pkg.description),
            turso::Value::Text(pkg.filename),
            turso::Value::Text(pkg.hash),
            turso::Value::Text(pkg.entry),
            match dependencies_json {
                Some(s) => turso::Value::Text(s),
                None => turso::Value::Null,
            },
        ];
        conn.execute(
            "INSERT INTO LocalRepo (name, version, url, description, filename, hash, entry, dependencies) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params,
        )
        .await
        .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?;
        Ok(())
    }

    pub async fn read_all(&self) -> ClientResult<Vec<DbPackage>> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT name, version, url, description, filename, hash, entry, dependencies FROM LocalRepo",
                (),
            )
            .await
            .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?;
        let mut packages = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?
        {
            packages.push(Self::row_to_package(row)?);
        }
        Ok(packages)
    }

    pub async fn read_one(&self, target_name: &str) -> ClientResult<Option<DbPackage>> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT name, version, url, description, filename, hash, entry, dependencies FROM LocalRepo WHERE name = ?1",
                [target_name],
            )
            .await
            .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?;
        match rows
            .next()
            .await
            .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?
        {
            Some(row) => Ok(Some(Self::row_to_package(row)?)),
            None => Ok(None),
        }
    }

    pub async fn update_version(&self, target_name: &str, new_version: &str) -> ClientResult<()> {
        let conn = self.connect().await?;
        conn.execute(
            "UPDATE LocalRepo SET version = ?1 WHERE name = ?2",
            [new_version, target_name],
        )
        .await
        .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?;
        Ok(())
    }

    pub async fn delete(&self, target_name: &str) -> ClientResult<()> {
        let conn = self.connect().await?;
        conn.execute("DELETE FROM LocalRepo WHERE name = ?1", [target_name])
            .await
            .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?;
        Ok(())
    }

    pub async fn drop_table(&self, tname: &str) -> ClientResult<()> {
        self.execute_query(&format!("DROP TABLE IF EXISTS {}", tname))
            .await
    }

    pub async fn clear_table(&self, tname: &str) -> ClientResult<()> {
        self.execute_query(&format!("DELETE FROM {}", tname)).await
    }

    pub async fn download_file(&self, name: &str, dest_path: &Path) -> ClientResult<()> {
        let package = self
            .read_one(name)
            .await?
            .ok_or_else(|| ClientError::Core(PackageNotFound(name.to_string())))?;
        let url = &package.url;
        let req = reqwest::get(url)
            .await
            .map_err(|e| ClientError::Core(NetworkError(e.to_string())))?;
        if !req.status().is_success() {
            return Err(ClientError::Core(NetworkError(format!(
                "Failed to download file: HTTP {}",
                req.status()
            ))));
        }
        let mut file = tokio::fs::File::create(dest_path)
            .await
            .map_err(|e| ClientError::Core(IoError(e)))?;
        let mut stream = req.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk
                .map_err(|e| ClientError::SystemError(format!("Failed to read chunk: {}", e)))?;
            file.write_all(&chunk)
                .await
                .map_err(|e| ClientError::SystemError(format!("Failed to write chunk: {}", e)))?;
        }
        println!("File downloaded to: {}", dest_path.display());
        Ok(())
    }
}
