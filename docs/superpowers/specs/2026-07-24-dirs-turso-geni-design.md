# 目錄配置 + turso + geni 遷移設計

日期:2026-07-24

## 背景與動機

`dpm`(client)目前有兩處寫死的假設:

1. **目錄**:`crates/dpm/src/lib.rs::set_globle_var()` 把 `MAIN_DIR`/`BIN_DIR`/`INSTALL_DIR`/`CONFIG` 全部寫死指向 `/opt/DPM/*`,且 `main.rs` 在 Linux 上不分青紅皂白對每個指令都呼叫 `sudo::escalate_if_needed()`。這代表 `dpm` 永遠是 system-wide 安裝、永遠要 root,無法給單一使用者安裝套件。
2. **資料庫**:本地套件索引 `LocalRepo.db` 用 `diesel` + `diesel_migrations`(`crates/dpm/src/utils/db.rs`、`schema.rs`、`diesel.toml`)。`diesel.toml` 的 `migrations_directory` 是舊 repo 分割前留下的絕對路徑,已經指向不存在的位置(CLAUDE.md 已記錄這個坑),`just migration-new`/`migration-run` 實際上對不上 `crates/dpm/migrations/`。

這次要:
- 導入 `directories` crate 的 `ProjectDirs`,讓 `dpm` 同時支援「幫自己裝」跟「幫全部使用者裝」兩種情境。
- 把資料庫引擎從 diesel/SQLite 換成 `turso`(純 Rust、async、SQLite 相容的嵌入式資料庫)。
- migration 改用 `geni` crate(library 用法,不是外部 CLI),徹底解掉 `diesel.toml` 那個壞掉的舊路徑問題。

## 目標

- `dpm` 預設以 per-user 模式執行,不需要 root/sudo。
- 加上 `--system` flag 後,走現有的 shared 安裝模式(root 擁有,chown,sudo 逐指令執行),安裝路徑從 `/opt/DPM` 改成 `/opt/com.duacodie/DPM`。
- 本地套件索引資料庫從 diesel/SqliteConnection 換成 `turso::Database`,對外 API(`insert`/`read_all`/`read_one`/`update_version`/`delete`/`execute_query`/`clear_table`)行為不變,呼叫端(`action.rs`)只需要補 `.await`。
- migration 用 `geni` 執行,SQL 檔案內容編進 binary(`include_str!`),執行前攤開到使用者本機的 data 目錄。
- 移除死掉的 `diesel`/`diesel_migrations`/`rusqlite`/`schema.rs`/`diesel.toml`。

## 非目標

- 不處理舊使用者機器上已存在的 `/opt/DPM/LocalRepo.db`(diesel 產生)資料搬移。這次換引擎後,新舊路徑都視為全新空 DB,不做跨版本資料遷移。
- 不改 `dpm-server` 的資料層(它本來就是 JSON 檔 + `RepoInfo.json`,沒有資料庫)。
- 不修 CLAUDE.md 已知待處理清單裡跟這次改動無關的項目(例如 `PackageManager::Unknown` 的 `panic!`)。
- `dotenv`/`git2` 這兩個目前也是死依賴(grep 不到任何呼叫),但跟本次改動無關,不在此次範圍內動它們。

## 架構

### 1. 目錄配置(directories crate)

新增 `directories = "6.0.0"` 到 `crates/dpm/Cargo.toml`。

**per-user(預設,不需要 root)**

```rust
let proj_dirs = ProjectDirs::from("com", "duacodie", "dpm")
    .expect("no valid home directory found");
```

| 用途 | 路徑 |
|---|---|
| 已安裝套件 | `proj_dirs.data_dir().join("Software")` |
| 執行檔連結(`bin`) | `proj_dirs.data_dir().join("bin")` |
| 設定檔(`config.json`) | `proj_dirs.config_dir()` |
| 本地索引 DB | `proj_dirs.data_dir().join("LocalRepo.db")` |
| DB lock 檔 | `proj_dirs.data_dir().join("LocalRepo.lock")` |

macOS 實際會落在 `~/Library/Application Support/com.duacodie.dpm/...`,Linux 落在 `$XDG_DATA_HOME/dpm/...`(沒設就是 `~/.local/share/dpm/...`)、config 落在 `$XDG_CONFIG_HOME/dpm/...`。

**system(`--system` flag,需要 root)**

固定常數,`directories` crate 沒有「系統共用」的概念,這段維持手動組路徑,子目錄名稱跟 per-user 對齊:

```
/opt/com.duacodie/DPM/
├── bin/
├── Software/
├── Settings/
├── LocalRepo.db
└── LocalRepo.lock
```

**scope 決定時機與權限模型**

`--system` 是一個 global clap flag。`main.rs` 目前的順序是「先 escalate 再解析參數」,必須反過來:

```
1. 用一個輕量預掃描(不吃全部 clap 規則,只認 --system/-S)決定 scope
2. scope == System 且 target_os = linux → sudo::escalate_if_needed()
3. 依 scope 決定目錄常數(ProjectDirs vs /opt/com.duacodie/DPM)
4. 正式跑 clap 解析拿完整 Cli
5. scope == PerUser → 直接 std::fs::create_dir_all,不呼叫 permision_check()/system_command_runner()
   scope == System → 沿用現有 permision_check()/system_command_runner()(Linux chown root:root、macOS chown user:admin、每個系統指令都過 sudo)
```

`SystemController::init()`/`permision_check()`/`system_command_runner()` 三個既有方法簽名不變,只是 per-user 模式下整條路徑都不會被呼叫到。

### 2. 資料庫引擎(turso)

`crates/dpm/Cargo.toml`:
- 移除 `diesel`、`diesel_migrations`、`rusqlite`(rusqlite 目前 grep 不到任何呼叫,是死依賴)。
- 新增 `turso = "0.7.1"`。
- `fs2`(lock file)維持不變。

刪除:
- `crates/dpm/src/utils/schema.rs`(diesel table! 巨集,turso 沒有對應物)。
- `crates/dpm/diesel.toml`。

`crates/dpm/src/utils/models.rs`:
- `DbPackage`/`NewDbPackage` 拿掉 `#[derive(Queryable, Selectable, Insertable)]` 與 `#[diesel(table_name = ...)]`,只留 `Serialize`/`Deserialize`/`Clone`,變成純資料結構。

`crates/dpm/src/utils/db.rs` 重寫:
- `Db` 內部欄位從 `conn: Arc<Mutex<SqliteConnection>>` 改成 `db: turso::Database`(`turso::Database` 可 `Clone`,官方範例是每次操作各自 `db.connect()` 拿一個 `Connection`,不需要手動包 `Mutex`)。
- 所有方法簽名改成 `async fn`,內部用 `conn.execute(sql, params)` / `conn.query(sql, params)` + `rows.next().await?` + `row.get_value(i)` 手動組出 `DbPackage`,取代原本 diesel 的 `.load::<(...)>()`。
- API 清理:
  - `read_by_condition`(依賴 diesel 的 `BoxableExpression`,目前 grep 不到任何外部呼叫者)直接刪除。
  - `execute_query`/`drop_table`/`clear_table` 保留(`clear_table` 有被 `action.rs::update()` 使用),簽名改 async。

`crates/dpm/src/action.rs` 連鎖影響:
- `list`/`uninstall`/`search`/`upgrade` 目前是 sync fn 但內部呼叫 `get_db().read_all()` 等,DB 方法變 async 後這四個 fn 要一起改成 `async fn`,呼叫處補 `.await`。`entry()` 本身已經是 `#[tokio::main] async fn`,呼叫端不需要額外包 runtime。

### 3. Migration(geni)

新增 `geni = "1.3.3"` 到 `crates/dpm/Cargo.toml`(它自己依賴 `turso`,版號釘的是 `^0.6.1`,跟我們直接依賴的 `0.7.1` 在 Cargo 眼中不算相容,依賴樹裡會並存兩個版本 — 不影響編譯,先記一筆,將來 geni 對齊版本時可以清掉)。

**卡點與解法**:geni 的 `migrate_database(...)` 是吃一個真實存在的資料夾路徑去讀 `.up.sql`/`.down.sql`,不是編譯進 binary 的內容。`dpm` 發布出去是單一執行檔,使用者機器上不會有原始碼的 `migrations/` 資料夾。

處理方式:
1. migration SQL 檔案(`crates/dpm/migrations/0001_init.up.sql`、`0001_init.down.sql`)照舊放在 repo 裡,用 `include_str!()` 編進 `dpm` binary。
2. `Db::new()`/啟動流程裡,把這些內容寫到 `<data_dir>/migrations/` 底下(per-user 或 system 的 data 目錄,依 scope 而定),每次啟動覆寫一次 — 檔案很小、冪等,不影響效能。
3. 寫完後呼叫 `geni::migrate_database(database_url, None, "schema_migrations".to_string(), migrations_dir, "schema.sql".to_string(), Some(30), false)`,指向剛攤開的資料夾。`database_token` 傳 `None`(本地檔案不需要),`dump_schema` 傳 `false`(不需要額外 dump 出 `schema.sql`)。

> 確切函式簽名與參數型別在實作時要對照 `docs.rs/geni` 或原始碼再次核對,目前掌握的是函式存在且支援 turso driver,細節型別以實作階段為準。

Schema 目前只有一張 table,`0001_init.up.sql` 內容對應現有 `LocalRepo` table 結構:

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

## 影響範圍(檔案清單)

- `Cargo.toml`(根)— 若把 `directories`/`turso`/`geni` 版本統一放進 `[workspace.dependencies]`(跟現有慣例一致)。
- `crates/dpm/Cargo.toml` — 增刪依賴。
- `crates/dpm/diesel.toml` — 刪除。
- `crates/dpm/src/utils/schema.rs` — 刪除。
- `crates/dpm/src/utils/models.rs` — 拿掉 diesel derive。
- `crates/dpm/src/utils/db.rs` — 重寫成 turso + geni。
- `crates/dpm/src/utils/system.rs` — `permision_check`/`system_command_runner` 邏輯不變,但呼叫時機變成 scope-aware。
- `crates/dpm/src/lib.rs` — `MAIN_DIR` 等 `OnceLock` 改用 `ProjectDirs`/system 常數,`set_globle_var()` 邏輯改寫。
- `crates/dpm/src/main.rs` — 執行順序改成先判斷 `--system` 再決定是否 escalate。
- `crates/dpm/src/cli_parse.rs` — 新增 `--system` global flag(手刻 `build_cli()`/`get_args()` 都要改,依 CLAUDE.md 記載的既有慣例)。
- `crates/dpm/src/action.rs` — `list`/`uninstall`/`search`/`upgrade` 改 async。
- `crates/dpm/migrations/0001_init.{up,down}.sql` — 新增。
- 專案 CLAUDE.md — 架構重點段落(目錄路徑、diesel migration 說明)需要同步更新。

## 已知風險

- turso 是仍在快速開發中的 SQLite 重寫(前身 Limbo),部分 SQL 方言/PRAGMA 行為可能跟成熟版 SQLite 有落差,實作時要針對現有 8 欄位的簡單 schema 實測讀寫是否正常。
- geni 內部 turso 版號(`^0.6.1`)與專案直接依賴(`0.7.1`)不一致,依賴樹會有兩份 turso,增加編譯時間與 binary 體積,非阻塞但值得後續追蹤。
- per-user/system 雙 scope 上線後,舊有「永遠 system-wide、永遠 root」的使用者操作習慣改變,需要在 README/CLI help 文字明確說明預設行為變成 per-user。
