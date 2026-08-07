# TODO

DPM-Workspace 全 workspace 掃描結果(2026-07-24)。之後修 bug / 加功能都跟著這份走,新發現的問題也加進來,修完打勾並簡短寫怎麼修的。

優先度:P1 = 會實際炸掉或不安全,P2 = 品質/維護性問題,P3 = 瑣碎/文件。

---

## P1 — 正確性 / 穩健性

- [x] **`install()` 對下載下來的套件內容全部 `.unwrap()`,惡意或損毀的 zip 會直接 panic** — 於 `crates/dpm/src/utils/fetcher.rs` 將 `read_file_from_zip` 與 `JsonStorage::from_str_to` 的錯誤統一映射為 `ClientError::Core(CoreError::InvalidPackage(...))`,避免壞 zip 導致 panic。
- [x] **`system.rs::init()` 的 `repo_url`/`repo_info` 沒寫回 `config.json`** — 在 TOML layered config 重構中,`init_first_run` 已透過 `TomlStorage::to_toml(&default_setting, &config_path)` 將包含 `repo_url` 與 `repo_info` 的預設設定寫入 `config.toml`,並新增單元測試驗證持久化與 re-read 正常。
- [x] **`PackageManager::Unknown` 與 unsupported OS 全部用 `panic!`** — `PackageManager::command_for` 與 `system_command_runner` 全數採用回傳 `ClientResult<()>` 錯誤變體 (`ClientError::SystemError`),移除任何潛在 `panic!`,並移除路徑處的 `.unwrap()`。

- [x] **`place_package` 的 entry-point symlink 建立不是 idempotent 的** — 根因:`place_package` 裡 opt/subdir symlink 都走 `create_relative_or_abs_symlink`(先移除已存在的目標再建),唯獨 entry-point symlink 直接呼叫 `ln -s`(無 `-f`),重裝/升格 explicit 重跑同一條路徑時對已存在的 `bin/<pkg>` 報 "File exists"。修法:`crates/dpm/src/utils/placer.rs` 的 `ln` 呼叫改成 `-sf`,跟既有 idempotent symlink 模式一致。新增測試 `reinstalling_same_package_overwrites_existing_entry_symlink` 驗證。

## P2 — 安全 / 硬化

- [x] **`Db::execute_query`/`drop_table`/`clear_table` 用 `format!` 組 SQL,table 名稱直接字串插入** — 在 `crates/dpm/src/utils/db.rs` 新增 `validate_table_name` 嚴格白名單檢查（僅允許 `"LocalRepo"`、`"schema_migrations"` 或合法識別碼）,在 `drop_table`/`clear_table` 執行前驗證 table 名稱,並補上單元測試。
- [x] **`diesel.toml` 的 `migrations_directory` 是分割前留下的絕對路徑,已不存在** — 重新建立 `crates/dpm/diesel.toml` 並將 `migrations_directory` 設定為相對路徑 `"migrations"`。
- [x] **`diesel.toml` 的 `[print_schema] file` 也是相對路徑錯誤** — 在 `crates/dpm/diesel.toml` 中將 `[print_schema] file` 修正為 `"src/utils/schema.rs"`。

- [x] **權限模型不一致:Linux `chown -R root:root`,macOS `chown user:admin`** — 2026-08-07 評估過,決定維持現狀,不是漏掉。追過程式碼後發現這不是字串不一致,是提權機制本質不同:Linux `--system` 整個 process 靠 `sudo::escalate_if_needed()` 提權成 root,所有檔案操作(包含 `uninstall()` 用的原始 `fs::remove_file`/`remove_dir_all`,沒走 `system_command_runner`)天生以 root 執行;macOS `--system` process 本身從不整體提權,`chown <user>:admin` 是同一批原始 fs 呼叫在非提權 process 下還能寫入/刪除的必要條件,不是隨便選的。兩個「統一」方向都要牽動比改一行字串更大的範圍且都是實質安全取捨:Linux 改鬆會失去「沒 sudo 完全動不了」的防護,還缺 macOS `admin` 那種通用群組可借用;macOS 改鎖死會讓 `uninstall --system` 直接 Permission Denied,除非把好幾處原始 fs 寫入重新接上逐指令 sudo,還違背 macOS 當初「絕不整包提權」的設計初衷。細節寫進 `docs/CONTRIBUTE.MD`「為什麼不統一成同一種 chown 規則」一節。

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

- [x] **`list --outdated`** — `List` 指令加 `--outdated` flag(跟 `--sys-mgr` 互斥,只查 dpm 自己管的套件)。比對邏輯在 `crates/dpm/src/utils/outdated.rs::find_outdated`:對每個已裝套件,只在它安裝當下的那個 source 內找同名套件的最高 semver 版本(不跨 source 比,避免跟 `resolve_install_set` 一樣要處理 ambiguous source),版本字串解析失敗的項目跳過不影響其他結果。讀的是本地 `AvailablePackages` 快取(`dpm update` 抓下來的那份),不會現查網路,跟 `search` 同慣例。

- [x] **Pin / hold 版本** — `dpm pin <pkg>...`(別名 `hold`)/`dpm unpin <pkg>...`(別名 `unhold`)。只擋 `upgrade()` 這條路徑(`dpm install pkg@version` 不擋,那是使用者主動指名版本,語意不同)。`InstalledPackages` 加 `pinned INTEGER NOT NULL DEFAULT 0` 欄位,寫法照抄當初加 `explicit` 欄位的 pattern(`CREATE TABLE IF NOT EXISTS` 含新欄位 + idempotent `ALTER TABLE ADD COLUMN`)。刻意**不**把 `pinned` 塞進共用的 `DbPackage`/`DbPackage::new`(牽動 20 處呼叫點,且只對 `InstalledPackages` 有意義),改用獨立的 `Db::set_pinned`/`Db::pinned_names`。`upgrade()` 呼叫 `install_resolved` 前用 `pinned_names()` 把 pinned 套件從 `is` 濾掉,印提示但不中斷其他套件的升級。`list`/`info` 順便標註 `(pinned)`。

- [x] **Downgrade / 版本回退** — 追過整條路徑(`resolve_install_set`/`swap_into_install_dir`/`Db::insert`)後發現 `dpm install pkg@<舊版本>` 其實**已經**能 downgrade:pubgrub 解析不管目前裝了什麼版本,換裝跟 DB UPSERT 都是無條件覆蓋,沒有任何「不准裝比目前舊」的檢查。原本完全沒測試覆蓋這件事(也沒有任何測試做過完整 happy-path `install()` round trip)。補了 `install_pkg_at_older_version_downgrades_the_already_installed_one`(`action.rs`)鎖住這個行為,順便是第一個完整 install round-trip 測試。不新增 `dpm downgrade` 指令——決定只驗證+補測試,不做別名指令。

- [x] **`info`/`show` 詳細資訊指令** — `dpm info <pkg>...`(別名 `show`),比照 `search`/`uninstall` 的 `Vec<String>` 多名稱慣例。印已裝狀態(版本/來源/explicit)+ description/entry/dependencies/author 簽章狀態 + 本地索引所有 `(source, version)`。`license`/`size`/`changelog` 這幾個欄位資料模型(`PackageVersionInfo`/`DbPackage`)根本沒有,不在範圍內,要做得先改 schema。實作在 `action.rs::info()`,一次讀完 `read_available()`/`read_all()` 在記憶體篩選(精確比對,不像 `search` 模糊比對)。

- [x] **本機離線檔案安裝**(類似 `apt install ./x.deb`,不是不管依賴的 `dpkg -i`)— `dpm install <路徑>`:`self.pkgs` 每個項目先 `Path::is_file()` 判斷是不是本機檔案,是就走 `install_local_file`。附帶把套件封裝格式副檔名全面從 `.zip` 改成 `.dpm`(`dpm-server` 的 `hash`/`build` 輸出、`crates/dpm-server/Repo/hello.zip`→`hello.dpm` 連同 `RepoInfo.db` 那筆的 `url`/`filename` 一起改、README/CLAUDE.md 文件說明;測試裡當任意 placeholder 用的 `.zip` 字串不動,沒有行為意義)。驗證模型類比 `apt install ./x.deb`:沒有索引項目可比對雜湊/簽章,略過那層,但保留 `packageInfo.json.hash` 對 `hashes.json` 自身紀錄的自洽性檢查(擋壞檔)。依賴自動解析:讀 `packageInfo.json.dependencies`,沒裝的丟進既有 `install_resolved`/pubgrub 管線裝(能裝多層 transitive),解不出來整個安裝失敗,不留半殘狀態。新增 `Db::set_explicit`:因為 `install_resolved` 的 `is`/`promote` 機制只要名字在 `is` 裡就一定標 `explicit=true`(現有架構原本沒有「用 is 裝但標成 auto」這條路),裝完本機套件自己解出來的直接依賴要手動改回 `explicit=false`,`autoremove` 才抓得到孤兒。`InstalledPackages.source="local"` 標記非索引來源。

- [ ] **Conflicts / Provides / 虛擬套件** — `dpm-core::PackageVersionInfo` 沒有這幾個欄位,沒法表達「A 跟 B 互斥」或「這個套件提供 X 這個介面」,多套件同提供一功能時無法選替代。

- [ ] **第三方 source(非 official)簽章驗證** — 見上方 P2 安全項目,刻意延後,需要先有 key pin 機制。

## 已完成

- [x] Infisical secrets 整合(justfile 全 recipe 透過 `infisical run` 注入、`crates/dpm/.env` 退役、CLAUDE.md 補文件)— 2026-07-24,`feat/infisical-secrets` 分支,已 merge 進 main(`97b7170`)
