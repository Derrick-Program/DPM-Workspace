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

`<args>` 是直接轉給該 binary 的參數,子指令、flag 都照原生 CLI 語法。

### 文件與維護

| 指令              | 說明                                       |
| ----------------- | ------------------------------------------ |
| `just doc`      | `cargo doc --workspace --no-deps --open` |
| `just clean`    | `cargo clean`                            |
| `just outdated` | 檢查過期 dependency(需`cargo-outdated`)  |
| `just audit`    | 檢查安全性漏洞(需`cargo-audit`)          |
| `just update`   | 更新`Cargo.lock`                         |

### 安裝

```bash
just install-client   # cargo install --path crates/dpm,裝到 ~/.cargo/bin
just install-server   # cargo install --path crates/dpm-server
```

### Secrets(Infisical)

| 指令                             | 說明                                         |
| -------------------------------- | -------------------------------------------- |
| `just env-login`               | 互動登入(每台機器一次)                       |
| `just env-init`                | 建立/連結`.infisical.json`(每個 repo 一次) |
| `just env-list`                | 列出目前 environment 的 secret key(不印值)   |
| `just env-push <dotenv> <env>` | 批次匯入既有 dotenv 檔案到指定 environment   |

## Client(`dpm`)使用方式

透過 `just run-client <args>` 執行——`run-client` recipe 本身已經是 `cargo run -p DPM -- {{args}}`,呼叫時**不要**再自己加一層 `--`,不然會變成 `cargo run -- -- <args>`,clap 收到的第一個字被誤判成跳脫符號,`-l` 這類 flag 會解析失敗。

### 安裝 scope:per-user vs system

`dpm` 預設是 **per-user** 模式,安裝目錄在使用者自己的資料夾下,完全不需要 root/sudo:

- macOS:`~/Library/Application Support/com.duacodie.dpm/`
- Linux:`$XDG_DATA_HOME/dpm`(通常是 `~/.local/share/dpm`)

加上全域 flag `--system`(或 `-S`)才會切到 **shared 安裝**,路徑固定在 `/opt/com.duacodie/DPM/`,需要 root/sudo(Linux 會自動整進程提權,macOS 逐指令 `sudo`)。`--system` 要放在子指令**之前**:

```bash
just run-client --system list -l
```

#### `--system` 的擁有權:Linux vs macOS 不一樣

每次執行 dpm(`entry()` 開頭的 `init_first_run()`/`init_existing()`、結尾都會跑一次 `permision_check()`)都會把整個 `/opt/com.duacodie/DPM/` 樹重新 `chown -R`,兩個平台歸屬對象不同:

- **Linux**:`chown -R root:root`。process 本身透過 `sudo::escalate_if_needed()` 整個提權成 root 在跑,裝完東西擁有者也是 root——**連原本下指令的人自己都要再用 sudo** 才能 upgrade/uninstall/管理,不會因為是自己裝的就有寫權限。
- **macOS**:process 從不整個提權(逐指令個別 `sudo`),裝完後 `chown -R <你的帳號>:admin`,擁有者變回你自己——**之後你不用 sudo** 就能再管理,但別的使用者一樣沒寫權限。

兩邊預設目錄權限都是 `755`(`rwxr-xr-x`),owner/group 可讀寫執行,**其他使用者只有讀+執行**——能跑已裝好的東西,不能自己 upgrade/uninstall。且 dpm 完全不碰 PATH(不改 `.zshrc`/`/etc/profile`/`/etc/paths` 等),其他使用者要能直接打指令得自己把 `/opt/com.duacodie/DPM/bin` 加進 PATH。

#### 跟 apt/dnf 對比

`apt`/`dnf` 裝的東西也是同一套 Unix 權限模型,只是把整個標準 Linux 目錄結構當共用樹在用:

| 內容       | 位置                                               | 擁有者/權限                |
| ---------- | -------------------------------------------------- | -------------------------- |
| 二進位檔   | `/usr/bin`、`/usr/sbin`                        | `root:root`,`755`      |
| 函式庫     | `/usr/lib`、`/usr/lib/x86_64-linux-gnu`        | `root:root`,`755`      |
| 設定檔     | `/etc`                                           | `root:root`,通常 `644` |
| 套件資料庫 | dpkg:`/var/lib/dpkg/`;rpm(dnf):`/var/lib/rpm/` | `root:root`              |
| 下載快取   | `/var/cache/apt/archives`、`/var/cache/dnf`    | `root:root`              |

要不要 sudo,看操作是不是要**寫**這些 root 擁有的目錄:

- **要 sudo**:`apt install`/`remove`/`upgrade`(寫 `/usr`、`/etc`,還要改套件資料庫)、`apt update`(寫 `/var/lib/apt/lists/` 快取)
- **不用 sudo**:`apt list`/`search`/`show`、`dpkg -l`、`rpm -qa`(只讀資料庫檔案,雖然 root 擁有但 `644` 全部人可讀)、直接執行已裝好的指令如 `curl`/`git`(`/usr/bin/curl` 是 `755`,誰都能執行)、`apt-get download`/`apt-get source`(只寫進目前工作目錄,你自己的地盤)

跟上面 dpm `--system` 的 `chown` 規則本質相同:寫共用系統目錄要 root,讀/執行不用。

**這個規則對「自己裝的套件」一樣適用**——`permision_check()` 掃的是整棵 `MAIN_DIR`(`/opt/com.duacodie/DPM/`),`Software/<pkg>/` 跟 `bin/` 下的東西都在裡面,不是只管 dpm 自己。所以任何 `--system` 裝的套件,在 Linux 上裝完都是 `root:root`,在 macOS 上裝完都是 `<你>:admin`,其他使用者一樣只能讀+執行、不能寫,一樣得自己加 PATH。per-user 模式(不加 `--system`)才完全沒有這問題——裝在自己家目錄下,其他使用者本來就進不去。

### 子指令

| 子指令                               | 別名                        | 說明                                                                  | 範例                                                                      |
| ------------------------------------ | --------------------------- | --------------------------------------------------------------------- | ------------------------------------------------------------------------- |
| `install <name...>`                | `i`, `add`, `inst`    | 安裝套件(先查本地索引,沒有就交給系統套件管理員)                       | `just run-client install foo`                                        |
| `update`                           | `ud`, `upda`, `up`    | 從遠端 repo 更新本地套件索引                                          | `just run-client update`                                             |
| `uninstall <name...>`              | `un`, `i!`, `unin`    | 移除套件                                                              | `just run-client uninstall foo`                                      |
| `search <name...>`                 | `s`, `se`, `sea`      | 搜尋套件                                                              | `just run-client search foo`                                         |
| `list [-l\|--list] [-s\|--list-sys]` | `l`, `li`, `ll`       | 列出套件(`-l` 已安裝、`-s` 系統套件管理員已安裝)                  | `just run-client list -l`                                            |
| `upgrade <name...>`                | `U`, `UP`, `grade`    | 升級套件                                                              | `just run-client upgrade foo`                                        |
| `upgrade-self`                     | `US`, `UPS`, `grades` | 升級 dpm 自己                                                         | `just run-client upgrade-self`                                       |
| `source add <URL> [--as ALIAS]`    | -                           | 新增套件來源(repo_url 需為 git 可 clone 的遠端;alias 預設取 URL host) | `just run-client source add https://github.com/org/repo --as myrepo` |
| `source remove <ALIAS>`            | -                           | 移除套件來源(連同該 source 在本地 DB 的所有套件紀錄)                  | `just run-client source remove myrepo`                               |
| `source list`                      | -                           | 列出目前設定的所有套件來源                                            | `just run-client source list`                                        |

大部分子指令都吃 `-v`/`--verbose`。全域還有 `-g`/`--gen <shell>` 產生 shell 自動完成腳本。

`install <name>` 支援單純套件名(不用 `source/name` 語法):本地索引裡該名字只在一個 source 有 → 自動選用;不存在 → `PackageNotFound`;存在於多個 source → `AmbiguousPackage`,需先 `source remove` 掉不要的來源再重試。

### 相依解析(pubgrub)

`install` 支援 `[source/]name[@constraint]` 語法(比照 npm):

- `dpm install foo` —— 名字沒指定來源,跟之前一樣走 0/1/2+ 來源數規則自動解析或報錯
- `dpm install official/foo` —— 明確指定來源
- `dpm install foo@^1.2` —— 版本約束,`^`/`~`/比較運算子沿用 Cargo 風格語意(`^1.2.3`、`~1.2.3`、`>=1.0.0, <2.0.0`);裸版號(`1.2.3`,沒有任何前綴)是 **npm 風格的精確釘版本**,不是 Cargo.toml 那種「裸版號等於 `^`」——因為 `1.2.3`/`^1.2.3` 在 `semver::VersionReq` 解析後都是同一個 `Op::Caret`,無法事後分辨,dpm 在丟進 `VersionReq` 前就把純數字+點的裸字串改寫成 `=1.2.3` 明確精確比對。不寫約束預設 `*`(任何版本)。
- `dpm install official/foo@^1.2` —— 來源 + 約束一起寫

一次 `install` 裝多個套件時,所有套件(以及它們各自的 `dependencies`)會一起丟給 `pubgrub` 做**聯合求解**——不是每個套件獨立挑「目前最新版」,而是在滿足全部套件、全部相依限制的前提下,每個套件仍然挑得到的最新版本。任兩個套件的相依限制衝突(例如 A 需要 `lib@^2.0`、B 需要 `lib@^1.0`)會直接報錯並印出 `pubgrub` 產生的衝突鏈說明,不會裝一半。

已知限制:套件的 `dependencies` 欄位只有 `name`+版本約束,没有 `source` 欄位——如果某個相依名稱同時存在於多個來源,現在無法在該相依關係裡指定要哪個來源,會直接報 `AmbiguousPackage`(跟 CLI 上裝到同名衝突套件的報錯規則一致)。`upgrade`/`uninstall`/`search` 目前還是吃純套件名,沒有 `source/name@constraint` 語法。

### 套件種類:Prebuilt vs Source

`update` 拉回來的索引裡,每個版本是 `Prebuilt`(預編譯檔案,下載後直接安裝)或 `Source`(需要本地 clone + 執行 build command)兩種之一:

- **Prebuilt**:`install` 直接下載 `url` 指到的檔案,驗證 blake3 hash 後裝進 install 目錄。
- **Source**:`install` 會先用 `git2` shallow clone 該 source 的 `repo_url`,再對 `packages/<pkg>/` 這個子目錄執行 `RepoInfo.json` 裡記錄的 `build` 指令(等同 shell 執行任意字串,概念上跟 AUR PKGBUILD 一樣是信任該 source 才能用)。

  執行前會印警告:

  ```
  Warning: installing a source package from a third-party source, not vetted by the DPM team
  ```

  （`official` source 例外,不印。）

  安全性細節(`--system` 模式下適用,Linux):

  - build 指令一律透過 `drop_privileges_for_build` 丟棄 root 權限後才執行(讀 `SUDO_UID`/`SUDO_GID`,`setgroups`→`setgid`→`setuid`;解不到就直接報錯,不會靜默用 root 跑)
  - clone/build 用的暫存目錄跑完會 `chown` 回呼叫 `sudo` 的原始使用者
  - build 完成後要 symlink 的 `entry` 路徑會做 path-safety 檢查(拒絕絕對路徑、`..`,並用 canonicalize 確認實際落在 install 目錄內,防 symlink 逃逸)

  目前**沒有**額外的互動確認關卡或 OS 級沙箱(bubblewrap/landlock 等)—— 裝 Source 套件前請自行確認來源可信。

### 測試流程範例

```bash
# per-user,預設,不用 sudo
just run-client list -l
ls -la ~/Library/Application\ Support/com.duacodie.dpm/   # 確認建在自己家目錄下

# system,會跳 sudo 密碼
just run-client --system list -l
sudo ls -la /opt/com.duacodie/DPM/   # 確認擁有者是 root(Linux)或 user:admin(macOS)
```

清掉測試殘留:

```bash
rm -rf ~/Library/Application\ Support/com.duacodie.dpm   # macOS per-user
sudo rm -rf /opt/com.duacodie/DPM                         # system(需要時)
```

## Server(`dpm-server`)使用方式

透過 `just run-server <args>` 執行,在 repo 根目錄操作套件索引。套件原始碼放在 `packages/<name>/`(不是 `Repo/src/`,那是舊路徑,已改掉),索引檔是根目錄的 `RepoInfo.json`。

| 子指令                                                    | 說明                                                                                                                                    | 範例                                                                 |
| --------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------- |
| `init <name> <entry> [-v ver] [-d description]`         | 建立套件骨架(`packages/<name>/`,含空的 `entry` 檔、`hashes.json`、`packageInfo.json`)                                           | `just run-server init foo bin/foo -v 0.1.0 -d "my pkg"`         |
| `hash <package_name>`                                   | 對`packages/<pkg>/` 下所有檔案算 blake3,寫入 `hashes.json`,回填 `packageInfo.json.hash`                                           | `just run-server hash foo`                                      |
| `build <package_name>`                                  | 把套件打包成`Repo/<pkg>.zip`(本地手動測試用,`Repo/` 已 gitignore,不會被 commit,也不是發布流程的一部分)                              | `just run-server build foo`                                     |
| `fix add <project_name> url <URL> [--file-name NAME]`  | 發布**Prebuilt** 版本:下載 `URL` 算 blake3 hash,寫進 `RepoInfo.json`(不在本地留檔案副本);`URL` 必須是 `https://`       | `just run-server fix add foo url https://example.com/foo.zip`   |
| `fix add <project_name> build <SHELL_CMD>`              | 發布**Source** 版本:把建置指令字串存進 `RepoInfo.json`,client 端 `install` 時才實際執行(見下方 client 說明)                      | `just run-server fix add foo build "cargo build --release"`     |
| `fix del <project_name> [version]`                      | 把套件版本從`RepoInfo.json` 移除(已發布版本不可覆寫/修改,只能整版刪除;只有一個版本時 `version` 可省略)                              | `just run-server fix del foo 0.1.0`                             |

典型發布流程:`init` → 把原始碼放進 `packages/<pkg>/` → `hash` → (`url` 走)自行把打包好的檔案上傳到某個 https 位置 → `fix add ... url ...`(或直接 `fix add ... build "..."` 走 Source 流程,不需要上傳檔案)。`url`/`build` 是 `fix add` 底下的子指令(而非 flag),clap 在解析期就強制「恰好一種」,不會有兩者皆給或皆未給的執行期錯誤。
