# DPM-Workspace

DPM(Derrick Package Manager)的 Cargo workspace,包含 client CLI(`dpm`)、server CLI(`dpm-server`)與共用 lib(`dpm-core`)。

```
crates/
├── dpm/         # Client CLI (bin: dpm)
├── dpm-server/  # Server CLI (bin: dpm-server)
└── dpm-core/    # 共用 lib
```

架構細節、已知問題見 [`CLAUDE.md`](./CLAUDE.md)。

## 前置設定

指令一律透過 `just` 執行,secret 由 [Infisical](https://infisical.com) 在執行期注入,不落地到硬碟。

```bash
just env-login   # 互動 OAuth 登入,每台機器一次
just env-init    # 產生/連結 .infisical.json,每個 repo 一次
```

沒設定過 Infisical project 的話,`env-init` 會需要有人先在 Infisical dashboard 建好 project 並把 `dev`/`staging`/`prod` 三個 environment 填好基本 secret(至少 client 端不需要額外 secret 也能跑,`DATABASE_URL` 已經退役)。

預設用 `dev` environment,要切換用:

```bash
DPM_ENV=staging just <recipe>
```

## justfile 指令總覽

跑 `just --list` 可以隨時列出全部指令。以下依用途分類。

### 開發

| 指令 | 說明 |
|---|---|
| `just check` | `cargo check --workspace`,快速檢查編譯 |
| `just build` | `cargo build --workspace`(debug) |
| `just release` | `cargo build --workspace --release`(開 lto + strip) |
| `just test` | `cargo test --workspace` |
| `just test-p <crate>` | 只測單一 crate,例:`just test-p DPM` |
| `just lint` | `cargo clippy --workspace --all-targets -- -D warnings` |
| `just lint-fix` | clippy 自動修 |
| `just fmt` | `cargo fmt --all` |
| `just fmt-check` | 檢查格式,不修改(CI 用) |
| `just pre-commit` | `fmt` + `lint` + `test` 一次跑完,commit 前建議先跑這個 |

### 執行(Client / Server)

```bash
just run-client <args>   # cargo run -p DPM -- <args>
just run-server <args>   # cargo run -p DPM-Server -- <args>
```

`<args>` 是直接轉給該 binary 的參數,子指令、flag 都照原生 CLI 語法。

### 文件與維護

| 指令 | 說明 |
|---|---|
| `just doc` | `cargo doc --workspace --no-deps --open` |
| `just clean` | `cargo clean` |
| `just outdated` | 檢查過期 dependency(需 `cargo-outdated`) |
| `just audit` | 檢查安全性漏洞(需 `cargo-audit`) |
| `just update` | 更新 `Cargo.lock` |

### 安裝

```bash
just install-client   # cargo install --path crates/dpm,裝到 ~/.cargo/bin
just install-server   # cargo install --path crates/dpm-server
```

### Secrets(Infisical)

| 指令 | 說明 |
|---|---|
| `just env-login` | 互動登入(每台機器一次) |
| `just env-init` | 建立/連結 `.infisical.json`(每個 repo 一次) |
| `just env-list` | 列出目前 environment 的 secret key(不印值) |
| `just env-push <dotenv> <env>` | 批次匯入既有 dotenv 檔案到指定 environment |

## Client(`dpm`)使用方式

透過 `just run-client -- <args>` 執行(注意 `--` 是 just 的參數分隔,不是 clap 的)。

### 安裝 scope:per-user vs system

`dpm` 預設是 **per-user** 模式,安裝目錄在使用者自己的資料夾下,完全不需要 root/sudo:

- macOS:`~/Library/Application Support/com.duacodie.dpm/`
- Linux:`$XDG_DATA_HOME/dpm`(通常是 `~/.local/share/dpm`)

加上全域 flag `--system`(或 `-S`)才會切到 **shared 安裝**,路徑固定在 `/opt/com.duacodie/DPM/`,需要 root/sudo(Linux 會自動整進程提權,macOS 逐指令 `sudo`)。`--system` 要放在子指令**之前**:

```bash
just run-client -- --system list -l
```

### 子指令

| 子指令 | 別名 | 說明 | 範例 |
|---|---|---|---|
| `install <name...>` | `i`, `add`, `inst` | 安裝套件(先查本地索引,沒有就交給系統套件管理員) | `just run-client -- install foo` |
| `update` | `ud`, `upda`, `up` | 從遠端 repo 更新本地套件索引 | `just run-client -- update` |
| `uninstall <name...>` | `un`, `i!`, `unin` | 移除套件 | `just run-client -- uninstall foo` |
| `search <name...>` | `s`, `se`, `sea` | 搜尋套件 | `just run-client -- search foo` |
| `list [-l\|--list] [-s\|--list-sys]` | `l`, `li`, `ll` | 列出套件(`-l` 已安裝、`-s` 系統套件管理員已安裝) | `just run-client -- list -l` |
| `upgrade <name...>` | `U`, `UP`, `grade` | 升級套件 | `just run-client -- upgrade foo` |
| `upgradeSelf` | `US`, `UPS`, `grades` | 升級 dpm 自己 | `just run-client -- upgradeSelf` |

大部分子指令都吃 `-v`/`--verbose`。全域還有 `-g`/`--gen <shell>` 產生 shell 自動完成腳本。

### 測試流程範例

```bash
# per-user,預設,不用 sudo
just run-client -- list -l
ls -la ~/Library/Application\ Support/com.duacodie.dpm/   # 確認建在自己家目錄下

# system,會跳 sudo 密碼
just run-client -- --system list -l
sudo ls -la /opt/com.duacodie/DPM/   # 確認擁有者是 root(Linux)或 user:admin(macOS)
```

清掉測試殘留:

```bash
rm -rf ~/Library/Application\ Support/com.duacodie.dpm   # macOS per-user
sudo rm -rf /opt/com.duacodie/DPM                         # system(需要時)
```

## Server(`dpm-server`)使用方式

透過 `just run-server -- <args>` 執行,在 repo 根目錄(`Repo/`)操作套件索引。

| 子指令 | 說明 | 範例 |
|---|---|---|
| `init <name> <entry> [-v ver] [-d description]` | 建立套件骨架(`Repo/src/<name>/`) | `just run-server -- init foo bin/foo -v 0.1.0 -d "my pkg"` |
| `hash <packagename>` | 對 `Repo/src/<pkg>/` 下所有檔案算 SHA256,寫入 `hashes.json`,回填 `packageInfo.json.hash` | `just run-server -- hash foo` |
| `build <packagename>` | 把套件打包成 `Repo/<pkg>.zip` | `just run-server -- build foo` |
| `fix add <project_name>` | 把套件加入 `RepoInfo.json` 索引 | `just run-server -- fix add foo` |
| `fix del <project_name>` | 把套件從 `RepoInfo.json` 移除 | `just run-server -- fix del foo` |

典型發布流程:`init` → 把原始碼放進 `Repo/src/<pkg>/` → `hash` → `build` → `fix add`。
