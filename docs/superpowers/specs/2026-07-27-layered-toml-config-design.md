# 分層 TOML 配置系統設計

## 背景與動機

`dpm` 目前唯一的「config」是 `Setting { sources: Vec<Source> }`,存成 JSON,路徑依 `Context` 的 `Scope`(per-user / `--system`)而定——兩個 scope 各自一份,互不相通。`dpm-server` 完全沒有配置檔概念,`packages/`(`project_src`)、`Repo/`(`repo_dir`)、`keys/`(`keys_dir`)、`RepoInfo.json` 四個路徑全部硬編碼相對 `current_dir()`。

這次要把兩邊的設定都換成 TOML,並引入跟現有 `--system`/per-user 安裝 scope**脫鉤**的三層優先權:系統設定 < 使用者設定 < 環境變數,後者覆寫前者。

## 目標

- `dpm`、`dpm-server` 都改用 TOML 格式配置檔,透過 `config` crate 做三層合併讀取。
- 三層優先權:系統層(machine-wide,dpm 自己不會寫,只有系管手動編輯)< 使用者層(`ProjectDirs::config_dir()`,`dpm`/`dpm-server` 會自動產生與寫入)< 環境變數(最高優先權,per-run 覆寫)。
- 這套分層設定的位置**跟 `--system`/per-user 安裝 scope 無關**——不管目前用哪種 scope 裝套件,讀到的都是同一份分層設定。
- 首次安裝/首次執行時,若使用者層設定檔不存在,自動產生預設值(沿用現有「第一次執行寫預設 Setting」的行為,只是格式換成 TOML、路徑換成跟 scope 脫鉤的使用者層路徑)。
- 新增 `gen-config` subcommand(`dpm` 跟 `dpm-server` 都加),讓使用者手動(重新)產生使用者層預設設定檔;檔案已存在時預設拒絕,需要 `--force` 才覆寫。
- 既有讀寫這些設定的呼叫端(`dpm::action.rs::source()`、`dpm::lib.rs::entry()`、`dpm::utils::system.rs::init()`、`dpm::context.rs`、`dpm-server::main.rs`)全部遷移到新系統。

## 非目標

- 不新增 `sources` 以外,`dpm` 端目前沒有的其他可配置項——沒有具體需求就不無中生有。
- `sources`(`Vec<Source>`)本身不支援環境變數層覆寫——清單型欄位透過 `dpm source add/remove` 修改使用者層檔案,環境變數只覆寫純量欄位(`dpm-server` 的四個路徑)。
- 不做 `--system` scope 下系統管理員權限相關的新提權/降權邏輯——系統層設定檔一律唯讀,`dpm`/`dpm-server` 自己永遠不寫入,不會碰到既有 `SystemController` 的 sudo/chown 機制。
- 不引入 `toml_edit` 之類會保留註解/格式的編輯器——沿用現有 `JsonStorage` 的「整包讀出、整包寫回」simplicity,新增的 `TomlStorage` 鏡像同一個模式。

## 架構

### 新增:`dpm-core::config_layer`

```rust
pub fn load_layered<T: Default + Serialize + DeserializeOwned>(
    system_path: &Path,
    user_path: &Path,
    env_prefix: &str,
) -> CoreResult<T>
```

用 `config` crate 的 `Config::builder()` 依序疊三層:

1. `add_source(File::from(system_path).required(false))`
2. `add_source(File::from(user_path).required(false))`
3. `add_source(Environment::with_prefix(env_prefix).separator("__"))`

後加入的來源覆寫先加入的欄位,兩個檔案來源都 `required(false)`(不存在就跳過,缺的欄位由 `T::default()` 補)。這個函式只負責「讀取+合併」,不負責寫檔。

### 新增:`dpm-core::TomlStorage`

鏡像既有 `JsonStorage` 的 `from_json`/`to_json`,改存 TOML(用 `toml` crate 的 `to_string_pretty`/`from_str`):

```rust
pub trait TomlStorage: Sized {
    fn from_toml(path: &Path) -> CoreResult<Self>;
    fn to_toml(&self, path: &Path) -> CoreResult<()>;
}
```

專門給「寫回使用者層」用——`source add`/`remove`、首次執行自動產生預設值、`gen-config` 都透過這個 trait,一律只讀寫使用者層路徑,系統層/環境變數不受影響(跟 `git config --local` 只動 repo 層檔案同樣的概念)。

### `dpm` client 改動

- `Setting { sources: Vec<Source> }` 格式從 JSON 換 TOML(`[[sources]]` 區塊),struct 形狀不變。
- 系統層路徑:Linux `/etc/dpm/config.toml`;macOS `/Library/Application Support/com.duacodie.dpm/config.toml`(machine-wide,`/Library` 開頭,只有系管手動編輯)。
- 使用者層路徑:沿用現有 `ProjectDirs::from("com", "duacodie", "dpm").config_dir()` 下的 `config.toml`(`Context.config_dir` 現在已經在算這個路徑,不用新開)。
- `Context::config_path()` 拿掉現有的 scope 分支(不再依 `--system`/per-user 給不同路徑),一律指向使用者層路徑;另外新增一個回傳系統層路徑的常數/函式。
- `sources` 欄位不支援環境變數覆寫(見「非目標」)。

### `dpm-server` 改動

- 新增 `ServerConfig { project_src: String, repo_dir: String, keys_dir: String, repo_info: String }`,預設值等於現在硬編碼的 `"packages"`/`"Repo"`/`"keys"`/`"RepoInfo.json"`(相對 cwd)。
- 系統層路徑:Linux `/etc/dpm-server/config.toml`;macOS `/Library/Application Support/com.duacodie.dpm-server/config.toml`。
- 使用者層路徑:`ProjectDirs::from("com", "duacodie", "dpm-server").config_dir()` 下的 `config.toml`。
- 環境變數前綴 `DPM_SERVER__`(例:`DPM_SERVER__REPO_DIR=/srv/dpm-repo`)——四個欄位都是純量字串,三層都直接支援覆寫。
- `main.rs` 的 `project_src`/`repo_dir`/`keys_dir`/`software_repo_info` 四行,從「`current_dir()?.join(...)` 硬編碼」換成「`load_layered::<ServerConfig>(...)` 讀出來的欄位,再各自 `join` 上」——四個路徑本身相對或絕對都支援,設定檔裡填絕對路徑就是絕對路徑,填相對路徑就相對 cwd(維持現有語意)。

### 新 subcommand:`gen-config`

`dpm`、`dpm-server` 的 `Commands` enum(`cli_parse.rs`)都新增 `GenConfig { #[arg(long)] force: bool }`:

- 用 `T::default()` 產生預設值,透過 `TomlStorage::to_toml` 寫進**使用者層路徑**(從不寫系統層)。
- 使用者層檔案已存在且沒帶 `--force`:回傳錯誤,提示已存在、要 `--force` 才覆寫。
- 帶 `--force`:直接覆寫。
- 首次安裝/首次執行時的自動產生預設值,呼叫同一個底層函式(不帶 `--force` 語意的錯誤分支,因為那個路徑本來就是「檔案不存在才觸發」)。

### 首次安裝自動產生

- `dpm`:沿用現有 `SystemController::init()` 回傳 `(Setting, bool)` 的 `bool`(是不是第一次執行)這個既有機制——第一次執行時,若使用者層 `config.toml` 不存在,用上面同一個「產生預設值+`TomlStorage::to_toml`」邏輯寫入,不需要新狀態機。
- `dpm-server`:目前是短命一次性 CLI,沒有「第一次執行」的狀態追蹤——每次執行時,若使用者層 `config.toml` 不存在,一樣自動用預設值產生(冪等:產生後之後每次執行都直接讀到)。

## 資料流

1. **啟動時讀取**(`dpm`/`dpm-server` 都一樣):呼叫 `load_layered::<Setting>(...)`/`load_layered::<ServerConfig>(...)`,拿到三層合併後的「有效設定」。
2. **修改設定**(`dpm source add/remove`、`gen-config`):只透過 `TomlStorage` 讀寫使用者層那個實體檔案,不碰系統層、不碰環境變數(環境變數本來就無法被程式寫入,只能讓執行時的 shell 環境決定)。
3. **系統層**:全程唯讀,`dpm`/`dpm-server` 自己永遠不會寫入這個路徑,只有系統管理員手動用文字編輯器改。

## 錯誤處理

- 系統層/使用者層檔案不存在:視為「這層沒設定」,不是錯誤(`required(false)`)。
- 檔案存在但 TOML 語法錯誤:`load_layered` 回傳 `CoreError`,清楚指出是哪個路徑解析失敗,不靜默吞掉退回預設值(避免「明明改過設定,卻悄悄套用預設值」這種難查的行為)。
- `gen-config` 沒帶 `--force` 但檔案已存在:回傳錯誤,不覆寫。

## 測試計畫

- `dpm-core::config_layer` 單元測試:system-only、user-only、兩者都有(使用者層贏)、環境變數覆寫(環境變數贏)四種情境,用臨時目錄驗證優先權正確。
- `dpm-core::TomlStorage` 單元測試:寫入後讀回是同一個值(round-trip),對應 `JsonStorage` 既有測試的 TOML 版本。
- `dpm` `config_tests.rs` 補:`Context::config_path()` 在 per-user/`--system` 兩種 scope 下都指向同一份使用者層檔案路徑(證明脫鉤)。
- `dpm` `action.rs` 補 `gen-config` 的測試:預設拒絕已存在檔案、`--force` 正確覆寫、首次執行自動產生預設值。
- `dpm-server` `action.rs` 底部既有 `mod tests` 補:`ServerConfig` 三層合併正確、`gen-config` 行為同上。

## 驗證清單

- [ ] `cargo check --workspace` 通過
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` 通過
- [ ] `cargo test --workspace` 通過,含新增的 config_layer/TomlStorage/gen-config 測試
- [ ] 手動:刪掉使用者層 `config.toml`,執行 `dpm list -l`,確認自動產生預設檔案
- [ ] 手動:`dpm gen-config` 對已存在檔案報錯,`dpm gen-config --force` 正確覆寫
- [ ] 手動:設一個 `DPM_SERVER__REPO_DIR` 環境變數,確認 `dpm-server` 真的讀到覆寫後的路徑,優先權高於使用者層/系統層檔案
