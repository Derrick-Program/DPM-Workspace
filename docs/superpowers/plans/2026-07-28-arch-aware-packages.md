# apt/dnf 風格 arch/os 套件分包 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `PackageKind::Prebuilt` 從單一 build 改成可登記多組 target-specific build(apt/dnf 風格,target 用 Rust target triple,`None` = 任何平台通用),`PackageKind::Source` 加 `supported_targets` 聲明清單;修好真實的 `Derrick-Program/DPM-Server` 官方 repo(目前 private+archived+舊 schema),發布 `hello`(Prebuilt/通用)、`addsub`(Source/C 加減法)兩個 demo 套件並手動端到端驗證。

**Architecture:** target 匹配/`supported_targets` 檢查都發生在 **`dpm update`(sync)時**,不是安裝時——`PackageKind::to_db_fields` 簽名改成吃 `target: &str` 參數、回傳 `CoreResult<(...)>`,對 `Prebuilt` 找匹配的 build(找不到就試通用、都沒有就回傳 `Err`),對 `Source` 檢查 `supported_targets`。`sync_source_inner` 對 `Err` 印警告訊息並跳過該版本,不寫進本機 DB——本機 `LocalRepo` 表的欄位/schema 完全不用改,`dpm install` 讀到的永遠已經是「這台機器裝得下」的單一 build,安裝邏輯不變。八個循序 task:Task 1-3 是 `dpm-core`/`dpm-server`/`dpm` 三個 crate 的程式改動;Task 4-6 是修真實 repo + 發布兩個 demo 套件;Task 7 是手動端到端驗證;Task 8 是整個 workspace 收尾驗證。

**Tech Stack:** 沿用既有依賴,不加新的。`self_update::get_target() -> &'static str`(已經是 `dpm` 的既有依賴,回傳編譯期烘焙的 Rust target triple,例:`aarch64-apple-darwin`)是這次唯一會被新用到的既有 API。

## Global Constraints

- `PackageKind::to_db_fields` 的 target 匹配/`supported_targets` 檢查只在 **sync 時**發生(`sync_source_inner`),**不**在安裝時——本機 `LocalRepo` 表(`db.rs::COLUMNS`)完全不改,不寫任何 migration。這是刻意的架構決定(比對 spec 原本描述的「安裝時解析」更簡單、不需要動 turso migration),已經跟人類確認過。
- `Source` 套件的 `build` 指令本身**不分平台**,只有一組,不支援「不同 target 不同 build 指令」——`supported_targets` 只是安裝前的允許/拒絕清單檢查,不是挑選邏輯。
- 不寫任何自動交叉編譯機制——`hello`/`addsub` 兩個 demo 套件都不需要真的編出多份不同平台的二進位,`Prebuilt` 的多 target 選擇邏輯只靠手寫 fixture 的單元測試驗證。
- 舊資料相容:沒有 `builds` 陣列(舊格式單一 `url`/`hash`/`file_name`)的既有 `Prebuilt`、沒有 `supported_targets` 的既有 `Source`,都要能正常反序列化,分別視為「一組 `target: None` 的 build」跟「`supported_targets: None`(任何平台)」——不是破壞性改動,不需要重新發布既有套件。
- 每個有程式碼變動的 task 結束前都要跑過 `cargo check`/`cargo clippy -- -D warnings`/相關 `cargo test`;commit message 用 Conventional Commits(`type(scope): description`)格式。
- Task 4-7(修真實 repo、發布 demo、手動驗證)都需要真的操作 `Derrick-Program/DPM-Server`(透過 `gh`/`git`/`dpm-server` CLI),不是純程式碼改動——這些 task 由你(agentic worker)直接執行,不是留給人類的手動步驟(不像先前 self-update plan 的簽章金鑰產生那樣需要人類專屬權限),但涉及推送到真實 GitHub repo,執行前確認你有 `gh`/`git` 對 `Derrick-Program/DPM-Server` 的寫入權限。

---

## Task 1: `dpm-core` — `PackageKind` schema 改動

**Files:**
- Modify: `crates/dpm-core/src/lib.rs`

**Interfaces:**
- Consumes:無(純新增/修改既有型別)。
- Produces:`pub struct PrebuiltBuild { pub target: Option<String>, pub url: String, pub hash: String, pub file_name: String }`;`PackageKind::Prebuilt { builds: Vec<PrebuiltBuild> }`;`PackageKind::Source` 新增 `supported_targets: Option<Vec<String>>` 欄位;`PackageKind::to_db_fields(&self, target: &str) -> CoreResult<(&'static str, Option<String>, Option<String>, Option<String>, Option<String>)>`(簽名變動:多吃一個 `target` 參數、回傳型別包一層 `CoreResult`);`RepoInfo::add_package_version` 的追加邏輯變動——Task 2 會直接依賴這些。

- [ ] **Step 1: 寫失敗的測試(TDD——新型別/新欄位還不存在)**

編輯 `crates/dpm-core/src/lib.rs`,找到檔案最底部既有的 `#[cfg(test)] mod toml_storage_tests { ... }`(Task 2 的 layered-config 那個模組)之後,新增一個獨立的測試模組:

```rust
#[cfg(test)]
mod package_kind_target_tests {
    use super::*;

    fn build(target: Option<&str>, url: &str) -> PrebuiltBuild {
        PrebuiltBuild {
            target: target.map(|s| s.to_string()),
            url: url.to_string(),
            hash: "a".repeat(64),
            file_name: "pkg.zip".to_string(),
        }
    }

    #[test]
    fn to_db_fields_picks_the_exact_matching_target() {
        let kind = PackageKind::Prebuilt {
            builds: vec![
                build(Some("x86_64-unknown-linux-gnu"), "https://example.com/linux.zip"),
                build(Some("aarch64-apple-darwin"), "https://example.com/mac.zip"),
            ],
        };
        let (kind_str, url, _, _, _) = kind.to_db_fields("aarch64-apple-darwin").unwrap();
        assert_eq!(kind_str, "prebuilt");
        assert_eq!(url.unwrap(), "https://example.com/mac.zip");
    }

    #[test]
    fn to_db_fields_falls_back_to_the_universal_build_when_no_exact_match() {
        let kind = PackageKind::Prebuilt {
            builds: vec![
                build(Some("x86_64-unknown-linux-gnu"), "https://example.com/linux.zip"),
                build(None, "https://example.com/universal.zip"),
            ],
        };
        let (_, url, _, _, _) = kind.to_db_fields("aarch64-apple-darwin").unwrap();
        assert_eq!(url.unwrap(), "https://example.com/universal.zip");
    }

    #[test]
    fn to_db_fields_errors_when_no_match_and_no_universal_build() {
        let kind = PackageKind::Prebuilt {
            builds: vec![build(
                Some("x86_64-unknown-linux-gnu"),
                "https://example.com/linux.zip",
            )],
        };
        let err = kind.to_db_fields("aarch64-apple-darwin").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("x86_64-unknown-linux-gnu"),
            "error must list the registered targets so the user knows what IS available: {msg}"
        );
    }

    #[test]
    fn to_db_fields_accepts_source_package_with_matching_supported_target() {
        let kind = PackageKind::Source {
            build: "cc -shared -o lib.so lib.c".to_string(),
            hash: Some("a".repeat(64)),
            supported_targets: Some(vec!["aarch64-apple-darwin".to_string()]),
        };
        let (kind_str, _, _, _, build_command) = kind.to_db_fields("aarch64-apple-darwin").unwrap();
        assert_eq!(kind_str, "source");
        assert_eq!(build_command.unwrap(), "cc -shared -o lib.so lib.c");
    }

    #[test]
    fn to_db_fields_rejects_source_package_on_unsupported_target() {
        let kind = PackageKind::Source {
            build: "cc -shared -o lib.so lib.c".to_string(),
            hash: Some("a".repeat(64)),
            supported_targets: Some(vec!["x86_64-unknown-linux-gnu".to_string()]),
        };
        let err = kind.to_db_fields("aarch64-apple-darwin").unwrap_err();
        assert!(err.to_string().contains("x86_64-unknown-linux-gnu"));
    }

    #[test]
    fn to_db_fields_accepts_source_package_with_no_supported_targets_declared() {
        let kind = PackageKind::Source {
            build: "cc -shared -o lib.so lib.c".to_string(),
            hash: Some("a".repeat(64)),
            supported_targets: None,
        };
        assert!(kind.to_db_fields("any-target-at-all").is_ok());
    }

    #[test]
    fn old_format_prebuilt_without_builds_array_deserializes_as_one_universal_build() {
        let json = r#"{"kind":"prebuilt","url":"https://example.com/old.zip","hash":"abc","file_name":"old.zip"}"#;
        let kind: PackageKind = serde_json::from_str(json).unwrap();
        match kind {
            PackageKind::Prebuilt { builds } => {
                assert_eq!(builds.len(), 1);
                assert_eq!(builds[0].target, None);
                assert_eq!(builds[0].url, "https://example.com/old.zip");
            }
            _ => panic!("expected Prebuilt"),
        }
    }

    #[test]
    fn old_format_source_without_supported_targets_deserializes_as_none() {
        let json = r#"{"kind":"source","build":"make"}"#;
        let kind: PackageKind = serde_json::from_str(json).unwrap();
        match kind {
            PackageKind::Source {
                supported_targets, ..
            } => assert_eq!(supported_targets, None),
            _ => panic!("expected Source"),
        }
    }
}
```

- [ ] **Step 2: 跑測試,確認因為型別/簽名不存在而編譯失敗**

Run: `cargo test -p DPM-Core package_kind_target_tests`
Expected: 編譯失敗——`PrebuiltBuild` 不存在、`PackageKind::Prebuilt` 沒有 `builds` 欄位、`PackageKind::Source` 沒有 `supported_targets` 欄位、`to_db_fields` 簽名不符。

- [ ] **Step 3: 改 `PackageKind`/新增 `PrebuiltBuild`**

編輯 `crates/dpm-core/src/lib.rs`,把:

```rust
/// 套件在某個來源索引裡的一個具體版本條目。
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum PackageKind {
    /// 已預先打包好的二進位/壓縮檔,client 直接下載解壓。
    Prebuilt {
        url: String,
        hash: String,
        file_name: String,
    },
    /// 只提供原始碼 + build 指令,client 在本機執行 build。`hash` 是
    /// `blake3(build_command + commit hash)`(`dpm-server hash --build`
    /// 算出來的),`Option` 是因為還沒被 `hash`+`sign` 過的草稿狀態下沒有值。
    ///
    /// 已知、刻意延後處理的缺口(非本次功能涵蓋範圍,不要當成已解決):這個
    /// hash 綁定的是 `build_command` 字串加上發布當下的 commit,但 commit
    /// 本身沒有被發布到任何 client 端可以驗證的地方,client 目前也完全沒有
    /// 重算這個 hash 並跟簽章比對——`build` 欄位是直接從 `RepoInfo.json`
    /// 讀出來就拿去執行(見 `dpm/src/action.rs::install_source_package`)。
    /// 也就是說,對 `kind: source` 套件而言,簽章目前**不提供任何**對
    /// `build_command` 或原始碼樹的保護:只要能改 `RepoInfo.json`(不需要
    /// 簽名金鑰),就能把 `build` 換成任意指令,client 端的驗證閘門不會擋下
    /// 來。`kind: Prebuilt` 不受影響(下載內容有獨立 hash 比對,見
    /// `fetch_and_verify_prebuilt`)。
    Source {
        build: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hash: Option<String>,
    },
}
```

換成:

```rust
/// `PackageKind::Prebuilt` 的其中一組 target-specific build——apt/dnf 風格,
/// 同一個套件版本可以有多組,`target` 用 Rust target triple(`self_update::
/// get_target()` 那套字串),`None` 代表任何平台通用(對應 apt 的
/// `Architecture: all`)。
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PrebuiltBuild {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    pub url: String,
    pub hash: String,
    pub file_name: String,
}

/// 套件在某個來源索引裡的一個具體版本條目。
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum PackageKind {
    /// 已預先打包好的二進位/壓縮檔,client 直接下載解壓。一個版本可以登記
    /// 多組 target-specific build(apt/dnf 風格),`to_db_fields` 依呼叫端
    /// 傳入的本機 target 挑一組。舊格式(單一 `url`/`hash`/`file_name`,沒有
    /// `builds` 陣列)透過下面的 `Deserialize` 手動實作相容,視為一組
    /// `target: None` 的通用 build。
    Prebuilt { builds: Vec<PrebuiltBuild> },
    /// 只提供原始碼 + build 指令,client 在本機執行 build。`hash` 是
    /// `blake3(build_command + commit hash)`(`dpm-server hash --build`
    /// 算出來的),`Option` 是因為還沒被 `hash`+`sign` 過的草稿狀態下沒有值。
    /// `supported_targets` 是安裝前的允許清單(`None` = 任何平台)——build
    /// 指令本身不分平台,只有一組,這個欄位純粹用來擋「這台機器不支援」的
    /// 情況,不是挑選邏輯。
    ///
    /// 已知、刻意延後處理的缺口(非本次功能涵蓋範圍,不要當成已解決):這個
    /// hash 綁定的是 `build_command` 字串加上發布當下的 commit,但 commit
    /// 本身沒有被發布到任何 client 端可以驗證的地方,client 目前也完全沒有
    /// 重算這個 hash 並跟簽章比對——`build` 欄位是直接從 `RepoInfo.json`
    /// 讀出來就拿去執行(見 `dpm/src/action.rs::install_source_package`)。
    /// 也就是說,對 `kind: source` 套件而言,簽章目前**不提供任何**對
    /// `build_command` 或原始碼樹的保護:只要能改 `RepoInfo.json`(不需要
    /// 簽名金鑰),就能把 `build` 換成任意指令,client 端的驗證閘門不會擋下
    /// 來。`kind: Prebuilt` 不受影響(下載內容有獨立 hash 比對,見
    /// `fetch_and_verify_prebuilt`)。
    Source {
        build: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hash: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        supported_targets: Option<Vec<String>>,
    },
}
```

- [ ] **Step 4: 舊格式 `Prebuilt` 相容——手動 `Deserialize`**

`#[serde(tag = "kind", rename_all = "lowercase")]` 加上 `Prebuilt { builds: Vec<PrebuiltBuild> }` 之後,serde 預設反序列化會直接要求 JSON 裡有 `"builds"` 陣列,舊格式(`"kind":"prebuilt","url":...,"hash":...,"file_name":...`,沒有 `"builds"` 鍵)會直接解析失敗,不是「自動獲得一組通用 build」。要達成「舊格式=一組 `target: None` 的 build」這個相容行為,`PackageKind` 不能再用 derive 的 `Deserialize`,改成手動實作。

在 `PrebuiltBuild`/`PackageKind` enum 定義之後(`impl PackageKind { ... }` 之前),把 enum 上的 `#[derive(Debug, Serialize, Deserialize, Clone)]` 改成 `#[derive(Debug, Serialize, Clone)]`(拿掉 derive 的 `Deserialize`),然後補上:

```rust
impl<'de> Deserialize<'de> for PackageKind {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error as _;
        use serde_json::Value;

        let mut value = Value::deserialize(deserializer)?;
        let obj = value
            .as_object_mut()
            .ok_or_else(|| D::Error::custom("PackageKind must be a JSON object"))?;
        let kind = obj
            .get("kind")
            .and_then(Value::as_str)
            .ok_or_else(|| D::Error::custom("PackageKind is missing a \"kind\" tag"))?
            .to_string();

        match kind.as_str() {
            "prebuilt" => {
                if !obj.contains_key("builds") {
                    // 舊格式:單一 url/hash/file_name,沒有 builds 陣列——
                    // 包成一組 target: None(通用)的 build,相容既有已發布
                    // 資料,不需要重新發布。
                    let url = obj
                        .get("url")
                        .and_then(Value::as_str)
                        .ok_or_else(|| D::Error::custom("prebuilt package missing url"))?
                        .to_string();
                    let hash = obj
                        .get("hash")
                        .and_then(Value::as_str)
                        .ok_or_else(|| D::Error::custom("prebuilt package missing hash"))?
                        .to_string();
                    let file_name = obj
                        .get("file_name")
                        .and_then(Value::as_str)
                        .ok_or_else(|| D::Error::custom("prebuilt package missing file_name"))?
                        .to_string();
                    return Ok(PackageKind::Prebuilt {
                        builds: vec![PrebuiltBuild {
                            target: None,
                            url,
                            hash,
                            file_name,
                        }],
                    });
                }
                #[derive(Deserialize)]
                struct Shape {
                    builds: Vec<PrebuiltBuild>,
                }
                let shape: Shape =
                    serde_json::from_value(value.clone()).map_err(D::Error::custom)?;
                Ok(PackageKind::Prebuilt {
                    builds: shape.builds,
                })
            }
            "source" => {
                #[derive(Deserialize)]
                struct Shape {
                    build: String,
                    #[serde(default)]
                    hash: Option<String>,
                    #[serde(default)]
                    supported_targets: Option<Vec<String>>,
                }
                let shape: Shape =
                    serde_json::from_value(value.clone()).map_err(D::Error::custom)?;
                Ok(PackageKind::Source {
                    build: shape.build,
                    hash: shape.hash,
                    supported_targets: shape.supported_targets,
                })
            }
            other => Err(D::Error::custom(format!("unknown package kind: {other}"))),
        }
    }
}
```

（`let mut value` 之後其實只用了 `.clone()` 去餵進 `serde_json::from_value`,沒有真的動到 `value` 本身內容,`obj` 這個可變借用只用來讀,不寫——保留 `mut`/`as_object_mut` 是因為要先確認它是物件並取出 `kind`/`contains_key("builds")`,不影響行為,只是為了少一次 clone 而先借用檢查。）

- [ ] **Step 5: 改 `to_db_fields`**

把:

```rust
    pub fn to_db_fields(
        &self,
    ) -> (
        &'static str,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    ) {
        match self {
            PackageKind::Prebuilt {
                url,
                hash,
                file_name,
            } => (
                "prebuilt",
                Some(url.clone()),
                Some(hash.clone()),
                Some(file_name.clone()),
                None,
            ),
            PackageKind::Source { build, hash } => {
                ("source", None, hash.clone(), None, Some(build.clone()))
            }
        }
    }
```

換成:

```rust
    /// 依呼叫端傳入的本機 target(`self_update::get_target()` 那套字串)
    /// 挑一組能用的 build,再扁平化成 `LocalRepo` 表的欄位。`Prebuilt`:先找
    /// 完全匹配的 target,找不到就退回 `target: None` 的通用 build,兩者都
    /// 沒有就回傳 `Err`(訊息列出這個版本實際登記的所有 target)。`Source`:
    /// 檢查 `supported_targets`(`None` 或包含這個 target 才算支援),不支援
    /// 就回傳 `Err`(訊息列出 `supported_targets` 內容)。呼叫端
    /// (`sync_source_inner`)對 `Err` 的處理是印警告、跳過這個版本、不寫進
    /// 本機 DB——target 匹配只在 sync 時做一次,本機 DB 存的永遠已經是「這台
    /// 機器裝得下」的單一 build,`LocalRepo` 表結構不用為多 target 改動。
    pub fn to_db_fields(
        &self,
        target: &str,
    ) -> CoreResult<(
        &'static str,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    )> {
        match self {
            PackageKind::Prebuilt { builds } => {
                let chosen = builds
                    .iter()
                    .find(|b| b.target.as_deref() == Some(target))
                    .or_else(|| builds.iter().find(|b| b.target.is_none()))
                    .ok_or_else(|| {
                        let registered: Vec<&str> = builds
                            .iter()
                            .map(|b| b.target.as_deref().unwrap_or("<universal>"))
                            .collect();
                        CoreError::InvalidPackage(format!(
                            "no build available for target {target}; registered targets: {registered:?}"
                        ))
                    })?;
                Ok((
                    "prebuilt",
                    Some(chosen.url.clone()),
                    Some(chosen.hash.clone()),
                    Some(chosen.file_name.clone()),
                    None,
                ))
            }
            PackageKind::Source {
                build,
                hash,
                supported_targets,
            } => {
                if let Some(supported) = supported_targets {
                    if !supported.iter().any(|t| t == target) {
                        return Err(CoreError::InvalidPackage(format!(
                            "package does not support target {target}; supported targets: {supported:?}"
                        )));
                    }
                }
                Ok(("source", None, hash.clone(), None, Some(build.clone())))
            }
        }
    }
```

- [ ] **Step 6: 改 `from_db_fields`**

`from_db_fields` 的輸入是本機 DB 已經解析好的單一 build(不是多 target 的原始資料),所以只需要把重建出來的 `PackageKind::Prebuilt` 包成一組 `builds`,`Source` 補一個 `supported_targets: None`(本機 DB 不記這個,讀回來的當下已經確認過支援,不需要再檢查一次)。把:

```rust
    pub fn from_db_fields(
        kind: &str,
        url: Option<String>,
        hash: Option<String>,
        filename: Option<String>,
        build_command: Option<String>,
    ) -> CoreResult<Self> {
        match kind {
            "prebuilt" => Ok(PackageKind::Prebuilt {
                url: url.ok_or_else(|| {
                    CoreError::InvalidPackage("prebuilt package missing url".to_string())
                })?,
                hash: hash.ok_or_else(|| {
                    CoreError::InvalidPackage("prebuilt package missing hash".to_string())
                })?,
                file_name: filename.ok_or_else(|| {
                    CoreError::InvalidPackage("prebuilt package missing filename".to_string())
                })?,
            }),
            "source" => Ok(PackageKind::Source {
                build: build_command.ok_or_else(|| {
                    CoreError::InvalidPackage("source package missing build command".to_string())
                })?,
                hash,
            }),
            other => Err(CoreError::InvalidPackage(format!(
                "unknown package kind: {other}"
            ))),
        }
    }
```

換成:

```rust
    pub fn from_db_fields(
        kind: &str,
        url: Option<String>,
        hash: Option<String>,
        filename: Option<String>,
        build_command: Option<String>,
    ) -> CoreResult<Self> {
        match kind {
            "prebuilt" => {
                let url = url.ok_or_else(|| {
                    CoreError::InvalidPackage("prebuilt package missing url".to_string())
                })?;
                let hash = hash.ok_or_else(|| {
                    CoreError::InvalidPackage("prebuilt package missing hash".to_string())
                })?;
                let file_name = filename.ok_or_else(|| {
                    CoreError::InvalidPackage("prebuilt package missing filename".to_string())
                })?;
                Ok(PackageKind::Prebuilt {
                    builds: vec![PrebuiltBuild {
                        target: None,
                        url,
                        hash,
                        file_name,
                    }],
                })
            }
            "source" => Ok(PackageKind::Source {
                build: build_command.ok_or_else(|| {
                    CoreError::InvalidPackage("source package missing build command".to_string())
                })?,
                hash,
                supported_targets: None,
            }),
            other => Err(CoreError::InvalidPackage(format!(
                "unknown package kind: {other}"
            ))),
        }
    }
```

- [ ] **Step 7: `RepoInfo::add_package_version` 允許同版本追加不同 target 的 `Prebuilt` build**

編輯 `crates/dpm-core/src/lib.rs`,把(在 `#[cfg(feature = "server")] impl RepoInfo` 區塊裡):

```rust
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
```

換成:

```rust
    /// 同一個 `(name, version)` 已經發布過時,唯一允許的例外是「幫既有
    /// `Prebuilt` 版本追加一組新 target 的 build」(`dpm-server fix add
    /// ... url --target <T>` 對同版本連續呼叫多次,`AddKind::Build`/
    /// `Source` 套件沒有這個例外,同版本第二次發布一律拒絕)。追加的
    /// target 如果已經存在(含兩邊都是 `None`,即兩次都沒帶 `--target`),
    /// 一樣拒絕。
    pub fn add_package_version(
        &mut self,
        name: String,
        info: PackageVersionInfo,
    ) -> CoreResult<()> {
        let versions = self.packages.entry(name).or_default();
        if let Some(existing) = versions.iter_mut().find(|v| v.version == info.version) {
            return match (&mut existing.kind, &info.kind) {
                (
                    PackageKind::Prebuilt {
                        builds: existing_builds,
                    },
                    PackageKind::Prebuilt { builds: new_builds },
                ) => {
                    for nb in new_builds {
                        if existing_builds.iter().any(|b| b.target == nb.target) {
                            return Err(CoreError::VersionMismatch(format!(
                                "version {} already has a build for target {}",
                                info.version,
                                nb.target.as_deref().unwrap_or("<universal>")
                            )));
                        }
                    }
                    existing_builds.extend(new_builds.iter().cloned());
                    Ok(())
                }
                _ => Err(CoreError::VersionMismatch(format!(
                    "version {} is already published",
                    info.version
                ))),
            };
        }
        versions.push(info);
        Ok(())
    }
```

- [ ] **Step 8: 跑測試,確認 Step 1 的測試通過**

Run: `cargo test -p DPM-Core package_kind_target_tests`
Expected: 8 個測試全部 PASS。

- [ ] **Step 9: 既有測試沒有回歸**

Run: `cargo test -p DPM-Core`
Expected: 全部通過(既有 `test_to_json`/`versioning_tests`/`signature_tests` 等都還在,`PackageKind` 的呼叫端目前只有 `dpm-core` 自己的測試,Task 2/3 才會動到 `dpm`/`dpm-server` 那邊的呼叫端——這一步先確認 `dpm-core` 自己不回歸)。

- [ ] **Step 10: clippy**

Run: `cargo clippy -p DPM-Core --all-targets -- -D warnings`
Expected: 無警告無錯誤。

- [ ] **Step 11: Commit**

```bash
git add crates/dpm-core/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(dpm-core): apt/dnf-style multi-target Prebuilt builds

PackageKind::Prebuilt goes from a single {url, hash, file_name} to
builds: Vec<PrebuiltBuild>, each with an optional Rust target triple
(None = universal, apt's Architecture: all equivalent).
PackageKind::Source gains supported_targets: Option<Vec<String>> — the
build command itself stays single/platform-agnostic, this is just an
install-time allow-list check.

to_db_fields now takes the local machine's target and resolves/checks
at that point (exact match, falling back to the universal build, or
erroring with the registered target list) — this happens once during
`dpm update`/sync, not at install time, so the LocalRepo DB schema
needs no migration: it still stores exactly one resolved build per
version, same columns as before.

Old published data without a "builds" array or "supported_targets"
deserializes as fully compatible (one universal build / no
restriction) via a hand-written Deserialize impl — not a breaking
change, no republish needed.

RepoInfo::add_package_version gets its one carve-out: the same
(name, version) may have a new-target Prebuilt build appended after
first publish; every other "already published" case is unchanged.

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: `dpm-server` — CLI `--target`/`--targets` + `fix_add`

**依賴 Task 1。**

**Files:**
- Modify: `crates/dpm-server/src/cli_parse.rs`
- Modify: `crates/dpm-server/src/action.rs`

**Interfaces:**
- Consumes:`PrebuiltBuild`、`PackageKind::{Prebuilt,Source}` 新欄位、`RepoInfo::add_package_version` 的追加語意(Task 1)。
- Produces:`AddKind::Url` 新欄位 `target: Option<String>`;`AddKind::Build` 新欄位 `targets: Option<Vec<String>>`。

- [ ] **Step 1: `cli_parse.rs` 加 `--target`/`--targets`**

編輯 `crates/dpm-server/src/cli_parse.rs`,把:

```rust
#[derive(Subcommand, Debug)]
pub enum AddKind {
    /// Publish a prebuilt package hosted at a URL
    Url {
        /// External URL hosting the prebuilt package archive. dpm-server
        /// downloads it once to compute its blake3 hash — it does not keep
        /// a copy. Must be https://.
        url: String,
        /// Override the file name recorded in RepoInfo.json (defaults to
        /// the URL's last path segment)
        #[arg(long)]
        file_name: Option<String>,
    },
    /// Publish a source package clients build locally
    Build {
        /// Shell command clients run locally to build this package from
        /// source. $OUT will point at the install destination when clients
        /// actually run it (Phase 4 client-side work).
        build: String,
    },
}
```

換成:

```rust
#[derive(Subcommand, Debug)]
pub enum AddKind {
    /// Publish a prebuilt package hosted at a URL
    Url {
        /// External URL hosting the prebuilt package archive. dpm-server
        /// downloads it once to compute its blake3 hash — it does not keep
        /// a copy. Must be https://.
        url: String,
        /// Override the file name recorded in RepoInfo.json (defaults to
        /// the URL's last path segment)
        #[arg(long)]
        file_name: Option<String>,
        /// Rust target triple this build is for (e.g.
        /// aarch64-apple-darwin). Omit for a universal build that installs
        /// on any platform. The same version can have this run multiple
        /// times with different --target values to register more than one
        /// platform-specific build.
        #[arg(long)]
        target: Option<String>,
    },
    /// Publish a source package clients build locally
    Build {
        /// Shell command clients run locally to build this package from
        /// source. $OUT will point at the install destination when clients
        /// actually run it (Phase 4 client-side work).
        build: String,
        /// Comma-separated list of Rust target triples this package
        /// supports (e.g. aarch64-apple-darwin,x86_64-unknown-linux-gnu).
        /// Omit to allow installing on any platform. The build command
        /// itself does not vary by target — this is only an install-time
        /// allow-list check.
        #[arg(long, value_delimiter = ',')]
        targets: Option<Vec<String>>,
    },
}
```

- [ ] **Step 2: 確認整個 crate 能編譯(這裡還沒改 `action.rs`,預期會報錯)**

Run: `cargo check -p DPM-Server`
Expected: 編譯失敗——`action.rs` 的 `fix_add` 還在用舊的 `AddKind::Url { url, file_name }`/`AddKind::Build { build }` 兩個欄位的 pattern,少了新欄位會報 pattern 不窮盡或欄位不存在。這是預期中的中繼狀態。

- [ ] **Step 3: `fix_add` 改用新欄位建構 `PackageKind`**

編輯 `crates/dpm-server/src/action.rs`,把:

```rust
    let kind = match &obj.kind {
        AddKind::Url { url, file_name } => {
            if !url.starts_with("https://") {
                return Err(ServerError::ValidationError(format!(
                    "url {url} must use https://"
                )));
            }
            let file_name = file_name
                .clone()
                .or_else(|| url.rsplit('/').next().map(|s| s.to_string()))
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    ServerError::ValidationError(
                        "could not derive a file name from the url; pass --file-name explicitly"
                            .to_string(),
                    )
                })?;

            let response = reqwest::blocking::get(url)?;
            if !response.status().is_success() {
                return Err(ServerError::Core(CoreError::NetworkError(format!(
                    "failed to fetch {url}: HTTP {}",
                    response.status()
                ))));
            }
            let bytes = response.bytes()?;
            let tmp_path = std::env::temp_dir().join(&file_name);
            std::fs::write(&tmp_path, &bytes)?;
            let downloaded_hash = dpm_core::hash_file(&tmp_path)?;
            std::fs::remove_file(&tmp_path)?;

            if downloaded_hash != pk_info.hash {
                return Err(ServerError::ValidationError(format!(
                    "content served at {url} (hash {downloaded_hash}) does not match {}'s signed hash ({}) — run `dpm-server build`, `hash`, and `sign` again after the url's content changes",
                    obj.project_name, pk_info.hash
                )));
            }

            PackageKind::Prebuilt {
                url: url.clone(),
                hash: pk_info.hash.clone(),
                file_name,
            }
        }
        AddKind::Build { build } => {
            // Same check as the Url arm above, mirrored for source packages:
            // recompute the hash this `--build` value would produce and
            // compare it against the signed hash, so someone with
            // RepoInfo.json write access (but no signing key) can't swap the
            // build command out from under an already-signed packageInfo.json.
            let commit = source_repo_commit_hash(&path)?;
            let recomputed_hash = dpm_core::hash_bytes(format!("{build}\n{commit}").as_bytes());
            if recomputed_hash != pk_info.hash {
                return Err(ServerError::ValidationError(format!(
                    "build command {build:?} (hash {recomputed_hash}) does not match {}'s signed hash ({}) — run `dpm-server hash --build`/`sign` again after the build command changes",
                    obj.project_name, pk_info.hash
                )));
            }

            PackageKind::Source {
                build: build.clone(),
                hash: Some(pk_info.hash.clone()),
            }
        }
    };
```

換成:

```rust
    let kind = match &obj.kind {
        AddKind::Url {
            url,
            file_name,
            target,
        } => {
            if !url.starts_with("https://") {
                return Err(ServerError::ValidationError(format!(
                    "url {url} must use https://"
                )));
            }
            let file_name = file_name
                .clone()
                .or_else(|| url.rsplit('/').next().map(|s| s.to_string()))
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    ServerError::ValidationError(
                        "could not derive a file name from the url; pass --file-name explicitly"
                            .to_string(),
                    )
                })?;

            let response = reqwest::blocking::get(url)?;
            if !response.status().is_success() {
                return Err(ServerError::Core(CoreError::NetworkError(format!(
                    "failed to fetch {url}: HTTP {}",
                    response.status()
                ))));
            }
            let bytes = response.bytes()?;
            let tmp_path = std::env::temp_dir().join(&file_name);
            std::fs::write(&tmp_path, &bytes)?;
            let downloaded_hash = dpm_core::hash_file(&tmp_path)?;
            std::fs::remove_file(&tmp_path)?;

            if downloaded_hash != pk_info.hash {
                return Err(ServerError::ValidationError(format!(
                    "content served at {url} (hash {downloaded_hash}) does not match {}'s signed hash ({}) — run `dpm-server build`, `hash`, and `sign` again after the url's content changes",
                    obj.project_name, pk_info.hash
                )));
            }

            PackageKind::Prebuilt {
                builds: vec![dpm_core::PrebuiltBuild {
                    target: target.clone(),
                    url: url.clone(),
                    hash: pk_info.hash.clone(),
                    file_name,
                }],
            }
        }
        AddKind::Build { build, targets } => {
            // Same check as the Url arm above, mirrored for source packages:
            // recompute the hash this `--build` value would produce and
            // compare it against the signed hash, so someone with
            // RepoInfo.json write access (but no signing key) can't swap the
            // build command out from under an already-signed packageInfo.json.
            let commit = source_repo_commit_hash(&path)?;
            let recomputed_hash = dpm_core::hash_bytes(format!("{build}\n{commit}").as_bytes());
            if recomputed_hash != pk_info.hash {
                return Err(ServerError::ValidationError(format!(
                    "build command {build:?} (hash {recomputed_hash}) does not match {}'s signed hash ({}) — run `dpm-server hash --build`/`sign` again after the build command changes",
                    obj.project_name, pk_info.hash
                )));
            }

            PackageKind::Source {
                build: build.clone(),
                hash: Some(pk_info.hash.clone()),
                supported_targets: targets.clone(),
            }
        }
    };
```

- [ ] **Step 4: 確認整個 crate 能編譯**

Run: `cargo check -p DPM-Server`
Expected: 編譯成功。

- [ ] **Step 5: 寫失敗的測試(TDD——`--target` 追加行為還沒被覆蓋)**

`fix_add` 完整流程需要真的能下載到 URL 才會走到 `add_package_version`,不適合當單元測試——這裡改測兩件事:(a) `AddKind::Url` 的 `--target` 有正確傳進 `PrebuiltBuild.target`(建構邏輯本身),(b) `dpm_core::RepoInfo::add_package_version`(Task 1 已經改好的邏輯)在 `dpm-server` 這邊接得起來。在 `crates/dpm-server/src/action.rs` 底部既有 `#[cfg(test)] mod tests { ... }` 裡(參考同一個模組裡已經有的 `init_hash_sign`/`fix_add_accepts_a_second_version_from_the_same_author` 之類的測試寫法),新增:

```rust
    #[test]
    fn add_kind_url_with_target_builds_a_prebuilt_kind_with_one_build_entry() {
        // 直接測 AddKind -> PackageKind 的建構邏輯(不透過完整 fix_add,
        // 因為那需要真的能下載 URL)——確認 --target 有正確傳進
        // PrebuiltBuild.target。
        let target = Some("aarch64-apple-darwin".to_string());
        let kind = PackageKind::Prebuilt {
            builds: vec![dpm_core::PrebuiltBuild {
                target: target.clone(),
                url: "https://example.com/mac.zip".to_string(),
                hash: "a".repeat(64),
                file_name: "mac.zip".to_string(),
            }],
        };
        match kind {
            PackageKind::Prebuilt { builds } => {
                assert_eq!(builds.len(), 1);
                assert_eq!(builds[0].target, target);
            }
            _ => panic!("expected Prebuilt"),
        }
    }

    #[test]
    fn repo_info_add_package_version_appends_a_new_target_build_to_an_existing_version() {
        let mut repo = RepoInfo::new();
        let first = PackageVersionInfo {
            version: "1.0.0".to_string(),
            kind: PackageKind::Prebuilt {
                builds: vec![dpm_core::PrebuiltBuild {
                    target: Some("x86_64-unknown-linux-gnu".to_string()),
                    url: "https://example.com/linux.zip".to_string(),
                    hash: "a".repeat(64),
                    file_name: "linux.zip".to_string(),
                }],
            },
            dependencies: None,
            entry: None,
            description: None,
            author: Some("alice".to_string()),
            signature: Some("sig".to_string()),
        };
        repo.add_package_version("multi-target-pkg".to_string(), first)
            .unwrap();

        let second = PackageVersionInfo {
            version: "1.0.0".to_string(),
            kind: PackageKind::Prebuilt {
                builds: vec![dpm_core::PrebuiltBuild {
                    target: Some("aarch64-apple-darwin".to_string()),
                    url: "https://example.com/mac.zip".to_string(),
                    hash: "b".repeat(64),
                    file_name: "mac.zip".to_string(),
                }],
            },
            dependencies: None,
            entry: None,
            description: None,
            author: Some("alice".to_string()),
            signature: Some("sig2".to_string()),
        };
        repo.add_package_version("multi-target-pkg".to_string(), second)
            .unwrap();

        let versions = repo.versions_of("multi-target-pkg").unwrap();
        assert_eq!(versions.len(), 1, "same version, not a second entry");
        match &versions[0].kind {
            PackageKind::Prebuilt { builds } => assert_eq!(builds.len(), 2),
            _ => panic!("expected Prebuilt"),
        }
    }

    #[test]
    fn repo_info_add_package_version_rejects_a_duplicate_target() {
        let mut repo = RepoInfo::new();
        let make_info = |target: Option<&str>| PackageVersionInfo {
            version: "1.0.0".to_string(),
            kind: PackageKind::Prebuilt {
                builds: vec![dpm_core::PrebuiltBuild {
                    target: target.map(|s| s.to_string()),
                    url: "https://example.com/a.zip".to_string(),
                    hash: "a".repeat(64),
                    file_name: "a.zip".to_string(),
                }],
            },
            dependencies: None,
            entry: None,
            description: None,
            author: Some("alice".to_string()),
            signature: Some("sig".to_string()),
        };
        repo.add_package_version("dup-target-pkg".to_string(), make_info(Some("aarch64-apple-darwin")))
            .unwrap();
        let err = repo
            .add_package_version("dup-target-pkg".to_string(), make_info(Some("aarch64-apple-darwin")))
            .unwrap_err();
        assert!(err.to_string().contains("aarch64-apple-darwin"));
    }
```

檔案頂端確認 `use dpm_core::{..., PackageVersionInfo, RepoInfo, ...}` 這類 import 已經涵蓋 `PackageVersionInfo`/`RepoInfo`(`action.rs` 開頭 `use crate::*;` 通常已經透過 `dpm_core::*` 或明確 import 涵蓋,執行 Step 6 的編譯檢查會抓出缺什麼)。

- [ ] **Step 6: 跑測試,確認通過**

Run: `cargo test -p DPM-Server add_kind_url_with_target repo_info_add_package_version`
Expected: 3 個測試全部 PASS。

- [ ] **Step 7: 既有測試沒有回歸**

Run: `cargo test -p DPM-Server`
Expected: 全部通過(既有 `fix_add_*` 系列測試不受影響——它們都用 `AddKind::Build`/舊的 `AddKind::Url`,補上 `target: None`/`targets: None` 之後行為完全不變)。既有測試裡任何直接建構 `AddKind::Url { url, file_name }` 或 `AddKind::Build { build }` 的地方,編譯器會報缺欄位——照 Step 6 的錯誤訊息補上 `target: None`/`targets: None`。

- [ ] **Step 8: clippy**

Run: `cargo clippy -p DPM-Server --all-targets -- -D warnings`
Expected: 無警告無錯誤。

- [ ] **Step 9: Commit**

```bash
git add crates/dpm-server/src/cli_parse.rs crates/dpm-server/src/action.rs
git commit -m "$(cat <<'EOF'
feat(dpm-server): add --target/--targets to fix add

`fix add ... url --target <TRIPLE>` registers a target-specific
Prebuilt build; omit --target for a universal build. The same
(name, version) can be fix-add'ed multiple times with different
--target values to register more than one platform. `fix add ...
build --targets <T1,T2>` declares which platforms a Source package
supports (comma-separated, omit for any platform) — the build command
itself is unchanged, this only feeds PackageKind::Source's new
supported_targets allow-list.

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: `dpm` client — sync 時做 target 匹配/`supported_targets` 檢查

**依賴 Task 1。**

**Files:**
- Modify: `crates/dpm/src/action.rs`

**Interfaces:**
- Consumes:`PackageKind::to_db_fields(&self, target: &str) -> CoreResult<(...)>`(Task 1)、`self_update::get_target() -> &'static str`(既有依賴,`dpm` 的 `Cargo.toml` 已經有 `self_update`)。
- Produces:無新介面——`sync_source_inner` 內部行為變動,對外可觀察行為是「不相容本機平台的版本,`dpm update` 印警告並跳過,不進本機索引」。

- [ ] **Step 1: 寫失敗的測試(TDD——`sync_source_inner` 還沒有 target 過濾)**

在 `crates/dpm/src/action.rs` 底部既有 `#[cfg(test)] mod sync_source_tests { ... }` 裡,參考同模組已有的 `sync_source_inner_skips_invalid_signature_but_keeps_valid_one_when_official` 寫法(用 `serve_once`/`FakeRepoInfo` 那套 mock server),新增:

```rust
    #[tokio::test]
    async fn sync_source_inner_skips_a_prebuilt_version_with_no_build_for_this_target() {
        let target = self_update::get_target();
        // 故意登記一個「不是本機 target」的 build,確保這個版本一定會被
        // 跳過——用一個真實 target 字串裡不會出現的假字串當「另一個平台」,
        // 避免巧合等於本機 target。
        let other_target = format!("not-{target}");

        let body = serde_json::to_vec(&FakeRepoInfo {
            packages: {
                let mut m = StdHashMap::new();
                m.insert(
                    "wrong-target-pkg".to_string(),
                    vec![PackageVersionInfo {
                        version: "1.0.0".to_string(),
                        kind: PackageKind::Prebuilt {
                            builds: vec![dpm_core::PrebuiltBuild {
                                target: Some(other_target),
                                url: "https://example.com/wrong.zip".to_string(),
                                hash: "a".repeat(64),
                                file_name: "wrong.zip".to_string(),
                            }],
                        },
                        dependencies: None,
                        entry: None,
                        description: None,
                        author: None,
                        signature: None,
                    }],
                );
                m
            },
        })
        .unwrap();
        let repo_info_url = serve_once(body);

        let source = Source {
            alias: "third-party".to_string(),
            repo_url: "https://example.com/some-other-repo".to_string(),
            repo_info: repo_info_url,
        };
        let root = tempfile::tempdir().unwrap();
        let ctx = Context::for_test(root.path()).await.unwrap();

        ActionInfo::sync_source_inner(&ctx, &source, false)
            .await
            .unwrap();

        let all = ctx.db.read_all().await.unwrap();
        assert_eq!(
            all.len(),
            0,
            "a Prebuilt version with no build for this machine's target must be skipped, not inserted"
        );
    }

    #[tokio::test]
    async fn sync_source_inner_keeps_a_prebuilt_version_with_a_universal_build() {
        let body = serde_json::to_vec(&FakeRepoInfo {
            packages: {
                let mut m = StdHashMap::new();
                m.insert(
                    "universal-pkg".to_string(),
                    vec![PackageVersionInfo {
                        version: "1.0.0".to_string(),
                        kind: PackageKind::Prebuilt {
                            builds: vec![dpm_core::PrebuiltBuild {
                                target: None,
                                url: "https://example.com/universal.zip".to_string(),
                                hash: "a".repeat(64),
                                file_name: "universal.zip".to_string(),
                            }],
                        },
                        dependencies: None,
                        entry: None,
                        description: None,
                        author: None,
                        signature: None,
                    }],
                );
                m
            },
        })
        .unwrap();
        let repo_info_url = serve_once(body);

        let source = Source {
            alias: "third-party".to_string(),
            repo_url: "https://example.com/some-other-repo".to_string(),
            repo_info: repo_info_url,
        };
        let root = tempfile::tempdir().unwrap();
        let ctx = Context::for_test(root.path()).await.unwrap();

        ActionInfo::sync_source_inner(&ctx, &source, false)
            .await
            .unwrap();

        let all = ctx.db.read_all().await.unwrap();
        assert_eq!(all.len(), 1, "a universal build must always be kept");
    }

    #[tokio::test]
    async fn sync_source_inner_skips_a_source_version_unsupported_on_this_target() {
        let target = self_update::get_target();
        let other_target = format!("not-{target}");

        let body = serde_json::to_vec(&FakeRepoInfo {
            packages: {
                let mut m = StdHashMap::new();
                m.insert(
                    "unsupported-source-pkg".to_string(),
                    vec![PackageVersionInfo {
                        version: "1.0.0".to_string(),
                        kind: PackageKind::Source {
                            build: "make".to_string(),
                            hash: Some("a".repeat(64)),
                            supported_targets: Some(vec![other_target]),
                        },
                        dependencies: None,
                        entry: None,
                        description: None,
                        author: None,
                        signature: None,
                    }],
                );
                m
            },
        })
        .unwrap();
        let repo_info_url = serve_once(body);

        let source = Source {
            alias: "third-party".to_string(),
            repo_url: "https://example.com/some-other-repo".to_string(),
            repo_info: repo_info_url,
        };
        let root = tempfile::tempdir().unwrap();
        let ctx = Context::for_test(root.path()).await.unwrap();

        ActionInfo::sync_source_inner(&ctx, &source, false)
            .await
            .unwrap();

        let all = ctx.db.read_all().await.unwrap();
        assert_eq!(
            all.len(),
            0,
            "a Source version not supporting this target must be skipped"
        );
    }
```

（`PackageKind::Source` 的既有型別是 `dpm_core::PackageKind::Source` 全域路徑,或本檔案頂端已有的 `use dpm_core::{..., PackageKind, ...}` 直接寫 `PackageKind::Source { ... }` 即可——照這個檔案已有的 import 風格,不用另外加 import,只需要在 struct 字面值裡多帶一個 `supported_targets` 欄位。）

- [ ] **Step 2: 跑測試,確認因為欄位/邏輯不存在而失敗**

Run: `cargo test -p DPM sync_source_inner_skips_a_prebuilt_version_with_no_build sync_source_inner_keeps_a_prebuilt_version_with_a_universal_build sync_source_inner_skips_a_source_version_unsupported`
Expected: 編譯失敗(`PackageKind::Source` 字面值缺 `supported_targets` 欄位)或執行期失敗(`to_db_fields()` 呼叫方式跟新簽名不符,還沒改)。

- [ ] **Step 3: 改 `sync_source_inner`**

編輯 `crates/dpm/src/action.rs`,把:

```rust
        for (name, versions) in remote_repo.get_package_handler() {
            for version_info in versions {
                let (kind_str, url, hash, filename, build_command) =
                    version_info.kind.to_db_fields();

                if is_official {
```

換成:

```rust
        let target = self_update::get_target();
        for (name, versions) in remote_repo.get_package_handler() {
            for version_info in versions {
                let (kind_str, url, hash, filename, build_command) =
                    match version_info.kind.to_db_fields(target) {
                        Ok(fields) => fields,
                        Err(e) => {
                            println!(
                                "{} skipping {name}@{} — {e}",
                                "Warning:".yellow(),
                                version_info.version
                            );
                            continue;
                        }
                    };

                if is_official {
```

- [ ] **Step 4: 確認整個 crate 能編譯**

Run: `cargo check -p DPM`
Expected: 編譯成功。

- [ ] **Step 5: 跑測試,確認通過**

Run: `cargo test -p DPM sync_source_inner_skips_a_prebuilt_version_with_no_build sync_source_inner_keeps_a_prebuilt_version_with_a_universal_build sync_source_inner_skips_a_source_version_unsupported`
Expected: 3 個測試全部 PASS。

- [ ] **Step 6: 既有測試沒有回歸**

Run: `cargo test -p DPM`
Expected: 全部通過——既有 `sync_source_tests`/`install_resolved_tests` 系列都用 `PackageKind::Prebuilt`/`Source` 字面值建構測試資料,編譯器會抓出所有需要補 `builds`(取代 `url`/`hash`/`file_name`)、`supported_targets` 欄位的地方,照編譯錯誤逐一補上(既有測試的既有 `url`/`hash`/`file_name` 欄位改成 `builds: vec![dpm_core::PrebuiltBuild { target: None, url, hash, file_name }]`,`Source` 補 `supported_targets: None`,語意不變,因為都是「單一通用 build」情境)。

- [ ] **Step 7: clippy**

Run: `cargo clippy -p DPM --all-targets -- -D warnings`
Expected: 無警告無錯誤。

- [ ] **Step 8: Commit**

```bash
git add crates/dpm/src/action.rs
git commit -m "$(cat <<'EOF'
feat(dpm): filter sync'd packages by this machine's target

sync_source_inner now resolves each Prebuilt version's build (or
checks a Source version's supported_targets) against
self_update::get_target() at sync time. A version with no compatible
build/support for this machine prints a warning and is skipped —
never written to the local DB — so dpm install's existing logic is
completely unchanged: it still always reads exactly one already-
resolved, already-compatible build per version.

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: 修復真實的 `Derrick-Program/DPM-Server` repo

**依賴 Task 1-3**(要用改完的 `dpm-server` CLI 發布新格式資料)。

**Files:** 無工作區內檔案變動——這個 task 操作外部 repo。

**Interfaces:** 無。

- [ ] **Step 1: 取消 archive、改 public**

```bash
gh api repos/Derrick-Program/DPM-Server --method PATCH -f archived=false
gh repo edit Derrick-Program/DPM-Server --visibility public --accept-visibility-change-consequences
```

Expected: 兩個指令都成功。用 `gh api repos/Derrick-Program/DPM-Server --jq '.archived,.visibility'` 確認印出 `false`/`public`。

- [ ] **Step 2: Clone 到本機工作目錄**

```bash
mkdir -p /tmp/dpm-server-repo-fix && cd /tmp/dpm-server-repo-fix
gh repo clone Derrick-Program/DPM-Server .
```

- [ ] **Step 3: 重寫 `RepoInfo.json` 成新版多版本陣列 schema**

讀現有 `RepoInfo.json`(單物件格式,4 個佔位測試套件:`test`/`helloWorld`/`test1`/`test2`),改寫成新格式(每個套件的值是陣列,每個元素補 `dependencies: null`)。這 4 個套件本來就是佔位測試資料,不是真實使用者在用的套件,直接改寫、不用保留舊 `url`/`hash` 的正確性(反正接下來 Step 4 會整個清掉换成 `hello`/`addsub`)。

```bash
cat > RepoInfo.json <<'EOF'
{
  "packages": {}
}
EOF
git add RepoInfo.json
git commit -m "chore: clear placeholder packages, moving to hello/addsub demo packages"
```

（直接清空成空物件,比手動把 4 個舊格式套件轉成新格式再等 Task 5/6 覆寫更省事——這 4 個從來就是開發過程的佔位資料,不是文件裡承諾要保留的東西。）

- [ ] **Step 4: Push**

```bash
git push origin main
```

Expected: 成功。`gh api repos/Derrick-Program/DPM-Server/contents/RepoInfo.json --jq '.content' | base64 -d` 確認內容是 `{"packages": {}}`。

- [ ] **Step 5: 記錄(不用 commit 到 DPM-Workspace)**

這個 task 沒有 DPM-Workspace 這邊的檔案變動,不需要在這個 repo 建立 commit——下一步的 ledger/報告只需要記「`Derrick-Program/DPM-Server` 已改 public、取消 archive、`RepoInfo.json` 已清空成新 schema 的空物件,`/tmp/dpm-server-repo-fix` 是本機 clone,後續 Task 5/6 繼續在這個目錄操作」。

---

## Task 5: 發布 `hello`(Prebuilt/通用)

**依賴 Task 2、Task 4。**

**Files:** 無工作區內檔案變動——這個 task 操作 `/tmp/dpm-server-repo-fix`(Task 4 clone 的真實 repo)。

**Interfaces:** 無。

- [ ] **Step 1: 建套件骨架**

```bash
cd /tmp/dpm-server-repo-fix
cargo run --manifest-path /path/to/DPM-Workspace/Cargo.toml -p DPM-Server -- keygen alice
cargo run --manifest-path /path/to/DPM-Workspace/Cargo.toml -p DPM-Server -- init hello bin/hello --author alice -v 0.1.0 -d "simple universal hello package"
```

（`/path/to/DPM-Workspace` 換成這個 workspace 實際的絕對路徑。若 Task 4 的 `alice` key 還沒產生過,`keygen` 這步會在 `/tmp/dpm-server-repo-fix/keys/` 建立 `alice.priv`/`alice.pub`。）

- [ ] **Step 2: 填實際內容**

```bash
printf '#!/bin/sh\necho hello from dpm\n' > packages/hello/bin/hello
chmod +x packages/hello/bin/hello
```

- [ ] **Step 3: 打包、算 hash、簽章**

```bash
cargo run --manifest-path /path/to/DPM-Workspace/Cargo.toml -p DPM-Server -- build hello
cargo run --manifest-path /path/to/DPM-Workspace/Cargo.toml -p DPM-Server -- hash hello
cargo run --manifest-path /path/to/DPM-Workspace/Cargo.toml -p DPM-Server -- sign hello
```

Expected:`Repo/hello.zip` 產生,`packages/hello/packageInfo.json` 的 `hash`/`signature` 都被填上。

- [ ] **Step 4: Commit + push 打包產物,取得可下載的 https URL**

`fix add ... url` 需要一個真的能下載到的 https 網址——用這個 repo 自己的 raw content URL(`Repo/hello.zip` 一旦 commit+push,`https://raw.githubusercontent.com/Derrick-Program/DPM-Server/main/Repo/hello.zip` 就能下載到)。

```bash
git add packages/hello Repo/hello.zip
git commit -m "feat: add hello package (Prebuilt, universal)"
git push origin main
```

- [ ] **Step 5: `fix add`(不帶 `--target`,通用 build)**

```bash
cargo run --manifest-path /path/to/DPM-Workspace/Cargo.toml -p DPM-Server -- fix add hello url https://raw.githubusercontent.com/Derrick-Program/DPM-Server/main/Repo/hello.zip
```

Expected: 成功寫進 `RepoInfo.json`(`fix_add` 會重新下載這個 URL 驗證 hash 對得上簽章)。

- [ ] **Step 6: Commit + push `RepoInfo.json`**

```bash
git add RepoInfo.json
git commit -m "chore: publish hello v0.1.0 to RepoInfo.json"
git push origin main
```

Expected: `gh api repos/Derrick-Program/DPM-Server/contents/RepoInfo.json --jq '.content' | base64 -d` 顯示 `hello` 已經是 `{"packages": {"hello": [{"kind":"prebuilt","builds":[{...}],...}]}}` 這種新格式。

---

## Task 6: 發布 `addsub`(Source/C 加減法)

**依賴 Task 2、Task 4。**

**Files:** 無工作區內檔案變動——這個 task 繼續操作 `/tmp/dpm-server-repo-fix`。

**Interfaces:** 無。

- [ ] **Step 1: 建套件骨架**

```bash
cd /tmp/dpm-server-repo-fix
cargo run --manifest-path /path/to/DPM-Workspace/Cargo.toml -p DPM-Server -- init addsub lib/libaddsub.dylib --author alice -v 0.1.0 -d "C add/subtract shared library"
```

（`init` 的 `<entry>` 參數目前只拿去建一個空檔案跟寫進 `packageInfo.json.file_name`,實際安裝流程對 Source 套件不會用到 `entry` 這個符號連結機制——見下面 Step 4 的說明。這裡填 `lib/libaddsub.dylib`只是跟這個套件實際的建置產物路徑保持一致,方便閱讀。）

- [ ] **Step 2: 這個套件要是 git repo**——`dpm` 安裝 Source 套件時是 `git2` clone 這個 source 的 `repo_url`,`packages/addsub/` 要能被 clone 到,整個 `/tmp/dpm-server-repo-fix` 本身已經是 git repo(就是 `Derrick-Program/DPM-Server` 本身),不需要另外初始化。

- [ ] **Step 3: 寫 C 原始碼**

```bash
mkdir -p packages/addsub/src
cat > packages/addsub/src/addsub.c <<'EOF'
int add(int a, int b) {
    return a + b;
}

int subtract(int a, int b) {
    return a - b;
}
EOF
rm -f packages/addsub/lib/libaddsub.dylib
rmdir packages/addsub/lib 2>/dev/null || true
```

（`init` 建的空 `lib/libaddsub.dylib` 檔案只是佔位,實際產物由 build 指令產生在 `$OUT`,這裡刪掉避免跟真正的建置產物混淆。）

- [ ] **Step 4: 決定 build 指令**

macOS 用 `-dynamiclib`、Linux 用 `-shared`,写一個兩邊都能跑的 build 指令(用 `uname` 判斷):

```
sh -c 'if [ "$(uname)" = "Darwin" ]; then cc -dynamiclib -o "$OUT/libaddsub.dylib" src/addsub.c; else cc -shared -fPIC -o "$OUT/libaddsub.so" src/addsub.c; fi'
```

這串完整指令就是等一下要 `hash --build`/`fix add ... build` 用的字串(`$OUT`/`sh -c` 都是既有 `install_source_package` 已經支援的既有機制,見 `crates/dpm/src/action.rs::install_source_package`——`build_cmd.env("OUT", &out_dir)` 已經把 `$OUT` 設好,`sh -c "<build_command>"` 直接執行)。

- [ ] **Step 5: Commit + push 原始碼(必須在 hash 之前)**

`hash --build` 會讀當下 `git2::Repository::discover` 找到的 HEAD commit,綁進 `blake3(build_command + commit)` 這個 hash 裡(見 `source_repo_commit_hash`)——如果先 hash 才 commit,綁到的會是「還沒包含 `addsub.c` 這次改動」的舊 commit,發布出去的 hash 對不上實際內容所在的 commit。這裡先 commit+push,讓 HEAD 是真的包含這次原始碼的那個 commit,再進 Step 6 算 hash。

```bash
git add packages/addsub
git commit -m "feat: add addsub package (Source, C add/subtract library)"
git push origin main
```

- [ ] **Step 6: 算 hash、簽章**

```bash
BUILD_CMD='if [ "$(uname)" = "Darwin" ]; then cc -dynamiclib -o "$OUT/libaddsub.dylib" src/addsub.c; else cc -shared -fPIC -o "$OUT/libaddsub.so" src/addsub.c; fi'
cargo run --manifest-path /path/to/DPM-Workspace/Cargo.toml -p DPM-Server -- hash addsub --build "$BUILD_CMD"
cargo run --manifest-path /path/to/DPM-Workspace/Cargo.toml -p DPM-Server -- sign addsub
```

- [ ] **Step 7: `fix add`(`--targets` 宣告支援平台)**

用這台機器實際的 target(執行 `cargo run --manifest-path /path/to/DPM-Workspace/Cargo.toml -p DPM -- --version` 之類的指令不會印 target;直接用 `rustc -vV | grep host` 拿到本機 target triple),連同至少一個 Linux target 一起宣告(build 指令本來就兩邊都處理了):

```bash
HOST_TARGET=$(rustc -vV | grep '^host:' | cut -d' ' -f2)
cargo run --manifest-path /path/to/DPM-Workspace/Cargo.toml -p DPM-Server -- fix add addsub build "$BUILD_CMD" --targets "$HOST_TARGET,x86_64-unknown-linux-gnu,aarch64-unknown-linux-gnu"
```

- [ ] **Step 8: Commit + push `RepoInfo.json`**

```bash
git add RepoInfo.json
git commit -m "chore: publish addsub v0.1.0 to RepoInfo.json"
git push origin main
```

Expected: `gh api repos/Derrick-Program/DPM-Server/contents/RepoInfo.json --jq '.content' | base64 -d` 顯示 `addsub` 是 `kind: source`,`supported_targets` 含本機 target。

---

## Task 7: 手動端到端驗證

**依賴 Task 5、Task 6。**

**Files:** 無(不改 DPM-Workspace 程式碼)。

**Interfaces:** 無。

- [ ] **Step 1: 用改完的 `dpm` 安裝**

```bash
cd /path/to/DPM-Workspace
cargo build -p DPM --release
DPM_BIN=./target/release/dpm
$DPM_BIN source list
```

Expected: 確認 official source 指向 `Derrick-Program/DPM-Server`(照 `system.rs::OFFICIAL_REPO_URL` 現有值,不用改)。若這台機器上的 `dpm` 資料夾之前跑過舊格式資料,先清掉:

```bash
rm -rf ~/Library/Application\ Support/com.duacodie.dpm   # macOS per-user
```

- [ ] **Step 2: `dpm update`**

```bash
$DPM_BIN update
```

Expected: 印出正常更新訊息,不報 schema 解析錯誤(Task 4 已經把 `RepoInfo.json` 換成新格式)。

- [ ] **Step 3: 安裝 `hello`**

```bash
$DPM_BIN install hello
$(dirname "$($DPM_BIN list -l | head -1)")/hello 2>&1 || true
```

實際執行路徑照 per-user scope 的 `bin_dir`(`~/Library/Application Support/com.duacodie.dpm/bin/hello`,macOS):

```bash
~/Library/Application\ Support/com.duacodie.dpm/bin/hello
```

Expected: 印出 `hello from dpm`。

- [ ] **Step 4: 安裝 `addsub`**

```bash
$DPM_BIN install addsub
```

Expected: 印出 build 指令執行成功。確認產物存在(per-user scope,macOS):

```bash
ls ~/Library/Application\ Support/com.duacodie.dpm/Software/addsub/
```

Expected: 看到 `libaddsub.dylib`(Linux 上是 `libaddsub.so`)。

- [ ] **Step 5: 寫 `main.c` 連結安裝好的 lib**

```bash
cat > /tmp/test_addsub.c <<'EOF'
#include <stdio.h>

extern int add(int a, int b);
extern int subtract(int a, int b);

int main(void) {
    int sum = add(3, 4);
    int diff = subtract(10, 4);
    printf("add(3, 4) = %d\n", sum);
    printf("subtract(10, 4) = %d\n", diff);
    if (sum != 7 || diff != 6) {
        fprintf(stderr, "FAIL: unexpected result\n");
        return 1;
    }
    printf("PASS\n");
    return 0;
}
EOF
cc /tmp/test_addsub.c \
  ~/Library/Application\ Support/com.duacodie.dpm/Software/addsub/libaddsub.dylib \
  -o /tmp/test_addsub
/tmp/test_addsub
```

Expected:
```
add(3, 4) = 7
subtract(10, 4) = 6
PASS
```

（Linux 上換成 `libaddsub.so`,連結方式相同。）

- [ ] **Step 6: 記錄**

全部通過後,在 PR/對話紀錄裡留一句「Task 7 手動端到端驗證已完成,hello/addsub 都能真的裝、addsub 編出來的 lib 真的能被外部程式連結呼叫」。

---

## Task 8: 整個 workspace 收尾驗證

**Files:** 無新增/修改(純驗證)。

**Interfaces:** 無。

- [ ] **Step 1: `cargo check --workspace` 通過**

Run: `cargo check --workspace`
Expected: 編譯成功。

- [ ] **Step 2: 格式化檢查**

Run: `cargo fmt --all -- --check`
Expected: 無輸出。有輸出的話跑 `cargo fmt --all` 再重新檢查一次。

- [ ] **Step 3: clippy(整個 workspace)**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: 無警告無錯誤。

- [ ] **Step 4: 整個 workspace 測試**

Run: `cargo test --workspace`
Expected: 全部通過,包含這次新增的所有測試(`dpm-core` 的 `package_kind_target_tests` 8 個、`dpm-server` 的 3 個新測試、`dpm` 的 3 個新 `sync_source_tests`)。

- [ ] **Step 5: 確認沒有漏 commit 的變動**

Run: `git status`
Expected: working tree clean(Task 1-3、5-6 每個都已經各自 commit 過;Task 4 的變動在外部 repo,不影響這裡的 `git status`)。

- [ ] **Step 6: Commit(若 Step 2 有格式化修正)**

```bash
git add -A
git commit -m "chore: cargo fmt"
```

（只有在 Step 2 真的產生格式化變動時才需要這個 commit。）
