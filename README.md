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

加上全域 flag `--system`(或 `-S`)才會切到 **shared 安裝**,路徑固定在 `/opt/com.duacodie/DPM/`,需要 root/sudo(Linux 會自動整進程提權,macOS 逐指令 `sudo`)。`--system` 要放在子指令**之前**:

```bash
dpm --system list -l
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

## Server(`dpm-server`)使用方式

在 repo 根目錄操作套件索引,套件原始碼放在 `packages/<name>/`(不是 `Repo/src/`,那是舊路徑,已改掉),索引檔是根目錄的 `RepoInfo.json`。`dpm-server` 目前沒有 prebuilt release,需自行從原始碼建置(`cargo install --path crates/dpm-server`,細節見 [`docs/CONTRIBUTE.MD`](./docs/CONTRIBUTE.MD)),裝好後一樣直接執行 `dpm-server <子指令>`。

| 子指令                                                    | 說明                                                                                                                                    | 範例                                                                 |
| --------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------- |
| `keygen <author_id> [--force]`                          | 產生該作者的 ed25519 金鑰對,寫進 `keys/<author_id>.priv`(0600)/`keys/<author_id>.pub`;私鑰不進版控(自動補 `keys/.gitignore`)      | `dpm-server keygen alice`                                  |
| `init <name> <entry> --author <author_id> [-v ver] [-d description]` | 建立套件骨架(`packages/<name>/`,含空的 `entry` 檔、`hashes.json`、`packageInfo.json`);`--author` 必填,且該作者必須已 `keygen` 過 | `dpm-server init foo bin/foo --author alice -v 0.1.0 -d "my pkg"` |
| `hash <package_name> [--build SHELL_CMD]`               | 預設(無 `--build`):對`packages/<pkg>/` 下所有檔案算 blake3,寫入 `hashes.json`,回填 `packageInfo.json.hash`(若 `Repo/<pkg>.zip` 已存在則直接雜湊該 zip)。`--build`:改雜湊 `build_command + 目前 git HEAD commit`,給 **Source** 套件用 | `dpm-server hash foo` / `dpm-server hash foo --build "cargo build --release"` |
| `sign <name>`                                           | 用該套件記錄的 author 的私鑰,對 `packageInfo.json.hash` 簽章,寫回 `packageInfo.json.signature`；`hash`/`sign` 每次改動內容都要重跑                      | `dpm-server sign foo`                                      |
| `build <package_name>`                                  | 把套件打包成`Repo/<pkg>.zip`(**Prebuilt** 發布流程的一步,`Repo/` 已 gitignore,不會被 commit;隨後的 `hash` 會直接雜湊這個 zip)          | `dpm-server build foo`                                     |
| `fix add <project_name> url <URL> [--file-name NAME]`  | 發布**Prebuilt** 版本:下載 `URL` 算 blake3 hash,寫進 `RepoInfo.json`(不在本地留檔案副本);`URL` 必須是 `https://`,且必須通過作者簽章驗證     | `dpm-server fix add foo url https://example.com/foo.zip`   |
| `fix add <project_name> build <SHELL_CMD>`              | 發布**Source** 版本:把建置指令字串存進 `RepoInfo.json`,client 端 `install` 時才實際執行(見下方 client 說明);同樣必須通過作者簽章驗證             | `dpm-server fix add foo build "cargo build --release"`     |
| `fix del <project_name> [version]`                      | 把套件版本從`RepoInfo.json` 移除(已發布版本不可覆寫/修改,只能整版刪除;只有一個版本時 `version` 可省略)                              | `dpm-server fix del foo 0.1.0`                             |

典型發布流程:`keygen <author_id>`(每個作者只需一次)→ `init --author <author_id>` → 把原始碼放進 `packages/<pkg>/` →(**Prebuilt** 走:`build` 打包 →)`hash`(**Source** 走 `hash --build "<SHELL_CMD>"`)→ `sign` → `fix add ... url ...`(或 `fix add ... build "..."`)。`fix add` 會驗證 `packageInfo.json` 裡的 `signature` 對得上 `author` 的公鑰跟 `hash`,同一套件名稱的後續版本作者也必須跟第一次發布時一致,任一項驗不過就拒絕寫入 `RepoInfo.json`。`url`/`build` 是 `fix add` 底下的子指令(而非 flag),clap 在解析期就強制「恰好一種」,不會有兩者皆給或皆未給的執行期錯誤。

## Development

Tips for Contributors

See [docs/CONTRIBUTE.MD](./docs/CONTRIBUTE.MD) to get some tips for contributing.
