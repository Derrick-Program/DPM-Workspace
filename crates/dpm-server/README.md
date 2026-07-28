# DPM-Server (Derrick Package Manager Server)

`dpm-server` 是 DPM (Derrick Package Manager) 的官方倉庫與發布端管理工具。它負責管理套件源資料庫 (`RepoInfo.db`)、產生 Ed25519 作者數位簽章金鑰對、計算套件檔案與編譯指令雜湊值、校驗數位簽章，以及發布預建 (Prebuilt) 與原始碼 (Source) 套件。

---

## 🛡️ 安全與架構概述

- **SQLite 資料庫 (`RepoInfo.db`)**：`dpm-server` 使用 Turso/SQLite 儲存 `Packages` 資料表，記錄所有可供 Client 端下載安裝的套件詮釋資料。
- **Ed25519 數位簽章**：每一份發布的套件版本都必須由作者私鑰 (`keys/<author>.priv`) 進行 Ed25519 數位簽署。Client 端 (`dpm`) 與 `dpm-server` 在匯入前皆會以作者公鑰 (`keys/<author>.pub`) 進行零信任安全驗證。

---

## 🔑 第一步：產生作者金鑰對 (Keygen)

發布任何套件前，必須先為套件作者產生一組 Ed25519 密碼學金鑰對：

```bash
cargo run -p DPM-Server -- keygen <author_id>
```
* **產出檔案**：
  - `keys/<author_id>.priv`：私鑰 (Private Key)，用於 `sign` 簽署套件。**切勿提交至 Git 倉庫**！
  - `keys/<author_id>.pub`：公鑰 (Public Key)，用於公開驗證。**需提交至 Git 倉庫**。

---

## 📦 第二步：選擇套件類型並進行發布

DPM 支援兩種套件發布類型：**預建二進位包 (Prebuilt Zip)** 與 **原始碼編譯包 (Source Package)**。

---

### 🅰️ 預建二進位包 (Prebuilt Zip Package) 完整發布流程

適用於跨平台或已編譯好的二進位可執行檔打包 (如 `.zip` 壓縮包)。

#### 1. 初始化套件專案 (`init`)
```bash
cargo run -p DPM-Server -- init <pkg_name> <entry_binary> --author <author_id> -v 0.1.0 -d "<description>"
```
* 例如：`cargo run -p DPM-Server -- init hello bin/hello --author alice -v 0.1.0 -d "simple universal hello package"`
* 這會在 `packages/<pkg_name>/` 下建立專案結構與 `packageInfo.json`。

#### 2. 打包二進位 ZIP 檔 (`build`) ⭐ 必須先打包
```bash
cargo run -p DPM-Server -- build <pkg_name>
```
* 將 `packages/<pkg_name>/` 內容打包至 `Repo/<pkg_name>.zip`。

#### 3. 計算 ZIP 檔雜湊值 (`hash`)
```bash
cargo run -p DPM-Server -- hash <pkg_name>
```
* `dpm-server` 會偵測到 `Repo/<pkg_name>.zip` 已存在，並直接計算該 Zip 壓縮檔的 Blake3 雜湊值，填入 `packageInfo.json` 的 `"hash"` 欄位。

#### 4. 數位簽署套件 (`sign`)
```bash
cargo run -p DPM-Server -- sign <pkg_name>
```
* 讀取 `keys/<author_id>.priv` 私鑰，對 `packageInfo.json` 裡的 Zip 雜湊值進行 Ed25519 數位簽署，填入 `"signature"` 欄位。

#### 5. 校驗並寫入 SQLite 索引庫 (`fix add url`)

* **本地測試 (未 Git Push 前)**：
  使用 `file://` 指向本機的 Zip 檔：
  ```bash
  cargo run -p DPM-Server -- fix add <pkg_name> url file://$(pwd)/crates/dpm-server/Repo/<pkg_name>.zip [--target <target_triple>]
  ```

* **正式發布 (Git Push 到 GitHub 後)**：
  使用遠端 HTTPS 網址：
  ```bash
  cargo run -p DPM-Server -- fix add <pkg_name> url https://raw.githubusercontent.com/<user>/<repo>/main/crates/dpm-server/Repo/<pkg_name>.zip [--target <target_triple>]
  ```
  > 💡 **防錯機制提示**：`fix add url` 在寫入 `RepoInfo.db` 前，會從 URL 下載檔案並比對雜湊是否等於簽署的雜湊。若是 HTTPS 網址，請確保已將最新的 `<pkg_name>.zip` `git push` 上雲端。

---

### 🅱️ 原始碼編譯包 (Source Package) 完整發布流程

適用於需要由 Client 端在安裝時於本機自行編譯的套件 (如 C/C++/Rust 原始碼專案)。**此類型無需打包 Zip 檔**。

#### 1. 初始化原始碼套件專案 (`init`)
```bash
cargo run -p DPM-Server -- init <pkg_name> <target_lib_or_bin> --author <author_id> -v 0.1.0 -d "<description>"
```
* 例如：`cargo run -p DPM-Server -- init addsub lib/libaddsub.dylib --author alice -v 0.1.0 -d "C shared library"`
* 將原始碼放入 `packages/<pkg_name>/src/`。

#### 2. 計算編譯指令與 Git Commit 雜湊 (`hash --build`)
```bash
cargo run -p DPM-Server -- hash <pkg_name> --build "<build_command>"
```
* 例如：`cargo run -p DPM-Server -- hash addsub --build "cc -dynamiclib -o \$OUT/libaddsub.dylib src/addsub.c"`
* `dpm-server` 會結合 **編譯指令** + **目前的 Git HEAD Commit SHA** 進行 Blake3 雜湊計算，填入 `packageInfo.json` 的 `"hash"` 欄位。

#### 3. 數位簽署套件 (`sign`)
```bash
cargo run -p DPM-Server -- sign <pkg_name>
```
* 使用 `keys/<author_id>.priv` 私鑰對該編譯指令與 Commit 雜湊進行 Ed25519 簽名，填入 `"signature"` 欄位。

#### 4. 校驗並寫入 SQLite 索引庫 (`fix add build`)
```bash
# 自動讀取 packageInfo.json 中的 build_command (無需重複輸入指令)
cargo run -p DPM-Server -- fix add <pkg_name> build [--targets "<comma_separated_targets>"]

# 或亦可手動指定編譯指令：
cargo run -p DPM-Server -- fix add <pkg_name> build "<build_command>" [--targets "<comma_separated_targets>"]
```
* 例如：`cargo run -p DPM-Server -- fix add addsub build --targets "aarch64-apple-darwin,x86_64-unknown-linux-gnu"`
* 自動讀取 `keys/<author_id>.pub` 驗證簽章，通過後寫入 `RepoInfo.db`。

---

## 📊 發布流程速查表 (Cheat Sheet)

| 步驟 | 預建二進位包 (Prebuilt Zip) | 原始碼編譯包 (Source Package) ⭐ |
| :--- | :--- | :--- |
| **1. 專案準備** | `build <pkg>` (產生 `.zip`) | 準備 `src/` 原始碼 |
| **2. 計算雜湊** | `hash <pkg>` | `hash <pkg> --build "<CMD>"` |
| **3. 數位簽署** | `sign <pkg>` | `sign <pkg>` |
| **4. 匯入 DB** | `fix add <pkg> url <URL> [--target <T>]` | `fix add <pkg> build [<CMD>] [--targets <T1,T2>]` |

---

## 🛠️ CLI 指令參考 (CLI Command Reference)

```bash
Derrick Package Manager Server (DPM-Server)

Commands:
  keygen <AUTHOR>     產生 Ed25519 作者簽署金鑰對 (keys/<author>.priv 與 keys/<author>.pub)
  init <NAME> <ENTRY> 初始化套件 packageInfo.json 骨架
  build <NAME>        將 packages/<NAME>/ 打包為 Repo/<NAME>.zip
  hash <NAME>         計算 Zip 或編譯指令的 Blake3 雜湊值
  sign <NAME>         使用作者私鑰對 packageInfo.json 的雜湊值進行 Ed25519 數位簽署
  fix add <NAME> ...  將已驗證的套件版本寫入 RepoInfo.db
  fix del <NAME>      從 RepoInfo.db 刪除套件版本
  gen-config          建立預設 config.toml 設定檔
```

---

## 🔍 二進位金鑰檢視工具 (Key Inspection Utilities)

`keys/<author>.pub` 為 32-byte 原始二進位 Ed25519 公鑰檔案，可以透過以下工具進行檢視與指紋比對：

```bash
# 檢視 64-char 十六進位字串 (Hex)
xxd -p keys/alice.pub

# 轉換為 Base64 字串
openssl base64 -in keys/alice.pub

# 計算 SHA256 指紋 (Fingerprint)
shasum -a 256 keys/alice.pub
```
