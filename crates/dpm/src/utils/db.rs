use super::{ClientError, ClientResult, DbPackage};
use dpm_core::CoreError::*;
use fs2::FileExt;
use std::fs::File;

/// Single source of truth for the `LocalRepo` column list — every query below
/// builds its SELECT/INSERT column list from this, and `decode_common_fields`
/// looks columns up by name, so reordering columns here can't silently
/// desync a query string from the decode logic.
const COLUMNS: &str =
    "source, name, version, kind, url, hash, filename, build_command, description, entry, dependencies, author, signature";

#[derive(Debug)]
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
    pub async fn run_migrations(&self, is_info: bool) -> ClientResult<()> {
        let conn = self.connect().await?;
        if is_info {
            conn.execute(
                r#"CREATE TABLE IF NOT EXISTS AvailablePackages (
                    source TEXT NOT NULL,
                    name TEXT NOT NULL,
                    version TEXT NOT NULL,
                    kind TEXT NOT NULL,
                    url TEXT,
                    hash TEXT,
                    filename TEXT,
                    build_command TEXT,
                    description TEXT NOT NULL,
                    entry TEXT,
                    dependencies TEXT,
                    author TEXT,
                    signature TEXT,
                    targets TEXT,
                    PRIMARY KEY (source, name, version)
                );"#,
                (),
            )
            .await
            .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?;
        } else {
            conn.execute(
                r#"CREATE TABLE IF NOT EXISTS InstalledPackages (
                    source TEXT NOT NULL,
                    name TEXT NOT NULL,
                    version TEXT NOT NULL,
                    kind TEXT NOT NULL,
                    url TEXT,
                    hash TEXT,
                    filename TEXT,
                    build_command TEXT,
                    description TEXT NOT NULL,
                    entry TEXT,
                    dependencies TEXT,
                    author TEXT,
                    signature TEXT,
                    explicit INTEGER NOT NULL DEFAULT 1,
                    installed_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                    PRIMARY KEY (name)
                );"#,
                (),
            )
            .await
            .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?;
            // `CREATE TABLE IF NOT EXISTS` above is a no-op on a DB file that
            // already has `InstalledPackages` from before this column
            // existed, so it wouldn't add `explicit` to it. This `ALTER
            // TABLE` covers that upgrade path; on a brand-new DB (or one
            // already migrated) the column already exists, so SQLite/turso
            // reports "duplicate column name", which we treat as success.
            if let Err(e) = conn
                .execute(
                    "ALTER TABLE InstalledPackages ADD COLUMN explicit INTEGER NOT NULL DEFAULT 1",
                    (),
                )
                .await
            {
                let msg = e.to_string();
                if !msg.to_lowercase().contains("duplicate column") {
                    return Err(ClientError::Core(DatabaseError(msg)));
                }
            }
            conn.execute(
                r#"CREATE TABLE IF NOT EXISTS installed_files (
                    package_name TEXT NOT NULL,
                    file_path TEXT NOT NULL,
                    PRIMARY KEY (package_name, file_path)
                );"#,
                (),
            )
            .await
            .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?;
        }
        Ok(())
    }

    fn decode_common_fields(row: &turso::Row) -> ClientResult<DbPackage> {
        // Index derived from `COLUMNS` itself (not a hand-copied number), so
        // reordering the column list here can't silently desync the query
        // string from the decode below — turso's `Row` carries no column
        // names of its own, only `Rows` does, so this is the closest we can
        // get to name-based lookup without threading `Rows` through.
        let col_idx = |name: &str| -> ClientResult<usize> {
            COLUMNS
                .split(", ")
                .position(|c| c == name)
                .ok_or_else(|| ClientError::Core(DatabaseError(format!("no column {name}"))))
        };
        let get_text = |name: &str| -> ClientResult<String> {
            row.get_value(col_idx(name)?)
                .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?
                .as_text()
                .cloned()
                .ok_or_else(|| {
                    ClientError::Core(DatabaseError(format!("column {name} is not text")))
                })
        };
        let get_opt_text = |name: &str| -> ClientResult<Option<String>> {
            Ok(row
                .get_value(col_idx(name)?)
                .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?
                .as_text()
                .cloned())
        };
        let dependencies_json = get_opt_text("dependencies")?;
        Ok(DbPackage {
            source: get_text("source")?,
            name: get_text("name")?,
            version: get_text("version")?,
            kind: get_text("kind")?,
            url: get_opt_text("url")?,
            hash: get_opt_text("hash")?,
            filename: get_opt_text("filename")?,
            build_command: get_opt_text("build_command")?,
            description: get_text("description")?,
            entry: get_opt_text("entry")?,
            dependencies: dependencies_json.and_then(|json| serde_json::from_str(&json).ok()),
            author: get_opt_text("author")?,
            signature: get_opt_text("signature")?,
            // Meaningless for `AvailablePackages` rows (no such column there);
            // `row_to_installed_package` below overwrites this with the real
            // value for `InstalledPackages` rows.
            explicit: true,
        })
    }

    fn row_to_available_package(row: turso::Row) -> ClientResult<DbPackage> {
        Self::decode_common_fields(&row)
    }

    fn row_to_installed_package(row: turso::Row) -> ClientResult<DbPackage> {
        let mut pkg = Self::decode_common_fields(&row)?;
        // `explicit` isn't part of the shared `COLUMNS` list (it doesn't
        // exist on `AvailablePackages`), so it's always the column
        // immediately after everything `COLUMNS` lists — callers select it
        // as `SELECT {COLUMNS}, explicit FROM InstalledPackages`.
        let explicit_idx = COLUMNS.split(", ").count();
        let explicit_value = row
            .get_value(explicit_idx)
            .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?
            .as_integer()
            .copied()
            .ok_or_else(|| {
                ClientError::Core(DatabaseError("column explicit is not an integer".to_string()))
            })?;
        pkg.explicit = explicit_value != 0;
        Ok(pkg)
    }

    pub async fn insert_available(&self, pkg: DbPackage) -> ClientResult<()> {
        let dependencies_json = pkg
            .dependencies
            .as_ref()
            .map(|deps| serde_json::to_string(deps).unwrap_or_else(|_| "[]".to_string()));
        let conn = self.connect().await?;
        let to_value = |opt: Option<String>| match opt {
            Some(s) => turso::Value::Text(s),
            None => turso::Value::Null,
        };
        let params: Vec<turso::Value> = vec![
            turso::Value::Text(pkg.source),
            turso::Value::Text(pkg.name),
            turso::Value::Text(pkg.version),
            turso::Value::Text(pkg.kind),
            to_value(pkg.url),
            to_value(pkg.hash),
            to_value(pkg.filename),
            to_value(pkg.build_command),
            turso::Value::Text(pkg.description),
            to_value(pkg.entry),
            to_value(dependencies_json),
            to_value(pkg.author),
            to_value(pkg.signature),
        ];
        conn.execute(
            &format!(
                "INSERT INTO AvailablePackages ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)"
            ),
            params,
        )
        .await
        .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?;
        Ok(())
    }

    pub async fn read_available(&self) -> ClientResult<Vec<DbPackage>> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(&format!("SELECT {COLUMNS} FROM AvailablePackages"), ())
            .await
            .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?;
        let mut packages = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?
        {
            packages.push(Self::row_to_available_package(row)?);
        }
        Ok(packages)
    }

    pub async fn read_one_available(
        &self,
        source: &str,
        name: &str,
        version: &str,
    ) -> ClientResult<Option<DbPackage>> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                &format!(
                    "SELECT {COLUMNS} FROM AvailablePackages WHERE source = ?1 AND name = ?2 AND version = ?3"
                ),
                [source, name, version],
            )
            .await
            .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?;
        match rows
            .next()
            .await
            .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?
        {
            Some(row) => Ok(Some(Self::row_to_available_package(row)?)),
            None => Ok(None),
        }
    }

    pub async fn search_available(&self, name: &str) -> ClientResult<Vec<DbPackage>> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                &format!("SELECT {COLUMNS} FROM AvailablePackages WHERE name = ?1"),
                [name],
            )
            .await
            .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?;
        let mut packages = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?
        {
            packages.push(Self::row_to_available_package(row)?);
        }
        Ok(packages)
    }

    pub async fn clear_source_available(&self, source: &str) -> ClientResult<()> {
        let conn = self.connect().await?;
        conn.execute("DELETE FROM AvailablePackages WHERE source = ?1", [source])
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
        let to_value = |opt: Option<String>| match opt {
            Some(s) => turso::Value::Text(s),
            None => turso::Value::Null,
        };
        let params: Vec<turso::Value> = vec![
            turso::Value::Text(pkg.source),
            turso::Value::Text(pkg.name),
            turso::Value::Text(pkg.version),
            turso::Value::Text(pkg.kind),
            to_value(pkg.url),
            to_value(pkg.hash),
            to_value(pkg.filename),
            to_value(pkg.build_command),
            turso::Value::Text(pkg.description),
            to_value(pkg.entry),
            to_value(dependencies_json),
            to_value(pkg.author),
            to_value(pkg.signature),
            turso::Value::Integer(if pkg.explicit { 1 } else { 0 }),
        ];
        conn.execute(
            &format!(
                "INSERT INTO InstalledPackages ({COLUMNS}, explicit) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14) \
                 ON CONFLICT(name) DO UPDATE SET \
                    source = excluded.source, \
                    version = excluded.version, \
                    kind = excluded.kind, \
                    url = excluded.url, \
                    hash = excluded.hash, \
                    filename = excluded.filename, \
                    build_command = excluded.build_command, \
                    description = excluded.description, \
                    entry = excluded.entry, \
                    dependencies = excluded.dependencies, \
                    author = excluded.author, \
                    signature = excluded.signature, \
                    explicit = CASE WHEN InstalledPackages.explicit = 1 THEN 1 ELSE excluded.explicit END"
            ),
            params,
        )
        .await
        .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?;
        Ok(())
    }

    /// Parameterized delete for a single `InstalledPackages` row by name —
    /// used by `uninstall()`/`autoremove()` instead of hand-formatting SQL
    /// with the package name spliced directly into the query string.
    pub async fn delete_installed(&self, name: &str) -> ClientResult<()> {
        let conn = self.connect().await?;
        conn.execute("DELETE FROM InstalledPackages WHERE name = ?1", [name])
            .await
            .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?;
        Ok(())
    }

    pub async fn read_all(&self) -> ClientResult<Vec<DbPackage>> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                &format!("SELECT {COLUMNS}, explicit FROM InstalledPackages"),
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
            packages.push(Self::row_to_installed_package(row)?);
        }
        Ok(packages)
    }

    pub async fn read_one(
        &self,
        source: &str,
        name: &str,
        version: &str,
    ) -> ClientResult<Option<DbPackage>> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                &format!(
                    "SELECT {COLUMNS}, explicit FROM InstalledPackages WHERE source = ?1 AND name = ?2 AND version = ?3"
                ),
                [source, name, version],
            )
            .await
            .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?;
        match rows
            .next()
            .await
            .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?
        {
            Some(row) => Ok(Some(Self::row_to_installed_package(row)?)),
            None => Ok(None),
        }
    }

    // ponytail: delete/versions_of/sources_of/latest_version below have no
    // production caller yet (only sync_source's insert + clear_table_for_source
    // are wired up today) — kept as tested library surface for the CLI
    // commands that will need them (per-version removal, multi-version
    // listing, source disambiguation), not speculative bloat. Remove if a
    // future review still finds them uncalled.
    pub async fn delete(&self, source: &str, name: &str, version: &str) -> ClientResult<()> {
        let conn = self.connect().await?;
        conn.execute(
            "DELETE FROM AvailablePackages WHERE source = ?1 AND name = ?2 AND version = ?3",
            [source, name, version],
        )
        .await
        .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?;
        Ok(())
    }

    pub fn validate_table_name(tname: &str) -> ClientResult<()> {
        let is_valid = matches!(
            tname,
            "InstalledPackages" | "AvailablePackages" | "schema_migrations"
        ) || (!tname.is_empty()
            && tname.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && !tname.starts_with(|c: char| c.is_ascii_digit()));
        if !is_valid {
            return Err(ClientError::Core(DatabaseError(format!(
                "Invalid or unauthorized table name: {tname}"
            ))));
        }
        Ok(())
    }

    pub async fn execute_query(&self, query: &str) -> ClientResult<()> {
        let conn = self.connect().await?;
        conn.execute(query, ())
            .await
            .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?;
        Ok(())
    }

    pub async fn drop_table(&self, tname: &str) -> ClientResult<()> {
        Self::validate_table_name(tname)?;
        self.execute_query(&format!("DROP TABLE IF EXISTS {}", tname))
            .await
    }

    pub async fn clear_table(&self, tname: &str) -> ClientResult<()> {
        Self::validate_table_name(tname)?;
        self.execute_query(&format!("DELETE FROM {}", tname)).await
    }

    pub async fn clear_table_for_source(&self, source: &str) -> ClientResult<()> {
        let conn = self.connect().await?;
        conn.execute("DELETE FROM AvailablePackages WHERE source = ?1", [source])
            .await
            .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?;
        Ok(())
    }

    pub async fn versions_of(&self, source: &str, name: &str) -> ClientResult<Vec<DbPackage>> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                &format!("SELECT {COLUMNS} FROM AvailablePackages WHERE source = ?1 AND name = ?2"),
                [source, name],
            )
            .await
            .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?;
        let mut packages = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?
        {
            packages.push(Self::row_to_available_package(row)?);
        }
        Ok(packages)
    }

    pub async fn sources_of(&self, name: &str) -> ClientResult<Vec<String>> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT DISTINCT source FROM AvailablePackages WHERE name = ?1",
                [name],
            )
            .await
            .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?;
        let mut sources = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?
        {
            let source = row
                .get_value(0)
                .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?
                .as_text()
                .cloned()
                .ok_or_else(|| {
                    ClientError::Core(DatabaseError("source column is not text".to_string()))
                })?;
            sources.push(source);
        }
        Ok(sources)
    }

    /// 「最新版本」= 這個 (source, name) 底下 `rowid` 最大的那一列,也就是最後
    /// 插入的那筆——不比較 semver。`dpm update` 每次整個 source 清空重灌,插入
    /// 順序等於 `RepoInfo.json` 的陣列順序,等於伺服器端發布順序。真正的版本
    /// 排序邏輯留給 Phase 5(pubgrub)。
    pub async fn latest_version(
        &self,
        source: &str,
        name: &str,
    ) -> ClientResult<Option<DbPackage>> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                &format!(
                    "SELECT {COLUMNS} FROM AvailablePackages WHERE source = ?1 AND name = ?2 ORDER BY rowid DESC LIMIT 1"
                ),
                [source, name],
            )
            .await
            .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?;
        match rows
            .next()
            .await
            .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?
        {
            Some(row) => Ok(Some(Self::row_to_available_package(row)?)),
            None => Ok(None),
        }
    }

    /// Record the exact list of linked/installed files for a package in the DB manifest.
    pub async fn record_installed_files(&self, pkg: &str, files: &[String]) -> ClientResult<()> {
        let conn = self.connect().await?;
        let _ = conn
            .execute("DELETE FROM installed_files WHERE package_name = ?1", [pkg])
            .await;
        for file in files {
            conn.execute(
                "INSERT INTO installed_files (package_name, file_path) VALUES (?1, ?2)",
                [pkg, file.as_str()],
            )
            .await
            .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?;
        }
        Ok(())
    }

    /// Retrieve the exact list of linked/installed files for a package from DB.
    pub async fn get_installed_files(&self, pkg: &str) -> ClientResult<Vec<String>> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT file_path FROM installed_files WHERE package_name = ?1",
                [pkg],
            )
            .await
            .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?;

        let mut files = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?
        {
            if let Some(path) = row.get_value(0).ok().and_then(|v| v.as_text().cloned()) {
                files.push(path);
            }
        }
        Ok(files)
    }

    /// Remove the installed files manifest for a package from DB.
    pub async fn remove_installed_files(&self, pkg: &str) -> ClientResult<()> {
        let conn = self.connect().await?;
        conn.execute("DELETE FROM installed_files WHERE package_name = ?1", [pkg])
            .await
            .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?;
        Ok(())
    }
}
