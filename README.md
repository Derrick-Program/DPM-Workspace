# DPM-Workspace

DPM(Derrick Package Manager)是一個套件管理工具,含 client CLI(`dpm`)與 server CLI(`dpm-server`,用來管理套件索引/registry)。

## 安裝(終端使用者)

macOS / Linux 一行裝好 `dpm`(從 [GitHub Releases](https://github.com/Derrick-Program/DPM-Workspace/releases) 抓對應平台的已簽章 prebuilt binary,不需要 Rust 工具鏈):

```bash
curl -fsSL https://raw.githubusercontent.com/Derrick-Program/DPM-Workspace/main/install.sh | bash
```

預設裝到 `~/.local/bin/dpm`,可用環境變數覆寫:

```bash
DPM_VERSION=v0.1.5 DPM_INSTALL_DIR=/usr/local/bin \
  curl -fsSL https://raw.githubusercontent.com/Derrick-Program/DPM-Workspace/main/install.sh | bash
```

- `DPM_VERSION`:指定版本(預設抓最新 release)
- `DPM_INSTALL_DIR`:安裝目錄(預設 `~/.local/bin`)

裝好後之後想更新版本,直接跑 `dpm upgrade-self` 即可——這條路徑會驗證 zipsign 簽章,簽章不符會拒絕安裝。

## Client(`dpm`)使用方式

安裝好之後直接執行 `dpm <子指令>` 即可,以下範例都假設 `dpm` 已在 PATH 上。

### 安裝 scope:per-user vs system

`dpm` 預設是 **per-user** 模式,安裝目錄在使用者自己的資料夾下,完全不需要 root/sudo:

- macOS:`~/Library/Application Support/com.duacodie.dpm/`
- Linux:`$XDG_DATA_HOME/dpm`(通常是 `~/.local/share/dpm`)

加上全域 flag `--system`(或 `-S`)才會切到 **shared 安裝**,路徑固定在 `/opt/com.duacodie/DPM/`,需要 root/sudo。`--system` 要放在子指令**之前**:

```bash
dpm --system list -l
```

之後要 upgrade/uninstall 這個安裝一樣需要 sudo(Linux);macOS 例外,裝完後擁有權會轉回你自己的帳號。

權限細節(chown 規則、跟 apt/dnf 的對比)見 [`docs/CONTRIBUTE.MD`](./docs/CONTRIBUTE.MD)。

### 子指令

| 子指令                               | 別名                        | 說明                                                                  | 範例                                                                      |
| ------------------------------------ | --------------------------- | --------------------------------------------------------------------- | ------------------------------------------------------------------------- |
| `install <name...>`                | `i`, `add`, `inst`    | 安裝套件(先查本地索引,沒有就交給系統套件管理員)                       | `dpm install foo`                                        |
| `update`                           | `ud`, `upda`, `up`    | 從遠端 repo 更新本地套件索引                                          | `dpm update`                                             |
| `uninstall <name...>`              | `un`, `i!`, `unin`    | 移除套件                                                              | `dpm uninstall foo`                                      |
| `search <name...>`                 | `s`, `se`, `sea`      | 搜尋套件                                                              | `dpm search foo`                                         |
| `list [-l\|--list] [-s\|--list-sys]` | `l`, `li`, `ll`       | 列出套件(`-l` 已安裝、`-s` 系統套件管理員已安裝)                  | `dpm list -l`                                            |
| `upgrade <name...>`                | `U`, `UP`, `grade`    | 升級套件                                                              | `dpm upgrade foo`                                        |
| `upgrade-self`                     | `US`, `UPS`, `grades` | 升級 dpm 自己                                                         | `dpm upgrade-self`                                       |
| `source add <URL> [--as ALIAS]`    | -                           | 新增套件來源(repo_url 需為 git 可 clone 的遠端;alias 預設取 URL host) | `dpm source add https://github.com/org/repo --as myrepo` |
| `source remove <ALIAS>`            | -                           | 移除套件來源(連同該 source 在本地 DB 的所有套件紀錄)                  | `dpm source remove myrepo`                               |
| `source list`                      | -                           | 列出目前設定的所有套件來源                                            | `dpm source list`                                        |

大部分子指令都吃 `-v`/`--verbose`。全域還有 `-g`/`--gen <shell>` 產生 shell 自動完成腳本。

`install <name>` 支援單純套件名(不用 `source/name` 語法):本地索引裡該名字只在一個 source 有就自動選用。

不存在會報 `PackageNotFound`;存在於多個 source 會報 `AmbiguousPackage`,需先 `source remove` 掉不要的來源再重試。

### 設定檔(config.toml)

`dpm` 的設定分三層,後者覆寫前者:系統層(`/etc/dpm/config.toml`,Linux;`/Library/Application Support/com.duacodie.dpm/config.toml`,macOS,唯讀,系統管理員維護)< 使用者層(`~/.config/dpm/config.toml`,Linux;`~/Library/Application Support/com.duacodie.dpm/config.toml`,macOS,`dpm` 自動產生與寫入)< 環境變數。`dpm source add/remove` 只會改使用者層,兩種安裝 scope(預設 / `--system`)讀到的都是同一份使用者層設定。

(「< 環境變數」是三層機制的通則;`dpm` 目前唯一的設定欄位 `sources` 是 array of tables,環境變數層沒有對應的表示法,實務上無法用環境變數覆寫。)

```bash
dpm gen-config          # 產生使用者層設定檔(空檔,未覆寫的欄位沿用系統層/預設值)
dpm gen-config --force  # 已存在時強制覆寫
```

### 版本約束語法

`install` 支援 `[source/]name[@constraint]` 語法(比照 npm):

- `dpm install foo` —— 不指定來源,索引裡只有一個來源會自動選用,有多個來源會報錯要求指定
- `dpm install official/foo` —— 明確指定來源
- `dpm install foo@^1.2` —— 版本約束,`^`/`~`/比較運算子沿用 Cargo 風格語意(`^1.2.3`、`~1.2.3`、`>=1.0.0, <2.0.0`)。純數字版號(`1.2.3`,無前綴)是 **npm 風格的精確釘版本**,跟 Cargo 不同
- `dpm install official/foo@^1.2` —— 來源 + 約束一起寫

不寫約束預設 `*`(任何版本)。一次裝多個套件時,dpm 會一起解出彼此相容的版本組合,不是每個套件各自挑「目前最新版」——衝突時會直接報錯,不會裝到一半。

已知限制:套件的相依關係無法指定來源,同名相依存在多個來源會報 `AmbiguousPackage`。`upgrade`/`uninstall`/`search` 也還沒有 `source/name@constraint` 語法。

內部求解演算法細節見 [`docs/CONTRIBUTE.MD`](./docs/CONTRIBUTE.MD)。

### 套件種類:Prebuilt vs Source

`update` 拉回來的索引裡,每個版本是 `Prebuilt`(預編譯檔案,下載後直接安裝)或 `Source`(下載原始碼後在本機執行 build 指令)兩種之一。

安裝 **Source** 套件前請先確認來源可信——build 指令等同在你的機器上執行任意 shell 指令,概念上跟 AUR PKGBUILD 一樣。非官方來源安裝時會印警告:

```
Warning: installing a source package from a third-party source, not vetted by the DPM team
```

目前**沒有**額外的互動確認關卡或 OS 級沙箱(bubblewrap/landlock 等)。`--system` 模式下的權限隔離實作細節見 [`docs/CONTRIBUTE.MD`](./docs/CONTRIBUTE.MD)。

## Server(`dpm-server`)使用方式

在 repo 根目錄操作套件索引,套件原始碼放在 `packages/<name>/`,索引檔是根目錄的 `RepoInfo.json`。

`dpm-server` 目前沒有 prebuilt release,需自行從原始碼建置(`cargo install --path crates/dpm-server`,細節見 [`docs/CONTRIBUTE.MD`](./docs/CONTRIBUTE.MD)),裝好後一樣直接執行 `dpm-server <子指令>`。

四個路徑(`project_src`/`repo_dir`/`keys_dir`/`repo_info`,預設 `packages`/`Repo`/`keys`/`RepoInfo.json`)可透過 `config.toml` 或 `DPM_SERVER__<FIELD>` 環境變數覆寫(例:`DPM_SERVER__REPO_DIR=/srv/dpm-repo`),同一套系統/使用者/環境變數三層規則(系統層 `/etc/dpm-server/config.toml`,Linux;`/Library/Application Support/com.duacodie.dpm-server/config.toml`,macOS。使用者層 `~/.config/dpm-server/config.toml`,Linux;`~/Library/Application Support/com.duacodie.dpm-server/config.toml`,macOS)。`dpm-server gen-config [--force]`(別名 `gc`)產生使用者層設定檔。

| 子指令                                                    | 說明                                                                                                                                    | 範例                                                                 |
| --------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------- |
| `keygen <author_id> [--force]`                          | 產生該作者的 ed25519 金鑰對,寫進 `keys/<author_id>.priv`(0600)/`keys/<author_id>.pub`;私鑰不進版控(自動補 `keys/.gitignore`)      | `dpm-server keygen alice`                                  |
| `init <name> <entry> --author <author_id> [-v ver] [-d description]` | 建立套件骨架(`packages/<name>/`,含空的 `entry` 檔、`hashes.json`、`packageInfo.json`);`--author` 必填,且該作者必須已 `keygen` 過 | `dpm-server init foo bin/foo --author alice -v 0.1.0 -d "my pkg"` |
| `hash <package_name> [--build SHELL_CMD]`               | 預設(無 `--build`):對`packages/<pkg>/` 下所有檔案算 blake3,寫入 `hashes.json`,回填 `packageInfo.json.hash`。`--build`:改雜湊 `build_command + 目前 git HEAD commit`,給 **Source** 套件用 | `dpm-server hash foo` / `dpm-server hash foo --build "cargo build --release"` |
| `sign <name>`                                           | 用該套件記錄的 author 的私鑰,對 `packageInfo.json.hash` 簽章,寫回 `packageInfo.json.signature`;`hash`/`sign` 每次改動內容都要重跑                      | `dpm-server sign foo`                                      |
| `build <package_name>`                                  | 把套件打包成 `Repo/<pkg>.zip`(**Prebuilt** 發布流程的一步)                                                                             | `dpm-server build foo`                                     |
| `fix add <project_name> url <URL> [--file-name NAME]`  | 發布**Prebuilt** 版本:下載 `URL` 算 blake3 hash,寫進 `RepoInfo.json`;`URL` 必須是 `https://`,且必須通過作者簽章驗證     | `dpm-server fix add foo url https://example.com/foo.zip`   |
| `fix add <project_name> build <SHELL_CMD>`              | 發布**Source** 版本:把建置指令字串存進 `RepoInfo.json`,client 端 `install` 時才實際執行;同樣必須通過作者簽章驗證             | `dpm-server fix add foo build "cargo build --release"`     |
| `fix del <project_name> [version]`                      | 把套件版本從 `RepoInfo.json` 移除(已發布版本不可覆寫/修改,只能整版刪除;只有一個版本時 `version` 可省略)                              | `dpm-server fix del foo 0.1.0`                             |

典型發布流程:

1. `keygen <author_id>`(每個作者只需一次)
2. `init --author <author_id>`,把原始碼放進 `packages/<pkg>/`
3. **Prebuilt** 走 `build` 打包 → `hash`;**Source** 走 `hash --build "<SHELL_CMD>"`
4. `sign`
5. `fix add ... url ...`(或 `fix add ... build "..."`)

`fix add` 會驗證簽章對得上作者的公鑰與 hash,同一套件名稱的後續版本作者也必須跟第一次發布時一致,任一項驗不過就拒絕寫入索引。

## Development

Tips for Contributors

See [docs/CONTRIBUTE.MD](./docs/CONTRIBUTE.MD) to get some tips for contributing.
