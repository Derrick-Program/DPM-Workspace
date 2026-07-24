# 目錄配置 + turso + geni 遷移 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 `dpm` client 的本地套件索引資料庫從 diesel/SQLite 換成 turso + geni migration,並把安裝目錄從寫死的 `/opt/DPM` 換成 `directories::ProjectDirs` 驅動的 per-user 預設(加 `--system` flag 才走現有 root/sudo 的 shared 安裝)。

**Architecture:** 分兩個獨立可測試階段。Task 1 先把資料層换成 turso(async)+ geni(migration),連鎖把 `action.rs`/`lib.rs`/`main.rs` 改成 async,這階段結束時整個 workspace 用舊的 `/opt/DPM` 硬寫路徑也能編譯過、測試過。Task 2 在此之上加 `Scope`(PerUser/System)+ `--system` flag + `ProjectDirs`,把路徑計算從寫死常數換成 scope-aware,並讓 `SystemController` 的 sudo/chown 呼叫自我短路(per-user 模式完全不執行)。Task 3 同步更新專案 CLAUDE.md。Task 4 做整體驗證。

**Tech Stack:** Rust 2021、tokio(async runtime)、turso 0.7.1(嵌入式 SQLite 相容 DB)、geni 1.3.3(migration library)、directories 6.0.0(`ProjectDirs`)、clap 4.5(手刻 `Command`)。

## Global Constraints

- per-user 路徑用 `ProjectDirs::from("com", "duacodie", "dpm")`。
- system 路徑固定為 `/opt/com.duacodie/DPM`,子目錄沿用 `bin/`、`Software/`、`Settings/`、`LocalRepo.db`、`LocalRepo.lock`。
- scope 觸發方式:預設 per-user,顯式 `--system`/`-S` flag 才走 system。
- `directories`/`turso`/`geni` 三個新依賴只加在 `crates/dpm/Cargo.toml`(dpm-only,不進 workspace root,比照現有 `diesel`/`libc`/`fs2` 的作法)。
- 完全移除 `diesel`、`diesel_migrations`、`rusqlite`、`crates/dpm/diesel.toml`、`crates/dpm/src/utils/schema.rs`。
- `Db` 對外方法名稱不變(`new`/`run_migrations`/`insert`/`read_all`/`read_one`/`update_version`/`delete`/`execute_query`/`clear_table`),簽名一律改成 `async fn`;`read_by_condition`(diesel-only、無外部呼叫者)直接刪除。
- 不處理舊 `/opt/DPM/LocalRepo.db`(diesel 產生)的資料搬移。
- 每個 task 完成後執行 `cargo build --workspace` 確認整個 workspace(含 `dpm-server`/`dpm-core`)仍能編譯。

---

## Task 1: 資料層換成 turso + geni

**Files:**
- Modify: `crates/dpm/Cargo.toml`
- Delete: `crates/dpm/diesel.toml`
- Delete: `crates/dpm/src/utils/schema.rs`
- Modify: `crates/dpm/src/utils/mod.rs`
- Modify: `crates/dpm/src/utils/models.rs`
- Modify: `crates/dpm/src/utils/db.rs`
- Create: `crates/dpm/migrations/0001_init.up.sql`
- Create: `crates/dpm/migrations/0001_init.down.sql`
- Modify: `crates/dpm/tests/db_tests.rs`
- Modify: `crates/dpm/src/action.rs`
- Modify: `crates/dpm/src/lib.rs`
- Modify: `crates/dpm/src/main.rs`

**Interfaces:**
- Produces (供 Task 2 使用):`pub async fn set_globle_var() -> ClientResult<()>`(Task 2 會把它改成吃 `scope: Scope` 參數,簽名/位置不變)、`pub struct Db`(欄位 `db: turso::Database`、`db_path: String`、`_lock_file: File`)、`Db::new(database_path: &str, lock_file_path: &str) -> ClientResult<Self>` 為 async fn。
- Consumes:無(這個 task 不依賴 Task 2 的任何東西,`set_globle_var()` 內部路徑常數暫時維持寫死 `/opt/DPM/*`)。

- [ ] **Step 1: 把既有測試改寫成 turso 風格(先讓它紅)**

覆寫 `crates/dpm/tests/db_tests.rs`:

```rust
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
```

- [ ] **Step 2: 確認現在編不過(紅燈)**

Run: `cargo test -p DPM --test db_tests 2>&1 | tail -30`
Expected: 編譯錯誤,類似 `no method named 'await' found` 或 `this function takes 0 arguments`(因為 `Db::new`/`run_migrations` 現在還是 diesel 的 sync 版本)。這是預期的紅燈,繼續下一步。

- [ ] **Step 3: 換依賴**

編輯 `crates/dpm/Cargo.toml`,移除 `diesel`/`diesel_migrations`/`rusqlite` 三行,`[dependencies]` 區塊改成(維持字母序):

```toml
[dependencies]
DPM-Core = { workspace = true, features = ["client"] }
anstyle.workspace = true
anyhow.workspace = true
clap.workspace = true
clap_complete.workspace = true
colored.workspace = true
digest.workspace = true
dotenv = "0.15.0"
flate2.workspace = true
fs2 = "0.4.3"
futures-util.workspace = true
geni = "1.3.3"
git2 = "0.18.1"
hex.workspace = true
libc = "0.2.153"
reqwest.workspace = true
serde.workspace = true
serde_json.workspace = true
sha2.workspace = true
sudo = "0.6.0"
thiserror.workspace = true
tokio.workspace = true
turso = "0.7.1"
walkdir.workspace = true
zip.workspace = true
```

Run: `cd crates/dpm && cargo doc --no-deps -p turso -p geni 2>&1 | tail -20`(觸發下載,順便確認兩個 crate 能正常解析版本;若本機沒網路連線這步會失敗,改成直接看 `~/.cargo/registry/src/**/turso-0.7.1/src/lib.rs` 與 `**/geni-1.3.3/src/lib.rs` 原始碼,對照下面 Step 7 用到的 `turso::Value`/`turso::Builder`/`geni::migrate_database` 簽名,若跟本計畫寫的不一致,以原始碼為準調整 Step 7 的程式碼)

- [ ] **Step 4: 刪除 diesel 專屬檔案**

```bash
rm crates/dpm/diesel.toml
rm crates/dpm/src/utils/schema.rs
```

編輯 `crates/dpm/src/utils/mod.rs`,移除 `schema` 相關兩行,變成:

```rust
pub mod db;
pub mod error;
// pub mod json_parse;
pub mod models;
pub mod system;
pub mod zip_file;
pub use self::db::*;
pub use self::error::*;
// pub use self::json_parse::*;
pub use self::models::*;
pub use self::system::*;
pub use self::zip_file::*;
```

- [ ] **Step 5: 重寫 models.rs,拿掉 diesel derive**

整份覆寫 `crates/dpm/src/utils/models.rs`:

```rust
use super::ClientError;
use super::ClientResult;
use dpm_core::CoreError::*;
use dpm_core::Dependency;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DbPackage {
    pub name: String,
    pub version: String,
    pub url: String,
    pub description: String,
    pub filename: String,
    pub hash: String,
    pub entry: String,
    pub dependencies: Option<Vec<Dependency>>,
}

#[allow(clippy::too_many_arguments)]
impl DbPackage {
    pub fn new(
        name: &str,
        version: &str,
        url: &str,
        description: &str,
        filename: &str,
        hash: &str,
        entry: &str,
        dependencies: Option<Vec<Dependency>>,
    ) -> Self {
        DbPackage {
            name: name.to_owned(),
            version: version.to_owned(),
            url: url.to_owned(),
            description: description.to_owned(),
            filename: filename.to_owned(),
            hash: hash.to_owned(),
            entry: entry.to_owned(),
            dependencies,
        }
    }

    /// 將結構轉為 JSON 字串
    pub fn to_json_string(&self) -> ClientResult<String> {
        serde_json::to_string(self).map_err(|e| ClientError::Core(JsonError(e)))
    }

    /// 從 JSON 字串解析為結構
    pub fn from_json_string(json: &str) -> ClientResult<Self> {
        serde_json::from_str(json).map_err(|e| ClientError::Core(JsonError(e)))
    }
}
```

`NewDbPackage` 整個刪除(diesel `Insertable` 專用,turso 不需要中間型別)。

- [ ] **Step 6: 寫 migration SQL 檔**

Create `crates/dpm/migrations/0001_init.up.sql`:

```sql
CREATE TABLE IF NOT EXISTS LocalRepo (
    name TEXT PRIMARY KEY,
    version TEXT NOT NULL,
    url TEXT NOT NULL,
    description TEXT NOT NULL,
    filename TEXT NOT NULL,
    hash TEXT NOT NULL,
    entry TEXT NOT NULL,
    dependencies TEXT
);
```

Create `crates/dpm/migrations/0001_init.down.sql`:

```sql
DROP TABLE IF EXISTS LocalRepo;
```

- [ ] **Step 7: 重寫 db.rs**

整份覆寫 `crates/dpm/src/utils/db.rs`:

```rust
use super::{ClientError, ClientResult, DbPackage};
use dpm_core::CoreError::*;
use fs2::FileExt;
use futures_util::StreamExt;
use std::{fs::File, path::Path};
use tokio::io::AsyncWriteExt;

pub struct Db {
    db: turso::Database,
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
        let db = turso::Builder::new_local(database_path)
            .build()
            .await
            .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?;
        Ok(Db {
            db,
            db_path: database_path.to_string(),
            _lock_file: lock_file,
        })
    }

    fn connect(&self) -> ClientResult<turso::Connection> {
        self.db
            .connect()
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
            migrations_dir.join("schema.sql").to_string_lossy().to_string(),
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
        let conn = self.connect()?;
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
        let conn = self.connect()?;
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
        let conn = self.connect()?;
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
        let conn = self.connect()?;
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
        let conn = self.connect()?;
        conn.execute(
            "UPDATE LocalRepo SET version = ?1 WHERE name = ?2",
            [new_version, target_name],
        )
        .await
        .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?;
        Ok(())
    }

    pub async fn delete(&self, target_name: &str) -> ClientResult<()> {
        let conn = self.connect()?;
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
        self.execute_query(&format!("DELETE FROM {}", tname))
            .await
    }

    pub async fn download_file(&self, name: &str) -> ClientResult<()> {
        let package = self
            .read_one(name)
            .await?
            .ok_or_else(|| ClientError::Core(PackageNotFound(name.to_string())))?;
        let url = &package.url;
        let filename = Path::new("/tmp").join(&package.filename);
        let req = reqwest::get(url)
            .await
            .map_err(|e| ClientError::Core(NetworkError(e.to_string())))?;
        if !req.status().is_success() {
            return Err(ClientError::Core(NetworkError(format!(
                "Failed to download file: HTTP {}",
                req.status()
            ))));
        }
        let mut file = tokio::fs::File::create(&filename)
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
        println!("File downloaded to: {}", filename.display());
        Ok(())
    }
}
```

> `turso::Value::Text`/`turso::Row::get_value`/`turso::Connection::query`/`execute` 的確切簽名是照 Step 3 查到的文件範例寫的。如果 Step 3 對照原始碼發現型別不同(例如 `Value::Text` 吃的不是 `String`,或 `get_value` 回傳型別不同),照原始碼調整這裡的程式碼,`ClientResult<...>` 回傳型別與函式名稱維持不變即可,不影響其他 task。

- [ ] **Step 8: 跑測試,確認變綠**

Run: `cargo test -p DPM --test db_tests -- --nocapture`
Expected: 5 個測試全過(`test_db_new_and_migrations`、`test_insert_and_read_all`、`test_read_one`、`test_update_version`、`test_delete`)。

- [ ] **Step 9: 把 async 傳染到 action.rs**

編輯 `crates/dpm/src/action.rs`:

1. `parse_mine` 維持 sync(它只呼叫 `read_all`,但 `read_all` 現在是 async——把 `parse_mine` 也改成 `async fn` 並在唯一呼叫處補 `.await`):

```rust
async fn parse_mine(&self) -> (Vec<String>, Vec<String>) {
    let mut is: Vec<String> = Vec::new();
    let mut isnot: Vec<String> = Vec::new();
    let all_packages = get_db().read_all().await.unwrap_or_else(|_| Vec::new());
    let package_names: Vec<String> = all_packages.into_iter().map(|pkg| pkg.name).collect();
    for pkg in &self.pkgs {
        if package_names.contains(pkg) {
            is.push(pkg.clone());
        } else {
            isnot.push(pkg.clone());
        }
    }
    (is, isnot)
}
```

2. `install`(已經是 `async fn`)裡把 `self.parse_mine()` 改成 `self.parse_mine().await`,並把兩個 `get_db().read_one(...)` / `get_db().insert(...)` 呼叫補上 `.await`(維持原本的 `.map_err(...)` 鏈,只在 `get_db().read_one(pkg)` 與 `.insert(...)` 之後、`.map_err` 之前插入 `.await`):

```rust
pub async fn install(&self) -> ClientResult<()> {
    let (is, isnot) = self.parse_mine().await;
    if !is.is_empty() {
        for pkg in is {
            let pkg = pkg.as_str();
            let repo_package_info = get_db()
                .read_one(pkg)
                .await
                .map_err(|e| ClientError::Core(CoreError::DatabaseError(e.to_string())))?
                .ok_or_else(|| {
                    ClientError::Core(CoreError::PackageNotFound(pkg.to_string()))
                })?;
            if self.verbose {
                println!("{}\n\n  {}", pkg.on_green(), "Downloading...".yellow());
            }
            get_db()
                .download_file(pkg)
                .await
                .map_err(|e| ClientError::Core(CoreError::NetworkError(e.to_string())))?;
            if self.verbose {
                println!("  {}", "Download successed!".green());
            }
            let ori_path = Path::new("/tmp").join(repo_package_info.filename);
            let package_info_test: String =
                read_file_from_zip(&ori_path, "packageInfo.json").unwrap();
            let package_info: PackageInfo =
                JsonStorage::from_str_to(package_info_test.as_str()).unwrap();
            let package_hash_info: Hashes = JsonStorage::from_str_to(
                read_file_from_zip(&ori_path, "hashes.json")
                    .unwrap()
                    .as_str(),
            )
            .unwrap();
            if self.verbose {
                println!(
                    "  {}",
                    "Checking Package Hash ...(May take a while)".yellow()
                );
            }
            let hash = Self::hasher(&ori_path)?;
            if repo_package_info.hash != hash {
                return Err(ClientError::Core(CoreError::HashMismatch {
                    expected: repo_package_info.hash,
                    actual: hash,
                }));
            }
            if &package_info.hash != package_hash_info.get("hashes.json").unwrap() {
                return Err(ClientError::Core(CoreError::HashMismatch {
                    expected: package_info.hash.clone(),
                    actual: package_hash_info.get("hashes.json").unwrap().clone(),
                }));
            }

            if self.verbose {
                println!("  {}", "Hashes Passed".green());
                println!("  {}", "Installing ...".yellow());
            }

            let install_path = INSTALL_DIR.get().unwrap().join(pkg);
            unzip_file(&ori_path, &install_path)
                .map_err(|e| ClientError::Core(CoreError::IoError(e)))?;
            if self.verbose {
                println!("  {}", "Installed!".green());
                println!("  {}", "Removing tmp file ...".blue());
            }
            remove_file(ori_path).map_err(|e| ClientError::Core(CoreError::IoError(e)))?;
            if self.verbose {
                println!("  {}", "Removed Success ...".green());
                println!("  {}", "Create Links ...".yellow());
            }
            let main_file = install_path.join(&package_info.file_name);
            let ln_path = BIN_DIR.get().unwrap().join(pkg);
            fs::set_permissions(&main_file, Permissions::from_mode(0o755))
                .map_err(|e| ClientError::SystemError(e.to_string()))?;
            self.system_controller.system_command_runner(
                "ln",
                vec![
                    "-s",
                    main_file.display().to_string().as_str(),
                    ln_path.display().to_string().as_str(),
                ],
                "Can't create link",
            )?;
            if self.verbose {
                println!("  {}", "Successed Create Link!".green());
            }
        }
    }
    if !isnot.is_empty() {
        for pkg in isnot {
            self.system_action.install_package(&pkg)?;
        }
    }
    Ok(())
}
```

3. `update`/`init_update`(已經是 `async fn`)裡的 `db.clear_table("LocalRepo")` 與兩處 `get_db().insert(...)` 補 `.await`:

```rust
pub async fn update(&self) -> ClientResult<()> {
    println!("{} Updating...", "==>".blue());
    let mut remote_repo = RepoInfo::new();

    let repo_info_url = self.setting_config.get("repo_info").ok_or_else(|| {
        ClientError::ConfigError("Missing 'repo_info' in settings".to_string())
    })?;

    remote_repo.fetch_update_repo_info(repo_info_url).await?;
    let db = get_db();

    db.clear_table("LocalRepo").await?;

    let repo_handler = remote_repo.get_package_handler();

    for (name, repo_info) in repo_handler {
        let dependencies1: Option<Vec<dpm_core::Dependency>> =
            repo_info.dependencies.as_ref().map(|deps| {
                deps.iter()
                    .map(|dep| Dependency::new(&dep.name, &dep.version))
                    .collect::<Vec<_>>()
            });
        let package_info = remote_repo.get_single_package_info(name).await?;
        println!("{} Updating...", name.green());
        get_db()
            .insert(DbPackage::new(
                name,
                repo_info.version.as_str(),
                repo_info.url.as_str(),
                package_info.description.as_str(),
                repo_info.file_name.as_str(),
                repo_info.hash.as_str(),
                package_info.file_name.as_str(),
                dependencies1,
            ))
            .await?;
    }
    println!("{} Updated!", "==>".green());
    Ok(())
}

pub async fn init_update(url_json: &str) -> ClientResult<()> {
    let mut remote_repo = RepoInfo::new();
    remote_repo.fetch_update_repo_info(url_json).await?;
    for (name, repo_info) in remote_repo.get_package_handler() {
        let dependencies1: Option<Vec<dpm_core::Dependency>> =
            repo_info.dependencies.as_ref().map(|deps| {
                deps.iter()
                    .map(|dep| Dependency::new(&dep.name, &dep.version))
                    .collect::<Vec<_>>()
            });
        get_db()
            .insert(DbPackage::new(
                name,
                repo_info.version.as_str(),
                repo_info.url.as_str(),
                repo_info
                    .description
                    .as_ref()
                    .unwrap_or(&String::new())
                    .as_str(),
                repo_info.file_name.as_str(),
                repo_info.hash.as_str(),
                repo_info.entry.as_ref().unwrap_or(&String::new()).as_str(),
                dependencies1,
            ))
            .await?;
    }
    Ok(())
}
```

4. `uninstall`/`search`/`list`/`upgrade` 從 `pub fn` 改成 `pub async fn`,並把內部的 `self.parse_mine()` 改成 `.await`:

```rust
pub async fn uninstall(&self) -> ClientResult<()> {
    let (is, isnot) = self.parse_mine().await;
    if !is.is_empty() {
        for pkg in is {
            let pre_rm_location = INSTALL_DIR.get().unwrap().join(&pkg);
            let pre_rm_ln = BIN_DIR.get().unwrap().join(&pkg);
            if self.verbose {
                println!("{}\n\n  {}", pkg.on_green(), "Removing...".red());
            }
            remove_dir_all(pre_rm_location)
                .map_err(|e| ClientError::Core(CoreError::IoError(e)))?;
            if self.verbose {
                println!("  {}", "Removed!".green());
                println!("  {}", "UnLinking...".red());
            }
            remove_file(pre_rm_ln).map_err(|e| ClientError::Core(CoreError::IoError(e)))?;
            if self.verbose {
                println!("  {}", "Done".green());
            }
        }
    }
    if !isnot.is_empty() {
        for pkg in isnot {
            self.system_action.uninstall_package(&pkg)?;
        }
    }
    Ok(())
}

pub async fn search(&self) -> ClientResult<()> {
    let (is, isnot) = self.parse_mine().await;
    if !is.is_empty() {
        println!();
        for pkg in is {
            println!("{} {}", pkg, "Found!!".green());
        }
    }
    if !isnot.is_empty() {
        for pkg in &self.pkgs {
            self.system_action.search_package(pkg.as_str())?;
        }
    }
    Ok(())
}

pub async fn list(&self, sys: bool) -> ClientResult<()> {
    if sys {
        self.system_action.list_packages()?;
    } else {
        let path = INSTALL_DIR.get().unwrap();
        for entry in WalkDir::new(path) {
            let entry = entry.map_err(|e| ClientError::Core(CoreError::IoError(e.into())))?;
            let _path = entry.path();
        }
    }
    Ok(())
}

pub async fn upgrade(&self) -> ClientResult<()> {
    let (is, isnot) = self.parse_mine().await;
    if !is.is_empty() {
        for pkg in is {
            println!("{:#?}", pkg);
        }
    }
    if !isnot.is_empty() {
        for pkg in isnot {
            self.system_action.upgrade_package(&pkg)?;
        }
    }
    Ok(())
}
```

`upgrade_self`/`hasher` 不動。

- [ ] **Step 10: entry() 補 await,main.rs 包成 async**

編輯 `crates/dpm/src/lib.rs`,`entry` 拿掉 `#[tokio::main]`(改由 `main.rs` 提供 runtime),`set_globle_var` 改成 `async fn`(路徑暫時還是寫死 `/opt/DPM`,Task 2 才會改成吃 `Scope`):

```rust
pub async fn entry(config: Cli) -> ClientResult<()> {
    let setting_config: Setting = SystemController.init().await?;
    let pass_info = ActionInfo::new(
        config.PackageName.unwrap_or_default(),
        config.Verbose,
        setting_config,
    );
    match config.Commands.unwrap() {
        CliCommands::Install => pass_info.install().await?,
        CliCommands::List => {
            if let Some(options) = &config.Other {
                if let Some(true) = options.List_sys_installed {
                    pass_info.list(true).await?;
                }
                if let Some(true) = options.List_installed {
                    pass_info.list(false).await?;
                }
            }
        }
        CliCommands::Search => pass_info.search().await?,
        CliCommands::Uninstall => pass_info.uninstall().await?,
        CliCommands::Update => pass_info.update().await?,
        CliCommands::Upgrade => pass_info.upgrade().await?,
        CliCommands::UpgradeSelf => pass_info.upgrade_self(),
        CliCommands::None => panic!("No command found"),
    }
    SystemController.permision_check()?;
    Ok(())
}

pub fn get_db() -> &'static Db {
    DB_INSTANCE
        .get()
        .expect("Database instance not initialized")
}

pub async fn set_globle_var() -> ClientResult<()> {
    MAIN_DIR.set(PathBuf::from("/opt/DPM")).unwrap();
    BIN_DIR.set(PathBuf::from("/opt/DPM/bin")).unwrap();
    INSTALL_DIR.set(PathBuf::from("/opt/DPM/Software")).unwrap();
    CONFIG.set(PathBuf::from("/opt/DPM/Settings")).unwrap();
    VERSION.set(env!("CARGO_PKG_VERSION").to_string()).unwrap();
    BIN.set("dpm".to_string()).unwrap();
    let main_dir = MAIN_DIR.get().unwrap();
    if !main_dir.exists() {
        SystemController.system_command_runner(
            "mkdir",
            vec!["-p", main_dir.to_str().unwrap()],
            "Can't create /opt/DPM dir",
        )?;
        SystemController.permision_check()?;
    }
    let db_path = MAIN_DIR.get().unwrap().join("LocalRepo.db");
    let db = Db::new(db_path.to_str().unwrap(), "/opt/DPM/LocalRepo.lock")
        .await
        .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?;
    db.run_migrations().await?;
    DB_INSTANCE
        .set(db)
        .map_err(|_| "Failed to set DB_INSTANCE")
        .unwrap();
    Ok(())
}
```

編輯 `crates/dpm/src/main.rs`:

```rust
use DPM::{set_globle_var, ClientError, ClientResult};

#[tokio::main]
async fn main() -> ClientResult<()> {
    if cfg!(target_os = "linux") {
        // 不是 root 時會自動重新以 sudo 執行自己;已是 root 則直接繼續
        sudo::escalate_if_needed().map_err(|e| ClientError::SystemError(e.to_string()))?;
    }
    // 權限確定後才初始化全域變數與資料庫（會碰 /opt/DPM）
    set_globle_var().await?;
    let args = DPM::get_args().map_err(|e| ClientError::SystemError(e.to_string()))?;
    if let Err(e) = DPM::entry(args).await {
        eprintln!("{}", e);
        std::process::exit(1);
    }
    Ok(())
}
```

- [ ] **Step 11: 整個 workspace 編譯 + 全部測試**

Run: `cargo build --workspace 2>&1 | tail -40`
Expected: 無錯誤(`dpm`/`dpm-server`/`dpm-core` 都過)。

Run: `cargo test -p DPM 2>&1 | tail -40`
Expected: 全部測試(含 `db_tests.rs` 5 個)通過。

- [ ] **Step 12: Commit**

```bash
git add crates/dpm/Cargo.toml crates/dpm/diesel.toml crates/dpm/src/utils/schema.rs \
  crates/dpm/src/utils/mod.rs crates/dpm/src/utils/models.rs crates/dpm/src/utils/db.rs \
  crates/dpm/migrations crates/dpm/tests/db_tests.rs crates/dpm/src/action.rs \
  crates/dpm/src/lib.rs crates/dpm/src/main.rs
git commit -m "$(cat <<'EOF'
feat(dpm): replace diesel/SQLite with turso + geni migrations

Db is now async and backed by turso::Database instead of a
Mutex-wrapped diesel SqliteConnection. Migrations run through geni
against SQL files embedded via include_str! and unpacked next to the
db file at startup. Drops diesel/diesel_migrations/rusqlite and the
stale diesel.toml migrations_directory.

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: 目錄 scope(directories + `--system` flag)

**Files:**
- Modify: `crates/dpm/Cargo.toml`
- Modify: `crates/dpm/src/arch.rs`
- Modify: `crates/dpm/src/cli_parse.rs`
- Modify: `crates/dpm/src/utils/system.rs`
- Modify: `crates/dpm/src/lib.rs`
- Modify: `crates/dpm/src/main.rs`
- Test: `crates/dpm/src/lib.rs`(inline `#[cfg(test)]` module,測 `compute_paths`)

**Interfaces:**
- Consumes:Task 1 產生的 `Db::new(path, lock_path) -> ClientResult<Self>`(async)、`set_globle_var() -> ClientResult<()>`(async,這個 task 改成吃 `scope` 參數)。
- Produces:`pub enum Scope { PerUser, System }`(`arch.rs`)、`pub async fn set_globle_var(scope: Scope) -> ClientResult<()>`、`pub fn init_cli_metadata()`,兩者都給 `main.rs` 呼叫。

- [ ] **Step 1: 加 `directories` 依賴**

編輯 `crates/dpm/Cargo.toml`,`digest.workspace = true` 之後、`dotenv = "0.15.0"` 之前插入一行:

```toml
directories = "6.0.0"
```

- [ ] **Step 2: 加 `Scope` enum 與 `Cli.System` 欄位(先寫會壞的型別,驅動後續實作)**

編輯 `crates/dpm/src/arch.rs`,在檔案裡加入 `Scope`,並在 `Cli` struct 加一個欄位:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    PerUser,
    System,
}
```

`Cli` struct 改成(在既有欄位後加 `System`):

```rust
pub struct Cli {
    pub Commands: Option<CliCommands>,
    pub PackageName: Option<Vec<String>>,
    pub Verbose: bool,
    pub Other: Option<Option_set>,
    pub System: bool,
}
```

Run: `cargo build -p DPM 2>&1 | tail -20`
Expected: 編譯失敗,`cli_parse.rs` 裡建構 `Cli { ... }` 缺少 `System` 欄位(`missing field 'System'`)。這是預期的紅燈,下一步補上。

- [ ] **Step 3: cli_parse.rs 加 `--system`/`-S` flag**

編輯 `crates/dpm/src/cli_parse.rs`,`build_cli()` 最後的 `.arg(generator...)` 之後再加一個 global arg:

```rust
    .arg(
        Arg::new("generator")
            .short('g')
            .long("gen")
            .action(ArgAction::Set)
            .aliases(["gen", "generator", "autocomplete", "complete"])
            .value_parser(value_parser!(Shell)),
    )
    .arg(
        Arg::new("system")
            .short('S')
            .long("system")
            .help("Operate on the shared system-wide install (requires root)")
            .action(ArgAction::SetTrue),
    )
}
```

`get_args()` 開頭(`let matches = build_cli().get_matches();` 之後)讀取這個 flag,並在最後回傳的 `Cli { ... }` 補上 `System`:

```rust
pub fn get_args() -> Result<Cli> {
    let matches = build_cli().get_matches();
    if let Some(generator) = matches.get_one::<Shell>("generator").copied() {
        let mut cmd = build_cli();
        eprintln!("Generating completion file for {generator}...");
        print_completions(generator, &mut cmd);
    }
    let system = matches.get_flag("system");
    let mut Commands: Option<CliCommands> = Option::<CliCommands>::None;
    let mut Verbose = false;
    let mut PN = vec![];
    let mut Other = Option_set::default();

    let config = match matches.subcommand() {
        Some(("install", sub_command)) => {
            Commands = Some(CliCommands::Install);
            Verbose = sub_command.get_flag("verbose");
            PN = sub_command
                .get_many::<String>("PN")
                .unwrap_or_default()
                .map(|v| v.to_string())
                .collect::<Vec<String>>();
        }
        Some(("update", sub_command)) => {
            Commands = Some(CliCommands::Update);
            Verbose = sub_command.get_flag("verbose");
        }
        Some(("uninstall", sub_command)) => {
            Commands = Some(CliCommands::Uninstall);
            Verbose = sub_command.get_flag("verbose");
            PN = sub_command
                .get_many::<String>("PN")
                .unwrap_or_default()
                .map(|v| v.to_string())
                .collect::<Vec<String>>();
        }
        Some(("search", sub_command)) => {
            Commands = Some(CliCommands::Search);
            Verbose = sub_command.get_flag("verbose");
            PN = sub_command
                .get_many::<String>("PN")
                .unwrap_or_default()
                .map(|v| v.to_string())
                .collect::<Vec<String>>();
        }
        Some(("list", sub_command)) => {
            Commands = Some(CliCommands::List);
            Verbose = sub_command.get_flag("verbose");
            Other.List_installed = Some(sub_command.get_flag("list-installed"));
            Other.List_sys_installed = Some(sub_command.get_flag("list-sys-installed"));
        }
        Some(("upgrade", sub_command)) => {
            Commands = Some(CliCommands::Upgrade);
            Verbose = sub_command.get_flag("verbose");
            PN = sub_command
                .get_many::<String>("PN")
                .unwrap_or_default()
                .map(|v| v.to_string())
                .collect::<Vec<String>>();
        }
        Some(("upgradeSelf", sub_command)) => {
            Commands = Some(CliCommands::UpgradeSelf);
            Verbose = sub_command.get_flag("verbose");
        }
        _ => return Err(anyhow!("Unrecognized command")),
    };
    let PackageName = if PN.is_empty() { None } else { Some(PN) };
    Ok(Cli {
        Commands,
        PackageName,
        Verbose,
        Other: Some(Other),
        System: system,
    })
}
```

（跟原檔案唯一差異:開頭多了 `let system = matches.get_flag("system");`,結尾回傳結構多了 `System: system,`,中間整段 `match matches.subcommand()` 逐字不變。）

Run: `cargo build -p DPM 2>&1 | tail -20`
Expected: `cli_parse.rs` 編譯通過(`Cli` 欄位補齊)。`system.rs`/`lib.rs` 還沒改,可能還有其他錯誤,繼續下一步。

- [ ] **Step 4: system.rs 讓 `permision_check`/`system_command_runner` 依 scope 自我短路**

編輯 `crates/dpm/src/utils/system.rs`,import 加 `Scope`、`SCOPE`:

```rust
use crate::{ActionInfo, Scope, Setting, BIN_DIR, CONFIG, INSTALL_DIR, MAIN_DIR, SCOPE};
```

`permision_check` 開頭加 early return:

```rust
pub fn permision_check(&self) -> ClientResult<()> {
    if SCOPE.get() != Some(&Scope::System) {
        return Ok(());
    }
    // 若透過 sudo 執行,getuid() 會拿到 root;優先用 SUDO_USER 取得原始使用者
    let username = std::env::var("SUDO_USER")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| self.get_current_username())
        .ok_or_else(|| {
            ClientError::SystemError("Could not get current username".to_string())
        })?;
    if cfg!(target_os = "linux") {
        self.system_command_runner(
            "chown",
            vec!["-R", "root:root", MAIN_DIR.get().unwrap().to_str().unwrap()],
            "Can't run chown",
        )?;
    } else if cfg!(target_os = "macos") {
        self.system_command_runner(
            "chown",
            vec![
                "-R",
                format!("{}:admin", username).as_str(),
                MAIN_DIR.get().unwrap().to_str().unwrap(),
            ],
            "Can't run chown",
        )?;
    }
    Ok(())
}
```

`system_command_runner` 改成只在 `Scope::System` 時才在指令前面加 `sudo`,per-user 直接跑原指令(不需要 root):

```rust
pub fn system_command_runner(
    &self,
    command: &str,
    args: Vec<&str>,
    err_message: &str,
) -> ClientResult<()> {
    if !(cfg!(target_os = "linux") || cfg!(target_os = "macos")) {
        panic!("Unsupported OS");
    }
    let mut cmd = if SCOPE.get() == Some(&Scope::System) {
        let mut c = Command::new("sudo");
        c.arg(command);
        c
    } else {
        Command::new(command)
    };
    cmd.args(&args);
    let status = cmd
        .status()
        .map_err(|e| ClientError::SystemError(e.to_string()))?;
    if !status.success() {
        let err = err_message.to_string();
        return Err(ClientError::SystemError(err));
    }
    Ok(())
}
```

`SystemAction::command_runner`(用在 `install_package`/`search_package` 等呼叫系統套件管理員的地方,例如 `apt`/`brew`)**不動**——那是裝系統層級套件本來就需要 root 的邏輯,跟 dpm 自己的安裝目錄 scope 無關。

- [ ] **Step 5: lib.rs 加 `SCOPE`、`init_cli_metadata`、把 `set_globle_var` 改成吃 `Scope`**

編輯 `crates/dpm/src/lib.rs`,檔案最上面的 use/mod/static 區塊改成(在 `use std::sync::OnceLock;` 後面插入 `use directories::ProjectDirs;`,在既有 `static` 列表最後加 `SCOPE`,其餘逐行不變):

```rust
#![allow(non_snake_case)]
use std::collections::HashMap;
use std::path::PathBuf;
pub type Setting = HashMap<String, String>;
pub type Hashes = HashMap<String, String>;
use std::sync::OnceLock;
use directories::ProjectDirs;
mod action;
mod arch;
mod cli_parse;
mod utils;
pub use action::*;
pub use arch::*;
pub use cli_parse::*;
use dpm_core::CoreError::DatabaseError;
pub use utils::*;
static MAIN_DIR: OnceLock<PathBuf> = OnceLock::new();
static BIN_DIR: OnceLock<PathBuf> = OnceLock::new();
static INSTALL_DIR: OnceLock<PathBuf> = OnceLock::new();
static CONFIG: OnceLock<PathBuf> = OnceLock::new();
static VERSION: OnceLock<String> = OnceLock::new();
static BIN: OnceLock<String> = OnceLock::new();
static DB_INSTANCE: OnceLock<Db> = OnceLock::new();
static SCOPE: OnceLock<Scope> = OnceLock::new();
```

新增一個純函式算路徑(方便單元測試,不碰任何全域狀態):

```rust
fn compute_paths(scope: Scope) -> ClientResult<(PathBuf, PathBuf, PathBuf, PathBuf)> {
    match scope {
        Scope::PerUser => {
            let proj_dirs = ProjectDirs::from("com", "duacodie", "dpm").ok_or_else(|| {
                ClientError::SystemError("no valid home directory found".to_string())
            })?;
            let data_dir = proj_dirs.data_dir().to_path_buf();
            Ok((
                data_dir.clone(),
                data_dir.join("bin"),
                data_dir.join("Software"),
                proj_dirs.config_dir().to_path_buf(),
            ))
        }
        Scope::System => {
            let root = PathBuf::from("/opt/com.duacodie/DPM");
            Ok((
                root.clone(),
                root.join("bin"),
                root.join("Software"),
                root.join("Settings"),
            ))
        }
    }
}

/// 設定跟 scope 無關的 CLI metadata(clap 建構 Command 需要),
/// 必須在 get_args() 之前呼叫,因為 scope 要等 get_args() 解析完 --system 才知道。
pub fn init_cli_metadata() {
    VERSION.set(env!("CARGO_PKG_VERSION").to_string()).unwrap();
    BIN.set("dpm".to_string()).unwrap();
}

pub async fn set_globle_var(scope: Scope) -> ClientResult<()> {
    SCOPE.set(scope).unwrap();
    let (main_dir, bin_dir, install_dir, config_dir) = compute_paths(scope)?;
    MAIN_DIR.set(main_dir).unwrap();
    BIN_DIR.set(bin_dir).unwrap();
    INSTALL_DIR.set(install_dir).unwrap();
    CONFIG.set(config_dir).unwrap();
    // 第一次執行時目錄可能不存在,先建立目錄並修正擁有者(system scope 才需要),
    // 否則下面 Db::new 建立 lock 檔會直接 Permission denied
    let main_dir = MAIN_DIR.get().unwrap();
    if !main_dir.exists() {
        SystemController.system_command_runner(
            "mkdir",
            vec!["-p", main_dir.to_str().unwrap()],
            "Can't create DPM main dir",
        )?;
        SystemController.permision_check()?;
    }
    let db_path = MAIN_DIR.get().unwrap().join("LocalRepo.db");
    let lock_path = MAIN_DIR.get().unwrap().join("LocalRepo.lock");
    let db = Db::new(
        db_path.to_str().unwrap(),
        lock_path.to_str().unwrap(),
    )
    .await
    .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?;
    db.run_migrations().await?;
    DB_INSTANCE
        .set(db)
        .map_err(|_| "Failed to set DB_INSTANCE")
        .unwrap();
    Ok(())
}

#[cfg(test)]
mod scope_tests {
    use super::*;

    #[test]
    fn per_user_and_system_scopes_produce_different_roots() {
        let (per_user_main, _, _, _) = compute_paths(Scope::PerUser).unwrap();
        let (system_main, system_bin, system_install, system_config) =
            compute_paths(Scope::System).unwrap();
        assert_ne!(per_user_main, system_main);
        assert_eq!(system_main, PathBuf::from("/opt/com.duacodie/DPM"));
        assert_eq!(system_bin, PathBuf::from("/opt/com.duacodie/DPM/bin"));
        assert_eq!(
            system_install,
            PathBuf::from("/opt/com.duacodie/DPM/Software")
        );
        assert_eq!(
            system_config,
            PathBuf::from("/opt/com.duacodie/DPM/Settings")
        );
    }
}
```

(`system_command_runner`/`permision_check` 呼叫維持不改寫法——它們現在會依 `SCOPE` 自動短路,`set_globle_var` 本身不用再自己判斷 scope。)

Run: `cargo test -p DPM scope_tests -- --nocapture`
Expected: `per_user_and_system_scopes_produce_different_roots` 通過。

- [ ] **Step 6: main.rs 改成先解析參數才決定 scope/是否 escalate**

整份覆寫 `crates/dpm/src/main.rs`:

```rust
use DPM::{ClientError, ClientResult, Scope};

#[tokio::main]
async fn main() -> ClientResult<()> {
    DPM::init_cli_metadata();
    let args = DPM::get_args().map_err(|e| ClientError::SystemError(e.to_string()))?;
    let scope = if args.System {
        Scope::System
    } else {
        Scope::PerUser
    };
    if scope == Scope::System && cfg!(target_os = "linux") {
        // 不是 root 時會自動重新以 sudo 執行自己;已是 root 則直接繼續
        sudo::escalate_if_needed().map_err(|e| ClientError::SystemError(e.to_string()))?;
    }
    // scope 確定後才初始化全域路徑與資料庫
    DPM::set_globle_var(scope).await?;
    if let Err(e) = DPM::entry(args).await {
        eprintln!("{}", e);
        std::process::exit(1);
    }
    Ok(())
}
```

- [ ] **Step 7: 整個 workspace 編譯 + 測試**

Run: `cargo build --workspace 2>&1 | tail -40`
Expected: 無錯誤。

Run: `cargo test -p DPM 2>&1 | tail -40`
Expected: 全部測試(`db_tests.rs` 5 個 + `scope_tests` 1 個)通過。

- [ ] **Step 8: Commit**

```bash
git add crates/dpm/Cargo.toml crates/dpm/src/arch.rs crates/dpm/src/cli_parse.rs \
  crates/dpm/src/utils/system.rs crates/dpm/src/lib.rs crates/dpm/src/main.rs
git commit -m "$(cat <<'EOF'
feat(dpm): add per-user/system install scope via directories crate

dpm now defaults to a per-user install (ProjectDirs, no root needed).
--system/-S opts into the previous shared /opt install (moved to
/opt/com.duacodie/DPM), keeping the existing sudo/chown behavior.
SystemController::permision_check/system_command_runner now
short-circuit based on scope so no other call site needs to know
about it.

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: 更新專案 CLAUDE.md

**Files:**
- Modify: `/Users/derrick/Documents/Program/rust/Project/DPM-Workspace/CLAUDE.md`

**Interfaces:**
- Consumes:Task 1、Task 2 的最終行為(async `Db`、`Scope`、`--system` flag)。
- Produces:無(純文件)。

- [ ] **Step 1: 更新「Client 資料層」bullet**

把:

```
- **Client 資料層**:diesel + SQLite,DB 在 `/opt/DPM/LocalRepo.db`,migration 用 `embed_migrations!` 內嵌(`crates/dpm/migrations/`),schema 在 `src/utils/schema.rs`。用 fs2 file lock(`/opt/DPM/LocalRepo.lock`)防多實例。`crates/dpm/diesel.toml` 的 `migrations_directory` 是舊 repo 分割前留下的絕對路徑,已經指向不存在的位置 —— 執行 `just migration-new`/`migration-run` 前先確認 diesel CLI 實際寫入/讀取到哪裡,別假設它對著 `crates/dpm/migrations`。
```

換成:

```
- **Client 資料層**:`turso`(純 Rust、async、SQLite 相容)+ `geni` 做 migration。DB 檔案位置依安裝 scope 而定(見下方權限模型),migration SQL 檔放 `crates/dpm/migrations/`,用 `include_str!` 編進 binary,啟動時攤開到 DB 檔案同層的 `migrations/` 資料夾再交給 `geni::migrate_database` 執行。用 fs2 file lock(`<data_dir>/LocalRepo.lock`)防多實例。舊的 `diesel.toml`/`schema.rs`/diesel migration 機制已完全移除。
```

- [ ] **Step 2: 更新「權限模型」bullet**

把:

```
- **權限模型**:Linux 需 root(`sudo::escalate_if_needed` 自動提權),macOS 用 `sudo` 呼叫個別指令。`SUDO_USER` 用來在提權後拿真實使用者做 chown。初始化順序:先確保 `/opt/DPM` 存在 → 開 DB → 其他動作(見 `dpm/src/lib.rs::set_globle_var` 與 `main.rs`)。
```

換成:

```
- **權限模型**:雙 scope。預設 per-user,安裝路徑用 `directories::ProjectDirs::from("com", "duacodie", "dpm")`,完全不需要 root,`SystemController::permision_check`/`system_command_runner` 內部依 `SCOPE`(`OnceLock<Scope>`)自我短路,per-user 模式下不會呼叫 sudo/chown。加上 `--system`/`-S` flag 才走 shared 安裝(`/opt/com.duacodie/DPM`),行為跟舊版一致:Linux 整進程 `sudo::escalate_if_needed()` 提權,macOS 逐指令 `sudo`,`SUDO_USER` 用來取得原始使用者做 chown。scope 由 `main.rs` 在呼叫 `set_globle_var(scope)` 前,從解析出來的 `Cli.System` 決定(見 `dpm/src/lib.rs::set_globle_var` 與 `main.rs`)。
```

- [ ] **Step 3: 在「已知待處理問題」加一筆 turso/geni 版本備註**

在該區塊清單最後加一行:

```
- `geni` 內部釘的 `turso` 版號(`^0.6.1`)跟專案直接依賴的 `turso`(`0.7.1`)不一致,依賴樹裡會有兩份 turso,非阻塞但之後 `geni` 出新版對齊時可以清掉。
```

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md
git commit -m "$(cat <<'EOF'
docs(claude-md): document turso/geni data layer and dual install scope

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: 最終驗證

**Files:** 無新增/修改,純驗證。

**Interfaces:** 無。

- [ ] **Step 1: 完整 pre-commit 檢查**

Run: `just pre-commit`
Expected: fmt + clippy + test 全過(`just` 會透過 Infisical 注入 env,照 `CLAUDE.md` 既有慣例執行,不用額外設環境變數)。

- [ ] **Step 2: per-user 預設行為 smoke test(不應該跳出 sudo 密碼提示)**

Run: `just run-client -- list -l`
Expected: 指令直接執行完(可能因為目錄剛建立、套件為空而輸出很少),過程中**不會**出現 `Password:` 之類的 sudo 提示——這是驗證 per-user 預設不需要 root 的關鍵訊號。

- [ ] **Step 3: `--system` 模式(需要人工確認,agent sandbox 通常沒有 sudo 權限,無法在這裡自動驗證)**

記錄給使用者手動驗證:在有 sudo 權限的機器上執行 `dpm --system list -l`,預期會跳出 sudo 密碼提示,且安裝目錄落在 `/opt/com.duacodie/DPM`。若沒有 sudo 權限跑這步,在 PR/交付說明裡註明「`--system` 路徑未經人工驗證,需要使用者自行測試」。

- [ ] **Step 4: 確認沒有殘留的 diesel 痕跡**

Run: `rg -n "diesel|rusqlite" crates/dpm/Cargo.toml crates/dpm/src`
Expected: 沒有任何輸出(空結果)。
