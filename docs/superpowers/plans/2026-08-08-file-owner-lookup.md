# dpm owns — 檔案反查套件 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 新增 `dpm owns <path>...` 指令,輸入檔案路徑,查出這個檔案是 DPM 建的哪個(些)已裝套件的 symlink。

**Architecture:** 三層薄封裝——`Db::find_owners`(新 SQL 查詢,`installed_files` 表的反向查詢)→ `ActionInfo::owns`(路徑正規化 + 呼叫 `find_owners` + 印結果)→ `Commands::Owns`(CLI 定義)+ `lib.rs` 一行 dispatch。無 schema 變動,`installed_files` 表跟 `record_installed_files`/`get_installed_files` 已存在。

**Tech Stack:** Rust 2021、turso(async SQLite)、clap derive、colored(終端顏色)、tokio test。

## Global Constraints

- 不查 `Software/<pkg>/` 私有安裝目錄裡的原始檔案——只查 `installed_files` 表裡已有的 symlink 路徑(`opt/`、`bin/`、`sbin/`、`lib/`、`share/<pkg>/` 等環境目錄下的連結)。
- 路徑正規化用 `std::path::absolute()`(lexical,不解析 symlink),不用 `fs::canonicalize`——後者會把 symlink 解成目標路徑,跟 `installed_files` 存的原始 symlink 字串對不上。
- `find_owners` 回傳 `Vec<String>` 不是 `Option<String>`——同一個 `file_path` 理論上可以被兩個套件登記(namespace share 情境)。
- 沿用現有 `ClientError`/`ClientResult` 模式,不新增 error variant。
- 查無結果印黃字提示、continue,不中斷其他路徑、不改變指令的 exit code(跟 `info()` 對「查無此套件」的處理一致)。

參見設計文件:`docs/superpowers/specs/2026-08-08-file-owner-lookup-design.md`。

---

### Task 1: `Db::find_owners` — 反向查詢方法

**Files:**
- Modify: `crates/dpm/src/utils/db.rs`(在 `remove_installed_files` 方法之後、`impl Db` 區塊結束的 `}` 之前新增方法)
- Test: `crates/dpm/tests/db_tests.rs`(在既有 `test_record_get_and_remove_installed_files` 之後新增測試)

**Interfaces:**
- Produces: `pub async fn find_owners(&self, file_path: &str) -> ClientResult<Vec<String>>` — 給 Task 2 的 `ActionInfo::owns` 呼叫。

- [ ] **Step 1: 寫失敗測試**

打開 `crates/dpm/tests/db_tests.rs`,在 `test_record_get_and_remove_installed_files`(第 335-355 行)之後加入:

```rust
    #[tokio::test]
    async fn test_find_owners() -> TestResult {
        let dir = tempdir()?;
        let db = setup_db(dir.path(), false).await?;

        db.record_installed_files(
            "hello",
            &["/opt/dpm/bin/hello".to_string(), "/opt/dpm/opt/hello".to_string()],
        )
        .await?;
        db.record_installed_files(
            "world",
            &["/opt/dpm/bin/world".to_string()],
        )
        .await?;

        let owners = db.find_owners("/opt/dpm/bin/hello").await?;
        assert_eq!(owners, vec!["hello".to_string()]);

        let owners = db.find_owners("/opt/dpm/bin/world").await?;
        assert_eq!(owners, vec!["world".to_string()]);

        let owners = db.find_owners("/opt/dpm/bin/does-not-exist").await?;
        assert!(owners.is_empty());

        Ok(())
    }

    #[tokio::test]
    async fn test_find_owners_returns_every_package_sharing_a_file_path() -> TestResult {
        let dir = tempdir()?;
        let db = setup_db(dir.path(), false).await?;

        // 兩個套件登記同一個 file_path(namespace share 情境,例如 bin/
        // 底下沒有 is_namespaced 保護,兩個套件都連了同名檔案)。
        let shared = "/opt/dpm/bin/shared-name".to_string();
        db.record_installed_files("pkg-a", &[shared.clone()]).await?;
        db.record_installed_files("pkg-b", &[shared.clone()]).await?;

        let mut owners = db.find_owners(&shared).await?;
        owners.sort();
        assert_eq!(owners, vec!["pkg-a".to_string(), "pkg-b".to_string()]);

        Ok(())
    }
```

- [ ] **Step 2: 跑測試確認失敗(方法還不存在,編譯失敗)**

Run: `cd crates/dpm && cargo test --test db_tests find_owners`
Expected: FAIL —`error[E0599]: no method named 'find_owners' found for struct 'Db'`

- [ ] **Step 3: 實作 `find_owners`**

打開 `crates/dpm/src/utils/db.rs`,在 `remove_installed_files` 方法(第 681-687 行)之後、`impl Db` 區塊結束的 `}` 之前加入:

```rust
    /// Reverse-lookup: which package(s) registered `file_path` in their
    /// `installed_files` manifest. Returns every match, not just the first
    /// — `installed_files`' PRIMARY KEY is `(package_name, file_path)`, so
    /// two packages can legitimately register the same `file_path` (e.g.
    /// two packages both linking a same-named file into `bin/`, which has
    /// no per-package namespacing the way `share/`/`docs/`/`etc/`/`var/`
    /// do — see `placer.rs::link_subdirs_to_env`'s `is_namespaced` split).
    pub async fn find_owners(&self, file_path: &str) -> ClientResult<Vec<String>> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT package_name FROM installed_files WHERE file_path = ?1",
                [file_path],
            )
            .await
            .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?;

        let mut owners = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?
        {
            if let Some(name) = row.get_value(0).ok().and_then(|v| v.as_text().cloned()) {
                owners.push(name);
            }
        }
        Ok(owners)
    }
```

- [ ] **Step 4: 跑測試確認通過**

Run: `cd crates/dpm && cargo test --test db_tests find_owners`
Expected: PASS(兩個新測試都過)

- [ ] **Step 5: Commit**

```bash
git add crates/dpm/src/utils/db.rs crates/dpm/tests/db_tests.rs
git commit -m "feat(dpm): add Db::find_owners reverse file-lookup query

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

### Task 2: `ActionInfo::owns` — 路徑正規化 + 印結果

**Files:**
- Modify: `crates/dpm/src/action.rs`(在 `pub async fn info(&self)` 方法結束後、`pub async fn list` 之前新增方法,即現有第 1138 行 `Ok(())\n    }` 之後)
- Test: `crates/dpm/src/action.rs`(`#[cfg(test)] mod tests` 區塊內,比照 `install_pkg_at_older_version_downgrades_the_already_installed_one`(第 2305 行起)的 fixture 風格新增)

**Interfaces:**
- Consumes: `self.ctx.db.find_owners(file_path: &str) -> ClientResult<Vec<String>>`(Task 1)、`self.pkgs: Vec<String>`(既有 `ActionInfo` 欄位,這裡語意是「使用者傳入的檔案路徑清單」)。
- Produces: `pub async fn owns(&self) -> ClientResult<()>`,給 Task 3 的 CLI dispatch 呼叫。

- [ ] **Step 1: 寫失敗測試**

打開 `crates/dpm/src/action.rs`,找到 `install_pkg_at_older_version_downgrades_the_already_installed_one` 測試(第 2305-2383 行),在它之後(下一個 `#[tokio::test]` 之前)插入:

```rust
    #[tokio::test]
    async fn owns_finds_the_package_that_registered_a_symlink() {
        let root = tempfile::tempdir().unwrap();
        let ctx = Context::for_test(root.path()).await.unwrap();
        let setting = Setting {
            sources: vec![Source {
                alias: "official".to_string(),
                repo_url: "http://127.0.0.1:1".to_string(),
                repo_info: "http://127.0.0.1:1".to_string(),
            }],
        };

        let fixtures_dir = tempfile::tempdir().unwrap();
        let (zip_bytes, zip_hash) =
            build_fixture_zip(fixtures_dir.path(), "1.0.0", "owns test content\n");
        let url = serve_once(zip_bytes);

        let row = DbPackage::new(
            "official",
            "downgrade-pkg",
            "1.0.0",
            "prebuilt",
            Some(url),
            Some(zip_hash),
            Some("pkg-1.0.0.zip".to_string()),
            None,
            "test fixture",
            Some("main".to_string()),
            None,
            None,
            None,
            true,
        );
        ctx.info_db.insert_available(row).await.unwrap();

        let install = ActionInfo::new(
            ctx.clone(),
            vec!["downgrade-pkg@1.0.0".to_string()],
            false,
            setting.clone(),
        );
        install.install().await.unwrap();

        let entry_link = ctx.bin_dir.join("downgrade-pkg");
        let owners = ctx
            .db
            .find_owners(&entry_link.display().to_string())
            .await
            .unwrap();
        assert_eq!(
            owners,
            vec!["downgrade-pkg".to_string()],
            "install() must have recorded the entry-point symlink in installed_files"
        );

        // owns() 本身目前只印到 stdout,不回傳結構化結果——這裡直接驗證它
        // 對已知會命中/不會命中的路徑都能跑完不報錯(印什麼由人工驗證,
        // 見任務 3 的手動驗證步驟)。
        let owns_hit = ActionInfo::new(
            ctx.clone(),
            vec![entry_link.display().to_string()],
            false,
            setting.clone(),
        );
        owns_hit.owns().await.unwrap();

        let owns_miss = ActionInfo::new(
            ctx.clone(),
            vec!["/definitely/does/not/exist".to_string()],
            false,
            setting,
        );
        owns_miss.owns().await.unwrap();
    }
```

- [ ] **Step 2: 跑測試確認失敗(方法還不存在,編譯失敗)**

Run: `cd crates/dpm && cargo test owns_finds_the_package_that_registered_a_symlink`
Expected: FAIL — `error[E0599]: no method named 'owns' found for struct 'ActionInfo'`

- [ ] **Step 3: 實作 `owns`**

打開 `crates/dpm/src/action.rs`,在 `pub async fn info(&self) -> ClientResult<()>` 方法的結束(第 1138 行 `Ok(())` 後的 `}`)之後、`pub async fn list` 之前加入:

```rust
    /// Reverse-lookup: for each path in `self.pkgs` (here holding file
    /// paths, not package names — same field, different CLI-level meaning,
    /// see `Commands::Owns`), print which installed package(s) registered
    /// it as a tracked symlink in `installed_files`. Only matches DPM-built
    /// symlinks (`opt/`, `bin/`, `sbin/`, `lib/`, `share/<pkg>/`, ...) —
    /// raw files inside a package's private `Software/<pkg>/` install
    /// directory are never individually tracked, so they never match here.
    pub async fn owns(&self) -> ClientResult<()> {
        for raw_path in &self.pkgs {
            let absolute = std::path::absolute(raw_path)
                .map_err(|e| ClientError::Core(CoreError::IoError(e)))?;
            let owners = self
                .ctx
                .db
                .find_owners(&absolute.display().to_string())
                .await?;
            if owners.is_empty() {
                println!(
                    "{}",
                    format!("{raw_path}: not owned by any installed package").yellow()
                );
            } else {
                println!("{}: {}", raw_path, owners.join(", ").bold());
            }
        }
        Ok(())
    }
```

- [ ] **Step 4: 跑測試確認通過**

Run: `cd crates/dpm && cargo test owns_finds_the_package_that_registered_a_symlink`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/dpm/src/action.rs
git commit -m "feat(dpm): add ActionInfo::owns file-owner lookup

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

### Task 3: CLI 指令 `dpm owns` + dispatch + 收尾

**Files:**
- Modify: `crates/dpm/src/cli_parse.rs`(在 `Info` variant(第 67-74 行)之後、`List` variant 之前新增 `Owns` variant)
- Modify: `crates/dpm/src/lib.rs`(在 `Commands::Info` 分支(第 89-93 行)之後新增 `Commands::Owns` 分支)
- Modify: `TODO.md`(把「檔案反查套件」項目打勾)

**Interfaces:**
- Consumes: `ActionInfo::owns(&self) -> ClientResult<()>`(Task 2)。
- Produces: 使用者可執行的 `dpm owns <path>...` / `dpm of <path>...` 指令(此任務是這個功能的最後一塊,不再有後續任務依賴它)。

- [ ] **Step 1: 加入 `Commands::Owns` variant**

打開 `crates/dpm/src/cli_parse.rs`,在 `Info` variant(第 67-74 行)結束的 `},` 之後、`/// List installed packages`(第 75 行)之前插入:

```rust
    /// Show which installed package owns a file (matches DPM-built
    /// symlinks only — opt/bin/sbin/lib/share/<pkg> links, not raw files
    /// inside a package's private install directory)
    #[command(visible_aliases = ["of"], arg_required_else_help = true)]
    Owns {
        #[arg(value_name = "File path", required = true)]
        pn: Vec<String>,
        #[arg(short, long)]
        verbose: bool,
    },
```

- [ ] **Step 2: 加入 `lib.rs` dispatch 分支**

打開 `crates/dpm/src/lib.rs`,在 `Some(Commands::Info { pn, verbose }) => { ... }` 分支(第 89-93 行)之後、`Some(Commands::List { ... })` 之前插入:

```rust
        Some(Commands::Owns { pn, verbose }) => {
            ActionInfo::new(ctx.clone(), pn, verbose, setting_config)
                .owns()
                .await?
        }
```

- [ ] **Step 3: 編譯確認**

Run: `cargo check --workspace`
Expected: 編譯成功,無錯誤

- [ ] **Step 4: 手動驗證**

```bash
just run-client install hello   # 或任何本機索引裡已有的套件
just run-client owns "$(command -v hello || echo /opt/com.duacodie/dpm/bin/hello)"
just run-client owns /definitely/does/not/exist
```

Expected: 第一個指令印出 `<路徑>: hello`(套件名粗體);第二個指令印出黃字 `<路徑>: not owned by any installed package`。

- [ ] **Step 5: TODO.md 打勾**

打開 `TODO.md`,把「功能缺口 — 跟一般套件管理器比較,第二輪(2026-08-07)」區塊裡的「**檔案反查套件**」項目從 `- [ ]` 改成 `- [x]`,並在項目說明後面補一句實作結果,例如:

```
- [x] **檔案反查套件** — `dpm owns <path>...`(別名 `of`)。反查範圍限定 `installed_files` 表裡的 DPM symlink(opt/bin/sbin/lib/share/<pkg> 等環境目錄連結),不含套件私有安裝目錄裡的原始檔案。`db.rs::find_owners` 回傳 `Vec<String>`(不是單一結果)因為同一個 file_path 理論上可以被兩個套件登記。設計見 `docs/superpowers/specs/2026-08-08-file-owner-lookup-design.md`。
```

- [ ] **Step 6: 全套驗證 + Commit**

```bash
cd /Users/derrick/Documents/Program/rust/Project/DPM-Workspace
just pre-commit
git add crates/dpm/src/cli_parse.rs crates/dpm/src/lib.rs TODO.md
git commit -m "feat(dpm): wire up 'dpm owns' CLI command

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## 驗證清單

- [ ] `cargo check --workspace` 通過
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` 通過
- [ ] `cargo test --workspace` 通過(含 Task 1/2 新增的測試)
- [ ] 手動驗證:`dpm owns <已裝套件的 bin symlink>` 印出正確套件名;`dpm owns <不存在路徑>` 印出查無結果提示
- [ ] TODO.md 檔案反查套件項目打勾
