# TODO

DPM-Workspace 全 workspace 掃描結果(2026-07-24)。之後修 bug / 加功能都跟著這份走,新發現的問題也加進來,修完打勾並簡短寫怎麼修的。

優先度:P1 = 會實際炸掉或不安全,P2 = 品質/維護性問題,P3 = 瑣碎/文件。

---

## P1 — 正確性 / 穩健性

- [x] **`install()` 對下載下來的套件內容全部 `.unwrap()`,惡意或損毀的 zip 會直接 panic** — 於 `crates/dpm/src/utils/fetcher.rs` 將 `read_file_from_zip` 與 `JsonStorage::from_str_to` 的錯誤統一映射為 `ClientError::Core(CoreError::InvalidPackage(...))`,避免壞 zip 導致 panic。
- [x] **`system.rs::init()` 的 `repo_url`/`repo_info` 沒寫回 `config.json`** — 在 TOML layered config 重構中,`init_first_run` 已透過 `TomlStorage::to_toml(&default_setting, &config_path)` 將包含 `repo_url` 與 `repo_info` 的預設設定寫入 `config.toml`,並新增單元測試驗證持久化與 re-read 正常。
- [x] **`PackageManager::Unknown` 與 unsupported OS 全部用 `panic!`** — `PackageManager::command_for` 與 `system_command_runner` 全數採用回傳 `ClientResult<()>` 錯誤變體 (`ClientError::SystemError`),移除任何潛在 `panic!`,並移除路徑處的 `.unwrap()`。

## P2 — 安全 / 硬化

- [x] **`Db::execute_query`/`drop_table`/`clear_table` 用 `format!` 組 SQL,table 名稱直接字串插入** — 在 `crates/dpm/src/utils/db.rs` 新增 `validate_table_name` 嚴格白名單檢查（僅允許 `"LocalRepo"`、`"schema_migrations"` 或合法識別碼）,在 `drop_table`/`clear_table` 執行前驗證 table 名稱,並補上單元測試。
- [x] **`diesel.toml` 的 `migrations_directory` 是分割前留下的絕對路徑,已不存在** — 重新建立 `crates/dpm/diesel.toml` 並將 `migrations_directory` 設定為相對路徑 `"migrations"`。
- [x] **`diesel.toml` 的 `[print_schema] file` 也是相對路徑錯誤** — 在 `crates/dpm/diesel.toml` 中將 `[print_schema] file` 修正為 `"src/utils/schema.rs"`。

- [ ] **權限模型不一致:Linux `chown -R root:root`,macOS `chown user:admin`**
      `crates/dpm/src/utils/system.rs:54-70`
      兩個平台裝完之後檔案的擁有者/群組邏輯不同,值得重新設計成一致的模型(例如都用「執行者所屬使用者 + 一個固定群組」)。

## P2 — 重複 / 死碼

- [ ] **`zip_folder`/`unzip_file`/`read_file_from_zip` 在 client 和 server 各自複製一份,簽名還不一樣**
      `crates/dpm/src/utils/zip_file.rs` vs `crates/dpm-server/src/zip_file.rs`
      client 版 `unzip_file(zip_file_path, output_folder)` 少一個 `name` 參數,跟 server 版 `unzip_file(zip_file_path, output_folder, name)` 不同步。應該把這組函式搬進 `dpm-core`,用 `client`/`server` feature(或乾脆兩邊都要,不需要 gate)共用一份。

- [ ] **`hasher()`(SHA256)在 client 和 server 各自重寫一份**
      `crates/dpm/src/action.rs:289-299` vs `crates/dpm-server/src/action.rs:11-19`
      邏輯一模一樣,應搬進 `dpm-core`。

- [ ] **`crates/dpm-server/src/json_parse1.rs` 是分割前留下的死碼**
      整份檔案(`PackageInfo`/`RepoInfo`/`JsonStorage` 舊版複本)沒被 `main.rs` 的 `mod` 列進去,不會編譯進二進位檔,純粹佔位置容易誤改。直接刪掉整份檔案。

- [ ] **`dpm` 的 `Cargo.toml` 有多個宣告但完全沒用到的 dependency**
      `crates/dpm/Cargo.toml`: `rusqlite = "0.31.0"`(client 資料層已經全面走 diesel,沒有任何 `rusqlite::` 呼叫)、`dotenv = "0.15.0"`(沒有任何 `dotenv::dotenv()` 呼叫,且 `.env` 已經被 Infisical 取代退役)。`digest`(workspace dependency,`dpm`/`dpm-server`/`dpm-core` 都宣告但沒有任何 `digest::` 直接呼叫,單純被 `sha2` 內部依賴)。`dpm`/`dpm-server` 各自宣告的 `flate2` 也沒有任何直接使用。`dpm-server` 的 `hex-literal` 同樣沒被使用到。全部可以拿掉,縮短編譯時間。
      (~~`git2 = "0.18.1"` — 已由 Task 3 的 `clone_package_source()` 使用,非死依賴,已解決。~~)

## P3 — 版本 / 一致性

- [ ] **`dpm-core` 版本跟 workspace 版本脫鉤**
      `crates/dpm-core/Cargo.toml:3`(`version = "0.1.2"`,沒用 `version.workspace = true`)vs 根 `Cargo.toml`(`workspace.package.version = "0.1.0"`)vs `dpm`/`dpm-server`(都用 `version.workspace = true` → 0.1.0)。確認這是刻意的(`dpm-core` 獨立發布到 crates.io/docs.rs,版本節奏不同)還是遺漏,是的話在 CLAUDE.md 補一句說明,避免以後有人「順手」幫它改成 workspace 版本。

## P3 — 文件 / 瑣碎

- [ ] **`dpm-core/README.md` 的 `add_package` 範例跟現在的函式簽名對不上,照抄會編譯不過**
      `crates/dpm-core/README.md:100-111`
      範例只傳 6 個位置參數(`name`/`url`/`file_name`/`version`/`hash`/`dependencies`),但 `RepoInfo::add_package`(`crates/dpm-core/src/lib.rs:190-211`)現在要 8 個參數,多了 `entry: Option<String>`、`description: Option<String>`。順手也修一下同檔案 `PackageInfo` 欄位表那行空白的 `` `` ``(應該是 `description`)。

- [ ] **`crates/dpm-server/Repo/`、`RepoInfo.json` 裡混了測試用假套件(`test`/`test1`/`test2`/`helloWorld`)跟真正的 repo 資料結構**
      這幾個 4KB 小 zip + 對應的 `Repo/src/<pkg>/` 原始碼看起來是開發時期留下的手動測試 fixture,直接躺在「正式」的 Repo 目錄結構裡,容易讓人誤以為是真實可安裝的套件。評估要嘛移到專門的測試 fixture 目錄、要嘛清掉。

- [ ] **`crates/dpm/README.md` 幾乎是空的**(只有安裝/`-h` 兩行),`crates/dpm-server/README.md` 有基本用法但沒提到 `hash`/`build`/`fix`/`init` 各自的參數細節。有餘力再補。

---

## 已完成

- [x] Infisical secrets 整合(justfile 全 recipe 透過 `infisical run` 注入、`crates/dpm/.env` 退役、CLAUDE.md 補文件)— 2026-07-24,`feat/infisical-secrets` 分支,已 merge 進 main(`97b7170`)
