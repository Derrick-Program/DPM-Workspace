# 套件作者身份驗證(Package Author Verification) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 為透過官方來源(`OFFICIAL_REPO_URL`)發布的套件加上 ed25519 作者簽章驗證——`dpm-server` 新增 `keygen`/`sign` 子指令、`init`/`fix add` 加上作者一致性檢查;`dpm` 在 `sync_source()`(`update`)跟安裝路徑(`install_resolved`)各自重新驗證一次簽章,驗不過就拒絕(sync 時跳過該筆、安裝時整筆拒裝)。

**Architecture:** 12 個循序 task。Task 1-2 是 `dpm-core` 的共用層:簽章/驗證 primitive(ed25519 + hex,ungated)跟 schema 變更(`PackageInfo`/`PackageVersionInfo` 加 `author`/`signature`,`PackageKind::Source` 加 `hash`),兩者是後面所有 task 的依賴。Task 3-7 是 `dpm-server` 的五個垂直切片(`keygen`→`init --author`→`hash` 擴充→`sign`→`fix add` 驗證),依序疊加,因為 `fix add`(Task 7)要讀的 `packageInfo.json.author/signature/hash` 得先有前四個指令才會被填好。Task 8 是本機 DB schema(migration 0004 + `db.rs`/`models.rs`),`dpm` client 端 Task 9-11 都依賴它。Task 9-11 是 `dpm` client 三個垂直切片:官方來源 URL 常數可見性/公鑰 URL 推導、`sync_source()` 整合(驗證失敗跳過該筆+記憶體快取)、安裝路徑整合(驗證失敗整筆拒裝,`INSECURE:` 標記)。Task 12 是整個 workspace 的收尾驗證與人工端對端檢查清單。

**Tech Stack:** Rust 2021、`ed25519-dalek = "2.1"`(簽章/驗證,不需要 `rand_core` feature——金鑰產生直接用 `getrandom` 填 32 bytes seed,不透過 `SigningKey::generate`)、`hex = "0.4"`(簽章的 hex 編解碼)、`getrandom = "0.2"`(`keygen` 的隨機性來源)、`git2 = "0.18.1"`(`dpm-server` 新增依賴,讀取 repo HEAD commit hash,版本比照 `dpm` crate 既有的 pin)。

## Global Constraints

- `ed25519-dalek`/`hex`/`getrandom` 只加進 `crates/dpm-core/Cargo.toml`(比照既有的 `blake3` 放置慣例,不進根 `Cargo.toml` 的 `[workspace.dependencies]`,因為只有 `dpm-core` 自己直接依賴它們)。`dpm`/`dpm-server` 透過 `dpm_core::{SigningKey, VerifyingKey, Signature, ...}` 重新匯出的型別使用,不各自加 `ed25519-dalek` 依賴——避免重蹈 CLAUDE.md 記錄過的 `turso`/`geni` 版本不同步炸掉的覆轍。
- `git2` 只加進 `crates/dpm-server/Cargo.toml`,版本字串跟 `crates/dpm/Cargo.toml` 現有的 `git2 = "0.18.1"` 完全一致(兩邊各自宣告,不促成 workspace-wide 依賴,這不是這次改動的範圍)。
- **範圍限定**:所有新的簽章驗證邏輯只在 `source.repo_url == OFFICIAL_REPO_URL` 為真時觸發(client 端);`dpm-server` 這邊(伺服端)本來就只服務官方 repo 自己的發布流程,不需要額外判斷。第三方來源維持零驗證現狀,不要誤觸動 `install_source_package` 裡既有的 `source_alias != "official"` 警告邏輯(那是完全獨立、印警告用的既有程式碼,不是這次的安全閘門)。
- **設計決策(spec 沒有明講、本計畫據以實作,務必遵守,不要自行改動)**:
  1. `packageInfo.json.hash`(由 `dpm-server hash` 算出、`dpm-server sign` 簽署)在 `fix add` 寫進 `RepoInfo.json` 時,**逐字複製**成該版本的 `PackageKind` hash(`Prebuilt.hash`/`Source.hash`)——也就是「簽的 hash」跟「client 端看到、用來驗證簽章的 hash」永遠是同一個值,不是兩個獨立算出來、可能對不上的數字。
  2. 因此 `AddKind::Url` 分支不再自己重新計算下載內容的 hash 當作最終值,而是下載後比對是否等於 `pk_info.hash`(對不上就拒絕,提示重新 `build`+`hash`+`sign`),確認相符後才把 `pk_info.hash` 寫進 `PackageKind::Prebuilt.hash`。這代表 `dpm-server hash` 必須在 `dpm-server build` **之後**執行才能讓兩者對得上(`hash` 會偵測 `Repo/<name>.zip` 是否已存在,存在就直接雜湊那個檔案,不存在才退回舊的專案目錄逐檔雜湊)。
  3. `kind: source` 沒有下載內容可比對,`dpm-server hash --build <cmd>` 直接把 `blake3(build_command + "\n" + 目前 git HEAD commit hash)` 當作 `packageInfo.json.hash`,`fix add` 的 `AddKind::Build` 分支原封不動把它複製進 `PackageKind::Source.hash`。
- 測試一律用專案既有慣例:同檔案底部 `#[cfg(test)] mod xxx_tests { use super::*; ... }`,不要另開檔案(除非該檔案本來就沒有 test 模組)。網路呼叫一律用 `crates/dpm/src/utils/fetcher.rs::serve_once` 那種原始 TCP mock(每個 URL 一個獨立的 `TcpListener`,只接受一次連線),不要引入 mock crate。
- 每個有程式碼變動的 task 結束前都要跑過該 crate 的 `cargo check -p <crate>`/`cargo clippy -p <crate> --all-targets -- -D warnings`/`cargo test -p <crate>`,不要留到最後一次總跑。Commit message 用 Conventional Commits(`type(scope): description`)格式,每個 task 結束各自 commit 一次。
- 提交前完整跑一次 `just pre-commit`(fmt + clippy + test)是 Task 12 的內容;個別 task 的中途檢查直接下 `cargo fmt`/`cargo clippy`/`cargo test` 對應指令即可,不需要 Infisical session。

---

## Task 1: `dpm-core` — ed25519 簽章/驗證共用 primitive

**Files:**
- Modify: `crates/dpm-core/Cargo.toml`
- Modify: `crates/dpm-core/src/error.rs`
- Modify: `crates/dpm-core/src/lib.rs`(新增函式,加在 `hash_file` 附近)
- Modify: `crates/dpm-core/tests/test.rs`(新增測試模組)

**Interfaces:**
- Produces: `dpm_core::{SigningKey, VerifyingKey, Signature}`(重新匯出自 `ed25519_dalek`)。
- Produces: `dpm_core::generate_signing_key() -> CoreResult<SigningKey>`。
- Produces: `dpm_core::signing_key_from_bytes(bytes: &[u8]) -> CoreResult<SigningKey>`。
- Produces: `dpm_core::verifying_key_from_bytes(bytes: &[u8]) -> CoreResult<VerifyingKey>`。
- Produces: `dpm_core::sign_hash(signing_key: &SigningKey, hash_hex: &str) -> String`。
- Produces: `dpm_core::verify_hash_signature(verifying_key: &VerifyingKey, hash_hex: &str, signature_hex: &str) -> CoreResult<()>`。
- Produces: `dpm_core::hash_bytes(data: &[u8]) -> String`(blake3,跟 `hash_file` 同演算法,吃記憶體內容而非檔案路徑——Task 5 的 `kind: source` hash 計算需要,沒有對應實體檔案可餵給 `hash_file`)。
- Produces: `CoreError::SignatureInvalid(String)` — 所有簽章/金鑰格式錯誤統一回傳這個 variant(hex 解析失敗、長度不對、驗證不過都算)。

- [ ] **Step 1: 加 Cargo 依賴**

編輯 `crates/dpm-core/Cargo.toml`,在 `[dependencies]` 區塊加入(維持既有的按字母排序慣例,插在 `clap.workspace = true` 之後、`reqwest.workspace = true` 之前):

```toml
clap.workspace = true
ed25519-dalek = "2.1"
getrandom = "0.2"
hex = "0.4"
reqwest.workspace = true
```

- [ ] **Step 2: 加 `CoreError::SignatureInvalid`**

編輯 `crates/dpm-core/src/error.rs`,在 `SecurityError` variant 之後加入:

```rust
    #[error("Security error: {0}")]
    SecurityError(String),

    #[error("Signature verification failed: {0}")]
    SignatureInvalid(String),

    #[error("Ambiguous package '{0}': exists in multiple sources, specify source/name")]
    AmbiguousPackage(String),
```

- [ ] **Step 3: 寫失敗的測試**

在 `crates/dpm-core/tests/test.rs` 檔案最底部、`mod tests` 的 `}` 之前加入(緊接在 `test_hash_file_is_deterministic_and_content_sensitive` 後面):

```rust
    mod signature_tests {
        use dpm_core::*;

        #[test]
        fn sign_and_verify_round_trips() {
            let key = generate_signing_key().unwrap();
            let hash = "deadbeef".repeat(8);
            let sig = sign_hash(&key, &hash);
            assert!(verify_hash_signature(&key.verifying_key(), &hash, &sig).is_ok());
        }

        #[test]
        fn verify_rejects_a_signature_over_a_different_hash() {
            let key = generate_signing_key().unwrap();
            let hash_a = "a".repeat(64);
            let hash_b = "b".repeat(64);
            let sig = sign_hash(&key, &hash_a);
            assert!(verify_hash_signature(&key.verifying_key(), &hash_b, &sig).is_err());
        }

        #[test]
        fn verify_rejects_a_signature_from_a_different_key() {
            let key_a = generate_signing_key().unwrap();
            let key_b = generate_signing_key().unwrap();
            let hash = "c".repeat(64);
            let sig = sign_hash(&key_a, &hash);
            assert!(verify_hash_signature(&key_b.verifying_key(), &hash, &sig).is_err());
        }

        #[test]
        fn verify_rejects_malformed_hex_signature() {
            let key = generate_signing_key().unwrap();
            let hash = "d".repeat(64);
            let err = verify_hash_signature(&key.verifying_key(), &hash, "not hex!!").unwrap_err();
            assert!(matches!(err, CoreError::SignatureInvalid(_)));
        }

        #[test]
        fn verify_rejects_wrong_length_signature() {
            let key = generate_signing_key().unwrap();
            let hash = "e".repeat(64);
            // 合法 hex,但長度不是 64 bytes(真正的簽章一定是 64 bytes)。
            let err = verify_hash_signature(&key.verifying_key(), &hash, "ab").unwrap_err();
            assert!(matches!(err, CoreError::SignatureInvalid(_)));
        }

        #[test]
        fn verifying_key_from_bytes_rejects_wrong_length() {
            let err = verifying_key_from_bytes(&[0u8; 10]).unwrap_err();
            assert!(matches!(err, CoreError::SignatureInvalid(_)));
        }

        #[test]
        fn signing_key_round_trips_through_bytes() {
            let key = generate_signing_key().unwrap();
            let bytes = key.to_bytes();
            let restored = signing_key_from_bytes(&bytes).unwrap();
            assert_eq!(
                restored.verifying_key().to_bytes(),
                key.verifying_key().to_bytes()
            );
        }

        #[test]
        fn hash_bytes_is_deterministic_and_content_sensitive() {
            let a1 = hash_bytes(b"hello");
            let a2 = hash_bytes(b"hello");
            let b = hash_bytes(b"world");
            assert_eq!(a1, a2);
            assert_ne!(a1, b);
            assert_eq!(a1.len(), 64, "blake3 hex output is 32 bytes = 64 hex chars");
        }
    }
```

- [ ] **Step 4: 執行測試,確認因缺函式而編譯失敗**

Run: `cargo test -p DPM-Core --test test signature_tests`
Expected: 編譯錯誤,`generate_signing_key`/`sign_hash`/`verify_hash_signature`/`verifying_key_from_bytes`/`signing_key_from_bytes`/`hash_bytes` 均未定義。

- [ ] **Step 5: 實作 primitive**

編輯 `crates/dpm-core/src/lib.rs`,在檔案頂部 `use` 區塊加入重新匯出,並在 `hash_file` 函式後面加入新函式:

```rust
mod error;
mod zip_file;
pub use error::*;
pub use ed25519_dalek::{Signature, SigningKey, VerifyingKey};
use ed25519_dalek::{Signer, Verifier};
use serde::{Deserialize, Serialize};
use serde_json::to_writer_pretty;
use std::{collections::HashMap, io::Read, path::Path};
pub use zip_file::*;

/// 對檔案內容算 blake3 hash,回傳小寫十六進位字串。
/// client(安裝驗證)、server(發布時算 hash)共用同一份實作。
pub fn hash_file(path: &Path) -> CoreResult<String> {
    let mut file = std::fs::File::open(path)?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;
    Ok(blake3::hash(&buffer).to_hex().to_string())
}

/// 對任意 bytes 算 blake3 hash,回傳小寫十六進位字串——跟 [`hash_file`] 同一個
/// 演算法,只是輸入不是檔案而是記憶體內容(`kind: source` 套件的
/// `build_command` + commit hash 組合就是這樣算的,沒有對應的實體檔案可以餵
/// 給 `hash_file`)。
pub fn hash_bytes(data: &[u8]) -> String {
    blake3::hash(data).to_hex().to_string()
}

/// 產生一把新的 ed25519 簽章金鑰對——`dpm-server keygen` 用。簽章本身
/// (`sign_hash`)是確定性的,不需要隨機性,只有「產生新金鑰」這一步需要
/// CSPRNG,直接用 `getrandom` 填 32 bytes seed 建構 `SigningKey`,不透過
/// `SigningKey::generate`(那需要額外開 `ed25519-dalek` 的 `rand_core`
/// feature 並拉近一個版本可能跟其他依賴打架的 `rand_core` crate——這裡不需要)。
pub fn generate_signing_key() -> CoreResult<SigningKey> {
    let mut secret = [0u8; 32];
    getrandom::getrandom(&mut secret)
        .map_err(|e| CoreError::SecurityError(format!("failed to generate random key: {e}")))?;
    Ok(SigningKey::from_bytes(&secret))
}

/// 把讀出來的 32 bytes 私鑰檔案內容還原成 `SigningKey`。
pub fn signing_key_from_bytes(bytes: &[u8]) -> CoreResult<SigningKey> {
    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|_| CoreError::SignatureInvalid("private key must be 32 bytes".to_string()))?;
    Ok(SigningKey::from_bytes(&arr))
}

/// 把讀出來的 32 bytes 公鑰檔案內容(`keys/<author_id>.pub`)還原成
/// `VerifyingKey`。`dpm-server fix add`(讀本機 `keys/` 目錄)、`dpm`
/// client(讀從官方 repo 抓下來的 raw bytes)共用同一份實作。
pub fn verifying_key_from_bytes(bytes: &[u8]) -> CoreResult<VerifyingKey> {
    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|_| CoreError::SignatureInvalid("public key must be 32 bytes".to_string()))?;
    VerifyingKey::from_bytes(&arr).map_err(|e| CoreError::SignatureInvalid(e.to_string()))
}

/// 對一個 hex 編碼的 hash 字串(`packageInfo.json.hash`/`PackageKind` 的
/// hash 欄位本身,不是重新雜湊一次)簽章,回傳 hex 編碼的簽章字串。
pub fn sign_hash(signing_key: &SigningKey, hash_hex: &str) -> String {
    let sig: Signature = signing_key.sign(hash_hex.as_bytes());
    hex::encode(sig.to_bytes())
}

/// 驗證 `signature_hex` 是否是 `verifying_key` 對 `hash_hex` 的合法簽章。
/// hex 格式錯誤、簽章長度不對、驗證不過——任何一步失敗都回傳同一種
/// `CoreError::SignatureInvalid`,呼叫端不需要分辨失敗原因,一律視為
/// 「這個簽章不可信」。
pub fn verify_hash_signature(
    verifying_key: &VerifyingKey,
    hash_hex: &str,
    signature_hex: &str,
) -> CoreResult<()> {
    let sig_bytes = hex::decode(signature_hex)
        .map_err(|e| CoreError::SignatureInvalid(format!("signature is not valid hex: {e}")))?;
    let sig_bytes: [u8; 64] = sig_bytes
        .try_into()
        .map_err(|_| CoreError::SignatureInvalid("signature must be 64 bytes".to_string()))?;
    let signature = Signature::from_bytes(&sig_bytes);
    verifying_key
        .verify(hash_hex.as_bytes(), &signature)
        .map_err(|e| CoreError::SignatureInvalid(e.to_string()))
}
```

- [ ] **Step 6: 執行測試,確認通過**

Run: `cargo test -p DPM-Core --test test signature_tests`
Expected: 全部 8 個測試通過。

- [ ] **Step 7: `cargo check`/`clippy` 過整個 `dpm-core`**

Run: `cargo check -p DPM-Core && cargo clippy -p DPM-Core --all-targets -- -D warnings`
Expected: 無錯誤無警告。

- [ ] **Step 8: Commit**

```bash
git add crates/dpm-core/Cargo.toml crates/dpm-core/src/error.rs crates/dpm-core/src/lib.rs crates/dpm-core/tests/test.rs
git commit -m "feat(dpm-core): add ed25519 sign/verify primitives"
```

---

## Task 2: `dpm-core` — `PackageInfo`/`PackageVersionInfo`/`PackageKind` 新欄位

**Files:**
- Modify: `crates/dpm-core/src/lib.rs`(`PackageInfo`、`PackageKind::Source`、`PackageVersionInfo`、`to_db_fields`/`from_db_fields`)
- Modify: `crates/dpm-core/tests/test.rs`(`prebuilt()` helper、`test_to_json`)
- Modify: `crates/dpm-server/src/action.rs:82`(`PackageInfo::new` 呼叫端,`init()`)
- Modify: `crates/dpm-server/src/action.rs:386`(既有測試的 `PackageKind::Source` match 樣式)
- Modify: `crates/dpm/src/utils/fetcher.rs:121`(測試 fixture 的 `PackageInfo::new` 呼叫端)

**Interfaces:**
- Consumes: Task 1 的 `CoreResult`/`CoreError`(未變動,只是背景相依)。
- Produces: `PackageInfo { .., author: Option<String>, signature: Option<String> }`,`PackageInfo::new(package_name, file_name, version, description, hash, dependencies, author: Option<String>)`(`signature` 永遠從 `None` 開始,只能透過之後直接改欄位設定,跟既有 `hash()` 函式改 `package_info.hash` 同一個模式)。
- Produces: `PackageKind::Source { build: String, hash: Option<String> }`。
- Produces: `PackageVersionInfo { .., author: Option<String>, signature: Option<String> }`。
- Produces: `PackageKind::to_db_fields`/`from_db_fields` 對 `Source` 的 hash 欄位改成讀寫 DB 既有的 `hash` 欄位(不是新欄位,`Prebuilt`/`Source` 共用同一個 DB column)。

- [ ] **Step 1: 改 `PackageInfo`**

編輯 `crates/dpm-core/src/lib.rs`,把:

```rust
/// 儲存套件的完整資訊
#[derive(Debug, Serialize, Deserialize)]
pub struct PackageInfo {
    pub package_name: String,
    pub file_name: String,
    pub version: String,
    pub description: String,
    pub hash: String,
    pub dependencies: Option<Vec<Dependency>>,
}

impl PackageInfo {
    /// 建立一個新的 `PackageInfo` 實例
    ///
    /// # 參數
    /// - `package_name`: 套件名稱
    /// - `file_name`: 套件檔案名稱
    /// - `version`: 套件版本
    /// - `description`: 套件描述
    /// - `hash`: 套件檔案的雜湊值
    /// - `dependencies`: 可選的依賴列表
    ///
    /// # 回傳
    /// 回傳一個新的 `PackageInfo` 結構體
    pub fn new(
        package_name: String,
        file_name: String,
        version: String,
        description: String,
        hash: String,
        dependencies: Option<Vec<Dependency>>,
    ) -> PackageInfo {
        PackageInfo {
            package_name,
            file_name,
            version,
            description,
            hash,
            dependencies,
        }
    }
}
```

改成:

```rust
/// 儲存套件的完整資訊
#[derive(Debug, Serialize, Deserialize)]
pub struct PackageInfo {
    pub package_name: String,
    pub file_name: String,
    pub version: String,
    pub description: String,
    pub hash: String,
    pub dependencies: Option<Vec<Dependency>>,
    /// 發布這個版本的作者 id(`keys/<author_id>.pub` 的檔名)。`Option` 是為了
    /// 讓舊格式(這次改動之前產生)的 `packageInfo.json` 還能被解析——
    /// `dpm-server init --author` 之後一律會填。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// `dpm-server sign` 對 `hash` 欄位簽出來的 hex 簽章。`init` 建立時是
    /// `None`,只有 `sign` 這一個指令會寫入。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

impl PackageInfo {
    /// 建立一個新的 `PackageInfo` 實例
    ///
    /// # 參數
    /// - `package_name`: 套件名稱
    /// - `file_name`: 套件檔案名稱
    /// - `version`: 套件版本
    /// - `description`: 套件描述
    /// - `hash`: 套件檔案的雜湊值
    /// - `dependencies`: 可選的依賴列表
    /// - `author`: 發布這個版本的作者 id
    ///
    /// # 回傳
    /// 回傳一個新的 `PackageInfo` 結構體(`signature` 一律從 `None` 開始)
    pub fn new(
        package_name: String,
        file_name: String,
        version: String,
        description: String,
        hash: String,
        dependencies: Option<Vec<Dependency>>,
        author: Option<String>,
    ) -> PackageInfo {
        PackageInfo {
            package_name,
            file_name,
            version,
            description,
            hash,
            dependencies,
            author,
            signature: None,
        }
    }
}
```

- [ ] **Step 2: 改 `PackageKind::Source`**

把:

```rust
    /// 只提供原始碼 + build 指令,client 在本機執行 build(Phase 4 才會真的走這條路)。
    Source { build: String },
```

改成:

```rust
    /// 只提供原始碼 + build 指令,client 在本機執行 build。`hash` 是
    /// `blake3(build_command + commit hash)`(`dpm-server hash --build`
    /// 算出來的),`Option` 是因為還沒被 `hash`+`sign` 過的草稿狀態下沒有值。
    Source {
        build: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hash: Option<String>,
    },
```

- [ ] **Step 3: 改 `to_db_fields`/`from_db_fields`**

把:

```rust
            PackageKind::Source { build } => ("source", None, None, None, Some(build.clone())),
```

改成:

```rust
            PackageKind::Source { build, hash } => (
                "source",
                None,
                hash.clone(),
                None,
                Some(build.clone()),
            ),
```

把:

```rust
            "source" => Ok(PackageKind::Source {
                build: build_command.ok_or_else(|| {
                    CoreError::InvalidPackage("source package missing build command".to_string())
                })?,
            }),
```

改成:

```rust
            "source" => Ok(PackageKind::Source {
                build: build_command.ok_or_else(|| {
                    CoreError::InvalidPackage("source package missing build command".to_string())
                })?,
                hash,
            }),
```

(`hash` 是 `from_db_fields` 本來就有的參數,`Source` 這個 variant 現在也讀它了——不需要 `ok_or_else`,因為 `Source.hash` 本身就是 `Option`。)

- [ ] **Step 4: 改 `PackageVersionInfo`**

把:

```rust
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

改成:

```rust
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
    /// 發布這個版本的作者 id。只有 `source.repo_url == OFFICIAL_REPO_URL`
    /// 的來源會被 client 拿來做簽章驗證,其他來源忽略這個欄位。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// `dpm-server sign` 簽出來的 hex 簽章,簽的是 `kind` 裡的 hash 欄位。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}
```

- [ ] **Step 5: 更新 `dpm-core` 自己的既有測試**

編輯 `crates/dpm-core/tests/test.rs`,把 `prebuilt()` helper:

```rust
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
```

改成:

```rust
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
                author: None,
                signature: None,
            }
        }
```

把 `test_to_json` 裡的:

```rust
        let package_info = PackageInfo::new(
            "test_package".to_string(),
            "test_file.zip".to_string(),
            "1.0.0".to_string(),
            "A test package".to_string(),
            "hash123".to_string(),
            None,
        );
```

改成:

```rust
        let package_info = PackageInfo::new(
            "test_package".to_string(),
            "test_file.zip".to_string(),
            "1.0.0".to_string(),
            "A test package".to_string(),
            "hash123".to_string(),
            None,
            None,
        );
```

- [ ] **Step 6: 更新 `dpm-server`/`dpm` 的既有呼叫端**

編輯 `crates/dpm-server/src/action.rs`,把 `init()` 裡的:

```rust
    let package_info = PackageInfo::new(
        obj.name.to_string(),
        obj.entry.to_string(),
        obj.ver.to_string(),
        obj.description.to_string(),
        hash,
        None,
    );
```

改成:

```rust
    let package_info = PackageInfo::new(
        obj.name.to_string(),
        obj.entry.to_string(),
        obj.ver.to_string(),
        obj.description.to_string(),
        hash,
        None,
        None, // Task 4 會把這裡換成 Some(obj.author.clone())
    );
```

把既有測試 `fix_add_build_variant_records_a_source_kind_package` 裡的:

```rust
        match &version_info.kind {
            PackageKind::Source { build } => {
                assert_eq!(build, "cargo build --release");
            }
            other => panic!("expected PackageKind::Source, got {other:?}"),
        }
```

改成:

```rust
        match &version_info.kind {
            PackageKind::Source { build, .. } => {
                assert_eq!(build, "cargo build --release");
            }
            other => panic!("expected PackageKind::Source, got {other:?}"),
        }
```

(這個測試在 Task 7 還會再大改一次,這裡先讓它能編譯過。)

編輯 `crates/dpm/src/utils/fetcher.rs`,把 `build_fixture_zip` 裡的:

```rust
        let package_info = PackageInfo::new(
            "fixture".to_string(),
            "main".to_string(),
            "1.0.0".to_string(),
            "test fixture".to_string(),
            hashes_json_hash,
            None,
        );
```

改成:

```rust
        let package_info = PackageInfo::new(
            "fixture".to_string(),
            "main".to_string(),
            "1.0.0".to_string(),
            "test fixture".to_string(),
            hashes_json_hash,
            None,
            None,
        );
```

- [ ] **Step 7: 確認整個 workspace 還能編譯(功能還沒接線,只求型別過)**

Run: `cargo check --workspace`
Expected: 無編譯錯誤(`dpm-server`/`dpm` 目前都還沒使用新欄位,`DbPackage`/CLI 那邊的呼叫端在 Task 3-11 才會動到)。

- [ ] **Step 8: 跑 `dpm-core` 測試**

Run: `cargo test -p DPM-Core`
Expected: 全部通過。

- [ ] **Step 9: Commit**

```bash
git add crates/dpm-core/src/lib.rs crates/dpm-core/tests/test.rs crates/dpm-server/src/action.rs crates/dpm/src/utils/fetcher.rs
git commit -m "feat(dpm-core): add author/signature fields to PackageInfo and PackageVersionInfo"
```

---

## Task 3: `dpm-server` — `keygen` 子指令

**Files:**
- Modify: `crates/dpm-server/src/cli_parse.rs`(新增 `Keygen` struct + `Commands::Keygen` variant)
- Modify: `crates/dpm-server/src/action.rs`(新增 `keygen()` 函式 + 測試)
- Modify: `crates/dpm-server/src/main.rs`(新增 `keys_dir` 計算 + dispatch)

**Interfaces:**
- Consumes: Task 1 的 `dpm_core::generate_signing_key`。
- Produces: `keygen(obj: &Keygen, keys_dir: &Path) -> ServerResult<()>`,寫出 `keys_dir/<author_id>.priv`(32 bytes)、`keys_dir/<author_id>.pub`(32 bytes)、`keys_dir/.gitignore`(含 `*.priv`)。Task 4/6/7 的測試會呼叫這個函式來準備金鑰。

- [ ] **Step 1: 加 CLI 定義**

編輯 `crates/dpm-server/src/cli_parse.rs`,把:

```rust
#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Hash File or all in Project File
    Hash(Hash),
    /// Fix Packages.json
    Fix(Fix),
    /// Build Package
    Build(Build),
    ///Create Project
    Init(Init),
}
```

改成:

```rust
#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Hash File or all in Project File
    Hash(Hash),
    /// Fix Packages.json
    Fix(Fix),
    /// Build Package
    Build(Build),
    ///Create Project
    Init(Init),
    /// Generate an ed25519 signing key pair for a package author
    Keygen(Keygen),
}
```

(`Commands::Sign` 這個 variant **不要**在這個 task 加——`Commands` 是 `match` 在 `main.rs` 窮盡分派的 enum,一旦加了 variant 就必須同一個 task 內把對應的 `sign()` 函式跟 dispatch arm 都補齊,否則 `cargo check` 會因為 `match` 不窮盡直接編譯失敗。`Sign`/`Commands::Sign` 留到 Task 6——那裡 `sign()` 函式本身也會一起實作完成。)

在檔案最下面(`Del` struct 之後)加入:

```rust
#[derive(Args, Debug)]
pub struct Keygen {
    /// Author id (e.g. a GitHub username) this key belongs to
    pub author_id: String,
    /// Overwrite an existing key pair for this author
    #[arg(long)]
    pub force: bool,
}
```

- [ ] **Step 2: 寫失敗的測試**

在 `crates/dpm-server/src/action.rs` 的 `#[cfg(test)] mod tests` 區塊最後面加入:

```rust
    #[test]
    fn keygen_produces_32_byte_raw_key_files_and_a_gitignore() {
        let keys_dir = std::env::temp_dir().join(format!(
            "dpm-server-keygen-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&keys_dir).unwrap();

        keygen(
            &Keygen {
                author_id: "alice".to_string(),
                force: false,
            },
            &keys_dir,
        )
        .unwrap();

        let priv_bytes = std::fs::read(keys_dir.join("alice.priv")).unwrap();
        let pub_bytes = std::fs::read(keys_dir.join("alice.pub")).unwrap();
        assert_eq!(priv_bytes.len(), 32);
        assert_eq!(pub_bytes.len(), 32);

        let gitignore = std::fs::read_to_string(keys_dir.join(".gitignore")).unwrap();
        assert!(gitignore.lines().any(|l| l.trim() == "*.priv"));

        std::fs::remove_dir_all(&keys_dir).ok();
    }

    #[test]
    fn keygen_refuses_to_overwrite_without_force() {
        let keys_dir = std::env::temp_dir().join(format!(
            "dpm-server-keygen-overwrite-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&keys_dir).unwrap();

        keygen(
            &Keygen {
                author_id: "alice".to_string(),
                force: false,
            },
            &keys_dir,
        )
        .unwrap();
        let err = keygen(
            &Keygen {
                author_id: "alice".to_string(),
                force: false,
            },
            &keys_dir,
        )
        .unwrap_err();
        assert!(matches!(err, ServerError::ValidationError(_)));

        keygen(
            &Keygen {
                author_id: "alice".to_string(),
                force: true,
            },
            &keys_dir,
        )
        .unwrap();

        std::fs::remove_dir_all(&keys_dir).ok();
    }
```

- [ ] **Step 3: 執行測試,確認因缺函式而編譯失敗**

Run: `cargo test -p DPM-Server keygen`
Expected: 編譯錯誤,`keygen` 未定義。

- [ ] **Step 4: 實作 `keygen()`**

在 `crates/dpm-server/src/action.rs` 的 `hash()` 函式之前加入:

```rust
pub fn keygen(obj: &Keygen, keys_dir: &Path) -> ServerResult<()> {
    std::fs::create_dir_all(keys_dir)?;
    let priv_path = keys_dir.join(format!("{}.priv", obj.author_id));
    let pub_path = keys_dir.join(format!("{}.pub", obj.author_id));
    if !obj.force && (priv_path.exists() || pub_path.exists()) {
        return Err(ServerError::ValidationError(format!(
            "key for author '{}' already exists at {}; pass --force to overwrite",
            obj.author_id,
            keys_dir.display()
        )));
    }

    let signing_key = dpm_core::generate_signing_key()?;
    std::fs::write(&priv_path, signing_key.to_bytes())?;
    std::fs::write(&pub_path, signing_key.verifying_key().to_bytes())?;

    // 私鑰絕對不能被 commit——即使資料 repo 自己的 .gitignore 忘了擋,
    // 這裡也自己確保 keys/ 底下有一條 *.priv 規則。
    let gitignore_path = keys_dir.join(".gitignore");
    let existing = std::fs::read_to_string(&gitignore_path).unwrap_or_default();
    if !existing.lines().any(|l| l.trim() == "*.priv") {
        let mut updated = existing;
        if !updated.is_empty() && !updated.ends_with('\n') {
            updated.push('\n');
        }
        updated.push_str("*.priv\n");
        std::fs::write(&gitignore_path, updated)?;
    }

    println!(
        "Generated key pair for '{}':\n  private: {} (do not commit)\n  public:  {} (commit this)",
        obj.author_id,
        priv_path.display(),
        pub_path.display()
    );
    Ok(())
}
```

- [ ] **Step 5: 接線 `main.rs`**

編輯 `crates/dpm-server/src/main.rs`,把:

```rust
fn main() -> Result<()> {
    let cli = Cli::parse();
    let project_src = current_dir()?.join("packages");
    let repo_dir = current_dir()?.join("Repo");
    let software_repo_info = current_dir()?.join("RepoInfo.json");
    create_dir_all(&project_src)?;
    create_dir_all(&repo_dir)?;
```

改成:

```rust
fn main() -> Result<()> {
    let cli = Cli::parse();
    let project_src = current_dir()?.join("packages");
    let repo_dir = current_dir()?.join("Repo");
    let keys_dir = current_dir()?.join("keys");
    let software_repo_info = current_dir()?.join("RepoInfo.json");
    create_dir_all(&project_src)?;
    create_dir_all(&repo_dir)?;
    create_dir_all(&keys_dir)?;
```

把:

```rust
    match &cli.command {
        Commands::Hash(obj) => hash(obj, &project_src)?,
        Commands::Fix(obj) => fix(obj, &mut repo_info, &project_src)?,
        Commands::Build(obj) => build(obj, &project_src, &repo_dir)?,
        Commands::Init(obj) => init(obj, &project_src)?,
    }
```

改成(`Hash`/`Fix`/`Init` 的參數改動在 Task 5/7/4 才會真的變成這樣,這裡先只加 `Keygen` 這一個新 arm,`Hash`/`Fix`/`Init` 暫時保持原樣待後續 task 更新):

```rust
    match &cli.command {
        Commands::Hash(obj) => hash(obj, &project_src)?,
        Commands::Fix(obj) => fix(obj, &mut repo_info, &project_src)?,
        Commands::Build(obj) => build(obj, &project_src, &repo_dir)?,
        Commands::Init(obj) => init(obj, &project_src)?,
        Commands::Keygen(obj) => keygen(obj, &keys_dir)?,
    }
```

- [ ] **Step 6: 執行測試,確認通過**

Run: `cargo test -p DPM-Server keygen`
Expected: 兩個測試通過。

- [ ] **Step 7: `cargo check`/`clippy`**

Run: `cargo check -p DPM-Server && cargo clippy -p DPM-Server --all-targets -- -D warnings`
Expected: 無錯誤無警告。

- [ ] **Step 8: Commit**

```bash
git add crates/dpm-server/src/cli_parse.rs crates/dpm-server/src/action.rs crates/dpm-server/src/main.rs
git commit -m "feat(dpm-server): add keygen subcommand"
```

---

## Task 4: `dpm-server` — `init --author`

**Files:**
- Modify: `crates/dpm-server/src/cli_parse.rs`(`Init` struct 加 `author`)
- Modify: `crates/dpm-server/src/action.rs`(`init()` 簽名改動 + 測試)
- Modify: `crates/dpm-server/src/main.rs`(`Commands::Init` dispatch 補 `&keys_dir`)

**Interfaces:**
- Consumes: Task 3 的 `keygen()`(測試要先產生金鑰)。
- Produces: `init(obj: &Init, project_src: &Path, keys_dir: &Path) -> ServerResult<()>`——沒有對應公鑰時拒絕建立套件骨架。

- [ ] **Step 1: 改 CLI 定義**

編輯 `crates/dpm-server/src/cli_parse.rs`,把:

```rust
#[derive(Args, Debug)]
pub struct Init {
    /// Project Name
    pub name: String,
    ///Project Entry
    pub entry: String,
    #[arg(long, short = 'v', default_value = "0.1.0")]
    ///Project Version
    pub ver: String,
    #[arg(long, short = 'd', default_value = "description")]
    ///Project Description
    pub description: String,
}
```

改成:

```rust
#[derive(Args, Debug)]
pub struct Init {
    /// Project Name
    pub name: String,
    ///Project Entry
    pub entry: String,
    #[arg(long, short = 'v', default_value = "0.1.0")]
    ///Project Version
    pub ver: String,
    #[arg(long, short = 'd', default_value = "description")]
    ///Project Description
    pub description: String,
    /// Author id this package's key belongs to (see `dpm-server keygen`)
    #[arg(long)]
    pub author: String,
}
```

- [ ] **Step 2: 寫失敗的測試**

在 `crates/dpm-server/src/action.rs` 的 `mod tests` 裡加入:

```rust
    #[test]
    fn init_rejects_missing_author_key() {
        let project_src = std::env::temp_dir().join(format!(
            "dpm-server-init-no-key-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&project_src).unwrap();
        let keys_dir = project_src.join("keys");

        let err = init(
            &Init {
                name: "demo-pkg".to_string(),
                entry: "main.sh".to_string(),
                ver: "0.1.0".to_string(),
                description: "a demo package".to_string(),
                author: "nobody".to_string(),
            },
            &project_src,
            &keys_dir,
        )
        .unwrap_err();
        assert!(matches!(err, ServerError::ValidationError(_)));
        assert!(
            !project_src.join("demo-pkg").exists(),
            "must not create the package skeleton without a key"
        );

        std::fs::remove_dir_all(&project_src).ok();
    }

    #[test]
    fn init_records_author_in_package_info() {
        let project_src = std::env::temp_dir().join(format!(
            "dpm-server-init-author-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&project_src).unwrap();
        let keys_dir = project_src.join("keys");
        keygen(
            &Keygen {
                author_id: "alice".to_string(),
                force: false,
            },
            &keys_dir,
        )
        .unwrap();

        init(
            &Init {
                name: "demo-pkg".to_string(),
                entry: "main.sh".to_string(),
                ver: "0.1.0".to_string(),
                description: "a demo package".to_string(),
                author: "alice".to_string(),
            },
            &project_src,
            &keys_dir,
        )
        .unwrap();

        let package_info: PackageInfo =
            JsonStorage::from_json(&project_src.join("demo-pkg").join("packageInfo.json")).unwrap();
        assert_eq!(package_info.author.as_deref(), Some("alice"));
        assert_eq!(package_info.signature, None);

        std::fs::remove_dir_all(&project_src).ok();
    }
```

因為既有的 `init_creates_package_skeleton_under_given_project_src`、`build_zips_package_into_given_repo_dir`、`hash_records_entry_file_hash_and_updates_package_info`、`fix_add_build_variant_records_a_source_kind_package`、`fix_add_url_variant_rejects_non_https_before_any_network_call` 這五個既有測試都呼叫 `init(&Init { .. }, &project_src)`(舊的兩參數簽名,且 `Init` struct 字面值沒有 `author` 欄位),這一步之後它們會編譯失敗——這是預期的,Step 4 會一併修好。

- [ ] **Step 3: 執行測試,確認編譯失敗**

Run: `cargo test -p DPM-Server init`
Expected: 編譯錯誤——`Init` struct 缺 `author` 欄位(既有測試)、`init()` 只吃兩個參數但新測試傳三個。

- [ ] **Step 4: 改 `init()` 實作 + 修好既有測試**

把 `crates/dpm-server/src/action.rs` 裡的:

```rust
pub fn init(obj: &Init, project_src: &Path) -> ServerResult<()> {
    let project_path = project_src.join(obj.name.as_str());
    if !project_path.exists() {
        create_dir_all(&project_path)?;
    } else {
        return Err(ServerError::ValidationError(format!(
            "{} already exists",
            project_path.display()
        )));
    }
    File::create(project_path.join(obj.entry.as_str()))?;
    let file_path = project_path.join("hashes.json");
    File::create(&file_path)?;
    let hash = dpm_core::hash_file(&file_path)?;
    let package_info = PackageInfo::new(
        obj.name.to_string(),
        obj.entry.to_string(),
        obj.ver.to_string(),
        obj.description.to_string(),
        hash,
        None,
        None, // Task 4 會把這裡換成 Some(obj.author.clone())
    );
    JsonStorage::to_json(&package_info, &project_path.join("packageInfo.json"))?;
    Ok(())
}
```

改成:

```rust
pub fn init(obj: &Init, project_src: &Path, keys_dir: &Path) -> ServerResult<()> {
    let pubkey_path = keys_dir.join(format!("{}.pub", obj.author));
    if !pubkey_path.exists() {
        return Err(ServerError::ValidationError(format!(
            "no public key found for author '{}' at {}; run `dpm-server keygen {}` first",
            obj.author,
            pubkey_path.display(),
            obj.author
        )));
    }

    let project_path = project_src.join(obj.name.as_str());
    if !project_path.exists() {
        create_dir_all(&project_path)?;
    } else {
        return Err(ServerError::ValidationError(format!(
            "{} already exists",
            project_path.display()
        )));
    }
    File::create(project_path.join(obj.entry.as_str()))?;
    let file_path = project_path.join("hashes.json");
    File::create(&file_path)?;
    let hash = dpm_core::hash_file(&file_path)?;
    let package_info = PackageInfo::new(
        obj.name.to_string(),
        obj.entry.to_string(),
        obj.ver.to_string(),
        obj.description.to_string(),
        hash,
        None,
        Some(obj.author.clone()),
    );
    JsonStorage::to_json(&package_info, &project_path.join("packageInfo.json"))?;
    Ok(())
}
```

在既有五個測試裡,每個 `Init { name, entry, ver, description }` 字面值都補上 `author: "alice".to_string(),` 一行,並把每個對應的 `init(&Init { .. }, &project_src)` 呼叫改成三參數(先建一把 `alice` 的金鑰再呼叫)。以 `init_creates_package_skeleton_under_given_project_src` 為例,把:

```rust
        let obj = Init {
            name: "demo-pkg".to_string(),
            entry: "main.sh".to_string(),
            ver: "0.1.0".to_string(),
            description: "a demo package".to_string(),
        };

        init(&obj, &project_src).unwrap();
```

改成:

```rust
        let keys_dir = project_src.join("keys");
        keygen(
            &Keygen {
                author_id: "alice".to_string(),
                force: false,
            },
            &keys_dir,
        )
        .unwrap();

        let obj = Init {
            name: "demo-pkg".to_string(),
            entry: "main.sh".to_string(),
            ver: "0.1.0".to_string(),
            description: "a demo package".to_string(),
            author: "alice".to_string(),
        };

        init(&obj, &project_src, &keys_dir).unwrap();
```

其餘四個既有測試(`build_zips_package_into_given_repo_dir`、`hash_records_entry_file_hash_and_updates_package_info`、`fix_add_build_variant_records_a_source_kind_package`、`fix_add_url_variant_rejects_non_https_before_any_network_call`)套用同樣的修改模式:在呼叫 `init` 之前先 `keygen`,`Init` 字面值補 `author: "alice".to_string()`,`init(...)` 呼叫補上 `&keys_dir`。(`fix_add_*` 這兩個測試在 Task 7 還會再大改一次,這裡先讓它們能編譯、能跑。)

- [ ] **Step 5: 接線 `main.rs`**

編輯 `crates/dpm-server/src/main.rs`,把:

```rust
        Commands::Init(obj) => init(obj, &project_src)?,
```

改成:

```rust
        Commands::Init(obj) => init(obj, &project_src, &keys_dir)?,
```

- [ ] **Step 6: 執行測試,確認通過**

Run: `cargo test -p DPM-Server`
Expected: 全部通過(`Commands::Sign` 那個 dispatch arm 還沒加,`main.rs` 目前應該還能編譯,因為 Task 3 特意沒加那一行)。

- [ ] **Step 7: `cargo check`/`clippy`**

Run: `cargo check -p DPM-Server && cargo clippy -p DPM-Server --all-targets -- -D warnings`
Expected: 無錯誤無警告。

- [ ] **Step 8: Commit**

```bash
git add crates/dpm-server/src/cli_parse.rs crates/dpm-server/src/action.rs crates/dpm-server/src/main.rs
git commit -m "feat(dpm-server): require --author on init"
```

---

## Task 5: `dpm-server` — `hash` 擴充(`--build` / zip hash / 既有 file-walk fallback)

**Files:**
- Modify: `crates/dpm-server/Cargo.toml`(新增 `git2` 依賴)
- Modify: `crates/dpm-server/src/cli_parse.rs`(`Hash` struct 加 `build`)
- Modify: `crates/dpm-server/src/action.rs`(`hash()` 重寫 + `source_repo_commit_hash()` + 測試)
- Modify: `crates/dpm-server/src/main.rs`(`Commands::Hash` dispatch 補 `&repo_dir`)

**Interfaces:**
- Produces: `hash(obj: &Hash, project_src: &Path, repo_dir: &Path) -> ServerResult<()>`——三種模式:`obj.build` 有值 → `blake3(build_command + commit)`;沒有但 `repo_dir/<name>.zip` 存在 → 直接雜湊那個 zip;都沒有 → 既有的逐檔雜湊 `hashes.json` 邏輯(行為不變)。
- Produces: `source_repo_commit_hash(project_path: &Path) -> ServerResult<String>`(private helper,Task 7 的測試不直接呼叫,但理解 hash 邏輯需要知道它存在)。

- [ ] **Step 1: 加 `git2` 依賴**

編輯 `crates/dpm-server/Cargo.toml`,在 `[dependencies]` 加入(按字母序插入 `colored.workspace = true` 之後):

```toml
colored.workspace = true
git2 = "0.18.1"
reqwest = { workspace = true, features = ["blocking"] }
```

- [ ] **Step 2: 改 CLI 定義**

編輯 `crates/dpm-server/src/cli_parse.rs`,把:

```rust
#[derive(Args, Debug)]
pub struct Hash {
    /// Project Name
    pub package_name: String,
}
```

改成:

```rust
#[derive(Args, Debug)]
pub struct Hash {
    /// Project Name
    pub package_name: String,
    /// Build command for a `kind: source` package — when given, computes a
    /// signable hash from this command + the current git HEAD commit
    /// instead of walking `packages/<name>/`'s files.
    #[arg(long)]
    pub build: Option<String>,
}
```

- [ ] **Step 3: 寫失敗的測試**

在 `crates/dpm-server/src/action.rs` 的 `mod tests` 裡加入(需要一個真的 git repo 當 fixture,手法比照 `crates/dpm/src/utils/source_clone.rs` 測試裡的 `make_source_repo`):

```rust
    /// 在 `project_src` 這個目錄本身初始化一個 git repo 並 commit 一次
    /// (`--build` 模式的 `source_repo_commit_hash` 需要能在 `project_src`
    /// 底下找到 `.git`)。
    fn init_git_repo(project_src: &std::path::Path) {
        use git2::{Repository, Signature};
        let repo = Repository::init(project_src).unwrap();
        let mut index = repo.index().unwrap();
        index.add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None).unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = Signature::now("test", "test@example.com").unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "initial commit", &tree, &[])
            .unwrap();
    }

    #[test]
    fn hash_with_build_flag_hashes_build_command_plus_commit() {
        let project_src = std::env::temp_dir().join(format!(
            "dpm-server-hash-build-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&project_src).unwrap();
        let keys_dir = project_src.join("keys");
        keygen(
            &Keygen { author_id: "alice".to_string(), force: false },
            &keys_dir,
        )
        .unwrap();
        init(
            &Init {
                name: "demo-pkg".to_string(),
                entry: "main.sh".to_string(),
                ver: "0.1.0".to_string(),
                description: "a demo package".to_string(),
                author: "alice".to_string(),
            },
            &project_src,
            &keys_dir,
        )
        .unwrap();
        init_git_repo(&project_src);

        let repo_dir = project_src.join("unused-repo-dir");
        hash(
            &Hash {
                package_name: "demo-pkg".to_string(),
                build: Some("cargo build --release".to_string()),
            },
            &project_src,
            &repo_dir,
        )
        .unwrap();

        let package_info: PackageInfo =
            JsonStorage::from_json(&project_src.join("demo-pkg").join("packageInfo.json")).unwrap();
        assert_eq!(package_info.hash.len(), 64, "must be a full blake3 hex digest");

        // 同樣的 build_command,重跑一次必須得到一樣的 hash(HEAD 沒變)。
        hash(
            &Hash {
                package_name: "demo-pkg".to_string(),
                build: Some("cargo build --release".to_string()),
            },
            &project_src,
            &repo_dir,
        )
        .unwrap();
        let package_info_again: PackageInfo =
            JsonStorage::from_json(&project_src.join("demo-pkg").join("packageInfo.json")).unwrap();
        assert_eq!(package_info.hash, package_info_again.hash);

        // 換一個 build_command,hash 必須不同。
        hash(
            &Hash {
                package_name: "demo-pkg".to_string(),
                build: Some("cargo build".to_string()),
            },
            &project_src,
            &repo_dir,
        )
        .unwrap();
        let package_info_different: PackageInfo =
            JsonStorage::from_json(&project_src.join("demo-pkg").join("packageInfo.json")).unwrap();
        assert_ne!(package_info.hash, package_info_different.hash);

        std::fs::remove_dir_all(&project_src).ok();
    }

    #[test]
    fn hash_uses_the_zip_file_directly_when_it_already_exists() {
        let root = std::env::temp_dir().join(format!(
            "dpm-server-hash-zip-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let project_src = root.join("packages");
        let repo_dir = root.join("Repo");
        std::fs::create_dir_all(&project_src).unwrap();
        std::fs::create_dir_all(&repo_dir).unwrap();
        let keys_dir = root.join("keys");
        keygen(
            &Keygen { author_id: "alice".to_string(), force: false },
            &keys_dir,
        )
        .unwrap();
        init(
            &Init {
                name: "demo-pkg".to_string(),
                entry: "main.sh".to_string(),
                ver: "0.1.0".to_string(),
                description: "a demo package".to_string(),
                author: "alice".to_string(),
            },
            &project_src,
            &keys_dir,
        )
        .unwrap();
        build(
            &Build { package_name: "demo-pkg".to_string() },
            &project_src,
            &repo_dir,
        )
        .unwrap();
        let expected_hash = dpm_core::hash_file(&repo_dir.join("demo-pkg.zip")).unwrap();

        hash(
            &Hash { package_name: "demo-pkg".to_string(), build: None },
            &project_src,
            &repo_dir,
        )
        .unwrap();

        let package_info: PackageInfo =
            JsonStorage::from_json(&project_src.join("demo-pkg").join("packageInfo.json")).unwrap();
        assert_eq!(package_info.hash, expected_hash);

        std::fs::remove_dir_all(&root).ok();
    }
```

- [ ] **Step 4: 執行測試,確認因缺參數/邏輯而失敗**

Run: `cargo test -p DPM-Server hash_with_build_flag hash_uses_the_zip_file`
Expected: 編譯錯誤——`hash()` 目前只吃兩個參數,`Hash` struct 目前沒有 `build` 欄位。

- [ ] **Step 5: 改 `hash()` 實作**

把 `crates/dpm-server/src/action.rs` 裡的:

```rust
pub fn hash(obj: &Hash, project_src: &Path) -> ServerResult<()> {
    let project_path = project_src.join(&obj.package_name);
    let hashfile = &project_path.join("hashes.json");
    let project_info = &project_path.join("packageInfo.json");
    let mut hashes: HashMap<String, String> =
        JsonStorage::from_json(hashfile).unwrap_or_else(|_| HashMap::new());
    let mut counter: i32 = 0;
    if !project_path.exists() {
        return Err(ServerError::Core(CoreError::PackageNotFound(
            obj.package_name.clone(),
        )));
    }
    for entry in WalkDir::new(&project_path) {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() && path != hashfile {
            counter += 1;
            let hash = dpm_core::hash_file(path)?;
            let relative_path = path.strip_prefix(&project_path).unwrap_or(path);
            println!(
                "{} {} {} {}",
                counter,
                relative_path.display().to_string().yellow(),
                "===>".green(),
                hash.bold().blue(),
            );
            hashes.insert(relative_path.display().to_string(), hash);
        }
    }
    JsonStorage::to_json(&hashes, hashfile)?;
    counter += 1;
    let hash = dpm_core::hash_file(hashfile)?;
    println!(
        "{} {} {} {}",
        counter,
        "hashes.json".yellow(),
        "===>".green(),
        hash.bold().blue(),
    );
    hashes.insert("hashes.json".to_string(), hash.clone());
    JsonStorage::to_json(&hashes, hashfile)?;
    let mut package_info: PackageInfo = JsonStorage::from_json(project_info)?;
    package_info.hash = hash;
    JsonStorage::to_json(&package_info, project_info)?;
    Ok(())
}
```

改成:

```rust
pub fn hash(obj: &Hash, project_src: &Path, repo_dir: &Path) -> ServerResult<()> {
    let project_path = project_src.join(&obj.package_name);
    if !project_path.exists() {
        return Err(ServerError::Core(CoreError::PackageNotFound(
            obj.package_name.clone(),
        )));
    }
    let project_info = &project_path.join("packageInfo.json");

    let hash = if let Some(build_command) = &obj.build {
        // kind: source——沒有下載內容可雜湊,綁定 build_command 本身跟目前
        // git HEAD commit,讓 Source 套件也有東西可以簽、可以驗。
        let commit = source_repo_commit_hash(&project_path)?;
        dpm_core::hash_bytes(format!("{build_command}\n{commit}").as_bytes())
    } else {
        let zip_path = repo_dir.join(format!("{}.zip", obj.package_name));
        if zip_path.exists() {
            // kind: prebuilt,且 `dpm-server build` 已經跑過——直接雜湊那個
            // zip,讓「簽的 hash」等於「fix add 之後 client 會拿去驗證下載
            // 內容的 hash」,兩者是同一個值。
            dpm_core::hash_file(&zip_path)?
        } else {
            // 還沒 build(或者根本不是要發布的 prebuilt 套件)——退回舊行為:
            // 逐檔雜湊整個專案目錄寫進 hashes.json。
            let hashfile = &project_path.join("hashes.json");
            let mut hashes: HashMap<String, String> =
                JsonStorage::from_json(hashfile).unwrap_or_else(|_| HashMap::new());
            let mut counter: i32 = 0;
            for entry in WalkDir::new(&project_path) {
                let entry = entry?;
                let path = entry.path();
                if path.is_file() && path != hashfile {
                    counter += 1;
                    let file_hash = dpm_core::hash_file(path)?;
                    let relative_path = path.strip_prefix(&project_path).unwrap_or(path);
                    println!(
                        "{} {} {} {}",
                        counter,
                        relative_path.display().to_string().yellow(),
                        "===>".green(),
                        file_hash.bold().blue(),
                    );
                    hashes.insert(relative_path.display().to_string(), file_hash);
                }
            }
            JsonStorage::to_json(&hashes, hashfile)?;
            counter += 1;
            let hashes_json_hash = dpm_core::hash_file(hashfile)?;
            println!(
                "{} {} {} {}",
                counter,
                "hashes.json".yellow(),
                "===>".green(),
                hashes_json_hash.bold().blue(),
            );
            hashes.insert("hashes.json".to_string(), hashes_json_hash.clone());
            JsonStorage::to_json(&hashes, hashfile)?;
            hashes_json_hash
        }
    };

    let mut package_info: PackageInfo = JsonStorage::from_json(project_info)?;
    package_info.hash = hash;
    JsonStorage::to_json(&package_info, project_info)?;
    Ok(())
}

/// 解析出包含 `project_path` 的 git repo 目前 HEAD 的 commit hash(從
/// `project_path` 往上找 `.git`,所以不管 `dpm-server` 是從 repo 根目錄還是
/// 子目錄執行都找得到)。用來把一個 `kind: source` 套件簽出來的 hash 綁定
/// 在「發布當下原始碼樹的確切狀態」——光是 `build_command` 字串本身不能防止
/// 有人在不改 build 指令的情況下換掉底下的原始碼。
fn source_repo_commit_hash(project_path: &Path) -> ServerResult<String> {
    let repo = git2::Repository::discover(project_path).map_err(|e| {
        ServerError::ValidationError(format!(
            "could not find a git repository containing {}: {e}",
            project_path.display()
        ))
    })?;
    let head = repo
        .head()
        .map_err(|e| ServerError::ValidationError(format!("could not resolve HEAD: {e}")))?;
    let commit = head.peel_to_commit().map_err(|e| {
        ServerError::ValidationError(format!("could not resolve HEAD commit: {e}"))
    })?;
    Ok(commit.id().to_string())
}
```

- [ ] **Step 6: 更新既有呼叫端**

在 `crates/dpm-server/src/action.rs` 的 `mod tests` 裡,把 `hash_records_entry_file_hash_and_updates_package_info` 測試裡的:

```rust
        hash(
            &Hash {
                package_name: "demo-pkg".to_string(),
            },
            &project_src,
        )
        .unwrap();
```

改成:

```rust
        hash(
            &Hash {
                package_name: "demo-pkg".to_string(),
                build: None,
            },
            &project_src,
            &project_src.join("unused-repo-dir"),
        )
        .unwrap();
```

- [ ] **Step 7: 接線 `main.rs`**

編輯 `crates/dpm-server/src/main.rs`,把:

```rust
        Commands::Hash(obj) => hash(obj, &project_src)?,
```

改成:

```rust
        Commands::Hash(obj) => hash(obj, &project_src, &repo_dir)?,
```

- [ ] **Step 8: 執行測試,確認通過**

Run: `cargo test -p DPM-Server`
Expected: 全部通過。

- [ ] **Step 9: `cargo check`/`clippy`**

Run: `cargo check -p DPM-Server && cargo clippy -p DPM-Server --all-targets -- -D warnings`
Expected: 無錯誤無警告。

- [ ] **Step 10: Commit**

```bash
git add crates/dpm-server/Cargo.toml crates/dpm-server/src/cli_parse.rs crates/dpm-server/src/action.rs crates/dpm-server/src/main.rs
git commit -m "feat(dpm-server): extend hash to sign source and prebuilt content"
```

---

## Task 6: `dpm-server` — `sign` 子指令

**Files:**
- Modify: `crates/dpm-server/src/cli_parse.rs`(新增 `Sign` struct + `Commands::Sign` variant)
- Modify: `crates/dpm-server/src/action.rs`(新增 `sign()` 函式 + 測試)
- Modify: `crates/dpm-server/src/main.rs`(`Commands::Sign` dispatch)

**Interfaces:**
- Consumes: Task 1 的 `dpm_core::{signing_key_from_bytes, sign_hash}`,Task 4 的 `init()`(測試前置),Task 5 的 `hash()`(測試前置)。
- Produces: `sign(obj: &Sign, project_src: &Path, keys_dir: &Path) -> ServerResult<()>`——讀 `packageInfo.json.author`/`.hash`,用對應私鑰簽,寫回 `packageInfo.json.signature`。

- [ ] **Step 1: 加 CLI 定義**

編輯 `crates/dpm-server/src/cli_parse.rs`,把:

```rust
#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Hash File or all in Project File
    Hash(Hash),
    /// Fix Packages.json
    Fix(Fix),
    /// Build Package
    Build(Build),
    ///Create Project
    Init(Init),
    /// Generate an ed25519 signing key pair for a package author
    Keygen(Keygen),
}
```

改成:

```rust
#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Hash File or all in Project File
    Hash(Hash),
    /// Fix Packages.json
    Fix(Fix),
    /// Build Package
    Build(Build),
    ///Create Project
    Init(Init),
    /// Generate an ed25519 signing key pair for a package author
    Keygen(Keygen),
    /// Sign a package's packageInfo.json hash with its author's private key
    Sign(Sign),
}
```

在 `Keygen` struct 之後加入:

```rust
#[derive(Args, Debug)]
pub struct Sign {
    /// Project Name
    pub name: String,
}
```

- [ ] **Step 2: 寫失敗的測試**

在 `crates/dpm-server/src/action.rs` 的 `mod tests` 裡加入:

```rust
    #[test]
    fn sign_writes_a_verifiable_signature() {
        let project_src = std::env::temp_dir().join(format!(
            "dpm-server-sign-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&project_src).unwrap();
        let keys_dir = project_src.join("keys");
        keygen(
            &Keygen { author_id: "alice".to_string(), force: false },
            &keys_dir,
        )
        .unwrap();
        init(
            &Init {
                name: "demo-pkg".to_string(),
                entry: "main.sh".to_string(),
                ver: "0.1.0".to_string(),
                description: "a demo package".to_string(),
                author: "alice".to_string(),
            },
            &project_src,
            &keys_dir,
        )
        .unwrap();
        hash(
            &Hash { package_name: "demo-pkg".to_string(), build: None },
            &project_src,
            &project_src.join("unused-repo-dir"),
        )
        .unwrap();

        sign(
            &Sign { name: "demo-pkg".to_string() },
            &project_src,
            &keys_dir,
        )
        .unwrap();

        let package_info: PackageInfo =
            JsonStorage::from_json(&project_src.join("demo-pkg").join("packageInfo.json")).unwrap();
        let signature = package_info.signature.expect("sign must set a signature");

        let pubkey_bytes = std::fs::read(keys_dir.join("alice.pub")).unwrap();
        let verifying_key = dpm_core::verifying_key_from_bytes(&pubkey_bytes).unwrap();
        assert!(
            dpm_core::verify_hash_signature(&verifying_key, &package_info.hash, &signature).is_ok(),
            "the written signature must verify against the package's own hash and author's public key"
        );

        std::fs::remove_dir_all(&project_src).ok();
    }

    #[test]
    fn sign_rejects_a_package_with_no_recorded_author() {
        // 直接手刻一個沒有 author 的 packageInfo.json,模擬 init 之前手動
        // 亂改檔案的狀況(舊格式,或不小心刪掉了 author 欄位)。
        let project_src = std::env::temp_dir().join(format!(
            "dpm-server-sign-no-author-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let pkg_dir = project_src.join("demo-pkg");
        std::fs::create_dir_all(&pkg_dir).unwrap();
        let package_info = PackageInfo::new(
            "demo-pkg".to_string(),
            "main.sh".to_string(),
            "0.1.0".to_string(),
            "a demo package".to_string(),
            "0".repeat(64),
            None,
            None,
        );
        JsonStorage::to_json(&package_info, &pkg_dir.join("packageInfo.json")).unwrap();

        let keys_dir = project_src.join("keys");
        let err = sign(
            &Sign { name: "demo-pkg".to_string() },
            &project_src,
            &keys_dir,
        )
        .unwrap_err();
        assert!(matches!(err, ServerError::ValidationError(_)));

        std::fs::remove_dir_all(&project_src).ok();
    }
```

- [ ] **Step 3: 執行測試,確認因缺函式而編譯失敗**

Run: `cargo test -p DPM-Server sign`
Expected: 編譯錯誤,`sign` 未定義。

- [ ] **Step 4: 實作 `sign()`**

在 `crates/dpm-server/src/action.rs` 的 `hash()`/`source_repo_commit_hash()` 之後加入:

```rust
pub fn sign(obj: &Sign, project_src: &Path, keys_dir: &Path) -> ServerResult<()> {
    let path = project_src.join(&obj.name);
    let info_path = path.join("packageInfo.json");
    let mut package_info: PackageInfo = JsonStorage::from_json(&info_path)?;
    let author = package_info.author.clone().ok_or_else(|| {
        ServerError::ValidationError(format!(
            "{} has no author recorded; run `dpm-server init --author <id>` first",
            obj.name
        ))
    })?;

    let priv_path = keys_dir.join(format!("{author}.priv"));
    let priv_bytes = std::fs::read(&priv_path).map_err(|e| {
        ServerError::ValidationError(format!(
            "could not read private key for author '{author}' at {}: {e}",
            priv_path.display()
        ))
    })?;
    let signing_key = dpm_core::signing_key_from_bytes(&priv_bytes)?;

    let signature = dpm_core::sign_hash(&signing_key, &package_info.hash);
    package_info.signature = Some(signature);
    JsonStorage::to_json(&package_info, &info_path)?;
    println!("Signed {} (author: {author})", obj.name);
    Ok(())
}
```

- [ ] **Step 5: 接線 `main.rs`**

編輯 `crates/dpm-server/src/main.rs`,把:

```rust
        Commands::Keygen(obj) => keygen(obj, &keys_dir)?,
```

改成:

```rust
        Commands::Keygen(obj) => keygen(obj, &keys_dir)?,
        Commands::Sign(obj) => sign(obj, &project_src, &keys_dir)?,
```

- [ ] **Step 6: 執行測試,確認通過**

Run: `cargo test -p DPM-Server`
Expected: 全部通過。

- [ ] **Step 7: `cargo check`/`clippy`**

Run: `cargo check -p DPM-Server && cargo clippy -p DPM-Server --all-targets -- -D warnings`
Expected: 無錯誤無警告。

- [ ] **Step 8: Commit**

```bash
git add crates/dpm-server/src/action.rs crates/dpm-server/src/main.rs
git commit -m "feat(dpm-server): add sign subcommand"
```

---

## Task 7: `dpm-server` — `fix add` 作者一致性檢查

**Files:**
- Modify: `crates/dpm-server/src/action.rs`(`fix()`/`fix_add()` 簽名與邏輯改動、新增 `verify_publish_authorization()`、既有兩個 `fix_add_*` 測試改寫、新增作者不符測試)
- Modify: `crates/dpm-server/src/main.rs`(`Commands::Fix` dispatch 補 `&keys_dir`)

**Interfaces:**
- Consumes: Task 1 的 `dpm_core::{verifying_key_from_bytes, verify_hash_signature}`,Task 3-6 的 `keygen`/`init`/`hash`/`sign`(測試前置鏈)。
- Produces: `fix(obj: &Fix, repo: &mut RepoInfo, project_src: &Path, keys_dir: &Path) -> ServerResult<()>`、`fix_add(obj: &Add, repo: &mut RepoInfo, project_src: &Path, keys_dir: &Path) -> ServerResult<()>`——寫進 `RepoInfo.json` 之前一律驗證簽章與作者一致性。

- [ ] **Step 1: 寫失敗的測試(作者不符拒絕)**

在 `crates/dpm-server/src/action.rs` 的 `mod tests` 裡加入一個共用 helper 跟新測試(放在 `mod tests` 最上面,`use super::*;` 之後):

```rust
    /// 產生金鑰、跑 `init --author`+`hash`+`sign`,留下一份完整簽好名的
    /// `packageInfo.json`——Task 7 之後 `fix_add` 一律先驗證作者/簽章,
    /// 所有 `fix_add` 測試都需要這組前置。
    fn init_hash_sign(project_src: &Path, keys_dir: &Path, name: &str, author: &str) {
        keygen(
            &Keygen { author_id: author.to_string(), force: false },
            keys_dir,
        )
        .unwrap();
        init(
            &Init {
                name: name.to_string(),
                entry: "main.sh".to_string(),
                ver: "0.1.0".to_string(),
                description: "a demo package".to_string(),
                author: author.to_string(),
            },
            project_src,
            keys_dir,
        )
        .unwrap();
        hash(
            &Hash { package_name: name.to_string(), build: None },
            project_src,
            &project_src.join("unused-repo-dir"),
        )
        .unwrap();
        sign(&Sign { name: name.to_string() }, project_src, keys_dir).unwrap();
    }

    #[test]
    fn fix_add_rejects_a_second_version_signed_by_a_different_author() {
        let project_src = std::env::temp_dir().join(format!(
            "dpm-server-fix-add-author-mismatch-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&project_src).unwrap();
        let keys_dir = project_src.join("keys");

        init_hash_sign(&project_src, &keys_dir, "demo-pkg", "alice");
        let mut repo = RepoInfo::new();
        fix_add(
            &Add {
                project_name: "demo-pkg".to_string(),
                kind: AddKind::Build { build: "v1 build".to_string() },
            },
            &mut repo,
            &project_src,
            &keys_dir,
        )
        .unwrap();

        // 模擬「一個惡意 PR 想冒充既有作者發新版本」:同一個套件名稱,換一把
        // 不同作者的金鑰重新 init/hash/sign 出一份新的 packageInfo.json。
        keygen(
            &Keygen { author_id: "mallory".to_string(), force: false },
            &keys_dir,
        )
        .unwrap();
        let info_path = project_src.join("demo-pkg").join("packageInfo.json");
        let mut package_info: PackageInfo = JsonStorage::from_json(&info_path).unwrap();
        package_info.version = "0.2.0".to_string();
        package_info.author = Some("mallory".to_string());
        package_info.signature = None;
        JsonStorage::to_json(&package_info, &info_path).unwrap();
        hash(
            &Hash { package_name: "demo-pkg".to_string(), build: None },
            &project_src,
            &project_src.join("unused-repo-dir"),
        )
        .unwrap();
        sign(
            &Sign { name: "demo-pkg".to_string() },
            &project_src,
            &keys_dir,
        )
        .unwrap();

        let err = fix_add(
            &Add {
                project_name: "demo-pkg".to_string(),
                kind: AddKind::Build { build: "v2 build".to_string() },
            },
            &mut repo,
            &project_src,
            &keys_dir,
        )
        .unwrap_err();
        assert!(matches!(err, ServerError::ValidationError(_)));
        assert_eq!(
            repo.versions_of("demo-pkg").unwrap().len(),
            1,
            "the rejected v2 must not be added"
        );

        std::fs::remove_dir_all(&project_src).ok();
    }

    #[test]
    fn fix_add_rejects_a_tampered_signature() {
        let project_src = std::env::temp_dir().join(format!(
            "dpm-server-fix-add-bad-sig-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&project_src).unwrap();
        let keys_dir = project_src.join("keys");
        init_hash_sign(&project_src, &keys_dir, "demo-pkg", "alice");

        // 直接竄改已簽好的 signature 欄位(不重新 sign)。
        let info_path = project_src.join("demo-pkg").join("packageInfo.json");
        let mut package_info: PackageInfo = JsonStorage::from_json(&info_path).unwrap();
        package_info.signature = Some("0".repeat(128));
        JsonStorage::to_json(&package_info, &info_path).unwrap();

        let mut repo = RepoInfo::new();
        let err = fix_add(
            &Add {
                project_name: "demo-pkg".to_string(),
                kind: AddKind::Build { build: "cargo build".to_string() },
            },
            &mut repo,
            &project_src,
            &keys_dir,
        )
        .unwrap_err();
        assert!(matches!(err, ServerError::ValidationError(_)));
        assert!(repo.versions_of("demo-pkg").is_err());

        std::fs::remove_dir_all(&project_src).ok();
    }
```

- [ ] **Step 2: 執行測試,確認因簽名/邏輯缺失而失敗**

Run: `cargo test -p DPM-Server fix_add`
Expected: 編譯錯誤(`fix_add`/`fix` 目前只吃三個參數)或既有測試邏輯不符(尚未驗證作者)。

- [ ] **Step 3: 改 `fix()`/`fix_add()`,新增 `verify_publish_authorization()`**

把 `crates/dpm-server/src/action.rs` 裡的:

```rust
pub fn fix(obj: &Fix, repo: &mut RepoInfo, project_src: &Path) -> ServerResult<()> {
    match &obj.command {
        FixAction::Add(obj) => fix_add(obj, repo, project_src)?,
        FixAction::Del(obj) => fix_del(obj, repo)?,
    }
    Ok(())
}

fn fix_add(obj: &Add, repo: &mut RepoInfo, project_src: &Path) -> ServerResult<()> {
    let path = project_src.join(&obj.project_name);
    let pk_info: PackageInfo = JsonStorage::from_json(&path.join("packageInfo.json"))?;

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
            let hash = dpm_core::hash_file(&tmp_path)?;
            std::fs::remove_file(&tmp_path)?;

            PackageKind::Prebuilt {
                url: url.clone(),
                hash,
                file_name,
            }
        }
        AddKind::Build { build } => PackageKind::Source {
            build: build.clone(),
        },
    };

    let version_info = PackageVersionInfo {
        version: pk_info.version.clone(),
        kind,
        dependencies: pk_info.dependencies,
        entry: None,
        description: Some(pk_info.description),
    };
    repo.add_package_version(obj.project_name.clone(), version_info)?;
    Ok(())
}
```

改成:

```rust
pub fn fix(obj: &Fix, repo: &mut RepoInfo, project_src: &Path, keys_dir: &Path) -> ServerResult<()> {
    match &obj.command {
        FixAction::Add(obj) => fix_add(obj, repo, project_src, keys_dir)?,
        FixAction::Del(obj) => fix_del(obj, repo)?,
    }
    Ok(())
}

/// `fix_add` 寫進 `RepoInfo.json` 之前的守門檢查,兩種 kind 共用:
/// 1. `packageInfo.json` 一定要有 `author`/`signature`/`hash`。
/// 2. `signature` 必須是 `author` 的公鑰對 `hash` 的合法簽章。
/// 3. 如果這個套件名稱在 `repo` 裡已經有版本,新版本的 `author` 必須跟第一次
///    發布時登記的 author 相同——這是防冒名頂替的核心檢查。沒有既有版本代表
///    這是第一次發布,直接放行(沒有「跟誰比對」的問題)。
fn verify_publish_authorization(
    pk_info: &PackageInfo,
    repo: &RepoInfo,
    project_name: &str,
    keys_dir: &Path,
) -> ServerResult<()> {
    let author = pk_info.author.as_deref().ok_or_else(|| {
        ServerError::ValidationError(format!(
            "{project_name}'s packageInfo.json has no author; run `dpm-server init --author <id>`"
        ))
    })?;
    let signature = pk_info.signature.as_deref().ok_or_else(|| {
        ServerError::ValidationError(format!(
            "{project_name}'s packageInfo.json has no signature; run `dpm-server sign {project_name}` first"
        ))
    })?;

    let pubkey_path = keys_dir.join(format!("{author}.pub"));
    let pubkey_bytes = std::fs::read(&pubkey_path).map_err(|e| {
        ServerError::ValidationError(format!(
            "could not read public key for author '{author}' at {}: {e}",
            pubkey_path.display()
        ))
    })?;
    let verifying_key = dpm_core::verifying_key_from_bytes(&pubkey_bytes).map_err(|e| {
        ServerError::ValidationError(format!("invalid public key for author '{author}': {e}"))
    })?;
    dpm_core::verify_hash_signature(&verifying_key, &pk_info.hash, signature).map_err(|e| {
        ServerError::ValidationError(format!(
            "signature verification failed for {project_name}: {e}"
        ))
    })?;

    if let Ok(versions) = repo.versions_of(project_name) {
        if let Some(existing) = versions.first() {
            if existing.author.as_deref() != Some(author) {
                return Err(ServerError::ValidationError(format!(
                    "{project_name} was first published by author '{}', but this version is signed by '{author}' — authorship cannot change without manual review",
                    existing.author.as_deref().unwrap_or("<unknown>")
                )));
            }
        }
    }
    Ok(())
}

fn fix_add(obj: &Add, repo: &mut RepoInfo, project_src: &Path, keys_dir: &Path) -> ServerResult<()> {
    let path = project_src.join(&obj.project_name);
    let pk_info: PackageInfo = JsonStorage::from_json(&path.join("packageInfo.json"))?;

    verify_publish_authorization(&pk_info, repo, &obj.project_name, keys_dir)?;

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
        AddKind::Build { build } => PackageKind::Source {
            build: build.clone(),
            hash: Some(pk_info.hash.clone()),
        },
    };

    let version_info = PackageVersionInfo {
        version: pk_info.version.clone(),
        kind,
        dependencies: pk_info.dependencies.clone(),
        entry: None,
        description: Some(pk_info.description.clone()),
        author: pk_info.author.clone(),
        signature: pk_info.signature.clone(),
    };
    repo.add_package_version(obj.project_name.clone(), version_info)?;
    Ok(())
}
```

- [ ] **Step 4: 改寫既有兩個 `fix_add_*` 測試**

把 `fix_add_build_variant_records_a_source_kind_package`(這個測試 Task 4 Step 4 已經改過一次——補了 `keygen` 呼叫、`Init` 的 `author` 欄位、`init(...)` 的第三個 `&keys_dir` 參數——下面這段是 Task 4 跑完之後、Task 7 開始之前這個測試的實際內容,不是最原始版本):

```rust
    #[test]
    fn fix_add_build_variant_records_a_source_kind_package() {
        let project_src = std::env::temp_dir().join(format!(
            "dpm-server-action-fix-add-build-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&project_src).unwrap();
        let keys_dir = project_src.join("keys");
        keygen(
            &Keygen {
                author_id: "alice".to_string(),
                force: false,
            },
            &keys_dir,
        )
        .unwrap();

        init(
            &Init {
                name: "demo-pkg".to_string(),
                entry: "main.sh".to_string(),
                ver: "0.1.0".to_string(),
                description: "a demo package".to_string(),
                author: "alice".to_string(),
            },
            &project_src,
            &keys_dir,
        )
        .unwrap();

        let mut repo = RepoInfo::new();
        let add = Add {
            project_name: "demo-pkg".to_string(),
            kind: AddKind::Build {
                build: "cargo build --release".to_string(),
            },
        };
        fix_add(&add, &mut repo, &project_src).unwrap();

        let version_info = repo.latest_version("demo-pkg").unwrap();
        assert_eq!(version_info.version, "0.1.0");
        match &version_info.kind {
            PackageKind::Source { build, .. } => {
                assert_eq!(build, "cargo build --release");
            }
            other => panic!("expected PackageKind::Source, got {other:?}"),
        }

        std::fs::remove_dir_all(&project_src).ok();
    }
```

改成:

```rust
    #[test]
    fn fix_add_build_variant_records_a_source_kind_package() {
        let project_src = std::env::temp_dir().join(format!(
            "dpm-server-action-fix-add-build-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&project_src).unwrap();
        let keys_dir = project_src.join("keys");
        init_hash_sign(&project_src, &keys_dir, "demo-pkg", "alice");

        let mut repo = RepoInfo::new();
        let add = Add {
            project_name: "demo-pkg".to_string(),
            kind: AddKind::Build {
                build: "cargo build --release".to_string(),
            },
        };
        fix_add(&add, &mut repo, &project_src, &keys_dir).unwrap();

        let version_info = repo.latest_version("demo-pkg").unwrap();
        assert_eq!(version_info.version, "0.1.0");
        assert_eq!(version_info.author.as_deref(), Some("alice"));
        assert!(version_info.signature.is_some());
        match &version_info.kind {
            PackageKind::Source { build, hash } => {
                assert_eq!(build, "cargo build --release");
                assert!(hash.is_some());
            }
            other => panic!("expected PackageKind::Source, got {other:?}"),
        }

        std::fs::remove_dir_all(&project_src).ok();
    }
```

把 `fix_add_url_variant_rejects_non_https_before_any_network_call`(同樣先看 Task 4 Step 4 跑完之後的實際內容,不是最原始版本):

```rust
    #[test]
    fn fix_add_url_variant_rejects_non_https_before_any_network_call() {
        let project_src = std::env::temp_dir().join(format!(
            "dpm-server-action-fix-add-url-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&project_src).unwrap();
        let keys_dir = project_src.join("keys");
        keygen(
            &Keygen {
                author_id: "alice".to_string(),
                force: false,
            },
            &keys_dir,
        )
        .unwrap();

        init(
            &Init {
                name: "demo-pkg".to_string(),
                entry: "main.sh".to_string(),
                ver: "0.1.0".to_string(),
                description: "a demo package".to_string(),
                author: "alice".to_string(),
            },
            &project_src,
            &keys_dir,
        )
        .unwrap();

        let mut repo = RepoInfo::new();
        let add = Add {
            project_name: "demo-pkg".to_string(),
            kind: AddKind::Url {
                url: "http://example.com/pkg.zip".to_string(),
                file_name: None,
            },
        };
        let err = fix_add(&add, &mut repo, &project_src).unwrap_err();
        assert!(matches!(err, ServerError::ValidationError(_)));
        assert!(
            repo.versions_of("demo-pkg").is_err(),
            "a rejected url must not leave a partial entry in RepoInfo"
        );

        std::fs::remove_dir_all(&project_src).ok();
    }
```

改成:

```rust
    #[test]
    fn fix_add_url_variant_rejects_non_https_before_any_network_call() {
        let project_src = std::env::temp_dir().join(format!(
            "dpm-server-action-fix-add-url-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&project_src).unwrap();
        let keys_dir = project_src.join("keys");
        init_hash_sign(&project_src, &keys_dir, "demo-pkg", "alice");

        let mut repo = RepoInfo::new();
        let add = Add {
            project_name: "demo-pkg".to_string(),
            kind: AddKind::Url {
                url: "http://example.com/pkg.zip".to_string(),
                file_name: None,
            },
        };
        let err = fix_add(&add, &mut repo, &project_src, &keys_dir).unwrap_err();
        assert!(matches!(err, ServerError::ValidationError(_)));
        assert!(
            repo.versions_of("demo-pkg").is_err(),
            "a rejected url must not leave a partial entry in RepoInfo"
        );

        std::fs::remove_dir_all(&project_src).ok();
    }
```

- [ ] **Step 5: 接線 `main.rs`**

編輯 `crates/dpm-server/src/main.rs`,把:

```rust
        Commands::Fix(obj) => fix(obj, &mut repo_info, &project_src)?,
```

改成:

```rust
        Commands::Fix(obj) => fix(obj, &mut repo_info, &project_src, &keys_dir)?,
```

- [ ] **Step 6: 執行測試,確認通過**

Run: `cargo test -p DPM-Server`
Expected: 全部通過,包含新增的作者不符/簽章竄改測試。

- [ ] **Step 7: `cargo check`/`clippy`**

Run: `cargo check -p DPM-Server && cargo clippy -p DPM-Server --all-targets -- -D warnings`
Expected: 無錯誤無警告。

- [ ] **Step 8: Commit**

```bash
git add crates/dpm-server/src/action.rs crates/dpm-server/src/main.rs
git commit -m "feat(dpm-server): verify author signature and identity on fix add"
```

---

## Task 8: `dpm` client DB — migration 0004 + `db.rs`/`models.rs`

**Files:**
- Create: `crates/dpm/migrations/0004_package_signatures.up.sql`
- Create: `crates/dpm/migrations/0004_package_signatures.down.sql`
- Modify: `crates/dpm/src/utils/db.rs`(`run_migrations`、`COLUMNS`、`row_to_package`、`insert`)
- Modify: `crates/dpm/src/utils/models.rs`(`DbPackage` struct + `new()`)
- Modify: `crates/dpm/tests/db_tests.rs`、`crates/dpm/src/utils/resolver.rs`、`crates/dpm/src/utils/fetcher.rs`、`crates/dpm/src/context.rs`(所有 `DbPackage::new` 呼叫端補兩個新參數)
- Modify: `crates/dpm/src/action.rs`(`sync_source` 的 `DbPackage::new` 呼叫端——先補 `None, None`,Task 10 才會填真值)

**Interfaces:**
- Produces: `DbPackage { .., author: Option<String>, signature: Option<String> }`,`DbPackage::new(source, name, version, kind, url, hash, filename, build_command, description, entry, dependencies, author: Option<String>, signature: Option<String>)`。
- Produces: `LocalRepo` 表新增 nullable `author`/`signature` 兩欄,`Db::COLUMNS`/`row_to_package`/`insert` 同步。

- [ ] **Step 1: 新增 migration 檔**

建立 `crates/dpm/migrations/0004_package_signatures.up.sql`:

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
    entry TEXT,
    dependencies TEXT,
    author TEXT,
    signature TEXT,
    PRIMARY KEY (source, name, version)
);
```

建立 `crates/dpm/migrations/0004_package_signatures.down.sql`(還原成 0003 的形狀):

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
    entry TEXT,
    dependencies TEXT,
    PRIMARY KEY (source, name, version)
);
```

- [ ] **Step 2: 把新 migration 接進 `run_migrations`**

編輯 `crates/dpm/src/utils/db.rs`,在 `run_migrations` 裡既有的 0003 區塊之後加入:

```rust
        std::fs::write(
            migrations_dir.join("0003_nullable_entry.down.sql"),
            include_str!("../../migrations/0003_nullable_entry.down.sql"),
        )
        .map_err(|e| ClientError::Core(IoError(e)))?;
        std::fs::write(
            migrations_dir.join("0004_package_signatures.up.sql"),
            include_str!("../../migrations/0004_package_signatures.up.sql"),
        )
        .map_err(|e| ClientError::Core(IoError(e)))?;
        std::fs::write(
            migrations_dir.join("0004_package_signatures.down.sql"),
            include_str!("../../migrations/0004_package_signatures.down.sql"),
        )
        .map_err(|e| ClientError::Core(IoError(e)))?;
```

- [ ] **Step 3: 改 `DbPackage`**

編輯 `crates/dpm/src/utils/models.rs`,把:

```rust
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
    pub entry: Option<String>,
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
        entry: Option<String>,
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
            entry,
            dependencies,
        }
    }
```

改成:

```rust
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
    pub entry: Option<String>,
    pub dependencies: Option<Vec<Dependency>>,
    /// 發布這個版本的作者 id——只有官方來源(`repo_url == OFFICIAL_REPO_URL`)
    /// 的套件才會有值,第三方來源永遠是 `None`。
    pub author: Option<String>,
    /// `dpm-server sign` 簽出來的 hex 簽章,簽的是 `hash` 欄位。
    pub signature: Option<String>,
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
        entry: Option<String>,
        dependencies: Option<Vec<Dependency>>,
        author: Option<String>,
        signature: Option<String>,
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
            entry,
            dependencies,
            author,
            signature,
        }
    }
```

- [ ] **Step 4: 改 `db.rs` 的 `COLUMNS`/`row_to_package`/`insert`**

編輯 `crates/dpm/src/utils/db.rs`,把:

```rust
const COLUMNS: &str =
    "source, name, version, kind, url, hash, filename, build_command, description, entry, dependencies";
```

改成:

```rust
const COLUMNS: &str =
    "source, name, version, kind, url, hash, filename, build_command, description, entry, dependencies, author, signature";
```

把 `row_to_package` 裡的:

```rust
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
        })
```

改成:

```rust
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
        })
```

把 `insert` 裡的:

```rust
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
        ];
        conn.execute(
            &format!(
                "INSERT INTO LocalRepo ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)"
            ),
            params,
        )
```

改成:

```rust
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
                "INSERT INTO LocalRepo ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)"
            ),
            params,
        )
```

- [ ] **Step 5: 更新既有呼叫端**

在 `crates/dpm/tests/db_tests.rs`、`crates/dpm/src/utils/resolver.rs`(`pkg()` helper)、`crates/dpm/src/utils/fetcher.rs`(`fixture_db_row`)、`crates/dpm/src/context.rs`(`mod tests` 裡的 `DbPackage::new` 呼叫)這四個檔案裡,每個 `DbPackage::new(...)` 呼叫的最後補上兩個 `None`。例如 `crates/dpm/tests/db_tests.rs` 的 `sample_pkg`:

```rust
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
            Some("bin/test_pkg".to_string()),
            None,
            None,
            None,
        )
    }
```

同樣的模式套用到 `resolver.rs::pkg()`、`fetcher.rs::fixture_db_row()`、`context.rs` 裡 `two_for_test_contexts_are_fully_isolated_from_each_other` 測試中的呼叫。

在 `crates/dpm/src/action.rs` 的 `sync_source` 裡,把:

```rust
                ctx.db
                    .insert(DbPackage::new(
                        &source.alias,
                        name,
                        &version_info.version,
                        kind_str,
                        url,
                        hash,
                        filename,
                        build_command,
                        version_info.description.as_deref().unwrap_or(""),
                        version_info.entry.clone(),
                        dependencies,
                    ))
                    .await?;
```

改成(先補 `None, None` 佔位,Task 10 會把這裡換成真的驗證流程):

```rust
                ctx.db
                    .insert(DbPackage::new(
                        &source.alias,
                        name,
                        &version_info.version,
                        kind_str,
                        url,
                        hash,
                        filename,
                        build_command,
                        version_info.description.as_deref().unwrap_or(""),
                        version_info.entry.clone(),
                        dependencies,
                        None, // Task 10 會換成 version_info.author.clone()
                        None, // Task 10 會換成 version_info.signature.clone()
                    ))
                    .await?;
```

- [ ] **Step 6: 跑測試,確認通過**

Run: `cargo test -p DPM`
Expected: 全部通過,包含 migration 相關的 `db_tests.rs`。

- [ ] **Step 7: `cargo check`/`clippy`**

Run: `cargo check -p DPM && cargo clippy -p DPM --all-targets -- -D warnings`
Expected: 無錯誤無警告。

- [ ] **Step 8: Commit**

```bash
git add crates/dpm/migrations/0004_package_signatures.up.sql crates/dpm/migrations/0004_package_signatures.down.sql crates/dpm/src/utils/db.rs crates/dpm/src/utils/models.rs crates/dpm/tests/db_tests.rs crates/dpm/src/utils/resolver.rs crates/dpm/src/utils/fetcher.rs crates/dpm/src/context.rs crates/dpm/src/action.rs
git commit -m "feat(dpm): add author/signature columns to LocalRepo"
```

---

## Task 9: `dpm` client — `OFFICIAL_REPO_URL` 可見性 + `official_key_url`

**Files:**
- Modify: `crates/dpm/src/utils/system.rs`

**Interfaces:**
- Produces: `pub(crate) const OFFICIAL_REPO_URL: &str`(原本是 private `const`,改成 crate 內可見)。
- Produces: `pub(crate) fn official_key_url(repo_url: &str, author_id: &str) -> String`——回傳 `keys/<author_id>.pub` 的 raw content URL,跟既有 `official_repo_info_url` 共用同一個 `raw_content_url` 轉換 helper。

- [ ] **Step 1: 寫失敗的測試**

在 `crates/dpm/src/utils/system.rs` 的 `mod tests` 裡,`official_repo_info_url_derives_raw_content_url_from_repo_url` 之後加入:

```rust
    #[test]
    fn official_key_url_derives_raw_content_url_for_an_author() {
        assert_eq!(
            official_key_url("https://github.com/Derrick-Program/DPM-Server", "alice"),
            "https://raw.githubusercontent.com/Derrick-Program/DPM-Server/main/keys/alice.pub"
        );
    }
```

- [ ] **Step 2: 執行測試,確認因缺函式而編譯失敗**

Run: `cargo test -p DPM official_key_url`
Expected: 編譯錯誤,`official_key_url` 未定義。

- [ ] **Step 3: 改常數可見性 + 加 `raw_content_url`/`official_key_url`**

把:

```rust
const OFFICIAL_REPO_URL: &str = "https://github.com/Derrick-Program/DPM-Server";

/// 把 `https://github.com/<owner>/<repo>` 轉成該 repo 在 `main` 分支上
/// `RepoInfo.json` 的 raw content URL。
fn official_repo_info_url(repo_url: &str) -> String {
    format!(
        "{}/main/RepoInfo.json",
        repo_url.replacen(
            "https://github.com/",
            "https://raw.githubusercontent.com/",
            1
        )
    )
}
```

改成:

```rust
/// 「官方」套件來源的預設 git repo 位址——見上方模組文件註解。這次改成
/// `pub(crate)` 是因為簽章驗證(`action.rs` 的 `sync_source`/`install_resolved`)
/// 要拿它跟 `Source.repo_url` 比對,判斷是否該套用簽章驗證這個安全閘門
/// (刻意比對這個寫死的常數,不是使用者本機 `config.json` 可以自己編輯的
/// `alias` 字串)。
pub(crate) const OFFICIAL_REPO_URL: &str = "https://github.com/Derrick-Program/DPM-Server";

/// 把 `https://github.com/<owner>/<repo>` 轉成該 repo 在 `main` 分支上某個
/// 檔案路徑的 raw content URL。`official_repo_info_url`/`official_key_url`
/// 共用這個轉換,只差要抓的路徑。
fn raw_content_url(repo_url: &str, path: &str) -> String {
    format!(
        "{}/main/{path}",
        repo_url.replacen(
            "https://github.com/",
            "https://raw.githubusercontent.com/",
            1
        )
    )
}

/// 把 `https://github.com/<owner>/<repo>` 轉成該 repo 在 `main` 分支上
/// `RepoInfo.json` 的 raw content URL。
fn official_repo_info_url(repo_url: &str) -> String {
    raw_content_url(repo_url, "RepoInfo.json")
}

/// 把 `https://github.com/<owner>/<repo>` 轉成該 repo 在 `main` 分支上
/// `keys/<author_id>.pub` 的 raw content URL——跟 `official_repo_info_url`
/// 同一個 host 轉換規則,只差路徑。
pub(crate) fn official_key_url(repo_url: &str, author_id: &str) -> String {
    raw_content_url(repo_url, &format!("keys/{author_id}.pub"))
}
```

- [ ] **Step 4: 執行測試,確認通過**

Run: `cargo test -p DPM system::tests`
Expected: 全部通過,包含既有的 `official_repo_info_url_derives_raw_content_url_from_repo_url`(輸出不變)跟新的 `official_key_url_derives_raw_content_url_for_an_author`。

- [ ] **Step 5: `cargo check`/`clippy`**

Run: `cargo check -p DPM && cargo clippy -p DPM --all-targets -- -D warnings`
Expected: 無錯誤無警告(`OFFICIAL_REPO_URL`/`official_key_url` 目前還沒被其他地方使用,clippy 可能報 `dead_code`——這是預期的,Task 10/11 會消耗掉它們,若 clippy 在這一步報 dead_code,先確認是不是還沒被用到,若是則暫時忽略,等 Task 10 接上後重跑一次確認警告消失)。

- [ ] **Step 6: Commit**

```bash
git add crates/dpm/src/utils/system.rs
git commit -m "refactor(dpm): expose OFFICIAL_REPO_URL and derive official_key_url"
```

---

## Task 10: `dpm` client — `sync_source()` 簽章驗證整合

**Files:**
- Modify: `crates/dpm/src/action.rs`(新增 `verify_official_signature()`、`sync_source`/`sync_source_inner` 改動、測試)

**Interfaces:**
- Consumes: Task 1 的 `dpm_core::{VerifyingKey, verifying_key_from_bytes, verify_hash_signature}`,Task 9 的 `official_key_url`/`OFFICIAL_REPO_URL`,Task 8 的 `DbPackage::new` 新簽名。
- Produces: `async fn verify_official_signature(repo_url: &str, author: &str, hash: &str, signature: &str, key_cache: &mut HashMap<String, VerifyingKey>) -> ClientResult<()>`——Task 11 會重用同一個函式。
- Produces: `ActionInfo::sync_source_inner(ctx: &Context, source: &Source, is_official: bool) -> ClientResult<()>`(新的可測試 seam,`sync_source` 變成薄包裝)。

- [ ] **Step 1: 寫失敗的測試**

在 `crates/dpm/src/action.rs` 檔案最底部(`installed_package_names_tests` 之後)加入新的 `#[cfg(test)] mod sync_source_tests`:

```rust
#[cfg(test)]
mod sync_source_tests {
    use super::*;
    use dpm_core::{PackageKind, PackageVersionInfo};
    use std::collections::HashMap as StdHashMap;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// 跟 `crates/dpm/src/utils/fetcher.rs::serve_once` 同一個手法:接受一次
    /// 連線,回傳固定的 body,忽略實際請求的路徑——`official_key_url`/
    /// `official_repo_info_url` 算出來的路徑不影響這個 mock 的回應內容。
    fn serve_once(body: Vec<u8>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        format!("http://127.0.0.1:{port}")
    }

    /// JSON 形狀跟 `dpm_core::RepoInfo` 的 serde 表示法一致(`{"packages": {...}}`),
    /// 但不透過 `RepoInfo::add_package_version`(那個方法在 `server` feature
    /// 底下,`dpm` 沒有開這個 feature)——直接組出等價的 JSON 內容給
    /// `fetch_update_repo_info` 解析。
    #[derive(serde::Serialize)]
    struct FakeRepoInfo {
        packages: StdHashMap<String, Vec<PackageVersionInfo>>,
    }

    #[tokio::test]
    async fn sync_source_inner_skips_invalid_signature_but_keeps_valid_one_when_official() {
        let signing_key = dpm_core::generate_signing_key().unwrap();
        let pubkey_bytes = signing_key.verifying_key().to_bytes().to_vec();
        let good_hash = "a".repeat(64);
        let good_sig = dpm_core::sign_hash(&signing_key, &good_hash);
        let bad_sig = "0".repeat(128);

        let mut packages = StdHashMap::new();
        packages.insert(
            "good-pkg".to_string(),
            vec![PackageVersionInfo {
                version: "1.0.0".to_string(),
                kind: PackageKind::Prebuilt {
                    url: "https://example.com/good.zip".to_string(),
                    hash: good_hash.clone(),
                    file_name: "good.zip".to_string(),
                },
                dependencies: None,
                entry: None,
                description: None,
                author: Some("alice".to_string()),
                signature: Some(good_sig),
            }],
        );
        packages.insert(
            "bad-pkg".to_string(),
            vec![PackageVersionInfo {
                version: "1.0.0".to_string(),
                kind: PackageKind::Prebuilt {
                    url: "https://example.com/bad.zip".to_string(),
                    hash: good_hash.clone(),
                    file_name: "bad.zip".to_string(),
                },
                dependencies: None,
                entry: None,
                description: None,
                author: Some("alice".to_string()),
                signature: Some(bad_sig),
            }],
        );
        let body = serde_json::to_vec(&FakeRepoInfo { packages }).unwrap();

        let key_url = serve_once(pubkey_bytes);
        let repo_info_url = serve_once(body);

        let source = Source {
            alias: "official".to_string(),
            repo_url: key_url,
            repo_info: repo_info_url,
        };
        let root = tempfile::tempdir().unwrap();
        let ctx = Context::for_test(root.path()).await.unwrap();

        ActionInfo::sync_source_inner(&ctx, &source, true).await.unwrap();

        let all = ctx.db.read_all().await.unwrap();
        assert_eq!(all.len(), 1, "the badly-signed package must be skipped");
        assert_eq!(all[0].name, "good-pkg");
    }

    #[tokio::test]
    async fn sync_source_inner_skips_verification_when_not_official() {
        let mut packages = StdHashMap::new();
        packages.insert(
            "unsigned-pkg".to_string(),
            vec![PackageVersionInfo {
                version: "1.0.0".to_string(),
                kind: PackageKind::Prebuilt {
                    url: "https://example.com/unsigned.zip".to_string(),
                    hash: "irrelevant".to_string(),
                    file_name: "unsigned.zip".to_string(),
                },
                dependencies: None,
                entry: None,
                description: None,
                author: None,
                signature: None,
            }],
        );
        let body = serde_json::to_vec(&FakeRepoInfo { packages }).unwrap();
        let repo_info_url = serve_once(body);

        let source = Source {
            alias: "third-party".to_string(),
            repo_url: "https://example.com/some-other-repo".to_string(),
            repo_info: repo_info_url,
        };
        let root = tempfile::tempdir().unwrap();
        let ctx = Context::for_test(root.path()).await.unwrap();

        ActionInfo::sync_source_inner(&ctx, &source, false).await.unwrap();

        let all = ctx.db.read_all().await.unwrap();
        assert_eq!(
            all.len(),
            1,
            "unsigned packages from non-official sources are not gated"
        );
    }
}
```

- [ ] **Step 2: 執行測試,確認因缺函式而編譯失敗**

Run: `cargo test -p DPM sync_source_tests`
Expected: 編譯錯誤,`ActionInfo::sync_source_inner` 未定義。

- [ ] **Step 3: 加 `HashMap` 匯入 + `verify_official_signature` + 改寫 `sync_source`**

編輯 `crates/dpm/src/action.rs`,把檔案頂部的 `use` 區塊:

```rust
use crate::utils::privilege::{chown_dir_to_sudo_user, drop_privileges_for_build};
use crate::{
    clone_package_source, fetch_and_verify_prebuilt, parse_package_spec, place_package,
    resolve_install_set, system::*, unzip_file, ClientError, ClientResult, Context, DbPackage,
    Setting, Source, SourceAction,
};
use colored::Colorize;
use dpm_core::CoreError;
use dpm_core::{Dependency, JsonStorage, PackageKind, RepoInfo};
use std::fs::{remove_dir_all, remove_file};
use std::path::Path;
```

改成:

```rust
use crate::utils::privilege::{chown_dir_to_sudo_user, drop_privileges_for_build};
use crate::{
    clone_package_source, fetch_and_verify_prebuilt, parse_package_spec, place_package,
    resolve_install_set, system::*, unzip_file, ClientError, ClientResult, Context, DbPackage,
    Setting, Source, SourceAction,
};
use colored::Colorize;
use dpm_core::CoreError;
use dpm_core::{Dependency, JsonStorage, PackageKind, RepoInfo, VerifyingKey};
use std::collections::HashMap;
use std::fs::{remove_dir_all, remove_file};
use std::path::Path;
```

加在整個 `impl ActionInfo { ... }` 區塊**之前**(也就是檔案頂部 `use` 區塊之後、`type ParsedInstallSpec`/`struct ActionInfo`/`impl ActionInfo` 這些定義之前)——這是一個 module-level 的 free function,不是 `ActionInfo` 的 method。它下面的呼叫端(`sync_source_inner`、Task 11 的 `install_resolved_with_gate`)都是 unqualified 呼叫 `verify_official_signature(...)`,不是 `Self::verify_official_signature(...)`;如果把這段程式碼插在 `impl ActionInfo` 區塊內部(例如插在 `sync_source` 方法前面,那個位置實際上在 impl 區塊裡),它會被編譯器當成 `ActionInfo` 的 associated function,後面所有 unqualified 呼叫都會編譯失敗(「cannot find function `verify_official_signature` in this scope」)。**不要**只看字面上「在 sync_source 前面」就插進 impl 區塊裡:

```rust
/// 抓(或重用快取的)`author` 的 ed25519 公鑰(從官方來源的
/// `keys/<author>.pub`),驗證 `signature` 是不是 `hash` 的合法簽章。
/// `sync_source_inner`(同一次 sync,多個套件版本,值得快取)、
/// `install_resolved_with_gate`(單一套件安裝,獨立重新抓一次,不信任
/// `sync_source` 當初看到的任何東西——這就是縱深防禦的重點)共用同一份實作。
async fn verify_official_signature(
    repo_url: &str,
    author: &str,
    hash: &str,
    signature: &str,
    key_cache: &mut HashMap<String, VerifyingKey>,
) -> ClientResult<()> {
    if !key_cache.contains_key(author) {
        let key_url = official_key_url(repo_url, author);
        let bytes = reqwest::get(&key_url)
            .await
            .map_err(|e| ClientError::Core(CoreError::NetworkError(e.to_string())))?
            .bytes()
            .await
            .map_err(|e| ClientError::Core(CoreError::NetworkError(e.to_string())))?;
        let verifying_key = dpm_core::verifying_key_from_bytes(bytes.as_ref())?;
        key_cache.insert(author.to_string(), verifying_key);
    }
    let verifying_key = key_cache.get(author).expect("just inserted above");
    dpm_core::verify_hash_signature(verifying_key, hash, signature)?;
    Ok(())
}
```

把:

```rust
    /// 抓某一個來源的完整索引,清空該來源在本地 DB 的舊資料,把每個套件的每個
    /// 版本各自插入一列。`update()`(既有來源全部重整)、`init_update()`
    /// (`init()` 第一次執行時的初始灌入)共用這個邏輯——原本兩處各自複製一份
    /// 幾乎相同的程式碼。
    async fn sync_source(ctx: &Context, source: &Source) -> ClientResult<()> {
        let mut remote_repo = RepoInfo::new();
        remote_repo
            .fetch_update_repo_info(&source.repo_info)
            .await?;

        ctx.db.clear_table_for_source(&source.alias).await?;

        for (name, versions) in remote_repo.get_package_handler() {
            for version_info in versions {
                let dependencies: Option<Vec<dpm_core::Dependency>> =
                    version_info.dependencies.as_ref().map(|deps| {
                        deps.iter()
                            .map(|dep| Dependency::new(&dep.name, &dep.version))
                            .collect::<Vec<_>>()
                    });
                let (kind_str, url, hash, filename, build_command) =
                    version_info.kind.to_db_fields();
                ctx.db
                    .insert(DbPackage::new(
                        &source.alias,
                        name,
                        &version_info.version,
                        kind_str,
                        url,
                        hash,
                        filename,
                        build_command,
                        version_info.description.as_deref().unwrap_or(""),
                        version_info.entry.clone(),
                        dependencies,
                        None, // Task 10 會換成 version_info.author.clone()
                        None, // Task 10 會換成 version_info.signature.clone()
                    ))
                    .await?;
            }
        }
        Ok(())
    }
```

改成:

```rust
    /// 抓某一個來源的完整索引,清空該來源在本地 DB 的舊資料,把每個套件的每個
    /// 版本各自插入一列。`update()`(既有來源全部重整)、`init_update()`
    /// (`init()` 第一次執行時的初始灌入)共用這個邏輯——原本兩處各自複製一份
    /// 幾乎相同的程式碼。真正的邏輯在 `sync_source_inner`,這裡只是計算
    /// `is_official` 這個安全閘門再委派過去。
    async fn sync_source(ctx: &Context, source: &Source) -> ClientResult<()> {
        let is_official = source.repo_url == OFFICIAL_REPO_URL;
        Self::sync_source_inner(ctx, source, is_official).await
    }

    /// `sync_source` 的實際邏輯,`is_official` 從呼叫端算好傳進來(不是在
    /// 這裡重新比對 `OFFICIAL_REPO_URL`)——這樣測試才能在不打真網路的情況下
    /// 強制走簽章驗證路徑(`OFFICIAL_REPO_URL` 是寫死指向真實 GitHub 的常數,
    /// 測試不可能讓 `source.repo_url` 真的等於它)。
    async fn sync_source_inner(
        ctx: &Context,
        source: &Source,
        is_official: bool,
    ) -> ClientResult<()> {
        let mut remote_repo = RepoInfo::new();
        remote_repo
            .fetch_update_repo_info(&source.repo_info)
            .await?;

        ctx.db.clear_table_for_source(&source.alias).await?;

        let mut key_cache: HashMap<String, VerifyingKey> = HashMap::new();

        for (name, versions) in remote_repo.get_package_handler() {
            for version_info in versions {
                let (kind_str, url, hash, filename, build_command) =
                    version_info.kind.to_db_fields();

                if is_official {
                    let author = version_info.author.as_deref();
                    let signature = version_info.signature.as_deref();
                    let verified = match (author, signature, hash.as_deref()) {
                        (Some(author), Some(signature), Some(hash)) => {
                            verify_official_signature(
                                &source.repo_url,
                                author,
                                hash,
                                signature,
                                &mut key_cache,
                            )
                            .await
                        }
                        _ => Err(ClientError::Core(CoreError::SignatureInvalid(
                            "missing author, signature, or hash".to_string(),
                        ))),
                    };
                    if let Err(e) = verified {
                        println!(
                            "{} skipping {name}@{} (author: {}) — signature verification failed: {e}",
                            "Warning:".yellow(),
                            version_info.version,
                            author.unwrap_or("<none>"),
                        );
                        continue;
                    }
                }

                let dependencies: Option<Vec<dpm_core::Dependency>> =
                    version_info.dependencies.as_ref().map(|deps| {
                        deps.iter()
                            .map(|dep| Dependency::new(&dep.name, &dep.version))
                            .collect::<Vec<_>>()
                    });
                ctx.db
                    .insert(DbPackage::new(
                        &source.alias,
                        name,
                        &version_info.version,
                        kind_str,
                        url,
                        hash,
                        filename,
                        build_command,
                        version_info.description.as_deref().unwrap_or(""),
                        version_info.entry.clone(),
                        dependencies,
                        version_info.author.clone(),
                        version_info.signature.clone(),
                    ))
                    .await?;
            }
        }
        Ok(())
    }
```

- [ ] **Step 4: 執行測試,確認通過**

Run: `cargo test -p DPM sync_source_tests`
Expected: 兩個測試通過。

- [ ] **Step 5: `cargo check`/`clippy`**

Run: `cargo check -p DPM && cargo clippy -p DPM --all-targets -- -D warnings`
Expected: 無錯誤無警告。

- [ ] **Step 6: Commit**

```bash
git add crates/dpm/src/action.rs
git commit -m "feat(dpm): verify author signatures when syncing the official source"
```

---

## Task 11: `dpm` client — 安裝路徑簽章驗證

**Files:**
- Modify: `crates/dpm/src/action.rs`(`install_resolved`/`install_resolved_with_gate` + 測試)

**Interfaces:**
- Consumes: Task 10 的 `verify_official_signature`,Task 9 的 `OFFICIAL_REPO_URL`。
- Produces: `ActionInfo::install_resolved_with_gate(&self, all_packages: &[DbPackage], is: &[ParsedInstallSpec], is_official: impl Fn(&str) -> bool) -> ClientResult<()>`(新的可測試 seam,`install_resolved` 變成薄包裝)。

- [ ] **Step 1: 寫失敗的測試**

在 `crates/dpm/src/action.rs` 底部的 `mod sync_source_tests` 之後加入新的 `#[cfg(test)] mod install_resolved_tests`:

```rust
#[cfg(test)]
mod install_resolved_tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    fn serve_once(body: Vec<u8>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        format!("http://127.0.0.1:{port}")
    }

    #[tokio::test]
    async fn install_resolved_rejects_a_signature_that_does_not_match_the_recorded_hash() {
        // 模擬本機 DB 被竄改的情境:提供的公鑰是「真正的作者金鑰」,但
        // DB 裡記錄的 signature 其實是另一把金鑰簽的——代表 DB 裡的
        // author/signature/hash 三者已經兜不起來,驗證必須失敗。
        let real_author_key = dpm_core::generate_signing_key().unwrap();
        let attacker_key = dpm_core::generate_signing_key().unwrap();
        let hash = "b".repeat(64);
        let tampered_signature = dpm_core::sign_hash(&attacker_key, &hash);
        let pubkey_bytes = real_author_key.verifying_key().to_bytes().to_vec();

        let key_url = serve_once(pubkey_bytes);

        let root = tempfile::tempdir().unwrap();
        let ctx = Context::for_test(root.path()).await.unwrap();
        let setting = Setting {
            sources: vec![Source {
                alias: "official".to_string(),
                repo_url: key_url.clone(),
                repo_info: key_url,
            }],
        };
        let action = ActionInfo::new(ctx, vec![], false, setting);

        let pkg = DbPackage::new(
            "official",
            "tampered-pkg",
            "1.0.0",
            "prebuilt",
            Some("https://example.com/tampered.zip".to_string()),
            Some(hash),
            Some("tampered.zip".to_string()),
            None,
            "test",
            None,
            None,
            Some("alice".to_string()),
            Some(tampered_signature),
        );
        let all_packages = vec![pkg];
        let is = vec![(None, "tampered-pkg".to_string(), None)];

        let err = action
            .install_resolved_with_gate(&all_packages, &is, |_| true)
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("INSECURE"),
            "expected an INSECURE-tagged rejection, got: {msg}"
        );
    }

    #[tokio::test]
    async fn install_resolved_rejects_a_package_missing_author_or_signature_when_official() {
        let root = tempfile::tempdir().unwrap();
        let ctx = Context::for_test(root.path()).await.unwrap();
        let setting = Setting {
            sources: vec![Source {
                alias: "official".to_string(),
                repo_url: "http://127.0.0.1:1".to_string(),
                repo_info: "http://127.0.0.1:1".to_string(),
            }],
        };
        let action = ActionInfo::new(ctx, vec![], false, setting);

        let pkg = DbPackage::new(
            "official",
            "unsigned-pkg",
            "1.0.0",
            "prebuilt",
            Some("https://example.com/unsigned.zip".to_string()),
            Some("c".repeat(64)),
            Some("unsigned.zip".to_string()),
            None,
            "test",
            None,
            None,
            None,
            None,
        );
        let all_packages = vec![pkg];
        let is = vec![(None, "unsigned-pkg".to_string(), None)];

        let err = action
            .install_resolved_with_gate(&all_packages, &is, |_| true)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("INSECURE"));
    }
}
```

- [ ] **Step 2: 執行測試,確認因缺函式而編譯失敗**

Run: `cargo test -p DPM install_resolved_tests`
Expected: 編譯錯誤,`install_resolved_with_gate` 未定義。

- [ ] **Step 3: 改 `install_resolved`**

把 `crates/dpm/src/action.rs` 裡的:

```rust
    async fn install_resolved(
        &self,
        all_packages: &[DbPackage],
        is: &[ParsedInstallSpec],
    ) -> ClientResult<()> {
        if !is.is_empty() {
            let resolved = resolve_install_set(all_packages, is)?;
            for (source_alias, name, version) in resolved {
                let pkg = name.as_str();
                let repo_package_info = all_packages
                    .iter()
                    .find(|p| p.source == source_alias && p.name == name && p.version == version)
                    .ok_or_else(|| {
                        ClientError::Core(CoreError::PackageNotFound(format!(
                            "{source_alias}/{name}@{version}"
                        )))
                    })?;
                if self.verbose {
                    println!("{}\n\n  {}", pkg.on_green(), "Downloading...".yellow());
                }
```

改成:

```rust
    async fn install_resolved(
        &self,
        all_packages: &[DbPackage],
        is: &[ParsedInstallSpec],
    ) -> ClientResult<()> {
        self.install_resolved_with_gate(all_packages, is, |repo_url| repo_url == OFFICIAL_REPO_URL)
            .await
    }

    /// `install_resolved` 的實際邏輯,「這個來源是不是官方來源」透過
    /// `is_official` 閉包傳入,而不是在這裡直接比對 `OFFICIAL_REPO_URL`——
    /// 理由跟 `sync_source_inner` 一樣:讓測試能在不打真網路的情況下強制
    /// 走簽章驗證路徑。
    async fn install_resolved_with_gate(
        &self,
        all_packages: &[DbPackage],
        is: &[ParsedInstallSpec],
        is_official: impl Fn(&str) -> bool,
    ) -> ClientResult<()> {
        if !is.is_empty() {
            let resolved = resolve_install_set(all_packages, is)?;
            let mut key_cache: HashMap<String, VerifyingKey> = HashMap::new();
            for (source_alias, name, version) in resolved {
                let pkg = name.as_str();
                let repo_package_info = all_packages
                    .iter()
                    .find(|p| p.source == source_alias && p.name == name && p.version == version)
                    .ok_or_else(|| {
                        ClientError::Core(CoreError::PackageNotFound(format!(
                            "{source_alias}/{name}@{version}"
                        )))
                    })?;

                let source = self
                    .setting_config
                    .sources
                    .iter()
                    .find(|s| s.alias == source_alias)
                    .ok_or_else(|| {
                        ClientError::ConfigError(format!(
                            "source '{source_alias}' is not configured"
                        ))
                    })?;
                if is_official(&source.repo_url) {
                    let author = repo_package_info.author.as_deref();
                    let signature = repo_package_info.signature.as_deref();
                    let hash = repo_package_info.hash.as_deref();
                    match (author, signature, hash) {
                        (Some(author), Some(signature), Some(hash)) => {
                            verify_official_signature(
                                &source.repo_url,
                                author,
                                hash,
                                signature,
                                &mut key_cache,
                            )
                            .await
                            .map_err(|e| {
                                ClientError::SystemError(format!(
                                    "INSECURE: {pkg}@{version} failed signature verification (author: {author}): {e}"
                                ))
                            })?;
                        }
                        _ => {
                            return Err(ClientError::SystemError(format!(
                                "INSECURE: {pkg}@{version} is missing author/signature/hash from the official source; refusing to install"
                            )));
                        }
                    }
                }

                if self.verbose {
                    println!("{}\n\n  {}", pkg.on_green(), "Downloading...".yellow());
                }
```

檔案其餘部分(`staging_root_base` 之後到函式結尾)維持不變,只是現在整個函式體被包在 `install_resolved_with_gate` 裡而不是 `install_resolved` 裡——確認縮排/大括號配對正確,`install_resolved_with_gate` 的收尾 `}` 對應原本 `install_resolved` 的收尾 `}`。

- [ ] **Step 4: 執行測試,確認通過**

Run: `cargo test -p DPM install_resolved_tests`
Expected: 兩個測試通過。

- [ ] **Step 5: 跑整個 `dpm` 測試套件(確認 `install`/`upgrade` 既有測試沒被這次改動弄壞)**

Run: `cargo test -p DPM`
Expected: 全部通過。

- [ ] **Step 6: `cargo check`/`clippy`**

Run: `cargo check -p DPM && cargo clippy -p DPM --all-targets -- -D warnings`
Expected: 無錯誤無警告。

- [ ] **Step 7: Commit**

```bash
git add crates/dpm/src/action.rs
git commit -m "feat(dpm): verify author signatures before installing from the official source"
```

---

## Task 12: Workspace 收尾驗證 + 人工端對端檢查清單

**Files:**
- 無程式碼變動(這個 task 只跑驗證指令跟人工操作)。

**Interfaces:**
- Consumes: Task 1-11 的全部產出。

- [ ] **Step 1: 整個 workspace 格式化**

Run: `cargo fmt --all`
Expected: 無輸出(或只是套用格式,無需人工介入)。

- [ ] **Step 2: 整個 workspace clippy**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: 無錯誤無警告。若 Task 9 留下的 `dead_code` 警告還在,這裡必須是乾淨的(Task 10/11 應該已經消耗掉 `OFFICIAL_REPO_URL`/`official_key_url`)。

- [ ] **Step 3: 整個 workspace 測試**

Run: `cargo test --workspace`
Expected: 全部通過,包含 `DPM-Core`/`DPM-Server`/`DPM` 三個 crate 的全部單元測試與整合測試。

- [ ] **Step 4: 格式檢查(CI 用的嚴格模式)**

Run: `cargo fmt --all -- --check`
Expected: 無輸出(代表 Step 1 已經讓一切符合格式)。

- [ ] **Step 5: 若本機有設定 Infisical,額外跑一次 `just pre-commit`**

Run: `just pre-commit`
Expected: fmt + lint + test 全部通過(這個指令本質上是 Step 1-3 的封裝,差別只是透過 `infisical run` 注入環境變數執行——如果本機沒有 `just env-login` 過,跳過這一步,Step 1-4 已經涵蓋等價的檢查)。

- [ ] **Step 6: 人工端對端驗證——完整發布流程**

在一個乾淨的暫存目錄(不是這個 workspace,`dpm-server` 是獨立的一次性 CLI,吃 cwd 底下的 `packages/`/`Repo/`/`keys/`/`RepoInfo.json`)裡:

```bash
mkdir -p /tmp/dpm-server-e2e && cd /tmp/dpm-server-e2e
cargo run --manifest-path <workspace 路徑>/Cargo.toml -p DPM-Server -- keygen alice
cargo run --manifest-path <workspace 路徑>/Cargo.toml -p DPM-Server -- init demo-pkg main.sh --author alice
cargo run --manifest-path <workspace 路徑>/Cargo.toml -p DPM-Server -- hash demo-pkg
cargo run --manifest-path <workspace 路徑>/Cargo.toml -p DPM-Server -- sign demo-pkg
cargo run --manifest-path <workspace 路徑>/Cargo.toml -p DPM-Server -- fix add demo-pkg build "echo build"
cat RepoInfo.json
```

Expected: `RepoInfo.json` 的 `demo-pkg` 版本條目含 `"author":"alice"` 跟 `"signature":"<64-byte hex>"`。

- [ ] **Step 7: 人工端對端驗證——作者不符拒絕**

延續 Step 6 的目錄:

```bash
cargo run --manifest-path <workspace 路徑>/Cargo.toml -p DPM-Server -- keygen mallory
# 手動編輯 packages/demo-pkg/packageInfo.json,把 "author" 改成 "mallory",
# "signature" 欄位整個刪掉,"version" 改成 "0.2.0"
cargo run --manifest-path <workspace 路徑>/Cargo.toml -p DPM-Server -- hash demo-pkg
cargo run --manifest-path <workspace 路徑>/Cargo.toml -p DPM-Server -- sign demo-pkg
cargo run --manifest-path <workspace 路徑>/Cargo.toml -p DPM-Server -- fix add demo-pkg build "echo build v2"
```

Expected: 最後一個 `fix add` 指令印出 `ValidationError`,提到 `alice`/`mallory` 作者不符,`RepoInfo.json` 仍只有一個版本(`0.1.0`)。

- [ ] **Step 8: 人工確認驗證清單全部打勾**

比對 `docs/superpowers/specs/2026-07-26-package-author-verification-design.md` 的「驗證清單」章節:

- [ ] `cargo check --workspace` / `cargo clippy --workspace --all-targets` / `cargo test --workspace` 通過(Step 2-3 已涵蓋)
- [ ] `dpm-server keygen`/`init --author`/`sign`/`fix add` 走一輪完整流程,產出的 `RepoInfo.json` 含 `author`/`signature`(Step 6 已涵蓋)
- [ ] 手動測試:同套件用不同作者金鑰嘗試 `fix add` 第二版,確認被拒絕(Step 7 已涵蓋,Task 7 的 `fix_add_rejects_a_second_version_signed_by_a_different_author` 自動化測試也涵蓋同一情境)
- [ ] 手動測試:`dpm update` 對著一個混合正確/錯誤簽章的測試 `RepoInfo.json` 跑,確認錯誤簽章的版本被跳過、正確的照常進本機 DB,且 `update` 本身不報失敗(Task 10 的 `sync_source_inner_skips_invalid_signature_but_keeps_valid_one_when_official` 自動化測試涵蓋同一情境;若要人工再驗一次,可以把 Step 6 產出的 `RepoInfo.json` 複製一份、手動竄改某個版本的 `signature`,指到一個本機 `python3 -m http.server` 服務,設定 `dpm` 的 source 指向它跑 `dpm update` 觀察警告輸出)
- [ ] 手動測試:本機竄改 DB 裡某筆 `signature`,`dpm install` 該套件確認被 INSECURE 拒裝(Task 11 的 `install_resolved_rejects_a_signature_that_does_not_match_the_recorded_hash` 自動化測試涵蓋同一情境;若要人工再驗一次,可以直接對 `~/.local/share/com.duacodie.dpm/LocalRepo.db`——或對應平台的路徑——用 `sqlite3`/`turso` CLI 手動 `UPDATE LocalRepo SET signature = '00...'`,再跑 `dpm install demo-pkg` 觀察是否印出 `INSECURE:` 並拒絕安裝)

- [ ] **Step 9: 收尾 commit(若 Step 1 的 `cargo fmt` 有改動)**

```bash
git add -A
git status
```

若 `git status` 顯示只有格式化造成的變動,commit:

```bash
git commit -m "chore: cargo fmt after package author verification feature"
```

若沒有任何變動(Task 1-11 每個 task 都已經各自 commit 過格式化過的程式碼),跳過這一步。
