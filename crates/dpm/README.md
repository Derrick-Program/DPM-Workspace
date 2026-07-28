# DPM (Derrick Package Manager Client)

`dpm` 是 DPM 系統的用戶端命令列工具，負責搜尋、下載、安裝、升級與管理本地的二進位 (Prebuilt) 及原始碼 (Source) 套件，並支援多套件管理員 (Homebrew, APT, DNF 等) 整合與安全的自升級機制 (`dpm upgrade-self`)。

---

## 🛡️ 金鑰與安全機制 (Security Architecture)

DPM 的安全設計採用雙層金鑰隔離，分別保護「第三方套件發布」與「DPM 主程式自升級」：

| 比較項目 | 套件發布金鑰 (Package Author Keys) | DPM 官方主程式自升級金鑰 (Release Key) ⭐ |
| :--- | :--- | :--- |
| **金鑰名稱** | `keys/<author_id>.pub` (例如 `alice.pub`) | `keys/dpm-release-signing.pub` |
| **載入方式** | `dpm update` 時由伺服器動態下載 | 編譯時直接內嵌至二進位檔 (`include_bytes!`) |
| **保護對象** | 第三方應用套件 (如 `hello`, `addsub`) | **`dpm` 客戶端主程式本身 (`dpm upgrade-self`)** |
| **簽署工具** | `dpm-server sign` (Ed25519 簽章) | `zipsign` (Ed25519 壓縮檔簽署) |

---

## 🔐 官方主程式自升級金鑰 (`dpm-release-signing.pub`) 的產出與運作流程

DPM 客戶端自自我升級 (`dpm upgrade-self`) 使用 Rust 社群標準的 `zipsign` 工具進行防偽校驗：

### 1. 產出金鑰對
在安全的發布環境中安裝 `zipsign` 並產生官方 Release 金鑰對：
```bash
cargo install zipsign
zipsign keygen dpm-release-signing.priv dpm-release-signing.pub
```
- `dpm-release-signing.priv`（私鑰）：儲存在 GitHub Repository Secrets (`RELEASE_SIGNING_PRIVATE_KEY`)，專供 GitHub Actions CI/CD 發布新版 `dpm` 時簽署 Tar/Zip 二進位包。
- `dpm-release-signing.pub`（公鑰）：存放在本專案 `crates/dpm/keys/dpm-release-signing.pub`。

### 2. 內嵌公鑰至 `dpm` 客戶端
在 `crates/dpm/src/action.rs` 中，透過 `include_bytes!` 巨集將公鑰編譯內嵌至 `dpm` 執行檔：
```rust
const RELEASE_SIGNING_PUBLIC_KEY: &[u8; 32] = include_bytes!("../keys/dpm-release-signing.pub");
```

### 3. 自動化簽署與升級驗證流程
1. **發布端 (CI/CD)**：GitHub Actions 編譯出新的 `dpm` 執行檔後，使用 `dpm-release-signing.priv` 產出 `.zipsig` 簽章檔上傳至 GitHub Releases。
2. **用戶端 (`dpm upgrade-self`)**：執行 `dpm upgrade-self` 時，DPM 會從 GitHub Release 下載新版二進位包與簽章，並使用內嵌的 `RELEASE_SIGNING_PUBLIC_KEY` 進行 `zipsign` 校驗，驗證通過才替換舊版執行檔。

---

## 🚀 安裝與編譯 (Build & Installation)

```bash
# 編譯 Release 版本
cargo build -p DPM --release

# 執行 dpm
./target/release/dpm --help
```

---

## 🛠️ 主要指令說明 (CLI Commands)

- **`dpm update`**：同步並驗證遠端 `RepoInfo.db` 索引庫與作者公鑰。
- **`dpm search <pkg>`**：搜尋 DPM 可用套件庫與本機 Host OS 套件管理員。
- **`dpm install <pkg>`**：安全校驗雜湊並安裝指定套件。
- **`dpm uninstall <pkg>`**：移除本機已安裝的套件。
- **`dpm list`**：列出本機已安裝的 DPM 套件與 Host OS 套件。
- **`dpm upgrade-self`**：使用 `zipsign` 安全驗證並自自我更新 `dpm` 至最新版本。
- **`dpm source list/add/remove`**：管理遠端套件庫來源 (Sources)。