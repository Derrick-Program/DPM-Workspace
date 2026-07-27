# apt/dnf 風格的 arch/os 套件分包設計

## 背景與動機

`dpm-core::PackageKind::Prebuilt { url, hash, file_name }` 一個版本只能登記一組下載目標——同一個套件同一個版本,不管使用者在 macOS arm64 還是 Linux x86_64,`dpm install` 都抓同一個網址。作者要嘛自己保證那份 prebuilt 是通用的(純 shell/python 腳本),要嘛只能土法用不同版本號區分平台,`dpm` 完全沒有依本機平台自動選擇的機制。

這次要仿照 apt/dnf 的模式:同一個套件、同一個版本,repo 索引裡可以登記多組 target-specific 的 build,client 依本機平台自動挑對的那組。

同時,官方預設來源 `Derrick-Program/DPM-Server` 目前是 **private + archived**,`RepoInfo.json` 也還是 Phase 2 多版本 schema之前的舊版單物件格式——這兩個問題不解決,新功能沒辦法在真實環境端到端驗證,這次一併修掉。

## 目標

- `PackageKind::Prebuilt` 從單一 `{ url, hash, file_name }` 改成 `{ builds: Vec<PrebuiltBuild> }`,`PrebuiltBuild { target: Option<String>, url, hash, file_name }`。`target` 用 Rust target triple(跟 `dpm` 自我更新已經在用的 `self_update::get_target()` 同一套字串),`None` 代表「任何平台通用」(對應 apt 的 `Architecture: all`)。
- `PackageKind::Source` 加一個 `supported_targets: Option<Vec<String>>` 欄位(`None` = 任何平台),`build` 指令本身不分平台(跟 apt source package 一樣,同一份 recipe 在哪台機器編就編出哪台機器的東西),這個欄位只用來在安裝前做「這台機器有沒有被列入支援」的檢查。
- 舊資料(沒有 `target`/`supported_targets` 欄位的既有已發布版本)反序列化時預設成「任何平台通用」——不是破壞性改動,不需要重新發布。
- `dpm-server fix add`:`AddKind::Url` 加可選 `--target <TRIPLE>`;`AddKind::Build` 加可選 `--targets <T1,T2,...>`(逗號分隔)。同一個 `(name, version)` 允許對不同 `--target` 各跑一次 `fix add ... url ...` 累加進同一個版本的 `builds` 清單——這是對「已發布版本不可覆寫」規則的唯一例外:同版本 + 不同 target 允許追加,同版本 + 同 target(含兩邊都是 `None`)一樣拒絕。
- `dpm install`:用 `self_update::get_target()` 拿本機 target,依序找完全匹配 → 退回 `target: None` 的通用 build → 兩者都沒有就報錯並列出這個版本實際支援哪些 target。`Source` 套件安裝前檢查 `supported_targets`,不支援就清楚拒絕並列出支援清單。
- 修復 `Derrick-Program/DPM-Server`:取消 archive、改 public、`RepoInfo.json` 重寫成多版本陣列 schema。
- 發布兩個 demo 套件到這個修好的官方 repo,並手動端到端驗證整條路徑真的能用:
  - `hello`(`Prebuilt`,`target: None`,通用)
  - `addsub`(`Source`,C 寫的 `int add(int,int)`/`int subtract(int,int)`,`supported_targets` 實際填內容)

## 非目標

- `Source` 套件不支援「不同 target 不同 build 指令」——只有一組 build 指令 + 一份支援清單,不是 apt/dnf 那種每個 arch 各自的 source package variant。
- 不寫任何自動交叉編譯機制——`Source` 套件的 build 指令在安裝當下的機器上跑,產出什麼平台的東西完全看那台機器;`Prebuilt` 的多組 target build 由套件作者自己另外編好、各自上傳,`dpm-server`/`dpm` 都不負責交叉編譯。
- 這次的 demo 不含真的交叉編譯出多份不同平台二進位的 `Prebuilt` 範例——`hello` 刻意只示範「通用 target」這條路徑,`Prebuilt` 的多 target 選擇邏輯本身透過單元測試(手寫多組 fixture)驗證,不追求手動端到端測試涵蓋交叉編譯。
- 不寫 JSON→新 schema 的自動遷移工具給 `Derrick-Program/DPM-Server` 用——這個 repo 的 `RepoInfo.json` 內容量小(4 個測試套件),手動改寫比寫遷移工具划算。
- 不改動 `dpm-server`/`dpm` 現有的「已發布版本不可覆寫」規則本身,只在「同版本追加不同 target」這一個情境開例外。

## 架構

### `dpm-core` 資料模型

```rust
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum PackageKind {
    Prebuilt {
        builds: Vec<PrebuiltBuild>,
    },
    Source {
        build: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hash: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        supported_targets: Option<Vec<String>>,
    },
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PrebuiltBuild {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    pub url: String,
    pub hash: String,
    pub file_name: String,
}
```

舊格式相容:目前的 `Prebuilt { url, hash, file_name }`(單一組,無 `builds` 陣列)在反序列化時視為「一組 `builds`,`target: None`」——用 serde 的 untagged/自訂 `Deserialize` 或遷移前手動改寫既有資料達成(實作時擇一,寫進 implementation plan)。`Source` 的 `supported_targets` 用 `#[serde(default)]` 自然相容,無需特殊處理。

`to_db_fields`/`from_db_fields`(`PackageKind` 扁平化存進 `LocalRepo` 的邏輯)要跟著更新,把 `builds`/`supported_targets` 序列化進本地 DB 對應欄位(細節見 implementation plan)。

### `dpm-server` CLI

`AddKind::Url` 新增:
```rust
Url {
    url: String,
    #[arg(long)]
    file_name: Option<String>,
    /// Rust target triple this build is for (省略 = 任何平台通用)
    #[arg(long)]
    target: Option<String>,
}
```

`AddKind::Build` 新增:
```rust
Build {
    build: String,
    /// 逗號分隔的支援 target 清單(省略 = 任何平台)
    #[arg(long, value_delimiter = ',')]
    targets: Option<Vec<String>>,
}
```

`fix_add` 的「已發布版本不可覆寫」規則調整:同 `(name, version)` 已存在時,若是 `Prebuilt` 且新的 `--target` 在既有 `builds` 裡沒出現過,追加進去;若 `--target` 已存在(含兩邊都省略,即兩個 `None`),拒絕並報「這個版本的 <target 或「通用」> build 已經發布過」。`Source` 套件(單一 build 指令)維持原本「同版本整個拒絕覆寫」規則不變——`--targets` 只在**第一次** `fix add ... build ...` 時生效,之後同版本再 `fix add` 一律拒絕(不像 `Prebuilt` 可以追加)。

### `dpm` client 安裝邏輯

安裝 `Prebuilt` 套件時:
1. 用 `self_update::get_target()` 拿本機 target 字串。
2. 在 `builds` 裡找 `target` 完全匹配的那組(字串精確相等比對,不做前綴/模糊匹配)。
3. 找不到,退回找 `target: None` 的那組。
4. 兩者都沒有,回傳錯誤,訊息列出這個版本 `builds` 裡實際登記的所有 target(方便使用者知道要不要換一台機器或等作者補上)。

安裝 `Source` 套件時,先檢查 `supported_targets`:`None` 或包含本機 target 才繼續原有的 clone+build 流程;否則直接拒絕,訊息列出 `supported_targets` 內容。

### 修復 `Derrick-Program/DPM-Server`(人工操作 + `dpm-server` CLI)

1. `gh repo edit Derrick-Program/DPM-Server --visibility public` + 取消 archive。
2. 本機 clone 這個 repo,手動把 `RepoInfo.json` 的 4 個既有測試套件(`test`/`helloWorld`/`test1`/`test2`)從單物件改寫成多版本陣列格式(`"test": [{...}]`),或直接砍掉重建(這幾個本來就是佔位測試資料,不是真實使用者在用的套件)。
3. 用這次改完的 `dpm-server` CLI,`keygen`+`init`+`hash`+`sign`+`fix add` 發布 `hello`(`Prebuilt`,無 `--target`)、`addsub`(`Source`,`--targets` 填實際支援的平台)兩個新套件。
4. Push 回 GitHub。

## 資料流

**安裝 Prebuilt(例:`hello`)**:`dpm update` 拉回 `RepoInfo.json` → 本地 DB 存 `builds` 陣列 → `dpm install hello` 讀出這個版本的 `builds`,依上述「完全匹配→通用→報錯」規則挑一組 → 下載該組 `url`、驗證 `hash`、放進安裝目錄。

**安裝 Source(例:`addsub`)**:`dpm install addsub` 讀出 `supported_targets`,檢查本機 target 是否在清單內(或清單是 `None`)→ 通過才 `git2` clone `repo_url` 執行 `build` 指令 → 產出的 `libaddsub.dylib`/`.so` 透過既有 `entry` 符號連結機制放進安裝目錄(`entry` 這次指向編譯產物本身,不是可執行檔——`dpm-server init` 的 `entry` 參數語意不變,只是這次填的路徑不是傳統意義上的「執行檔」)。

## 錯誤處理

- `Prebuilt` 套件在本機 target 找不到匹配、也沒有通用 build:回傳清楚錯誤,列出這個版本實際登記的 target 清單。
- `Source` 套件本機 target 不在 `supported_targets` 內:回傳清楚錯誤,列出 `supported_targets` 內容。
- `dpm-server fix add` 對已存在的 `(name, version, target)` 組合:拒絕,訊息講明是哪個 target 已經發布過。

## 測試計畫

- `dpm-core`:`PrebuiltBuild`/`PackageKind::Source.supported_targets` 的 serde round-trip;舊格式(單一 `url`/`hash`/`file_name`,無 `builds` 陣列)反序列化相容性測試。
- `dpm-server`:`fix_add` 同版本追加不同 `--target` 成功;追加重複 `--target`(含兩邊都省略)被拒;`--targets` 正確寫入 `supported_targets`;`Source` 套件同版本第二次 `fix add` 依然整個拒絕(不因為加了 `--targets` 就變成可追加)。
- `dpm`:安裝時依 target 完全匹配挑對 build、退回通用 build、兩者都沒有時報錯且列出支援清單(這三種情境都用手寫 fixture 測,不需要真的交叉編譯);`Source` 套件在不支援的 target 上被拒、在支援的 target 上正常安裝。
- 手動端到端驗證(人工執行,比照 self-update plan 的 Task 6 模式):
  1. `Derrick-Program/DPM-Server` 取消 archive、改 public、`RepoInfo.json` 改寫成新 schema。
  2. 發布 `hello`(`Prebuilt`,無 `target`)、`addsub`(`Source`,`supported_targets` 填本機 target)兩個套件,push。
  3. `dpm source add`(或確認 official source 已指向這個 repo)+ `dpm update`,確認索引正確拉到含 `builds`/`supported_targets` 的新格式。
  4. `dpm install hello`,確認能跑。
  5. `dpm install addsub`,確認本機真的編出 `libaddsub.dylib`/`.so`。
  6. 寫 `main.c`,連結安裝好的 lib 的 `entry` 符號連結路徑,呼叫 `add`/`subtract`,印結果比對,證明真的能用。

## 驗證清單

- [ ] `cargo check --workspace`/`cargo clippy --workspace --all-targets -- -D warnings`/`cargo test --workspace` 通過
- [ ] `dpm-core`/`dpm-server` 的新單元測試涵蓋上述情境
- [ ] `Derrick-Program/DPM-Server` 已取消 archive、改 public
- [ ] `Derrick-Program/DPM-Server` 的 `RepoInfo.json` 已是新版多版本陣列 schema,且含 `hello`/`addsub` 兩個套件
- [ ] 手動跑過 `dpm install hello` 成功
- [ ] 手動跑過 `dpm install addsub`,本機真的編出共享庫檔案
- [ ] `main.c` 連結安裝好的 `addsub` lib,呼叫 `add`/`subtract` 印出正確結果
