# 套件管理工具庫

此工具庫提供管理套件資訊、以 JSON 格式存取數據，以及與套件儲存庫互動的功能。

## 功能

- **PackageInfo**：定義套件的元數據並追蹤其依賴。
- **JsonStorage**：處理 JSON 資料的序列化/反序列化，用於數據持久化。
- **RepoInfo**：管理並查詢儲存庫中的套件元數據。

---

## 結構體與方法

### `Dependency`
代表一個套件的依賴。

**欄位：**
- `name`：依賴的名稱。
- `version`：依賴的版本。

### `PackageInfo`
儲存套件的詳細資訊。

**欄位：**
- `package_name`：套件名稱。
- `file_name`：套件檔案名稱。
- `version`：套件版本。
- `description`：套件描述。
- `hash`：套件檔案的雜湊值。
- `dependencies`：可選的依賴列表。

**方法：**
- `new`：建立一個新的 `PackageInfo` 實例。

### `JsonStorage<T>`
處理 JSON 資料的序列化/反序列化。

**方法：**
- `from_json(path: &Path)`：從 JSON 檔案載入資料。
- `to_json(data: &T, path: &Path)`：將資料存儲為 JSON 檔案。
- `from_url(url: &str)`：從 URL 獲取並反序列化 JSON 資料（異步）。
- `from_str_to(file_contents: &str)`：從字串反序列化 JSON 資料。

### `RepoInfo`
管理一個來源自己的套件索引；套件名稱映射到該套件所有已發布版本的列表。

**欄位：**
- `packages`：一個 `HashMap`，將套件名稱映射到 `Vec<PackageVersionInfo>`。

**方法：**
- `new()`：建立一個新的 `RepoInfo` 實例。
- `has_package(package_name: &str)`：檢查儲存庫中是否存在指定套件（任何版本）。
- `versions_of(package_name: &str)`：取得某套件的所有已發布版本。
- `latest_version(package_name: &str)`：取得某套件最新發布的版本。
- `add_package_version`（`server` feature）：新增一個套件版本（拒絕重複版本號）。
- `remove_package_version`（`server` feature）：移除某套件的特定版本。
- `fetch_update_repo_info`（`client` feature）：從遠端整包覆蓋索引。

### `PackageVersionInfo` / `PackageKind`
套件的一個發布版本；已發布版本視為不可變。

**欄位：**
- `version`：套件版本。
- `kind`：`PackageKind::Prebuilt { builds: Vec<PrebuiltBuild> }`（每組 build 含 `target`/`url`/`hash`/`file_name`，`target: None` 代表任何平台通用）或 `PackageKind::Source { build, hash, supported_targets }`。
- `dependencies`：可選的依賴列表。
- `entry`：可選的進入點。
- `description`：可選的描述。
- `author`：可選的作者 ID（用於簽章驗證）。
- `signature`：可選的 ed25519 簽章。

---

## 使用範例

### 基本的 PackageInfo 使用
```rust
use crate::{Dependency, PackageInfo};

let package = PackageInfo::new(
    "example_package".to_string(),
    "example_file".to_string(),
    "1.0.0".to_string(),
    "An example package.".to_string(),
    "abc123".to_string(),
    None,
);
println!("{:?}", package);
```

### 使用 JsonStorage
```rust
use crate::JsonStorage;
use std::path::Path;

let path = Path::new("data.json");
let data = JsonStorage::from_json::<Vec<String>>(path).unwrap();
println!("{:?}", data);
```

### 管理儲存庫
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

if repo.has_package("my-package") {
    println!("Package found.");
}
```

---

## 相依套件

此工具庫使用以下 Crate：
- `serde`：用於序列化和反序列化。
- `serde_json`：用於 JSON 操作。
- `anyhow`：用於錯誤處理。
- `reqwest`：用於 HTTP 請求。

---

## 貢獻

歡迎貢獻！隨時提交 Issue 或 Pull Request。

---

## 授權

此專案採用 Apache-2.0 授權條款。詳情請參閱 LICENSE 檔案。
