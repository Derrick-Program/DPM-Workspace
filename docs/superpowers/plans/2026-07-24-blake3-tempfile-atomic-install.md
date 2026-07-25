# blake3 + tempfile 原子安裝 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 `dpm`/`dpm-server` 各自複製一份的 SHA256 `hasher()` 換成 `dpm-core` 共用的 blake3 版本,並把 `dpm` 的套件安裝流程從「下載到固定 `/tmp`、直接解壓進最終路徑」改成 staging + 原子 rename,失敗不留半殘狀態、升級中斷不影響舊版本。

**Architecture:** 兩個獨立可測試階段。Task 1 只動 hash 演算法(`dpm-core` 新增共用 `hash_file`,`dpm`/`dpm-server` 刪掉各自的 `hasher()` 改呼叫它),不碰安裝流程,風險最低、完全獨立。Task 2 建立在 Task 1 之上,把 `dpm` 的下載/解壓/安裝改成 staging 目錄 + 兩段式原子 rename,只影響 `crates/dpm`,不動 `dpm-server`。

**Tech Stack:** Rust 2021、`blake3`(取代 `sha2`)、`tempfile`(既有 dev-dependency 升成正式 dependency)。

## Global Constraints

- Hash 演算法乾脆換掉,不做多演算法相容(不加 `"sha256:"` 這種前綴),沒有需要相容的舊發布資料。
- `hasher`/`hash_file` 這個功能從 `dpm`/`dpm-server` 各自複製的版本收斂成 `dpm-core` 共用一份。
- staging 目錄必須跟目標安裝目錄在同一個檔案系統下(`std::fs::rename` 只有同檔案系統才是真原子操作),不能用系統 `/tmp`。
- 任何安裝步驟失敗,已安裝的舊版本(如果有)必須維持完整可用,不能留下半殘目錄。
- `tempfile` 從 `crates/dpm/Cargo.toml` 的 `[dev-dependencies]` 升成 `[dependencies]`。
- 每個 task 完成後執行 `cargo build --workspace` 確認整個 workspace 仍能編譯。

---

## Task 1: `dpm-core` 共用 `hash_file`(blake3 取代 sha2)

**Files:**

- Modify: `crates/dpm-core/Cargo.toml`
- Modify: `crates/dpm-core/src/lib.rs`
- Modify: `crates/dpm-core/tests/test.rs`
- Modify: `crates/dpm/Cargo.toml`
- Modify: `crates/dpm/src/action.rs`
- Modify: `crates/dpm-server/Cargo.toml`
- Modify: `crates/dpm-server/src/action.rs`
- Modify: `Cargo.toml`(根)

**Interfaces:**

- Produces:`pub fn hash_file(path: &Path) -> CoreResult<String>`(`dpm_core` 頂層,無 feature gate,client/server 都能用)。
- Consumes:無(這個 task 不依賴任何其他未完成的東西)。

- [ ] **Step 1: 寫失敗的測試(先紅)**

編輯 `crates/dpm-core/tests/test.rs`,在 `mod tests` 區塊(跟其他測試同一層,`test_dependency_serde` 之後)加入:

```rust
    #[test]
    fn test_hash_file_is_deterministic_and_content_sensitive() {
        let file_a = "hash_test_a.txt";
        let file_b = "hash_test_b.txt";
        std::fs::write(file_a, b"hello world").unwrap();
        std::fs::write(file_b, b"different content").unwrap();

        let hash_a1 = hash_file(Path::new(file_a)).unwrap();
        let hash_a2 = hash_file(Path::new(file_a)).unwrap();
        let hash_b = hash_file(Path::new(file_b)).unwrap();

        assert_eq!(
            hash_a1, hash_a2,
            "hashing the same file twice must be deterministic"
        );
        assert_ne!(hash_a1, hash_b, "different content must hash differently");
        assert_eq!(
            hash_a1.len(),
            64,
            "blake3 hex output is 32 bytes = 64 hex chars"
        );

        std::fs::remove_file(file_a).unwrap();
        std::fs::remove_file(file_b).unwrap();
    }
```

- [ ] **Step 2: 確認編不過(紅燈)**

Run: `cargo test -p dpm_core test_hash_file_is_deterministic_and_content_sensitive 2>&1 | tail -20`
Expected: 編譯錯誤,`cannot find function 'hash_file' in this scope`(`hash_file` 還不存在)。

- [ ] **Step 3: 加 blake3 依賴**

編輯 `crates/dpm-core/Cargo.toml`,`[dependencies]` 區塊加一行(維持字母序,插在 `anyhow.workspace = true` 之前):

```toml
[dependencies]
anyhow.workspace = true
blake3 = "1.8.5"
futures-util.workspace = true
reqwest.workspace = true
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
tokio.workspace = true
```

- [ ] **Step 4: 實作 `hash_file`**

編輯 `crates/dpm-core/src/lib.rs`,在 `use` 區塊補 `use std::io::Read;` 已經有了(第 6 行已經 `use std::{..., io::Read, ...};`),在 `Dependency` struct 定義之前(檔案第 9 行之前)加入:

```rust
/// 對檔案內容算 blake3 hash,回傳小寫十六進位字串。
/// client(安裝驗證)、server(發布時算 hash)共用同一份實作。
pub fn hash_file(path: &Path) -> CoreResult<String> {
    let mut file = std::fs::File::open(path)?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;
    Ok(blake3::hash(&buffer).to_hex().to_string())
}
```

(`Path` 已經在檔案頂端 `use std::{..., path::Path};` 匯入,`CoreResult` 是 `mod error; pub use error::*;` 帶出來的,不用額外 import。)

- [ ] **Step 5: 跑測試,確認變綠**

Run: `cargo test -p dpm_core test_hash_file_is_deterministic_and_content_sensitive -- --nocapture`
Expected: 1 passed。

- [ ] **Step 6: `dpm` 改用共用 `hash_file`,移除本地 `hasher()`**

編輯 `crates/dpm/src/action.rs`:

頂部 import 區塊,移除 `use sha2::{Digest, Sha256};`(整行刪除)。

刪除整個 `hasher` 方法(檔案最後,`fn hasher(file_path: &Path) -> ClientResult<String> { ... }` 那 10 行連同前面的空行一起刪掉,`impl ActionInfo` 的結尾 `}` 保留)。

`install()` 裡呼叫 `Self::hasher(&ori_path)?` 那一行(目前在 `let hash = Self::hasher(&ori_path)?;`)改成:

```rust
                let hash = dpm_core::hash_file(&ori_path)?;
```

(`ClientError` 對 `dpm_core::CoreError` 已經有 `#[from]`,`?` 會自動轉型,不用額外 `.map_err`。)

- [ ] **Step 7: `dpm` 移除 sha2/hex/digest 依賴**

編輯 `crates/dpm/Cargo.toml`,移除 `digest.workspace = true`、`hex.workspace = true`、`sha2.workspace = true` 三行(`digest`/`hex` 是這次順手清掉的死依賴——`digest` 現在整個 crate 沒有任何 `use digest::` 呼叫,`hex` 只有原本 `hasher()` 用到,兩個都跟 sha2 一起進、一起退)。

- [ ] **Step 8: `dpm-server` 改用共用 `hash_file`,移除本地 `hasher()`**

編輯 `crates/dpm-server/src/action.rs`:

頂部移除 `use sha2::{Digest, Sha256};`。

刪除整個 `hasher` function(檔案開頭,`pub fn hasher(file_path: &Path) -> Result<String> { ... }` 那幾行)。

檔案內所有 `hasher(...)` 呼叫(共 3 處:`hash()` function 裡兩次、`fix_add` 一次)改成 `dpm_core::hash_file(...)`:

```rust
pub fn hash(obj: &Hash) -> AnyhowResult<()> {
    let project_path = PROJECT_SRC.get().unwrap().join(&obj.packagename);
    let hashfile = &project_path.join("hashes.json");
    let project_info = &project_path.join("packageInfo.json");
    let mut hashes: HashMap<String, String> =
        JsonStorage::from_json(hashfile).unwrap_or_else(|_| HashMap::new());
    let mut counter: i32 = 0;
    if !project_path.exists() {
        return Err(anyhow::anyhow!(
            "\nPackage: {} {}",
            obj.packagename.yellow(),
            "Not found!".red()
        ));
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
    let mut hashes: HashMap<String, String> =
        JsonStorage::from_json(hashfile).unwrap_or_else(|_| HashMap::new());
    counter += 1;
    let hash = dpm_core::hash_file(hashfile)?;
    println!(
        "{} {} {} {}",
        counter,
        hashfile.file_name().unwrap().to_str().unwrap().yellow(),
        "===>".green(),
        hash.bold().blue(),
    );
    hashes.insert(
        hashfile.file_name().unwrap().to_str().unwrap().to_string(),
        hash.clone(),
    );
    JsonStorage::to_json(&hashes, hashfile)?;
    let mut package_info: PackageInfo = JsonStorage::from_json(project_info)?;
    package_info.hash = hash;
    JsonStorage::to_json(&package_info, project_info)?;
    Ok(())
}
```

`fix_add` 裡的 `hash: hasher(&package)?,` 改成 `hash: dpm_core::hash_file(&package)?,`。

- [ ] **Step 9: `dpm-server` 移除 sha2/hex/digest 依賴**

編輯 `crates/dpm-server/Cargo.toml`,移除 `digest.workspace = true`、`hex.workspace = true`、`sha2.workspace = true` 三行(理由同 Step 7)。

- [ ] **Step 10: 根 `Cargo.toml` 移除不再共用的依賴**

編輯根 `Cargo.toml` 的 `[workspace.dependencies]`,移除 `digest`、`hex`、`sha2` 三行(這三個現在整個 workspace 沒有任何 crate 使用)。

- [ ] **Step 11: 整個 workspace 編譯 + 全部測試**

Run: `cargo build --workspace 2>&1 | tail -40`
Expected: 無錯誤。

Run: `cargo test --workspace 2>&1 | tail -40`
Expected: 全部測試通過(含新的 `test_hash_file_is_deterministic_and_content_sensitive`)。

- [ ] **Step 12: Commit**

```bash
git add crates/dpm-core/Cargo.toml crates/dpm-core/src/lib.rs crates/dpm-core/tests/test.rs \
  crates/dpm/Cargo.toml crates/dpm/src/action.rs \
  crates/dpm-server/Cargo.toml crates/dpm-server/src/action.rs \
  Cargo.toml
git commit -m "$(cat <<'EOF'
feat: replace sha2 with shared blake3 hash_file in dpm-core

dpm and dpm-server each had their own copy of a SHA256 hasher()
function (a known duplication debt). Both now call dpm_core's shared
hash_file(), and the hash algorithm itself switches to blake3. No
multi-algorithm compatibility shim — there's no published data that
needs the old SHA256 values honored.

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: tempfile 原子安裝(`dpm` client)

**Files:**

- Modify: `crates/dpm/Cargo.toml`
- Modify: `crates/dpm/src/utils/db.rs`
- Modify: `crates/dpm/src/action.rs`

**Interfaces:**

- Consumes:Task 1 的 `dpm_core::hash_file(path: &Path) -> CoreResult<String>`。
- Produces:`Db::download_file(&self, name: &str, dest_path: &Path) -> ClientResult<()>`(簽名變更,新增 `dest_path` 參數,取代原本寫死 `/tmp`)。

- [ ] **Step 1: `tempfile` 升成正式依賴**

編輯 `crates/dpm/Cargo.toml`,`[dependencies]` 區塊(維持字母序,插在 `thiserror.workspace = true` 之前)加入:

```toml
tempfile = "3.10.1"
```

`[dev-dependencies]` 區塊的 `tempfile = "3.10.1"` 整行移除(升成正式依賴後,test target 自動就能用,不用重複列)。

- [ ] **Step 2: 寫失敗的測試(先紅)——原子換裝邏輯**

在 `crates/dpm/src/action.rs` 檔案最後(`impl ActionInfo { ... }` 區塊結束的 `}` 之後)加入一個私有輔助函式跟它的測試。先只加測試模組(函式本體先留空回傳 `unimplemented!()`,讓測試跑起來但失敗,驗證測試本身能正確抓到問題):

```rust
/// 把 staging 目錄裡已經驗證好的內容原子性換裝進最終安裝路徑。
/// 若 install_path 已存在(升級情境),先把舊的搬進 staging_root/previous
/// (同檔案系統 rename,不是複製),新內容才搬進最終路徑——任何一步失敗,
/// install_path 都維持在「舊版本完整存在」或「還沒開始換裝」其中一種完好
/// 狀態,不會出現半殘目錄。呼叫端的 staging TempDir drop 時會把搬出來的
/// 舊版本一併清掉。
fn swap_into_install_dir(
    new_dir: &Path,
    install_path: &Path,
    staging_root: &Path,
) -> ClientResult<()> {
    unimplemented!()
}

#[cfg(test)]
mod atomic_install_tests {
    use super::swap_into_install_dir;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn fresh_install_moves_new_dir_into_place() {
        let root = tempdir().unwrap();
        let new_dir = root.path().join("new");
        let install_path = root.path().join("install");
        fs::create_dir_all(&new_dir).unwrap();
        fs::write(new_dir.join("marker.txt"), b"v2").unwrap();

        swap_into_install_dir(&new_dir, &install_path, root.path()).unwrap();

        assert!(install_path.join("marker.txt").exists());
        assert_eq!(
            fs::read_to_string(install_path.join("marker.txt")).unwrap(),
            "v2"
        );
        assert!(!new_dir.exists(), "new_dir should have been moved, not copied");
    }

    #[test]
    fn upgrade_replaces_old_content_with_new() {
        let root = tempdir().unwrap();
        let new_dir = root.path().join("new");
        let install_path = root.path().join("install");
        fs::create_dir_all(&new_dir).unwrap();
        fs::write(new_dir.join("marker.txt"), b"v2").unwrap();
        fs::create_dir_all(&install_path).unwrap();
        fs::write(install_path.join("marker.txt"), b"v1").unwrap();

        swap_into_install_dir(&new_dir, &install_path, root.path()).unwrap();

        assert_eq!(
            fs::read_to_string(install_path.join("marker.txt")).unwrap(),
            "v2",
            "install_path must contain the new version's content after swap"
        );
    }

    #[test]
    fn old_install_survives_if_new_dir_is_missing() {
        let root = tempdir().unwrap();
        let missing_new_dir = root.path().join("does-not-exist");
        let install_path = root.path().join("install");
        fs::create_dir_all(&install_path).unwrap();
        fs::write(install_path.join("marker.txt"), b"v1").unwrap();

        let result = swap_into_install_dir(&missing_new_dir, &install_path, root.path());

        assert!(result.is_err(), "swap must fail if new_dir doesn't exist");
        assert_eq!(
            fs::read_to_string(install_path.join("marker.txt")).unwrap(),
            "v1",
            "old install must be untouched when the swap fails before completion"
        );
    }
}
```

- [ ] **Step 3: 確認測試失敗(紅燈)**

Run: `cargo test -p DPM atomic_install_tests -- --nocapture 2>&1 | tail -30`
Expected: 前兩個測試因為 `unimplemented!()` panic 而 FAILED,第三個測試(`old_install_survives_if_new_dir_is_missing`)可能也 FAILED(因為 `unimplemented!()` 會 panic 而不是回傳 `Err`)。三個都紅,符合預期。

- [ ] **Step 4: 實作 `swap_into_install_dir`**

把 Step 2 加入的函式本體從 `unimplemented!()` 改成:

```rust
fn swap_into_install_dir(
    new_dir: &Path,
    install_path: &Path,
    staging_root: &Path,
) -> ClientResult<()> {
    if install_path.exists() {
        let backup = staging_root.join("previous");
        std::fs::rename(install_path, &backup)
            .map_err(|e| ClientError::Core(CoreError::IoError(e)))?;
    }
    std::fs::rename(new_dir, install_path)
        .map_err(|e| ClientError::Core(CoreError::IoError(e)))?;
    Ok(())
}
```

- [ ] **Step 5: 跑測試,確認變綠**

Run: `cargo test -p DPM atomic_install_tests -- --nocapture`
Expected: 3 passed。

- [ ] **Step 6: `Db::download_file` 改吃目標路徑,不寫死 `/tmp`**

編輯 `crates/dpm/src/utils/db.rs`,把 `download_file` 方法整個換成:

```rust
    pub async fn download_file(&self, name: &str, dest_path: &Path) -> ClientResult<()> {
        let package = self
            .read_one(name)
            .await?
            .ok_or_else(|| ClientError::Core(PackageNotFound(name.to_string())))?;
        let url = &package.url;
        let req = reqwest::get(url)
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

(拿掉原本 `let filename = Path::new("/tmp").join(&package.filename);` 這行,呼叫端自己決定路徑。)

- [ ] **Step 7: `install()` 改用 staging 目錄 + 原子換裝**

編輯 `crates/dpm/src/action.rs`:

頂部 import 加 `MAIN_DIR`:

```rust
use crate::{
    get_db, read_file_from_zip, system::*, unzip_file, ClientError, ClientResult, DbPackage,
    Hashes, Setting, BIN_DIR, INSTALL_DIR, MAIN_DIR,
};
```

`install()` 內,`for pkg in is { ... }` 迴圈本體(從 `let pkg = pkg.as_str();` 到迴圈結束的 `}`)整段換成:

```rust
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

                let staging_root_base = MAIN_DIR.get().unwrap().join(".staging");
                std::fs::create_dir_all(&staging_root_base)
                    .map_err(|e| ClientError::Core(CoreError::IoError(e)))?;
                let staging = tempfile::Builder::new()
                    .prefix(pkg)
                    .tempdir_in(&staging_root_base)
                    .map_err(|e| ClientError::Core(CoreError::IoError(e)))?;

                let download_path = staging.path().join(&repo_package_info.filename);
                get_db()
                    .download_file(pkg, &download_path)
                    .await
                    .map_err(|e| ClientError::Core(CoreError::NetworkError(e.to_string())))?;
                if self.verbose {
                    println!("  {}", "Download successed!".green());
                }
                let package_info_test: String =
                    read_file_from_zip(&download_path, "packageInfo.json").unwrap();
                let package_info: PackageInfo =
                    JsonStorage::from_str_to(package_info_test.as_str()).unwrap();
                let package_hash_info: Hashes = JsonStorage::from_str_to(
                    read_file_from_zip(&download_path, "hashes.json")
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
                let hash = dpm_core::hash_file(&download_path)?;
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

                let extracted = staging.path().join("extracted");
                unzip_file(&download_path, &extracted)
                    .map_err(|e| ClientError::Core(CoreError::IoError(e)))?;

                let install_path = INSTALL_DIR.get().unwrap().join(pkg);
                swap_into_install_dir(&extracted, &install_path, staging.path())?;
                if self.verbose {
                    println!("  {}", "Installed!".green());
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
                // `staging` (tempfile::TempDir) drop 在這裡發生,連同任何被搬到
                // staging_root/previous 的舊版本一起清掉。
            }
```

拿掉原本 `remove_file(ori_path)...` 那一步(不再需要,`extracted`/下載檔都在 staging 目錄裡,隨 `staging` drop 一起清掉)。

- [ ] **Step 8: 整個 workspace 編譯 + 全部測試**

Run: `cargo build --workspace 2>&1 | tail -40`
Expected: 無錯誤。

Run: `cargo test --workspace 2>&1 | tail -40`
Expected: 全部測試通過(含 Task 1 的 `test_hash_file_is_deterministic_and_content_sensitive`、Task 2 的 3 個 `atomic_install_tests`、既有的 `db_tests`/`scope_tests`)。

- [ ] **Step 9: Commit**

```bash
git add crates/dpm/Cargo.toml crates/dpm/src/utils/db.rs crates/dpm/src/action.rs
git commit -m "$(cat <<'EOF'
feat(dpm): atomic package installs via tempfile staging

Downloads and unzips now happen in a staging directory on the same
filesystem as the install target (not /tmp), and the final swap into
place is a same-filesystem rename — a two-phase old-then-new rename
that keeps install_path fully valid (either the old version or the
new one) at every point, so a crash mid-install can't leave a
half-extracted package behind or destroy a working install during an
upgrade.

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>
EOF
)"
```
