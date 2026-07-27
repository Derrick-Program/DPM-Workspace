# Contributing to DPM-Workspace

給要動這個 repo 原始碼的人看。安裝、日常使用 `dpm`/`dpm-server` 的說明在 [`README.md`](../README.md);架構細節、已知問題見 [`CLAUDE.md`](../CLAUDE.md)。

```
crates/
├── dpm/         # Client CLI (bin: dpm)
├── dpm-server/  # Server CLI (bin: dpm-server)
└── dpm-core/    # 共用 lib
```

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

| 指令                    | 說明                                                         |
| ----------------------- | ------------------------------------------------------------ |
| `just check`          | `cargo check --workspace`,快速檢查編譯                     |
| `just build`          | `cargo build --workspace`(debug)                           |
| `just release`        | `cargo build --workspace --release`(開 lto + strip)        |
| `just test`           | `cargo test --workspace`                                   |
| `just test-p <crate>` | 只測單一 crate,例:`just test-p DPM`                        |
| `just lint`           | `cargo clippy --workspace --all-targets -- -D warnings`    |
| `just lint-fix`       | clippy 自動修                                                |
| `just fmt`            | `cargo fmt --all`                                          |
| `just fmt-check`      | 檢查格式,不修改(CI 用)                                       |
| `just pre-commit`     | `fmt` + `lint` + `test` 一次跑完,commit 前建議先跑這個 |

### 執行(Client / Server)

```bash
just run-client <args>   # cargo run -p DPM -- <args>
just run-server <args>   # cargo run -p DPM-Server -- <args>
```

`<args>` 是直接轉給該 binary 的參數,子指令、flag 都照原生 CLI 語法。README.md 裡的使用範例(`dpm install foo` 等)在開發時等價於 `just run-client install foo`——差別只在前者跑已安裝的 binary,後者從原始碼現編現跑。

### 文件與維護

| 指令              | 說明                                       |
| ----------------- | ------------------------------------------ |
| `just doc`      | `cargo doc --workspace --no-deps --open` |
| `just clean`    | `cargo clean`                            |
| `just outdated` | 檢查過期 dependency(需`cargo-outdated`)  |
| `just audit`    | 檢查安全性漏洞(需`cargo-audit`)          |
| `just update`   | 更新`Cargo.lock`                         |

### 安裝(從原始碼)

```bash
just install-client   # cargo install --path crates/dpm,裝到 ~/.cargo/bin
just install-server   # cargo install --path crates/dpm-server
```

一般使用者不需要原始碼、不需要 Rust 工具鏈——見 README.md 的 curl 安裝腳本。這裡的 `install-client`/`install-server` 是給要在本機用開發中版本測試的人用的。

### Secrets(Infisical)

| 指令                             | 說明                                         |
| -------------------------------- | -------------------------------------------- |
| `just env-login`               | 互動登入(每台機器一次)                       |
| `just env-init`                | 建立/連結`.infisical.json`(每個 repo 一次) |
| `just env-list`                | 列出目前 environment 的 secret key(不印值)   |
| `just env-push <dotenv> <env>` | 批次匯入既有 dotenv 檔案到指定 environment   |

## `dpm --system` 的擁有權細節

每次執行 dpm(`entry()` 開頭的 `init_first_run()`/`init_existing()`、結尾都會跑一次 `permision_check()`)都會把整個 `/opt/com.duacodie/DPM/` 樹重新 `chown -R`,兩個平台歸屬對象不同:

- **Linux**:`chown -R root:root`。process 本身透過 `sudo::escalate_if_needed()` 整個提權成 root 在跑,裝完東西擁有者也是 root——連原本下指令的人自己都要再用 sudo 才能 upgrade/uninstall/管理,不會因為是自己裝的就有寫權限。
- **macOS**:process 從不整個提權(逐指令個別 `sudo`),裝完後 `chown -R <你的帳號>:admin`,擁有者變回你自己——之後你不用 sudo 就能再管理,但別的使用者一樣沒寫權限。

兩邊預設目錄權限都是 `755`(`rwxr-xr-x`),owner/group 可讀寫執行,其他使用者只有讀+執行——能跑已裝好的東西,不能自己 upgrade/uninstall。且 dpm 完全不碰 PATH(不改 `.zshrc`/`/etc/profile`/`/etc/paths` 等),其他使用者要能直接打指令得自己把 `/opt/com.duacodie/DPM/bin` 加進 PATH。

這個規則對「自己裝的套件」一樣適用——`permision_check()` 掃的是整棵 `MAIN_DIR`(`/opt/com.duacodie/DPM/`),`Software/<pkg>/` 跟 `bin/` 下的東西都在裡面,不是只管 dpm 自己。per-user 模式(不加 `--system`)完全沒有這問題——裝在自己家目錄下,其他使用者本來就進不去。

### 跟 apt/dnf 對比

`apt`/`dnf` 裝的東西也是同一套 Unix 權限模型,只是把整個標準 Linux 目錄結構當共用樹在用:

| 內容       | 位置                                               | 擁有者/權限                |
| ---------- | -------------------------------------------------- | -------------------------- |
| 二進位檔   | `/usr/bin`、`/usr/sbin`                        | `root:root`,`755`      |
| 函式庫     | `/usr/lib`、`/usr/lib/x86_64-linux-gnu`        | `root:root`,`755`      |
| 設定檔     | `/etc`                                           | `root:root`,通常 `644` |
| 套件資料庫 | dpkg:`/var/lib/dpkg/`;rpm(dnf):`/var/lib/rpm/` | `root:root`              |
| 下載快取   | `/var/cache/apt/archives`、`/var/cache/dnf`    | `root:root`              |

要不要 sudo,看操作是不是要寫這些 root 擁有的目錄:

- **要 sudo**:`apt install`/`remove`/`upgrade`(寫 `/usr`、`/etc`,還要改套件資料庫)、`apt update`(寫 `/var/lib/apt/lists/` 快取)
- **不用 sudo**:`apt list`/`search`/`show`、`dpkg -l`、`rpm -qa`(只讀資料庫檔案,雖然 root 擁有但 `644` 全部人可讀)、直接執行已裝好的指令如 `curl`/`git`、`apt-get download`/`apt-get source`(只寫進目前工作目錄)

跟上面 dpm `--system` 的 `chown` 規則本質相同:寫共用系統目錄要 root,讀/執行不用。

## pubgrub 依賴解析內部實作

版本約束 `foo@^1.2` 這類語法解析上有個坑:`1.2.3`/`^1.2.3` 在 `semver::VersionReq` 解析後都是同一個 `Op::Caret`,無法事後分辨,所以 dpm 在丟進 `VersionReq` 前就把純數字+點的裸字串改寫成 `=1.2.3` 明確精確比對,才能區分 npm 風格的「裸版號=精確釘版本」跟 Cargo 風格的「裸版號=`^`」。

一次 `install` 裝多個套件時,所有套件(以及它們各自的 `dependencies`)會一起丟給 `pubgrub` 做聯合求解——不是每個套件獨立挑「目前最新版」,而是在滿足全部套件、全部相依限制的前提下,每個套件仍然挑得到的最新版本。任兩個套件的相依限制衝突(例如 A 需要 `lib@^2.0`、B 需要 `lib@^1.0`)會直接報錯並印出 `pubgrub` 產生的衝突鏈說明,不會裝一半。

已知限制:套件的 `dependencies` 欄位只有 `name`+版本約束,沒有 `source` 欄位——如果某個相依名稱同時存在於多個來源,現在無法在該相依關係裡指定要哪個來源,會直接報 `AmbiguousPackage`。

## Source 套件的權限隔離實作

`--system` 模式下(Linux)安裝 Source 套件的安全性細節:

- build 指令一律透過 `drop_privileges_for_build` 丟棄 root 權限後才執行(讀 `SUDO_UID`/`SUDO_GID`,`setgroups`→`setgid`→`setuid`;解不到就直接報錯,不會靜默用 root 跑)
- clone/build 用的暫存目錄跑完會 `chown` 回呼叫 `sudo` 的原始使用者
- build 完成後要 symlink 的 `entry` 路徑會做 path-safety 檢查(拒絕絕對路徑、`..`,並用 canonicalize 確認實際落在 install 目錄內,防 symlink 逃逸)

目前沒有額外的互動確認關卡或 OS 級沙箱(bubblewrap/landlock 等)。

## 手動測試流程(scope 隔離)

```bash
# per-user,預設,不用 sudo
dpm list -l
ls -la ~/Library/Application\ Support/com.duacodie.dpm/   # 確認建在自己家目錄下

# system,會跳 sudo 密碼
dpm --system list -l
sudo ls -la /opt/com.duacodie/DPM/   # 確認擁有者是 root(Linux)或 user:admin(macOS)
```

清掉測試殘留:

```bash
rm -rf ~/Library/Application\ Support/com.duacodie.dpm   # macOS per-user
sudo rm -rf /opt/com.duacodie/DPM                         # system(需要時)
```

## 提交前

至少跑一次 `just pre-commit`(fmt + clippy + test)。

## Spec-driven workflow

本專案用 **Superpowers** skill 套件做變更管理:`brainstorming` → `writing-plans` → `executing-plans` / `subagent-driven-development`。

- Design doc:`docs/superpowers/specs/YYYY-MM-DD-<topic>-design.md`
- Implementation plan:`docs/superpowers/plans/YYYY-MM-DD-<change-name>.md`
