# 多來源套件生態系設計(namespace + 版本化索引 + pubgrub + 發布模型 + CI/CD + 安全性)

日期:2026-07-24

## 背景與動機

現況(`dpm` client + `dpm-server`)是徹底單一來源設計:

- `config.json` 只有一組 `repo_url`/`repo_info` 字串,`dpm-core::RepoInfo.packages: HashMap<String, PackageBasicInfo>` 套件名是唯一 key,`fetch_update_repo_info` 抓回來就整包覆蓋本地視圖。
- 每個套件永遠只有「當前版本」一筆,`dpm-server build` 每次覆寫同一個 `Repo/<pkg>.zip`,沒有版本化儲存。
- 沒有真正的相依解析——`Dependency { name, version }` 只是存著,`action.rs::install()` 完全沒讀取它;`dpm-server` 甚至留著一段被註解掉、沒接上的遞迴解析嘗試。
- 貢獻流程等於「PR 進一個會一路長大的 git repo,裡面塞滿二進位 zip」——GitHub 單檔 100MB 硬限、PR 審查含二進位檔案、git history 只增不減。
- Hash 驗證(SHA256)只防「傳輸中被竄改」,防不了「一開始就是惡意的」,且沒有簽章機制。
- Client 安裝流程非原子:下載到固定 `/tmp` 路徑、直接解壓進最終安裝路徑,中途失敗會留下半殘狀態。

這份 spec 把上述問題一次想清楚:如何讓其他開發者方便貢獻(多來源、免二進位負擔的發布模型)、如何解相依關係(pubgrub)、如何保證安全性(簽章、CI 權限隔離、原子安裝)。

## 目標

- `dpm` 支援多個套件來源(source),使用者可自行加入第三方 tap,名稱衝突時強制要求明確標示來源。
- 套件索引支援多版本,搭配 pubgrub 做真正的相依版本解析。
- `dpm-server`(以及任何第三方 tap)的 git repo 只存文字/原始碼,不 host 任何預先打包的二進位——不管是自己寫的還是別人的套件。
- 貢獻流程全自動化:PR 觸發 CI 驗證,merge 後 CI 自動更新索引,人不用手動跑發布指令。
- 補齊安全性:hash 換 blake3、原子安裝(tempfile staging + rename)、CI workflow 權限隔離、強制 HTTPS,並為簽章機制(獨立子專案)預留 schema 空間。

## 非目標

- 簽章機制(minisign)本身的完整實作——這份 spec 只確保 schema/流程留有空間讓簽章之後接得上,不在此規劃逐項落地。
- Release-based 二進位儲存(先前討論過的 GitHub Release 方案)——已被本 spec 的「不 host 任何二進位」模型取代,不再需要。
- `source` 模式套件原始碼的確切傳輸機制(git shallow clone + sparse-checkout 的實作細節)——列為架構決策,實作時再定案,不影響本 spec 其他部分。
- Fuzzy 搜尋——獨立小功能,不在這輪範圍內。
- 舊版單一來源設定/本地索引資料的遷移——這是破壞性的 schema 大改版,不處理向後相容轉檔。

## 架構

### 1. Config schema(來源清單)

`config.json` 的 `repo_url`/`repo_info` 兩個字串鍵退役,改成陣列:

```json
{
  "sources": [
    { "alias": "official", "repo_info": "https://raw.githubusercontent.com/.../RepoInfo.json", "repo_url": "https://github.com/.../DPM-Server" }
  ]
}
```

- 出廠預設帶一個 `alias: "official"` 的來源。
- `alias` 使用者自取(`source add --as <alias>`,沒指定就用 URL host 當預設值),重複 alias 拒絕新增。
- `repo_url` 純資訊用途(給人看),`repo_info` 才是程式實際抓取的索引 URL。

### 2. CLI 指令(來源管理)

`dpm` 新增頂層子指令 `source`(nested subcommand,比照 `dpm-server` 的 `fix add/del` 手感):

```
dpm source add <url> [--as <alias>]
dpm source remove <alias>
dpm source list
```

- `source add` 對非 `official` 來源印警告(第三方,未經審查)。
- `source add` 不會立刻打網路抓 `RepoInfo.json`,要 `dpm update` 才會抓——維持現有「加來源」跟「更新索引」是兩個動作的慣例。

### 3. `dpm-core` 資料模型

```rust
pub struct RepoInfo {
    // key 從單一 name 改成 (source_alias, package_name),value 從單筆改成多版本
    packages: HashMap<(String, String), Vec<PackageVersionInfo>>,
}

pub enum PackageKind {
    Prebuilt { url: String, hash: String, file_name: String },
    Source { build: String }, // build 指令;實際原始碼位置 = 該 source 自己的 git repo + packages/<name>/
}

pub struct PackageVersionInfo {
    pub version: String,       // semver::Version(具體版本)
    pub kind: PackageKind,
    pub dependencies: Option<Vec<Dependency>>,
    pub entry: Option<String>,
    pub description: Option<String>,
}

pub struct Dependency {
    pub name: String,          // 可帶 "來源/名稱" 前綴,裸名規則跟 install 一致
    pub version: String,       // semver::VersionReq 約束語法(如 "^1.2"),不是精確版本
}
```

- `fetch_update_repo_info` 簽名加 `source_alias: &str`,只覆蓋屬於該來源的那部分,不動其他來源的資料。
- `find_package(name) -> Vec<(&str, &Vec<PackageVersionInfo>)>`:裸名查詢,回傳所有來源裡符合的項目,呼叫端(client)判斷 0/1/多筆。
- `get_package(source, name) -> &Vec<PackageVersionInfo>`:精確查詢(qualified 用)。
- 新增 workspace 依賴:`semver`(標準 `VersionReq`/`Version` 語法,跟 `Cargo.toml` 同一套,不自己刻約束解析器)。

### 4. 本地 DB schema(turso,多來源 + 多版本 + 兩種套件模式)

`LocalRepo` table 全面改版(`0002_multi_source.sql`,geni migration 系統第一次真的派上用場):

```sql
CREATE TABLE IF NOT EXISTS LocalRepo (
    source TEXT NOT NULL,
    name TEXT NOT NULL,
    version TEXT NOT NULL,
    kind TEXT NOT NULL,           -- 'prebuilt' | 'source'
    url TEXT,                     -- prebuilt 專用
    hash TEXT,                    -- prebuilt 專用
    filename TEXT,                -- prebuilt 專用
    build_command TEXT,           -- source 專用
    description TEXT NOT NULL,
    entry TEXT NOT NULL,
    dependencies TEXT,
    PRIMARY KEY (source, name, version)
);
```

`Db` API 調整:
- `read_one(source, name, version)` 取代舊的 `read_one(name)`(精確查詢)。
- `versions_of(source, name) -> Vec<DbPackage>`:給 pubgrub 的 `DependencyProvider` 查「這個套件有哪些版本可選」。
- `sources_of(name) -> Vec<String>`:給裸名衝突偵測用(`SELECT DISTINCT source FROM LocalRepo WHERE name = ?`)。

### 5. 名稱/版本解析流程(pubgrub 整合)

CLI 輸入語法(比照 npm):`[來源/]套件名[@版本約束]`,例:`dpm install foo`、`dpm install official/foo`、`dpm install foo@^1.2`。

1. 解析輸入:切 `/`(來源,可省略)跟 `@`(版本約束,可省略,預設 `*`)。
2. 沒給來源 → 查 `sources_of(name)`:0 筆報 `PackageNotFound`;2 筆以上報錯,要求改用 `來源/名稱` 明確指定,不做任何預設優先序;剛好 1 筆才自動帶入。
3. 把這次要裝的所有套件(CLI 可一次裝多個)組成 pubgrub 的 root 需求。
4. 實作 `pubgrub::DependencyProvider`,package identifier 用 `(source, name)` tuple(維持跟 install 解析同一套 collision-safety 規則,相依關係裡的名稱衝突處理方式跟 install 一致),版本查詢走 `versions_of`。
5. Solver 算出的每個 `(source, name, version)` 走 Section 8 的安裝路徑(依 `kind` 分岔)。
6. 已安裝套件是否滿足新的相依約束、要不要原地升級——這塊留給 implementation plan 階段定案,不在本 spec 展開演算法細節。

### 6. `dpm-server` 發布模型:不 host 任何二進位

`Repo/src/<pkg>/` 改名 `packages/<pkg>/`,底下兩種模式並存,由 `packageInfo.json.kind` 決定:

```json
{ "kind": "prebuilt", "url": "https://...", "hash": "...", "version": "1.2.0" }
```
```json
{ "kind": "source", "build": "make && make install PREFIX=$OUT", "version": "1.2.0" }
```

- **`prebuilt`**:`url` 一律指外部(不管是官方自己軟體的 repo release,還是第三方軟體自己 host 的地方),`dpm-server` 只負責去該 url 算 hash、寫進 `RepoInfo.json`。`Repo`(現 `packages`)目錄不會多出任何二進位。
- **`source`**:`packages/<pkg>/*` 直接放原始碼(小文字檔,PR 好審),沒有 `url`/`hash` 欄位,client 端直接抓這段原始碼在本機 build(Section 8)。
- `dpm-server build` 保留成**純本地開發便利工具**(想在自己機器先打包測試、或自己拿去別處 host),跟官方 repo 的發布流程完全脫鉤,不會被 CI 呼叫。
- 移除 `gh` CLI 依賴——沒有任何步驟需要建 GitHub Release。

### 7. GitHub Actions CI/CD(取代手動發布)

**PR 檢查(`.github/workflows/pr-check.yml`)**
1. 偵測 PR 動到哪個 `packages/<pkg>/`。
2. `kind: prebuilt`:抓 `url` 算 hash,跟 PR 裡宣稱的 hash 比對,對不上就紅燈。
3. `kind: source`:確認 `build` 欄位存在、`packages/<pkg>/` 有實際檔案。
4. 檢查套件名 + 版本號沒有跟既有 `RepoInfo.json` 撞(版本號不可重複發布)。
5. 全過才能 merge(GitHub branch protection 設「必須通過此 check」)。
6. **權限隔離**:此 workflow 用 `pull_request` trigger(不能用 `pull_request_target`),執行時不帶任何 secret、不給 write 權限——PR 內容是不信任輸入。

**發布(`.github/workflows/publish.yml`,merge 進 main 才觸發)**
1. 把新版本條目寫進 `RepoInfo.json`,bot commit 回 main。
2. 用 fine-grained PAT,只給這個 repo 的 contents write,不用帳號級 admin token。
3. 沒有 build、沒有 release 步驟。

第三方要開自己的 tap,套用同一套 `packages/` 目錄慣例 + 這兩個 workflow(寫成 reusable action,`uses:` 引用即可,不用重寫)。

### 8. Client 端兩種安裝路徑

依 `PackageVersionInfo.kind` 分岔:

- **`Prebuilt`**:維持現有流程——下載 zip、驗 hash(blake3,見 Section 10)、解壓、建 symlink。
- **`Source`**:client 用 git shallow clone + sparse-checkout 只拉該來源 repo 的 `packages/<pkg>/` 路徑(確切機制留待實作階段定案),抓下來後在暫存目錄執行 `build` 指令(環境變數 `$OUT` 指向安裝目的地),失敗就整個安裝失敗、不留殘骸。
- 兩種路徑最終都走 Section 11 的 staging + 原子 rename,不直接寫進最終安裝路徑。

### 9. 安全性補強

1. **`source` 模式的任意程式碼執行風險**(風險最高):`build` 指令永遠用當前呼叫者權限跑,**絕不因為 `--system` 就用 root 執行 build 本身**——即使是 system scope 安裝,build 階段一樣用一般權限跑在 staging 目錄,只有最後搬進 `/opt` 的原子 rename 步驟需要 elevated 權限。非官方來源的 `source` 套件,安裝前印警告。
2. **簽章**(呼應獨立子專案,見非目標):hash 只防竄改,不防「本來就是惡意但雜湊算得出來」。`RepoInfo.json` 整包用 minisign 簽,client 每個 source 釘一把公鑰,更新索引時驗簽章。本 spec 只確保 `sources` config 結構將來加得下一個 `pubkey` 欄位。
3. **CI workflow 權限隔離**:見 Section 7 第 6 點。
4. **強制 HTTPS**:`source add`/`prebuilt` 的 `url` 一律拒絕 `http://`。
5. **最小權限 token**:`publish.yml` 的 bot commit 用 fine-grained PAT,只給單一 repo 的 contents write。

### 10. blake3 取代 sha2

- `dpm`/`dpm-server` 兩邊各自複製一份的 `hasher()`(CLAUDE.md 已記錄的重複實作債務)這次一起搬進 `dpm-core` 共用,同時把演算法從 SHA256 換成 blake3。
- 乾脆換掉,不做多演算法相容(`"sha256:..."` 前綴),沒有需要相容的舊發布資料。
- `Cargo.toml` 移除 `sha2`,加 `blake3`。

### 11. tempfile 原子安裝

- 下載/build 產出物先進 staging 目錄,staging 目錄開在**目標安裝目錄同一個檔案系統下**(例如 `data_dir/.staging/<pkg>-<random>`),不是系統 `/tmp`——`std::fs::rename` 只有同檔案系統才是真原子操作,跨檔案系統會退化成 copy,失去原子性保證。
- 驗完 hash(或 build 成功)才 `rename` 進最終位置;任何步驟失敗,staging 目錄整個丟棄,已安裝的舊版本在 rename 那一刻前完全不受影響。
- `tempfile` 從現有 dev-dependency 升成正式 dependency。

## 影響範圍(粗略,細節留給 implementation plan)

- `dpm-core`:`RepoInfo`/`PackageBasicInfo`(重構為 `PackageVersionInfo`/`PackageKind`)、`Dependency`、`JsonStorage`、新增共用 `hasher()`。
- `dpm`:`config.json` schema、`cli_parse.rs`(`source` 子指令、`@version` 語法解析)、`db.rs`(schema 改版、pubgrub `DependencyProvider` 實作)、`action.rs`(install 流程分岔 prebuilt/source、原子安裝)、新增 `source add/remove/list` action。
- `dpm-server`:`packages/` 目錄改名、`fix add` 改吃 `--url`/`kind`、`build` 降級為本地便利工具、移除 `gh` 依賴。
- 新增 `.github/workflows/pr-check.yml`、`.github/workflows/publish.yml`(以及打算開放給第三方 tap 用的 reusable action)。
- 新增依賴:`semver`、`pubgrub`、`blake3`、`tempfile`(升級 dev → 正式)。移除:`sha2`。

## 已知風險

- 範圍非常大,橫跨 `dpm-core`/`dpm`/`dpm-server`/CI 四塊,寫 implementation plan 時必須拆成多份循序執行的 plan,不能一份做完——建議順序:(1) blake3 + tempfile(小、獨立、風險低,先墊地基)→ (2) 多來源/namespace(Section 1-4)→ (3) `dpm-server` 發布模型改版 + CI/CD(Section 6-7,`dpm` client 端讀新格式但先不用管 pubgrub)→ (4) client 端 `source` 安裝路徑(Section 8-9 的風險緩解要跟這個一起上)→ (5) pubgrub 真正接上(Section 5)。簽章機制之後另開子專案。
- `source` 模式的本地 build 是這次新增風險面最大的功能,即使做了權限隔離,執行第三方 shell 指令的本質風險還在,跟 AUR 面對的問題一樣——文件要明確告知使用者。
- pubgrub 的 `DependencyProvider` 需要能列出「某套件的所有版本」,效能上如果來源多、版本多,`dpm update` 要嘛全量重抓、要嘛之後要做增量更新,這次先接受全量重抓(維持現有 `clear_table` 全量覆蓋模式,只是現在 scope 到單一 source)。
- CI 的 `pr-check.yml` 對 `kind: source` 套件目前只檢查欄位存在、不會真的跑一次 build 驗證(因為 build 對不受信任 PR 內容執行有風險,呼應 Section 9);這代表 `source` 套件的 build 正確性要等使用者實際安裝時才會發現,是刻意的取捨,不是遺漏。
