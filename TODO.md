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

- [ ] **第三方 source(非 official)沒有簽章驗證**
      `crates/dpm/src/action.rs`(`verify_official_signature` 只在 `is_official(&source.repo_url)` 為真時呼叫)
      `author`/`signature` 欄位對第三方 source 直接忽略,裝這種來源的套件純粹信任來源本身。刻意延後,不是漏掉——2026-08-06 討論過:程式碼面成本低(`verify_hash_signature`/`validate_author_id` 已是 `dpm-core` 通用函式),但缺信任錨(third-party key 若也從同一個不受信任的 URL 抓,等於自己簽自己,防不了來源被竄改)。要做的話得先有 pin 機制(例如 `dpm source add --pubkey <fingerprint>`,TOFU 或手動 pin)。目前沒有第三方 source 生態,先不做,等真的有人開始發布非官方 source 再重新評估。

## P2 — 重複 / 死碼

- [x] **`zip_folder`/`unzip_file`/`read_file_from_zip` 在 client 和 server 各自複製一份,簽名還不一樣** — 已收斂進 `dpm-core/src/zip_file.rs` 單一實作,client/server 都呼叫同一份。

- [x] **`hasher()`(SHA256)在 client 和 server 各自重寫一份** — 已收斂成 `dpm-core::hash_file()`(blake3),client/server 共用。

- [x] **`crates/dpm-server/src/json_parse1.rs` 是分割前留下的死碼** — 檔案已刪除。

- [x] **`dpm` 的 `Cargo.toml` 有多個宣告但完全沒用到的 dependency** — `rusqlite`(commit `8e1ec19` 移除,client 資料層統一改走 turso)、`dotenv`、`digest`、`hex-literal` 均已移除。根 `Cargo.toml` `[workspace.dependencies]` 的 `flate2`(同樣零 `flate2::` 呼叫,`dpm` 用的是 `self_update` 內建 `compression-flate2` feature,不是這個 workspace dependency)也已拿掉。
      (~~`git2 = "0.18.1"` — 已由 Task 3 的 `clone_package_source()` 使用,非死依賴,已解決。~~)

## P3 — 版本 / 一致性

- [x] **`dpm-core` 版本跟 workspace 版本脫鉤** — 已統一,`crates/dpm-core/Cargo.toml` 改用 `version.workspace = true`,根 `Cargo.toml` 的 `workspace.package.version` 是唯一版本來源(見 CLAUDE.md)。

## P3 — 文件 / 瑣碎

- [x] **`dpm-core/README.md` 的 `add_package` 範例跟現在的函式簽名對不上** — API 已改名為 `add_package_version`,README 範例已同步更新。

- [x] **`crates/dpm-server/Repo/`、`RepoInfo.json` 裡混了測試用假套件** — 已清掉,`Repo/` 底下只剩真正的 `hello.zip`。

- [x] **`crates/dpm/README.md` 幾乎是空的** — 已擴充(64 行);`crates/dpm-server/README.md` 也已補上 `hash`/`build`/`fix`/`init` 的步驟細節(158 行)。

---

## 功能缺口 — 跟一般套件管理器(apt/dnf/pacman/brew/cargo)比較(2026-08-06)

- [x] **Autoremove / orphan 清理** — 已實作。`InstalledPackages` 新增 `explicit` 欄位區分主動安裝 vs 依賴拉進來,`find_orphans`(`crates/dpm/src/utils/orphan.rs`)遞迴找出不再被引用的 auto 套件,`dpm autoremove` 指令清除,`dpm uninstall` 收尾會印孤兒提示。設計見 `docs/superpowers/specs/2026-08-06-autoremove-design.md`。

- [ ] **`list --outdated`** — `List` 指令沒有這個 flag,想知道哪些套件能升級只能整批硬跑 `upgrade`。

- [ ] **Pin / hold 版本** — `dpm update`/`upgrade` 沒辦法鎖某個套件不被升級。

- [ ] **Downgrade / 版本回退** — 沒有使用者可下的指令。`placer.rs` 的 rollback 只是單次安裝失敗時的原子復原保護(裝到一半失敗換回舊的),不是「切回上一版」的功能。

- [ ] **`info`/`show` 詳細資訊指令** — CLI 目前只有 Install/Update/Uninstall/Search/List/Upgrade/UpgradeSelf/Source/GenConfig,沒有單看一個套件完整 metadata(license/size/dependencies/changelog)的指令。

- [ ] **本機離線檔案安裝**(類似 `dpkg -i local.deb`)— 沒有,一定要透過 source 索引才能裝,無法直接指定本機一個 zip/檔案安裝。

- [ ] **Conflicts / Provides / 虛擬套件** — `dpm-core::PackageVersionInfo` 沒有這幾個欄位,沒法表達「A 跟 B 互斥」或「這個套件提供 X 這個介面」,多套件同提供一功能時無法選替代。

- [ ] **第三方 source(非 official)簽章驗證** — 見上方 P2 安全項目,刻意延後,需要先有 key pin 機制。

## 已完成

- [x] Infisical secrets 整合(justfile 全 recipe 透過 `infisical run` 注入、`crates/dpm/.env` 退役、CLAUDE.md 補文件)— 2026-07-24,`feat/infisical-secrets` 分支,已 merge 進 main(`97b7170`)
