# 多來源 / namespace(Phase 2) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 讓 `dpm` 支援多個具名套件來源(source)、每個套件支援多版本索引,取代現行「單一 `repo_url`/`repo_info` 字串 + 每個套件只有一筆」的資料模型。這是 `docs/superpowers/specs/2026-07-24-multi-source-registry-design.md` 五階段裡的第 2 階段(第 1 階段 blake3+tempfile 已完成並合併)。

**Architecture:** 六個循序 task,前段(dpm-core 資料模型、DB schema)互相獨立可平行理解但依序實作,後段(config schema、CLI、action.rs 整合、dpm-server 發布端)依序疊上去。完成後 `dpm install <name>`/`dpm update` 在只有一個來源時行為與現在一致,多個來源時裸名衝突會報錯要求 `來源/名稱` 明確指定——但 CLI 還不支援解析 `來源/名稱`/`@版本` 語法(那是 Section 5,Phase 5 才做,原因見下方「與 spec 的差異」)。

**Tech Stack:** Rust 2021、現有 `dpm-core`/`turso`+`geni`/`clap`(手刻 `Command` builder,不用 derive)。這階段不新增任何 crate 依賴。

## 與 spec 的差異(刻意的範圍收斂,附理由)

閱讀 spec 全文(`docs/superpowers/specs/2026-07-24-multi-source-registry-design.md`)之後,以下幾點是這份 plan 對 spec Section 3 pseudocode 的具體化決策,不是遺漏:

1. **`RepoInfo.packages` 的 key 維持 `String`(套件名),不是 spec pseudocode 寫的 `(String, String)`(來源, 套件名)tuple。** 原因:`serde_json` 不支援 tuple 當 JSON object key(執行時會炸)。而且 spec 自己在 Section 4/5 已經把「跨來源聚合查詢」這件事交給 DB 層的 `versions_of`/`sources_of`(這兩個函式簽名 spec 也已經定案),不是交給 `dpm-core::RepoInfo`。所以 `RepoInfo` 保持「一個來源自己的索引」語意(套件名 → 多版本列表)最簡單,也最符合 `dpm-server` 本來就只管自己這一個來源的事實——`dpm-server` 完全不需要知道「來源」這個概念,那是 client 端 `dpm` 的 config 概念。多來源合併發生在 `dpm`(client)呼叫 `fetch_update_repo_info` 迴圈跑過每個 source 之後,寫進 DB 時才貼上 `source` 欄位。
2. **`install <name>` 還不支援 `[來源/]名稱[@版本]` CLI 語法。** spec Section 5 步驟 1-2(裸名衝突偵測、0/1/多筆判斷)是這個 phase 做,因為 Task 2 定義的 `sources_of` 本來就是「給裸名衝突偵測用」(spec Section 4 原話)。但 Section 5 步驟 3-6(組 pubgrub root 需求、真正呼叫 solver)是 Phase 5 的事。所以這個 plan 完成後:`dpm install foo` 在 `foo` 只存在一個來源時能裝,存在多個來源時會報錯附上來源清單(不會自動選、不會 crash);還不支援 `dpm install official/foo` 或 `dpm install foo@1.2.0` 這種帶限定字串的語法——那需要 CLI 參數解析 + pubgrub 才有意義,現在做了也是空接口。
3. **沒有版本比較/排序邏輯(不比 semver 大小)。** `PackageVersionInfo` 在伺服器端用 `add_package_version` 附加進 `Vec`(不能覆寫既有版本,只能新增或移除),client 端「最新版本」定義成「這個套件在 DB 裡 `rowid` 最大的那筆」——因為 `dpm update` 每次都是整個 source 清空重灌,插入順序 = `RepoInfo.json` 陣列順序 = 伺服器端 `add_package_version` 呼叫順序(通常就是發布順序)。這是刻意的簡化,真正的 semver 版本比較留給 Phase 5 接 `semver`/`pubgrub` 時處理——現在硬加 `semver` crate 只是多一個沒被真正使用的依賴。
4. **`RepoInfo::update_package`(舊版整包欄位覆寫語意)直接刪除,不保留相容版本。** 已發布版本的本質是不可變的(publish 新版本或撤下某版本,而不是「編輯現有版本的欄位」),`update_package` 的欄位覆寫語意在多版本模型下沒有對應的合理操作。刪除前確認過:唯一呼叫端是它自己的單元測試,沒有任何 production call site。
5. **`Db::update_version` 直接刪除,不遷就新 schema。** 全 repo 搜尋後確認零 production 呼叫端(唯一用到的地方是它自己的測試),而且「就地修改某一列的版本字串」在版本不可變的新模型下沒有意義——真的要換版本是「插入新版本那一列」,不是「改寫舊列」。

## Global Constraints

- 這個 phase 不新增 `semver`/`pubgrub` 依賴(見上方差異點 3)。
- `dpm-core` 的 struct 定義(`PackageKind`/`PackageVersionInfo`/`RepoInfo`)一律不掛 `#[cfg(feature = ...)]`,只有 `impl` 區塊可以 gate——這是 CLAUDE.md 記錄過的已知地雷(workspace feature unification 會讓單獨編譯跟整體編譯行為不一致)。
- `RepoInfo` 內部 `packages` 欄位維持 `HashMap<String, Vec<PackageVersionInfo>>`(套件名 → 多版本),不是 tuple key(見上方差異點 1)。
- 每個 task 完成後執行 `cargo build --workspace` 確認整個 workspace 仍能編譯;有新增/修改測試的 task 額外跑 `cargo test --workspace`。
- 現有的 `hash_file`(blake3)、`swap_into_install_dir`(原子安裝)兩個 Phase 1 功能不動,只在 `install()` 重寫時保留原樣呼叫。
- 新的 `CoreError` 變體照現有命名慣例:`Thing(String)` 簡單訊息變體,不用 struct variant(除非像既有的 `HashMismatch` 那樣真的需要兩個欄位)。

---

## Task 1: `dpm-core` 多版本 `RepoInfo` 資料模型

**Files:**
- Modify: `crates/dpm-core/src/lib.rs`
- Modify: `crates/dpm-core/src/error.rs`
- Modify: `crates/dpm-core/tests/test.rs`
- Modify: `crates/dpm-core/README.md`

**Interfaces:**
- Consumes:無(這個 task 不依賴任何其他未完成的東西,`Dependency`/`PackageInfo`/`JsonStorage`/`hash_file` 全部維持現狀不動)。
- Produces:
  - `pub enum PackageKind { Prebuilt { url: String, hash: String, file_name: String }, Source { build: String } }`
  - `pub struct PackageVersionInfo { pub version: String, pub kind: PackageKind, pub dependencies: Option<Vec<Dependency>>, pub entry: Option<String>, pub description: Option<String> }`
  - `pub struct RepoInfo { packages: HashMap<String, Vec<PackageVersionInfo>> }`(欄位維持 private,跟現在一樣)
  - `RepoInfo::new() -> Self`、`RepoInfo::has_package(&self, name: &str) -> bool`、`RepoInfo::versions_of(&self, name: &str) -> CoreResult<&Vec<PackageVersionInfo>>`、`RepoInfo::latest_version(&self, name: &str) -> CoreResult<&PackageVersionInfo>`、`RepoInfo::get_package_handler(&self) -> &HashMap<String, Vec<PackageVersionInfo>>`(全部無 feature gate)
  - `#[cfg(feature = "server")] RepoInfo::add_package_version(&mut self, name: String, info: PackageVersionInfo) -> CoreResult<()>`、`RepoInfo::remove_package_version(&mut self, name: &str, version: &str) -> CoreResult<PackageVersionInfo>`
  - `#[cfg(feature = "client")] RepoInfo::fetch_update_repo_info(&mut self, url: &str) -> CoreResult<()>`(簽名跟現在完全一樣,行為不變——單來源整包覆蓋)、`RepoInfo::get_package_info(&self, name: &str, version: &str) -> CoreResult<PackageInfo>`(取代 `get_single_package_info`,多一個 `version` 參數;`fetch_package` 直接刪除,repo 內零呼叫端)
  - `CoreError::AmbiguousPackage(String)` 新變體

- [ ] **Step 1: 寫失敗的測試(先紅)——不含 feature 的通用 API**

編輯 `crates/dpm-core/tests/test.rs`,把整個 `#[cfg(feature = "server")] mod server_tests { ... }` 區塊(第 6-84 行)換成:

```rust
    mod versioning_tests {
        use dpm_core::*;

        fn prebuilt(version: &str) -> PackageVersionInfo {
            PackageVersionInfo {
                version: version.to_string(),
                kind: PackageKind::Prebuilt {
                    url: format!("http://example.com/{version}"),
                    hash: "hash123".to_string(),
                    file_name: "file1.zip".to_string(),
                },
                dependencies: None,
                entry: None,
                description: None,
            }
        }

        #[test]
        fn has_package_false_before_any_version_added() {
            let repo = RepoInfo::new();
            assert!(!repo.has_package("package1"));
        }

        #[test]
        fn versions_of_missing_package_is_not_found() {
            let repo = RepoInfo::new();
            let result = repo.versions_of("nonexistent");
            assert!(result.is_err());
        }

        #[cfg(feature = "server")]
        mod server_tests {
            use super::prebuilt;
            use dpm_core::*;

            #[test]
            fn add_package_version_appends_and_is_queryable() {
                let mut repo = RepoInfo::new();
                repo.add_package_version("package1".to_string(), prebuilt("1.0.0"))
                    .unwrap();

                assert!(repo.has_package("package1"));
                let versions = repo.versions_of("package1").unwrap();
                assert_eq!(versions.len(), 1);
                assert_eq!(versions[0].version, "1.0.0");
            }

            #[test]
            fn add_package_version_keeps_multiple_versions() {
                let mut repo = RepoInfo::new();
                repo.add_package_version("package1".to_string(), prebuilt("1.0.0"))
                    .unwrap();
                repo.add_package_version("package1".to_string(), prebuilt("2.0.0"))
                    .unwrap();

                let versions = repo.versions_of("package1").unwrap();
                assert_eq!(versions.len(), 2);
                assert_eq!(repo.latest_version("package1").unwrap().version, "2.0.0");
            }

            #[test]
            fn add_package_version_rejects_duplicate_version() {
                let mut repo = RepoInfo::new();
                repo.add_package_version("package1".to_string(), prebuilt("1.0.0"))
                    .unwrap();

                let result = repo.add_package_version("package1".to_string(), prebuilt("1.0.0"));
                assert!(result.is_err(), "publishing the same version twice must fail");
            }

            #[test]
            fn remove_package_version_removes_only_that_version() {
                let mut repo = RepoInfo::new();
                repo.add_package_version("package1".to_string(), prebuilt("1.0.0"))
                    .unwrap();
                repo.add_package_version("package1".to_string(), prebuilt("2.0.0"))
                    .unwrap();

                let removed = repo.remove_package_version("package1", "1.0.0").unwrap();
                assert_eq!(removed.version, "1.0.0");
                let remaining = repo.versions_of("package1").unwrap();
                assert_eq!(remaining.len(), 1);
                assert_eq!(remaining[0].version, "2.0.0");
            }

            #[test]
            fn remove_last_package_version_drops_the_package_entirely() {
                let mut repo = RepoInfo::new();
                repo.add_package_version("package1".to_string(), prebuilt("1.0.0"))
                    .unwrap();

                repo.remove_package_version("package1", "1.0.0").unwrap();
                assert!(!repo.has_package("package1"));
            }

            #[test]
            fn remove_package_version_missing_version_errors() {
                let mut repo = RepoInfo::new();
                repo.add_package_version("package1".to_string(), prebuilt("1.0.0"))
                    .unwrap();

                let result = repo.remove_package_version("package1", "9.9.9");
                assert!(result.is_err());
            }
        }

        #[cfg(feature = "client")]
        mod client_tests {
            use dpm_core::*;

            #[tokio::test]
            async fn get_package_info_unknown_version_errors() {
                let repo = RepoInfo::new();
                let result = repo.get_package_info("package1", "1.0.0").await;
                assert!(result.is_err());
            }
        }
    }
```

保留同一個檔案裡其他既有測試(`test_from_json`/`test_to_json`/`test_from_url`/`test_from_str_to`/`test_dependency_serde`/`test_hash_file_is_deterministic_and_content_sensitive`)不動。

- [ ] **Step 2: 確認編不過(紅燈)**

Run: `cargo test -p DPM-Core --features server,client versioning_tests 2>&1 | tail -40`
Expected: 編譯錯誤——`PackageKind`/`PackageVersionInfo`/`add_package_version`/`versions_of`/`latest_version`/`remove_package_version`/`get_package_info` 都還不存在。

- [ ] **Step 3: 改 `CoreError`,加 `AmbiguousPackage`**

編輯 `crates/dpm-core/src/error.rs`,在 `SecurityError` 變體之後(`DatabaseError`/`HashMismatch`/`SecurityError` 三個之間任一順手位置皆可,這裡接在 `SecurityError` 後面)加入:

```rust
    #[error("Security error: {0}")]
    SecurityError(String),

    #[error("Ambiguous package '{0}': exists in multiple sources, specify source/name")]
    AmbiguousPackage(String),
```

- [ ] **Step 4: 重寫 `RepoInfo`/新增 `PackageKind`/`PackageVersionInfo`**

編輯 `crates/dpm-core/src/lib.rs`。把現有 `PackageBasicInfo` struct 定義(第 136-156 行)整個換成:

```rust
/// 套件在某個來源索引裡的一個具體版本條目。
#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum PackageKind {
    /// 已預先打包好的二進位/壓縮檔,client 直接下載解壓。
    Prebuilt {
        url: String,
        hash: String,
        file_name: String,
    },
    /// 只提供原始碼 + build 指令,client 在本機執行 build(Phase 4 才會真的走這條路)。
    Source { build: String },
}

/// 套件的一個發布版本。已發布的版本視為不可變——要變更只能發布新版本或撤下整個版本。
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PackageVersionInfo {
    pub version: String,
    #[serde(flatten)]
    pub kind: PackageKind,
    pub dependencies: Option<Vec<Dependency>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}
```

把 `PackageKind`/`PackageVersionInfo` 標記為使用了 `#[serde(tag = "kind", rename_all = "lowercase")]`——完整寫法是在 `PackageKind` enum 定義正上方加這個 attribute,連同上面的定義一起貼:

```rust
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum PackageKind {
    Prebuilt {
        url: String,
        hash: String,
        file_name: String,
    },
    Source { build: String },
}
```

(取代掉上一步暫時貼的沒有 `#[serde(tag = ...)]` 那版——最終版本只有這一份,上面那段是說明過程,實際檔案裡只留這個帶 `#[serde(tag = ...)]` 的版本。)

再把 `RepoInfo` struct 定義(第 130-135 行)的欄位型別換掉:

```rust
/// 儲存庫的資訊管理模組——代表「一個來源」自己的索引,不含來源名稱本身
/// (來源是 client 端 config 的概念,見 `dpm` crate 的 `Source`/`Setting`)。
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct RepoInfo {
    /// 套件名稱 -> 該套件所有已發布版本(依發布順序,不排序)
    packages: HashMap<String, Vec<PackageVersionInfo>>,
}
```

把無 feature gate 的 `impl RepoInfo`(第 157-190 行)整個換成:

```rust
impl RepoInfo {
    /// 建立一個新的 `RepoInfo` 實例
    pub fn new() -> Self {
        RepoInfo {
            packages: HashMap::new(),
        }
    }
    /// 檢查是否存在指定名稱的套件(任何版本)
    pub fn has_package(&self, package_name: &str) -> bool {
        self.packages.contains_key(package_name)
    }
    /// 取得某套件的所有已發布版本
    pub fn versions_of(&self, package_name: &str) -> CoreResult<&Vec<PackageVersionInfo>> {
        self.packages
            .get(package_name)
            .ok_or_else(|| CoreError::PackageNotFound(package_name.to_string()))
    }
    /// 取得某套件「最新」的版本——依發布順序(`Vec` 最後一筆),不比較 semver。
    pub fn latest_version(&self, package_name: &str) -> CoreResult<&PackageVersionInfo> {
        self.versions_of(package_name)?
            .last()
            .ok_or_else(|| CoreError::PackageNotFound(package_name.to_string()))
    }
    pub fn get_package_handler(&self) -> &HashMap<String, Vec<PackageVersionInfo>> {
        &self.packages
    }
}
```

把 `#[cfg(feature = "server")]` impl(第 191-282 行)整個換成:

```rust
#[cfg(feature = "server")]
impl RepoInfo {
    /// 新增一個套件版本。同一個套件名稱下,`info.version` 不能跟既有版本重複
    /// (已發布版本不可變——要換內容是撤下重發,不是原地覆寫)。
    pub fn add_package_version(
        &mut self,
        name: String,
        info: PackageVersionInfo,
    ) -> CoreResult<()> {
        let versions = self.packages.entry(name).or_default();
        if versions.iter().any(|v| v.version == info.version) {
            return Err(CoreError::VersionMismatch(format!(
                "version {} is already published",
                info.version
            )));
        }
        versions.push(info);
        Ok(())
    }

    /// 移除某套件的特定版本。移除後若該套件已無任何版本,連同套件名稱一起移除。
    pub fn remove_package_version(
        &mut self,
        package_name: &str,
        version: &str,
    ) -> CoreResult<PackageVersionInfo> {
        let versions = self
            .packages
            .get_mut(package_name)
            .ok_or_else(|| CoreError::PackageNotFound(package_name.to_string()))?;
        let idx = versions
            .iter()
            .position(|v| v.version == version)
            .ok_or_else(|| {
                CoreError::PackageNotFound(format!("{package_name}@{version}"))
            })?;
        let removed = versions.remove(idx);
        if versions.is_empty() {
            self.packages.remove(package_name);
        }
        Ok(removed)
    }
}
```

把 `#[cfg(feature = "client")]` impl(第 284-331 行)整個換成:

```rust
#[cfg(feature = "client")]
impl RepoInfo {
    /// 從遠端抓某個來源的完整索引,整包覆蓋 `self`(這個 `RepoInfo` 實例代表
    /// 單一來源;多來源合併是呼叫端——`dpm` crate——的責任,每個來源各自呼叫
    /// 一次這個方法在自己的 `RepoInfo` 實例上)。
    pub async fn fetch_update_repo_info(&mut self, url: &str) -> CoreResult<()> {
        let repo_info: RepoInfo = JsonStorage::from_url(url).await?;
        self.packages = repo_info.packages;
        Ok(())
    }

    /// 取得某套件某個特定版本的完整 `packageInfo.json`(只有 `Prebuilt` 版本
    /// 有 URL 可抓;`Source` 版本目前回傳 `InvalidPackage` 錯誤,Phase 4 client
    /// 端 source 安裝路徑落地後才會補上對應處理)。
    pub async fn get_package_info(
        &self,
        package_name: &str,
        version: &str,
    ) -> CoreResult<PackageInfo> {
        let versions = self.versions_of(package_name)?;
        let entry = versions
            .iter()
            .find(|v| v.version == version)
            .ok_or_else(|| {
                CoreError::PackageNotFound(format!("{package_name}@{version}"))
            })?;
        match &entry.kind {
            PackageKind::Prebuilt { url, file_name, .. } => {
                let new_url = url.replace(
                    file_name,
                    format!("src/{package_name}/packageInfo.json").as_str(),
                );
                JsonStorage::from_url(&new_url).await
            }
            PackageKind::Source { .. } => Err(CoreError::InvalidPackage(format!(
                "{package_name}@{version} is a source package, not yet installable"
            ))),
        }
    }
}
```

刪除舊的 `fetch_package` 方法(第 291-317 行,`pub async fn fetch_package` 那整段)——repo 內零呼叫端,已用 `grep -rn "fetch_package\b" crates/` 確認,同時刪除 `get_single_package_info` 這個名字(被上面的 `get_package_info` 取代)。

檢查檔案頂端 `use` 區塊:`env`(`std::env::temp_dir()`)只被剛刪除的 `fetch_package` 用到,若 `cargo build` 出現 `unused import` 警告就一併刪除該行 `use`。

- [ ] **Step 5: 跑測試,確認變綠**

Run: `cargo test -p DPM-Core --features server,client versioning_tests -- --nocapture 2>&1 | tail -60`
Expected: 全部 `versioning_tests::*` 通過(含 `server_tests`/`client_tests` 子模組)。

- [ ] **Step 6: 跑整個 `dpm-core` 測試(含未加 feature 版本)確認沒改壞既有東西**

Run: `cargo test -p DPM-Core 2>&1 | tail -30`
Expected: `test_from_json`/`test_to_json`/`test_from_url`/`test_from_str_to`/`test_dependency_serde`/`test_hash_file_is_deterministic_and_content_sensitive` 全部通過(這次 without feature,`versioning_tests::server_tests`/`client_tests` 因為沒開 feature 不會被編譯進來,`versioning_tests` 裡不帶 feature gate 的兩個測試—— `has_package_false_before_any_version_added`/`versions_of_missing_package_is_not_found`——仍然會跑且要過)。

- [ ] **Step 7: 順手修正 `dpm-core/README.md` 的過期範例(TODO.md 已記錄的 P3 項目)**

編輯 `crates/dpm-core/README.md`,找到 `add_package` 的使用範例(約在第 100-111 行),把它換成新 API `add_package_version` 的範例:

```rust
use dpm_core::{PackageKind, PackageVersionInfo, RepoInfo};

let mut repo = RepoInfo::new();
repo.add_package_version(
    "my-package".to_string(),
    PackageVersionInfo {
        version: "1.0.0".to_string(),
        kind: PackageKind::Prebuilt {
            url: "https://example.com/my-package.zip".to_string(),
            hash: "blake3-hash-here".to_string(),
            file_name: "my-package.zip".to_string(),
        },
        dependencies: None,
        entry: Some("bin/my-package".to_string()),
        description: Some("An example package".to_string()),
    },
)?;
```

同一個檔案裡如果有 `PackageInfo` 欄位表出現空白的 `` `` `` 那格(TODO.md 提到「應該是 `description`」),順手補上 `description`。

- [ ] **Step 8: 整個 workspace 編譯確認**

Run: `cargo build --workspace 2>&1 | tail -40`
Expected: 無錯誤(`dpm`/`dpm-server` 這兩個 crate 目前還在用舊的 `PackageBasicInfo`/`add_package`/`get_single_package_info` 等 API,這步會編不過——這是預期的,因為 Task 1 只改 `dpm-core`,`dpm`/`dpm-server` 的呼叫端要等到 Task 5/6 才會跟著改。這裡的「Expected: 無錯誤」只適用於 Task 1 是這個 plan 最後一個 task 的情況;由於還有後續 task,這步驟改成:確認 `cargo build -p DPM-Core --all-features` 無錯誤即可,`cargo build --workspace` 會在 Task 6 之後才真正全綠。)

Run: `cargo build -p DPM-Core --all-features 2>&1 | tail -40`
Expected: 無錯誤。

- [ ] **Step 9: Commit**

```bash
git add crates/dpm-core/src/lib.rs crates/dpm-core/src/error.rs \
  crates/dpm-core/tests/test.rs crates/dpm-core/README.md
git commit -m "$(cat <<'EOF'
feat(dpm-core): multi-version RepoInfo data model

Replaces PackageBasicInfo (one version per package) with
PackageVersionInfo + PackageKind, and RepoInfo now maps a package
name to a Vec of published versions instead of a single entry.
Published versions are treated as immutable: add_package_version
appends (rejecting duplicate version strings), remove_package_version
removes one specific version. update_package's old field-overwrite
semantics and the unused fetch_package are dropped — neither has a
production call site and neither maps cleanly onto immutable
versions.

This is Task 1 of the multi-source/namespace plan (Phase 2 of the
multi-source registry design) — dpm/dpm-server call sites are
updated in later tasks of the same plan.

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: `dpm` 本地 DB schema 改版(多來源 + 多版本)

**Files:**
- Create: `crates/dpm/migrations/0002_multi_source.up.sql`
- Create: `crates/dpm/migrations/0002_multi_source.down.sql`
- Modify: `crates/dpm/src/utils/db.rs`
- Modify: `crates/dpm/src/utils/models.rs`
- Modify: `crates/dpm/tests/db_tests.rs`

**Interfaces:**
- Consumes:無(這個 task 只動 `dpm` 自己的 DB 層,不依賴 Task 1 的 `dpm-core` API 變更——`DbPackage` 繼續用 `dpm_core::Dependency`,型別沒變)。
- Produces:
  - `DbPackage::new(source, name, version, kind, url, hash, filename, build_command, description, entry, dependencies)`(11 個參數,見 Step 4)
  - `Db::insert(&self, pkg: DbPackage) -> ClientResult<()>`(簽名不變,內容變)
  - `Db::read_all(&self) -> ClientResult<Vec<DbPackage>>`(簽名不變)
  - `Db::read_one(&self, source: &str, name: &str, version: &str) -> ClientResult<Option<DbPackage>>`(新增 `source`/`version` 參數)
  - `Db::versions_of(&self, source: &str, name: &str) -> ClientResult<Vec<DbPackage>>`(新方法)
  - `Db::sources_of(&self, name: &str) -> ClientResult<Vec<String>>`(新方法)
  - `Db::latest_version(&self, source: &str, name: &str) -> ClientResult<Option<DbPackage>>`(新方法,依 `rowid DESC LIMIT 1`)
  - `Db::clear_table_for_source(&self, source: &str) -> ClientResult<()>`(新方法,`DELETE FROM LocalRepo WHERE source = ?1`)
  - `Db::delete(&self, source: &str, name: &str, version: &str) -> ClientResult<()>`(新增 `source`/`version` 參數)
  - `Db::update_version` 整個刪除(見上方「與 spec 的差異」第 5 點)

- [ ] **Step 1: 寫新的 migration SQL**

建立 `crates/dpm/migrations/0002_multi_source.up.sql`:

```sql
DROP TABLE IF EXISTS LocalRepo;
CREATE TABLE IF NOT EXISTS LocalRepo (
    source TEXT NOT NULL,
    name TEXT NOT NULL,
    version TEXT NOT NULL,
    kind TEXT NOT NULL,
    url TEXT,
    hash TEXT,
    filename TEXT,
    build_command TEXT,
    description TEXT NOT NULL,
    entry TEXT NOT NULL,
    dependencies TEXT,
    PRIMARY KEY (source, name, version)
);
```

建立 `crates/dpm/migrations/0002_multi_source.down.sql`:

```sql
DROP TABLE IF EXISTS LocalRepo;
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

(`LocalRepo` 純粹是本機快取——每次 `dpm update` 都整個重抓重灌,`DROP`+`CREATE` 不會遺失任何使用者資料,不需要 `ALTER TABLE` 搬資料。)

- [ ] **Step 2: 寫失敗的測試(先紅)——`DbPackage`/`Db` 新 API**

編輯 `crates/dpm/tests/db_tests.rs`,把 `sample_pkg()` 函式跟所有測試整個換成:

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
        db.insert(sample_pkg("official", "0.1.0")).await?;

        let all = db.read_all().await?;
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "test_pkg");
        assert_eq!(all[0].source, "official");
        assert_eq!(all[0].version, "0.1.0");
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
        assert_eq!(sources, vec!["official".to_string(), "third-party".to_string()]);

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

        assert!(db.read_one("official", "test_pkg", "0.1.0").await?.is_none());
        assert!(db.read_one("third-party", "test_pkg", "0.1.0").await?.is_some());
        Ok(())
    }

    #[tokio::test]
    async fn test_delete_is_scoped_to_source_and_version() -> TestResult {
        let dir = tempdir()?;
        let db = setup_db(dir.path()).await?;
        db.insert(sample_pkg("official", "0.1.0")).await?;
        db.insert(sample_pkg("official", "0.2.0")).await?;

        db.delete("official", "test_pkg", "0.1.0").await?;

        assert!(db.read_one("official", "test_pkg", "0.1.0").await?.is_none());
        assert!(db.read_one("official", "test_pkg", "0.2.0").await?.is_some());
        Ok(())
    }
}
```

- [ ] **Step 3: 確認編不過(紅燈)**

Run: `cargo test -p DPM db_tests 2>&1 | tail -40`
Expected: 編譯錯誤——`DbPackage::new` 參數數量不對、`read_one`/`delete` 少參數、`versions_of`/`sources_of`/`latest_version`/`clear_table_for_source` 不存在。

- [ ] **Step 4: 重寫 `DbPackage`**

編輯 `crates/dpm/src/utils/models.rs`,整個檔案換成:

```rust
use super::ClientError;
use super::ClientResult;
use dpm_core::CoreError::*;
use dpm_core::Dependency;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DbPackage {
    pub source: String,
    pub name: String,
    pub version: String,
    /// "prebuilt" | "source"
    pub kind: String,
    pub url: Option<String>,
    pub hash: Option<String>,
    pub filename: Option<String>,
    pub build_command: Option<String>,
    pub description: String,
    pub entry: String,
    pub dependencies: Option<Vec<Dependency>>,
}

#[allow(clippy::too_many_arguments)]
impl DbPackage {
    pub fn new(
        source: &str,
        name: &str,
        version: &str,
        kind: &str,
        url: Option<String>,
        hash: Option<String>,
        filename: Option<String>,
        build_command: Option<String>,
        description: &str,
        entry: &str,
        dependencies: Option<Vec<Dependency>>,
    ) -> Self {
        DbPackage {
            source: source.to_owned(),
            name: name.to_owned(),
            version: version.to_owned(),
            kind: kind.to_owned(),
            url,
            hash,
            filename,
            build_command,
            description: description.to_owned(),
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

- [ ] **Step 5: 重寫 `Db` 的 migration 攤開邏輯,加上 0002**

編輯 `crates/dpm/src/utils/db.rs` 的 `run_migrations`,在寫 `0001_init.*` 那兩段 `std::fs::write` 之後、呼叫 `geni::migrate_database` 之前,加入 0002 的攤開:

```rust
        std::fs::write(
            migrations_dir.join("0001_init.down.sql"),
            include_str!("../../migrations/0001_init.down.sql"),
        )
        .map_err(|e| ClientError::Core(IoError(e)))?;
        std::fs::write(
            migrations_dir.join("0002_multi_source.up.sql"),
            include_str!("../../migrations/0002_multi_source.up.sql"),
        )
        .map_err(|e| ClientError::Core(IoError(e)))?;
        std::fs::write(
            migrations_dir.join("0002_multi_source.down.sql"),
            include_str!("../../migrations/0002_multi_source.down.sql"),
        )
        .map_err(|e| ClientError::Core(IoError(e)))?;
```

- [ ] **Step 6: 重寫 `row_to_package`、`insert`、`read_all`、`read_one`**

`row_to_package`(取代第 78-103 行)——欄位順序對齊新 schema 的 11 欄(`source, name, version, kind, url, hash, filename, build_command, description, entry, dependencies`):

```rust
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
        let get_opt_text = |idx: usize| -> ClientResult<Option<String>> {
            Ok(row
                .get_value(idx)
                .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?
                .as_text()
                .cloned())
        };
        let dependencies_json = get_opt_text(10)?;
        Ok(DbPackage {
            source: get_text(0)?,
            name: get_text(1)?,
            version: get_text(2)?,
            kind: get_text(3)?,
            url: get_opt_text(4)?,
            hash: get_opt_text(5)?,
            filename: get_opt_text(6)?,
            build_command: get_opt_text(7)?,
            description: get_text(8)?,
            entry: get_text(9)?,
            dependencies: dependencies_json.and_then(|json| serde_json::from_str(&json).ok()),
        })
    }
```

`insert`(取代第 113-139 行):

```rust
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
            turso::Value::Text(pkg.entry),
            to_value(dependencies_json),
        ];
        conn.execute(
            "INSERT INTO LocalRepo (source, name, version, kind, url, hash, filename, build_command, description, entry, dependencies) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params,
        )
        .await
        .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?;
        Ok(())
    }
```

`read_all`(取代第 141-159 行,只改 SELECT 的欄位清單):

```rust
    pub async fn read_all(&self) -> ClientResult<Vec<DbPackage>> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT source, name, version, kind, url, hash, filename, build_command, description, entry, dependencies FROM LocalRepo",
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
```

`read_one`(取代第 161-178 行):

```rust
    pub async fn read_one(
        &self,
        source: &str,
        name: &str,
        version: &str,
    ) -> ClientResult<Option<DbPackage>> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT source, name, version, kind, url, hash, filename, build_command, description, entry, dependencies FROM LocalRepo WHERE source = ?1 AND name = ?2 AND version = ?3",
                [source, name, version],
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
```

- [ ] **Step 7: 刪除 `update_version`,新增 `versions_of`/`sources_of`/`latest_version`/`clear_table_for_source`,改 `delete`**

刪除整個 `update_version` 方法(第 180-189 行)。

`delete`(取代第 191-197 行):

```rust
    pub async fn delete(&self, source: &str, name: &str, version: &str) -> ClientResult<()> {
        let conn = self.connect().await?;
        conn.execute(
            "DELETE FROM LocalRepo WHERE source = ?1 AND name = ?2 AND version = ?3",
            [source, name, version],
        )
        .await
        .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?;
        Ok(())
    }
```

在 `clear_table`(第 204-206 行)之後加入四個新方法:

```rust
    pub async fn clear_table_for_source(&self, source: &str) -> ClientResult<()> {
        let conn = self.connect().await?;
        conn.execute("DELETE FROM LocalRepo WHERE source = ?1", [source])
            .await
            .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?;
        Ok(())
    }

    pub async fn versions_of(&self, source: &str, name: &str) -> ClientResult<Vec<DbPackage>> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT source, name, version, kind, url, hash, filename, build_command, description, entry, dependencies FROM LocalRepo WHERE source = ?1 AND name = ?2",
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
            packages.push(Self::row_to_package(row)?);
        }
        Ok(packages)
    }

    pub async fn sources_of(&self, name: &str) -> ClientResult<Vec<String>> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT DISTINCT source FROM LocalRepo WHERE name = ?1",
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
                .ok_or_else(|| ClientError::Core(DatabaseError("source column is not text".to_string())))?;
            sources.push(source);
        }
        Ok(sources)
    }

    /// 「最新版本」= 這個 (source, name) 底下 `rowid` 最大的那一列,也就是最後
    /// 插入的那筆——不比較 semver。`dpm update` 每次整個 source 清空重灌,插入
    /// 順序等於 `RepoInfo.json` 的陣列順序,等於伺服器端發布順序。真正的版本
    /// 排序邏輯留給 Phase 5(pubgrub)。
    pub async fn latest_version(&self, source: &str, name: &str) -> ClientResult<Option<DbPackage>> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT source, name, version, kind, url, hash, filename, build_command, description, entry, dependencies FROM LocalRepo WHERE source = ?1 AND name = ?2 ORDER BY rowid DESC LIMIT 1",
                [source, name],
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
```

- [ ] **Step 8: `download_file` 改吃新 `read_one` 簽名**

編輯 `download_file`(現在第 208-236 行)——它內部呼叫 `self.read_one(name)`,現在要傳 source/version。改成一併接收這兩個參數:

```rust
    pub async fn download_file(
        &self,
        source: &str,
        name: &str,
        version: &str,
        dest_path: &Path,
    ) -> ClientResult<()> {
        let package = self
            .read_one(source, name, version)
            .await?
            .ok_or_else(|| ClientError::Core(PackageNotFound(name.to_string())))?;
        let url = package
            .url
            .ok_or_else(|| ClientError::Core(InvalidPackage(format!("{name} has no url"))))?;
        let req = reqwest::get(&url)
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
```

(這步之後 `db.rs` 頂端的 `use dpm_core::CoreError::*;` 需要多用到 `InvalidPackage`——已經是 glob import,不用改 `use` 那行。)

- [ ] **Step 9: 跑 `db_tests`,確認變綠**

Run: `cargo test -p DPM db_tests -- --nocapture 2>&1 | tail -60`
Expected: 全部 8 個測試通過。

- [ ] **Step 10: 整個 workspace 編譯(`dpm` 這裡會編不過,預期中)**

Run: `cargo build -p DPM 2>&1 | tail -60`
Expected: `crates/dpm/src/action.rs` 裡呼叫 `get_db().read_one(pkg)`/`download_file(pkg, &download_path)` 的地方會編不過(參數數量不對)——這是預期的,`action.rs` 要等 Task 5 才會跟著改。確認錯誤訊息「只」來自 `action.rs`(不是 `db.rs`/`models.rs` 自己),證明這個 task 自己的程式碼是對的。

- [ ] **Step 11: Commit**

```bash
git add crates/dpm/migrations/0002_multi_source.up.sql \
  crates/dpm/migrations/0002_multi_source.down.sql \
  crates/dpm/src/utils/db.rs crates/dpm/src/utils/models.rs \
  crates/dpm/tests/db_tests.rs
git commit -m "$(cat <<'EOF'
feat(dpm): multi-source, multi-version local DB schema

LocalRepo's primary key becomes (source, name, version) instead of
just name, with new columns for kind/build_command (prebuilt vs.
source packages) and nullable url/hash/filename (only prebuilt
packages have those). Adds versions_of/sources_of/latest_version/
clear_table_for_source to support per-source refresh and bare-name
collision detection across sources. update_version is dropped (zero
production callers, and "editing a version's version string in
place" doesn't map onto immutable published versions).

action.rs call sites are updated in a later task of the same plan —
this task alone leaves `cargo build -p DPM` red at those call sites,
which is expected.

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: `dpm` config schema(`sources` 陣列取代 `repo_url`/`repo_info`)

**Files:**
- Modify: `crates/dpm/src/lib.rs`
- Modify: `crates/dpm/src/utils/system.rs`
- Create: `crates/dpm/tests/config_tests.rs`

**Interfaces:**
- Consumes:無(這個 task 只動 config 層,不依賴 Task 1/2)。
- Produces:
  - `pub struct Source { pub alias: String, pub repo_url: String, pub repo_info: String }`
  - `pub struct Setting { pub sources: Vec<Source> }`(取代 `pub type Setting = HashMap<String, String>`)
  - `SystemController::init(&self) -> ClientResult<Setting>`(簽名不變,回傳型別的內容變了;這次會把預設 source 真正寫回 `config.json`,修掉現有「`repo_url`/`repo_info` 只留在記憶體、檔案永遠是 `{}`」的已知 bug——TODO.md 已記錄的 P1 項目)

- [ ] **Step 1: 寫失敗的測試(先紅)——`Source`/`Setting` round-trip**

建立 `crates/dpm/tests/config_tests.rs`:

```rust
#[cfg(test)]
mod config_tests {
    use DPM::{Setting, Source};

    #[test]
    fn setting_round_trips_through_json() {
        let setting = Setting {
            sources: vec![Source {
                alias: "official".to_string(),
                repo_url: "https://github.com/Derrick-Program/DPM-Server/tree/main/Repo"
                    .to_string(),
                repo_info:
                    "https://raw.githubusercontent.com/Derrick-Program/DPM-Server/main/RepoInfo.json"
                        .to_string(),
            }],
        };

        let json = serde_json::to_string(&setting).unwrap();
        let parsed: Setting = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.sources.len(), 1);
        assert_eq!(parsed.sources[0].alias, "official");
    }

    #[test]
    fn setting_defaults_to_empty_sources_when_missing() {
        let parsed: Setting = serde_json::from_str("{}").unwrap();
        assert!(parsed.sources.is_empty());
    }
}
```

- [ ] **Step 2: 確認編不過(紅燈)**

Run: `cargo test -p DPM config_tests 2>&1 | tail -30`
Expected: 編譯錯誤——`Source` 不存在,`Setting` 還是 `HashMap<String,String>`,沒有 `.sources` 欄位。

- [ ] **Step 3: 改 `crates/dpm/src/lib.rs` 的 `Setting` 型別**

把第 4 行 `pub type Setting = HashMap<String, String>;` 換成:

```rust
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Source {
    pub alias: String,
    pub repo_url: String,
    pub repo_info: String,
}

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct Setting {
    #[serde(default)]
    pub sources: Vec<Source>,
}
```

確認檔案頂端有 `use serde::{Deserialize, Serialize};`(目前應該還沒有,因為舊的 `Setting`/`Hashes` 都是 `HashMap` 型別別名,不需要 derive——這步要加上這個 `use`)。

`Hashes` 型別別名(第 5 行)維持不動,它跟安裝時驗 `hashes.json` 有關,跟這次的 config schema 無關。

- [ ] **Step 4: 重寫 `system.rs::init()`,修正 config 沒寫回硬碟的 bug**

編輯 `crates/dpm/src/utils/system.rs`,把整個 `init()` 方法(第 102-141 行)換成:

```rust
    pub async fn init(&self) -> ClientResult<Setting> {
        self.system_command_runner(
            "mkdir",
            vec!["-p", INSTALL_DIR.get().unwrap().to_str().unwrap()],
            "Can't create Software dir",
        )?;
        self.system_command_runner(
            "mkdir",
            vec!["-p", CONFIG.get().unwrap().to_str().unwrap()],
            "Can't create Settings dir",
        )?;
        self.system_command_runner(
            "mkdir",
            vec!["-p", BIN_DIR.get().unwrap().to_str().unwrap()],
            "Can't create bin dir",
        )?;
        self.permision_check()?;
        let config_path = CONFIG.get().unwrap().join("config.json");
        if !config_path.exists() {
            let default_setting = Setting {
                sources: vec![Source {
                    alias: "official".to_string(),
                    repo_url: "https://github.com/Derrick-Program/DPM-Server/tree/main/Repo"
                        .to_string(),
                    repo_info:
                        "https://raw.githubusercontent.com/Derrick-Program/DPM-Server/main/RepoInfo.json"
                            .to_string(),
                }],
            };
            JsonStorage::to_json(&default_setting, &config_path)?;
            for source in &default_setting.sources {
                ActionInfo::init_update(source).await?;
            }
        }
        self.permision_check()?;
        let config: Setting = JsonStorage::from_json(&config_path)?;
        Ok(config)
    }
```

這裡改掉的核心行為:舊版用 `File::create` + `write_all(b"{}")` 手動生一個空檔案再塞值進記憶體(從沒寫回硬碟);新版直接用既有的 `JsonStorage::to_json` 把完整的預設 `Setting` 寫進 `config_path`,一步到位,不會有「檔案跟記憶體對不上」的問題。`ActionInfo::init_update` 的簽名在 Task 5 會從 `(url_json: &str)` 改成 `(source: &Source)`——這裡先照新簽名寫,Task 5 會補上對應實作,這個 task 完成時 `cargo build -p DPM` 在這裡會因為 `init_update` 簽名還沒改而編不過,是預期的(下一步驗證)。

檔案開頭 `use` 區塊補上 `Source`:

```rust
use crate::{ActionInfo, Scope, Setting, Source, BIN_DIR, CONFIG, INSTALL_DIR, MAIN_DIR, SCOPE};
```

- [ ] **Step 5: 跑 `config_tests`,確認變綠**

Run: `cargo test -p DPM config_tests -- --nocapture 2>&1 | tail -30`
Expected: 兩個測試都過。

- [ ] **Step 6: 確認 `dpm` 因為 `init_update` 簽名不符而編不過(預期中的紅燈,證明改動確實落到呼叫端)**

Run: `cargo build -p DPM 2>&1 | tail -40`
Expected: 編譯錯誤,錯誤訊息指向 `system.rs` 呼叫 `ActionInfo::init_update(source)` 但 `action.rs` 裡 `init_update` 還是舊簽名 `(url_json: &str)`——這個不符會在 Task 5 解掉。

- [ ] **Step 7: Commit**

```bash
git add crates/dpm/src/lib.rs crates/dpm/src/utils/system.rs \
  crates/dpm/tests/config_tests.rs
git commit -m "$(cat <<'EOF'
feat(dpm): sources array replaces flat repo_url/repo_info config keys

Setting is now { sources: Vec<Source> } instead of a flat
HashMap<String, String> with two hardcoded keys. This also fixes a
pre-existing bug (documented in TODO.md): the old init() built the
default repo_url/repo_info pair only in memory and never wrote it
back to config.json, so the file was permanently "{}" after first
run. The new init() writes the full default Setting via
JsonStorage::to_json in one step.

action.rs's init_update signature is updated in a later task of the
same plan — this task alone leaves `cargo build -p DPM` red at that
call site, which is expected.

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: `dpm` CLI `source add/remove/list` 子指令

**Files:**
- Modify: `crates/dpm/src/arch.rs`
- Modify: `crates/dpm/src/cli_parse.rs`
- Create: `crates/dpm/tests/cli_parse_tests.rs`

**Interfaces:**
- Consumes:Task 3 的 `dpm_core`(其實是 `crate::{Source}`,`dpm` 自己的型別,不是 `dpm_core`)——`pub struct Source { pub alias: String, pub repo_url: String, pub repo_info: String }`。
- Produces:
  - `pub enum SourceAction { Add { url: String, alias: Option<String> }, Remove { alias: String }, List }`(加進 `arch.rs`)
  - `CliCommands::Source(SourceAction)` 新變體
  - `build_cli()`/`get_args()` 支援 `dpm source add <url> [--as <alias>]`、`dpm source remove <alias>`、`dpm source list`

- [ ] **Step 1: 寫失敗的測試(先紅)——CLI 解析結果**

建立 `crates/dpm/tests/cli_parse_tests.rs`。這個測試直接呼叫 `build_cli()` 拿到 `clap::Command`,用 `try_get_matches_from` 模擬命令列輸入,不呼叫 `get_args()`(因為 `get_args()` 目前簽名綁死讀真的 `std::env::args()`,呼叫真正的 `get_args()` 需要另外重構成吃 `Vec<String>` 才能單元測試——這個重構不在這個 task 範圍內,詳見 Step 4 說明)。改成直接斷言 `ArgMatches` 的內容:

```rust
#[cfg(test)]
mod cli_parse_tests {
    use DPM::build_cli;

    #[test]
    fn source_add_parses_url_and_alias() {
        let cli = build_cli();
        let matches = cli
            .try_get_matches_from(["dpm", "source", "add", "https://example.com/repo", "--as", "myrepo"])
            .unwrap();
        let (name, sub) = matches.subcommand().unwrap();
        assert_eq!(name, "source");
        let (inner_name, inner) = sub.subcommand().unwrap();
        assert_eq!(inner_name, "add");
        assert_eq!(
            inner.get_one::<String>("URL").unwrap(),
            "https://example.com/repo"
        );
        assert_eq!(inner.get_one::<String>("as").unwrap(), "myrepo");
    }

    #[test]
    fn source_add_alias_is_optional() {
        let cli = build_cli();
        let matches = cli
            .try_get_matches_from(["dpm", "source", "add", "https://example.com/repo"])
            .unwrap();
        let (_, sub) = matches.subcommand().unwrap();
        let (_, inner) = sub.subcommand().unwrap();
        assert!(inner.get_one::<String>("as").is_none());
    }

    #[test]
    fn source_remove_requires_alias() {
        let cli = build_cli();
        let matches = cli
            .try_get_matches_from(["dpm", "source", "remove", "myrepo"])
            .unwrap();
        let (_, sub) = matches.subcommand().unwrap();
        let (inner_name, inner) = sub.subcommand().unwrap();
        assert_eq!(inner_name, "remove");
        assert_eq!(inner.get_one::<String>("ALIAS").unwrap(), "myrepo");
    }

    #[test]
    fn source_list_takes_no_args() {
        let cli = build_cli();
        let matches = cli.try_get_matches_from(["dpm", "source", "list"]).unwrap();
        let (_, sub) = matches.subcommand().unwrap();
        assert_eq!(sub.subcommand().unwrap().0, "list");
    }

    #[test]
    fn source_without_subcommand_is_an_error() {
        let cli = build_cli();
        let result = cli.try_get_matches_from(["dpm", "source"]);
        assert!(result.is_err(), "source requires add/remove/list");
    }
}
```

- [ ] **Step 2: 確認編不過/測試失敗(紅燈)**

Run: `cargo test -p DPM cli_parse_tests 2>&1 | tail -40`
Expected: 失敗——目前 `build_cli()` 沒有 `source` 子指令,`try_get_matches_from` 對 `"source"` 這個字串會回傳「unrecognized subcommand」的 `Err`,所有斷言 `matches.subcommand().unwrap()` 那類的測試會 panic。

先確認 `build_cli` 這個函式目前有沒有 `pub`——如果目前是 private 只在同檔案用,這步驟要順手把它改成 `pub fn build_cli() -> Command`,並確認 `crates/dpm/src/lib.rs` 有 `pub use cli_parse::*;`(或等效的 re-export)讓 `DPM::build_cli` 這個路徑在整合測試裡可以被呼叫到。

- [ ] **Step 3: 在 `arch.rs` 加 `SourceAction`/`CliCommands::Source`**

編輯 `crates/dpm/src/arch.rs`,在 `CliCommands` enum(現有 8 個變體:`Search`/`Install`/`List`/`Uninstall`/`Update`/`Upgrade`/`UpgradeSelf`/`None`)裡加入 `Source`:

```rust
#[derive(Debug)]
pub enum CliCommands {
    Search,
    Install,
    List,
    Uninstall,
    Update,
    Upgrade,
    UpgradeSelf,
    Source(SourceAction),
    None,
}

#[derive(Debug)]
pub enum SourceAction {
    Add { url: String, alias: Option<String> },
    Remove { alias: String },
    List,
}
```

- [ ] **Step 4: 在 `cli_parse.rs` 加 `source` 子指令定義跟解析**

編輯 `crates/dpm/src/cli_parse.rs`。在 `build_cli()` 的 `.subcommands([...])` 陣列裡(比照 `install` 那段的縮排位置),加入:

```rust
                Command::new("source")
                    .about("Manage package sources")
                    .subcommand_required(true)
                    .arg_required_else_help(true)
                    .subcommand(
                        Command::new("add")
                            .about("Add a package source")
                            .arg(
                                Arg::new("URL")
                                    .value_name("URL")
                                    .required(true),
                            )
                            .arg(
                                Arg::new("as")
                                    .long("as")
                                    .value_name("ALIAS")
                                    .help("Alias for this source (defaults to the URL host)"),
                            ),
                    )
                    .subcommand(
                        Command::new("remove")
                            .about("Remove a package source")
                            .arg(
                                Arg::new("ALIAS")
                                    .value_name("ALIAS")
                                    .required(true),
                            ),
                    )
                    .subcommand(Command::new("list").about("List configured package sources")),
```

在 `get_args()` 的 `match matches.subcommand()`(第 202-281 行那個 match 區塊)裡加入對應分支,比照 `install` 分支的縮排/風格:

```rust
        Some(("source", sub_command)) => match sub_command.subcommand() {
            Some(("add", add_args)) => {
                Commands = Some(CliCommands::Source(SourceAction::Add {
                    url: add_args.get_one::<String>("URL").unwrap().to_string(),
                    alias: add_args.get_one::<String>("as").map(|s| s.to_string()),
                }));
            }
            Some(("remove", remove_args)) => {
                Commands = Some(CliCommands::Source(SourceAction::Remove {
                    alias: remove_args.get_one::<String>("ALIAS").unwrap().to_string(),
                }));
            }
            Some(("list", _)) => {
                Commands = Some(CliCommands::Source(SourceAction::List));
            }
            _ => unreachable!("clap enforces subcommand_required(true) on `source`"),
        },
```

檔案頂端 `use` 區塊補上 `SourceAction`(跟 `CliCommands`/`Scope` 那些一起從 `crate::arch::*` 或現有的 import 路徑進來——照現有 `CliCommands`/`Option_set` 是怎麼 `use` 進來的同樣方式加)。

如果 `build_cli()` 目前不是 `pub fn`,依 Step 2 的要求把它改成 `pub fn build_cli() -> Command`。

- [ ] **Step 5: 跑 `cli_parse_tests`,確認變綠**

Run: `cargo test -p DPM cli_parse_tests -- --nocapture 2>&1 | tail -40`
Expected: 5 個測試全過。

- [ ] **Step 6: 整個 workspace 編譯(`entry()` 那邊會編不過,預期中)**

Run: `cargo build -p DPM 2>&1 | tail -40`
Expected: `crates/dpm/src/lib.rs::entry()` 裡的 `match config.Commands.unwrap() { ... }` 沒有窮盡 `CliCommands::Source(_)` 這個新變體,會編不過(non-exhaustive match)。這是預期的,Task 5 會補上 `ActionInfo::source()` 跟對應的 match 分支。

- [ ] **Step 7: Commit**

```bash
git add crates/dpm/src/arch.rs crates/dpm/src/cli_parse.rs \
  crates/dpm/tests/cli_parse_tests.rs
git commit -m "$(cat <<'EOF'
feat(dpm): add `dpm source add/remove/list` CLI subcommand

Nested subcommand under `source`, hand-rolled with clap::Command
(matching this crate's existing build_cli()/get_args() pattern
rather than dpm-server's derive(Subcommand) style — CLAUDE.md already
documents these two crates use different clap styles and new dpm
subcommands should follow dpm's existing convention).

entry()'s dispatch match is updated in a later task of the same plan
— this task alone leaves `cargo build -p DPM` red on a non-exhaustive
match, which is expected.

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: `dpm` action 層整合(source 管理 + `update`/`init_update`/`install` 改多來源)

**Files:**
- Modify: `crates/dpm/src/action.rs`
- Modify: `crates/dpm/src/lib.rs`

**Interfaces:**
- Consumes:Task 1 的 `dpm_core::{PackageVersionInfo, PackageKind}`、Task 2 的 `Db::{versions_of, sources_of, latest_version, clear_table_for_source, read_one(source,name,version), download_file(source,name,version,dest)}`、Task 3 的 `Setting { sources: Vec<Source> }`、Task 4 的 `CliCommands::Source(SourceAction)`。
- Produces:`ActionInfo::source(&self, action: SourceAction) -> ClientResult<()>`、`ActionInfo::init_update(source: &Source) -> ClientResult<()>`(簽名變更,Task 3 已經改了呼叫端,這裡補實作)。

- [ ] **Step 1: 加共用的 `sync_source` 私有輔助函式,取代 `update()`/`init_update()` 重複的抓取+寫入邏輯**

編輯 `crates/dpm/src/action.rs`。這是既有程式碼裡已經存在的重複邏輯(`update()` 第 152-192 行、`init_update()` 第 194-220 行幾乎一模一樣的「抓 RepoInfo → 迴圈轉成 DbPackage → insert」),藉這次 schema 重寫的機會收斂成一個共用函式。在 `impl ActionInfo` 區塊內(找一個現有 private method 附近,例如 `hasher`/`swap_into_install_dir` 之類的輔助函式旁,或直接放在 `install`/`update` 之前)加入:

```rust
    /// 抓某一個來源的完整索引,清空該來源在本地 DB 的舊資料,把每個套件的每個
    /// 版本各自插入一列。`update()`(既有來源全部重整)、`init_update()`
    /// (`init()` 第一次執行時的初始灌入)共用這個邏輯——原本兩處各自複製一份
    /// 幾乎相同的程式碼。
    async fn sync_source(source: &Source) -> ClientResult<()> {
        let mut remote_repo = RepoInfo::new();
        remote_repo
            .fetch_update_repo_info(&source.repo_info)
            .await?;

        get_db().clear_table_for_source(&source.alias).await?;

        for (name, versions) in remote_repo.get_package_handler() {
            for version_info in versions {
                let dependencies: Option<Vec<dpm_core::Dependency>> =
                    version_info.dependencies.as_ref().map(|deps| {
                        deps.iter()
                            .map(|dep| Dependency::new(&dep.name, &dep.version))
                            .collect::<Vec<_>>()
                    });
                let (kind_str, url, hash, filename, build_command) = match &version_info.kind {
                    PackageKind::Prebuilt {
                        url,
                        hash,
                        file_name,
                    } => (
                        "prebuilt".to_string(),
                        Some(url.clone()),
                        Some(hash.clone()),
                        Some(file_name.clone()),
                        None,
                    ),
                    PackageKind::Source { build } => {
                        ("source".to_string(), None, None, None, Some(build.clone()))
                    }
                };
                get_db()
                    .insert(DbPackage::new(
                        &source.alias,
                        name,
                        &version_info.version,
                        &kind_str,
                        url,
                        hash,
                        filename,
                        build_command,
                        version_info.description.as_deref().unwrap_or(""),
                        version_info.entry.as_deref().unwrap_or(""),
                        dependencies,
                    ))
                    .await?;
            }
        }
        Ok(())
    }
```

- [ ] **Step 2: 重寫 `update()`,迴圈跑過每個 source**

把 `update()`(第 152-193 行)整個換成:

```rust
    pub async fn update(&self) -> ClientResult<()> {
        println!("{} Updating...", "==>".blue());
        for source in &self.setting_config.sources {
            println!("{} Updating source '{}'...", "==>".blue(), source.alias);
            Self::sync_source(source).await?;
        }
        println!("{} Updated!", "==>".green());
        Ok(())
    }
```

- [ ] **Step 3: 重寫 `init_update()`,吃 `&Source` 而不是裸 URL 字串**

把 `init_update()`(第 194-220 行)整個換成:

```rust
    pub async fn init_update(source: &Source) -> ClientResult<()> {
        Self::sync_source(source).await
    }
```

- [ ] **Step 4: 加 `ActionInfo::source()`,實作 add/remove/list**

在 `impl ActionInfo` 內加入(這個方法要讀寫 `config.json`,用既有的 `CONFIG` OnceLock 跟 `JsonStorage`):

```rust
    pub async fn source(&self, action: SourceAction) -> ClientResult<()> {
        let config_path = CONFIG.get().unwrap().join("config.json");
        let mut setting: Setting = JsonStorage::from_json(&config_path)?;

        match action {
            SourceAction::Add { url, alias } => {
                if !url.starts_with("https://") {
                    return Err(ClientError::ConfigError(
                        "source url must use https://".to_string(),
                    ));
                }
                let alias = alias.unwrap_or_else(|| {
                    url.trim_start_matches("https://")
                        .split('/')
                        .next()
                        .unwrap_or(&url)
                        .to_string()
                });
                if setting.sources.iter().any(|s| s.alias == alias) {
                    return Err(ClientError::ConfigError(format!(
                        "source alias '{alias}' already exists"
                    )));
                }
                if alias != "official" {
                    println!(
                        "{} third-party source, not vetted by the DPM team",
                        "Warning:".yellow()
                    );
                }
                setting.sources.push(Source {
                    alias,
                    repo_url: url.clone(),
                    repo_info: url,
                });
                JsonStorage::to_json(&setting, &config_path)?;
                println!("{}", "Source added. Run `dpm update` to fetch its index.".green());
            }
            SourceAction::Remove { alias } => {
                let before = setting.sources.len();
                setting.sources.retain(|s| s.alias != alias);
                if setting.sources.len() == before {
                    return Err(ClientError::ConfigError(format!(
                        "no source with alias '{alias}'"
                    )));
                }
                JsonStorage::to_json(&setting, &config_path)?;
                get_db().clear_table_for_source(&alias).await?;
                println!("{}", "Source removed.".green());
            }
            SourceAction::List => {
                for source in &setting.sources {
                    println!("{}  {}", source.alias.green(), source.repo_info);
                }
            }
        }
        Ok(())
    }
```

注意 `SourceAction::Add` 這裡把使用者傳進來的單一 `url` 同時當 `repo_url`(給人看)跟 `repo_info`(程式抓取用)——spec 原本設計這兩個是分開欄位(一個是 repo 首頁、一個是索引檔案直連),但 CLI 目前只吃一個 `<url>` 位置參數,沒有讓使用者分開輸入兩個 URL 的介面。這是刻意的簡化:`source add` 的入口 UX 留到之後有真實第三方 tap 的使用情境時再擴充成兩個參數,現在先讓資料結構(`Source` 有兩個獨立欄位)跟 CLI(只收一個 URL)保持一致但簡單堪用——之後要擴充只需要多加一個 CLI flag,不需要動資料結構。

`InvalidPackage` 引入需要在 `download_file` 那邊已經用到(Task 2 Step 8),這裡不用重複處理。

- [ ] **Step 5: 重寫 `install()` 的裸名解析,支援多來源衝突偵測**

編輯 `install()`(現在第 47-150 行左右)。把 `for pkg in is { let pkg = pkg.as_str(); let repo_package_info = get_db().read_one(pkg)...` 那一段(從迴圈開頭到拿到 `repo_package_info` 為止)換成:

```rust
            for pkg in is {
                let pkg = pkg.as_str();
                let sources = get_db()
                    .sources_of(pkg)
                    .await
                    .map_err(|e| ClientError::Core(CoreError::DatabaseError(e.to_string())))?;
                let source_alias = match sources.len() {
                    0 => {
                        return Err(ClientError::Core(CoreError::PackageNotFound(
                            pkg.to_string(),
                        )))
                    }
                    1 => sources.into_iter().next().unwrap(),
                    _ => {
                        return Err(ClientError::Core(CoreError::AmbiguousPackage(format!(
                            "{pkg} (found in: {})",
                            sources.join(", ")
                        ))))
                    }
                };
                let repo_package_info = get_db()
                    .latest_version(&source_alias, pkg)
                    .await
                    .map_err(|e| ClientError::Core(CoreError::DatabaseError(e.to_string())))?
                    .ok_or_else(|| {
                        ClientError::Core(CoreError::PackageNotFound(pkg.to_string()))
                    })?;
```

這段之後,原本的 `repo_package_info.filename`(用在 `staging.path().join(&repo_package_info.filename)`)現在型別是 `Option<String>`(Task 2 改的),要處理:

```rust
                let filename = repo_package_info
                    .filename
                    .clone()
                    .ok_or_else(|| {
                        ClientError::Core(CoreError::InvalidPackage(format!(
                            "{pkg} has no downloadable file (source package kind not yet installable)"
                        )))
                    })?;
                let download_path = staging.path().join(&filename);
                get_db()
                    .download_file(&source_alias, pkg, &repo_package_info.version, &download_path)
                    .await
                    .map_err(|e| ClientError::Core(CoreError::NetworkError(e.to_string())))?;
```

(取代原本 `let download_path = staging.path().join(&repo_package_info.filename); get_db().download_file(pkg, &download_path).await...` 那兩行。)

再往下,原本比對 hash 的地方用到 `repo_package_info.hash`,現在也是 `Option<String>`:

```rust
                let hash = dpm_core::hash_file(&download_path)?;
                let expected_hash = repo_package_info.hash.clone().ok_or_else(|| {
                    ClientError::Core(CoreError::InvalidPackage(format!(
                        "{pkg} has no hash recorded"
                    )))
                })?;
                if expected_hash != hash {
                    return Err(ClientError::Core(CoreError::HashMismatch {
                        expected: expected_hash,
                        actual: hash,
                    }));
                }
```

(取代原本 `if repo_package_info.hash != hash { ... }` 那一段。)

- [ ] **Step 6: 補 `entry()` 的 `Source` dispatch,補齊 `use`**

編輯 `crates/dpm/src/lib.rs`,在 `entry()` 的 `match config.Commands.unwrap() { ... }` 裡加入:

```rust
            CliCommands::Source(action) => pass_info.source(action).await?,
```

確認 `action.rs` 頂端 `use` 有補上這個 task 新用到的型別——`Source`、`SourceAction`、`PackageKind`(來自 `dpm_core`)。既有的 `use dpm_core::{Dependency, JsonStorage, PackageInfo, RepoInfo};` 改成:

```rust
use dpm_core::{Dependency, JsonStorage, PackageInfo, PackageKind, RepoInfo};
```

- [ ] **Step 7: 整個 workspace 編譯 + `dpm` 全部測試**

Run: `cargo build -p DPM 2>&1 | tail -60`
Expected: 無錯誤。

Run: `cargo test -p DPM 2>&1 | tail -80`
Expected: 全部通過(`db_tests`/`config_tests`/`cli_parse_tests`/`scope_tests`,以及 `action.rs` 裡既有的 `atomic_install_tests` 系列——這幾個測試呼叫 `swap_into_install_dir` 不受這次改動影響,應該原封不動繼續通過)。

- [ ] **Step 8: Commit**

```bash
git add crates/dpm/src/action.rs crates/dpm/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(dpm): wire multi-source config/DB into update/init/install

- New shared sync_source() replaces the near-duplicate fetch+insert
  logic that update()/init_update() each had their own copy of.
- update() now loops over every configured source instead of reading
  a single repo_info URL.
- ActionInfo::source() implements `source add/remove/list`:
  add validates https://, defaults the alias to the URL host, warns
  on non-official sources; remove also clears that source's rows
  from the local DB; list prints alias + repo_info.
- install() now resolves a bare package name via sources_of() —
  0 sources is PackageNotFound, 2+ is a new AmbiguousPackage error
  asking the user to qualify by source, exactly 1 auto-resolves and
  installs its latest_version() (rowid-order latest, no semver
  comparison yet — that's Phase 5/pubgrub).

This completes the multi-source/namespace plan (Phase 2). Existing
single-source setups keep working identically: one configured source
never hits the ambiguity path.

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: `dpm-server` 發布多版本索引(`RepoInfo.json` 格式跟著 Task 1 換)

**Files:**
- Modify: `crates/dpm-server/src/action.rs`
- Modify: `crates/dpm-server/src/cli_parse.rs`
- Modify: `crates/dpm-server/RepoInfo.json`(既有測試 fixture,格式跟著換)

**Interfaces:**
- Consumes:Task 1 的 `dpm_core::{PackageVersionInfo, PackageKind, RepoInfo::add_package_version, RepoInfo::remove_package_version}`。
- Produces:`fix_add`/`fix_del`/`repo_init` 改用新 API;`Del` clap struct 多一個 optional `version` 欄位。

- [ ] **Step 1: 改 `Del` clap struct,加 optional `version`**

編輯 `crates/dpm-server/src/cli_parse.rs`,把 `Del` struct(現有):

```rust
#[derive(Args, Debug)]
pub struct Del {
    /// Project Name
    pub project_name: String,
}
```

換成:

```rust
#[derive(Args, Debug)]
pub struct Del {
    /// Project Name
    pub project_name: String,
    /// Version to remove (required if the package has more than one published version)
    pub version: Option<String>,
}
```

- [ ] **Step 2: 重寫 `fix_add`,改成 append 版本而不是覆蓋整包**

編輯 `crates/dpm-server/src/action.rs`。把 `fix_add`(現有函式)換成:

```rust
fn fix_add(obj: &Add, repo: &mut RepoInfo) -> Result<()> {
    let path = std::env::current_dir()?
        .join("Repo/src")
        .join(&obj.project_name);
    let package = current_dir()?
        .join("Repo")
        .join(format!("{}.zip", obj.project_name));
    if !package.exists() {
        return Err(anyhow::anyhow!(
            "\nPackage: {} {}",
            format!("{}", package.display()).yellow(),
            "Not found!".red()
        ));
    }
    let pk_info: PackageInfo = JsonStorage::from_json(&path.join("packageInfo.json"))?;

    let version_info = PackageVersionInfo {
        version: pk_info.version.clone(),
        kind: PackageKind::Prebuilt {
            url: format!(
                "https://github.com/Derrick-Program/DPM-Server/raw/main/Repo/{}.zip",
                obj.project_name
            ),
            hash: dpm_core::hash_file(&package)?,
            file_name: format!("{}.zip", pk_info.package_name),
        },
        dependencies: pk_info.dependencies,
        entry: None,
        description: Some(pk_info.description),
    };
    repo.add_package_version(obj.project_name.clone(), version_info)?;
    Ok(())
}
```

`fix_del` 換成(要支援「只有一個版本時不用指定版本、有多個版本時要求明確指定」):

```rust
fn fix_del(obj: &Del, repo: &mut RepoInfo) -> Result<()> {
    let version = match &obj.version {
        Some(v) => v.clone(),
        None => {
            let versions = repo.versions_of(&obj.project_name)?;
            if versions.len() > 1 {
                return Err(anyhow::anyhow!(
                    "\nPackage {} has {} published versions — specify which one to remove",
                    obj.project_name.yellow(),
                    versions.len()
                ));
            }
            versions
                .first()
                .ok_or_else(|| anyhow::anyhow!("\nPackage {} not found", obj.project_name.yellow()))?
                .version
                .clone()
        }
    };
    repo.remove_package_version(&obj.project_name, &version)?;
    Ok(())
}
```

這取代掉現有的 `fix_del`(`crates/dpm-server/src/action.rs:148-152`,原本整個函式是):

```rust
fn fix_del(obj: &Del, repo: &mut RepoInfo) -> Result<()> {
    repo.remove_package(&obj.project_name)?;
    println!("Package '{}' removed successfully.", obj.project_name);
    Ok(())
}
```

新版本保留同樣的成功訊息,加在 `repo.remove_package_version(...)?` 之後:

```rust
fn fix_del(obj: &Del, repo: &mut RepoInfo) -> Result<()> {
    let version = match &obj.version {
        Some(v) => v.clone(),
        None => {
            let versions = repo.versions_of(&obj.project_name)?;
            if versions.len() > 1 {
                return Err(anyhow::anyhow!(
                    "\nPackage {} has {} published versions — specify which one to remove",
                    obj.project_name.yellow(),
                    versions.len()
                ));
            }
            versions
                .first()
                .ok_or_else(|| anyhow::anyhow!("\nPackage {} not found", obj.project_name.yellow()))?
                .version
                .clone()
        }
    };
    repo.remove_package_version(&obj.project_name, &version)?;
    println!("Package '{}@{}' removed successfully.", obj.project_name, version);
    Ok(())
}
```

檔案頂端 `use` 補上 `PackageKind`、`PackageVersionInfo`(從 `dpm_core::*` 應該已經是 glob import,不用改;如果是明確列舉 import,補上這兩個名字)。

- [ ] **Step 3: `repo_init` 改吃多版本**

取代現有的 `repo_init`(`crates/dpm-server/src/action.rs:154-185`)——原本每個掃到的 `.zip` 組一個 `PackageBasicInfo` 呼叫 `add_package_with_info`,換成組 `PackageVersionInfo` 呼叫 `add_package_version`,掃描迴圈本身(`find_zip_files_and_names_in_repo()`、逐一檢查 `project.exists()`)不動:

```rust
pub fn repo_init(repo: &mut RepoInfo) -> Result<()> {
    println!("Initializing Repo...");
    let ret = find_zip_files_and_names_in_repo()?;
    for (_, name) in ret {
        let name_witout_zip = name.trim_end_matches(".zip");
        let project = PROJECT_SRC.get().unwrap().join(name_witout_zip);
        if !project.exists() {
            return Err(anyhow::anyhow!(
                "\nPackage: {} {}",
                name_witout_zip.yellow(),
                "Not found!".red()
            ));
        }
        let pk_info: PackageInfo = JsonStorage::from_json(&project.join("packageInfo.json"))?;
        let version_info = PackageVersionInfo {
            version: pk_info.version.clone(),
            kind: PackageKind::Prebuilt {
                url: format!(
                    "https://github.com/Derrick-Program/DPM-Server/raw/main/Repo/{}.zip",
                    pk_info.package_name
                ),
                hash: pk_info.hash.clone(),
                file_name: name.clone(),
            },
            dependencies: pk_info.dependencies,
            entry: None,
            description: Some(pk_info.description),
        };
        repo.add_package_version(name_witout_zip.to_string(), version_info)?;
        println!("Done...");
    }

    Ok(())
}
```

- [ ] **Step 4: 更新測試 fixture `RepoInfo.json`,跟新格式對齊**

編輯 `crates/dpm-server/RepoInfo.json`,把既有內容(單版本 `PackageBasicInfo` 形狀)換成多版本 `PackageVersionInfo` 陣列形狀,語意保持一樣(同樣 4 個套件、同樣的 hash/url/version 值,只是外層多包一層陣列、`kind`/`file_name` 位置依 Task 1 的 `#[serde(flatten)]` 內部標籤格式調整):

```json
{
  "packages": {
    "test": [
      {
        "version": "0.1.0",
        "kind": "prebuilt",
        "url": "https://github.com/Derrick-Program/DPM-Server/raw/main/Repo/test.zip",
        "hash": "1d597c82fa67c71aa626f30c55e18a4ccdf64b66515bc7b8e135f4725efa290f",
        "file_name": "test.zip",
        "dependencies": null
      }
    ],
    "helloWorld": [
      {
        "version": "0.1.0",
        "kind": "prebuilt",
        "url": "https://github.com/Derrick-Program/DPM-Server/raw/main/Repo/helloWorld.zip",
        "hash": "85bb7197a99725e636a1f5c4b4a7b39907a1791670dec88d5922d2132f687b91",
        "file_name": "helloWorld.zip",
        "dependencies": null
      }
    ],
    "test1": [
      {
        "version": "0.1.0",
        "kind": "prebuilt",
        "url": "https://github.com/Derrick-Program/DPM-Server/raw/main/Repo/test1.zip",
        "hash": "fb8a7507d0ad468fc290e8039dc5d555987865cd006813d9b77c8125446c04f1",
        "file_name": "test1.zip",
        "dependencies": null
      }
    ],
    "test2": [
      {
        "version": "0.1.0",
        "kind": "prebuilt",
        "url": "https://github.com/Derrick-Program/DPM-Server/raw/main/Repo/test2.zip",
        "hash": "452e550d54994074eff63de7cd3c25fd78c19c3ff177aabf2269415e992fdec7",
        "file_name": "test2.zip",
        "dependencies": [
          { "name": "test1", "version": "0.1.0" }
        ]
      }
    ]
  }
}
```

- [ ] **Step 5: 整個 workspace 編譯 + 全部測試**

Run: `cargo build --workspace 2>&1 | tail -60`
Expected: 無錯誤——這是這個 phase 第一次整個 workspace 全部一起編過(前面每個 task 都刻意留了紅燈給後續 task 接手)。

Run: `cargo test --workspace 2>&1 | tail -100`
Expected: 全部測試通過,包含這個 phase 新增的所有測試(`versioning_tests`、`db_tests` 8 個、`config_tests` 2 個、`cli_parse_tests` 5 個)以及 Phase 1 留下的既有測試(`atomic_install_tests`、`scope_tests`、`dpm-core` 既有測試)。

- [ ] **Step 6: Commit**

```bash
git add crates/dpm-server/src/action.rs crates/dpm-server/src/cli_parse.rs \
  crates/dpm-server/RepoInfo.json
git commit -m "$(cat <<'EOF'
feat(dpm-server): publish multi-version package index

fix_add now appends a new PackageVersionInfo via
RepoInfo::add_package_version instead of overwriting a single-version
entry — publishing the same version twice is now a hard error at the
data-model layer (Task 1), not just a convention. fix_del gained an
optional `version` argument: required once a package has more than
one published version, auto-resolved when there's exactly one.
repo_init's full-rebuild path follows the same shape.

This completes Phase 2 (multi-source/namespace) of the multi-source
registry design — `cargo build --workspace` / `cargo test --workspace`
are green end to end for the first time in this plan.

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>
EOF
)"
```
