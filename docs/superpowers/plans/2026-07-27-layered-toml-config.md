# 分層 TOML 配置系統 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 `dpm` 的 `Setting`(目前 JSON,路徑依 `--system`/per-user scope 而定)跟 `dpm-server` 的四個硬編碼 cwd 相對目錄(`packages`/`Repo`/`keys`/`RepoInfo.json`),都換成 TOML 格式、三層優先權(系統 < 使用者 < 環境變數)的配置系統,並各自新增 `gen-config` subcommand。

**Architecture:** `dpm-core` 新增兩個泛型工具:`config_layer::load_layered::<T>`(用 `config` crate 疊系統/使用者/環境變數三層,只負責讀)跟 `TomlStorage<T>`(鏡像既有 `JsonStorage`,只負責寫回使用者層)。`dpm`(`Context`)、`dpm-server`(新 `config.rs`)各自擁有自己的 OS 標準路徑計算(machine-wide 系統路徑 + `directories::ProjectDirs` 使用者路徑),呼叫共用的 `load_layered`/`TomlStorage`。七個循序 task:Task 1-2 是 `dpm-core` 的兩個共用工具(可平行,互不依賴);Task 3-4 是 `dpm` client 的遷移跟新 subcommand;Task 5-6 是 `dpm-server` 的新 config 模組跟新 subcommand;Task 7 是整個 workspace 收尾驗證跟文件補充。

**Tech Stack:** `config = "0.15"`(`default-features = false, features = ["toml"]`,只要 TOML,不需要 JSON/YAML/RON/INI 那些預設 parser)、`toml = "1"`(寫回用,版本對齊 `config` 0.15.25 自己 transitively 依賴的 `toml ^1.0.6`,避免同一個 binary 裡編兩份不同 major 版本的 `toml`)、`directories = "6.0.0"`(`dpm-server` 新增,版本比照 `dpm` 既有的那份)。

## Global Constraints

- `config`/`toml` 只加進 `crates/dpm-core/Cargo.toml`(本地依賴,不進根 `Cargo.toml` 的 `[workspace.dependencies]`)——只有 `dpm-core` 直接用這兩個 crate,`dpm`/`dpm-server` 只透過 `dpm_core::load_layered`/`dpm_core::TomlStorage` 間接使用,不需要各自宣告依賴。這跟既有 `blake3`/`ed25519-dalek`/`getrandom`/`hex` 只在 `dpm-core` 本地宣告是同一個慣例(不是每個依賴都要進 workspace deps,只有「多個 crate 都直接用」的才進)。
- `directories` 這次變成 `dpm`、`dpm-server` 兩個 crate 都要直接用(`dpm-server` 之前完全沒有這個依賴)——但**不要**順手把它升級成 workspace dependency:`dpm` 目前是本地宣告(`directories = "6.0.0"`),這個 plan 只在 `dpm-server` 也加一行一樣版本的本地宣告,不動 `dpm` 既有的宣告方式,也不建立新的 workspace dep 條目——維持現狀最小變動,不是這次的目標。
- `config_layer::load_layered`/`TomlStorage` 兩個都**不能**被 `client`/`server` feature gate——`dpm-core` 的 `[features] client = [] / server = []` 只 gate `impl` 區塊(CLAUDE.md 既有規則),但這兩個是純泛型工具,跟 client/server 業務邏輯無關,直接放在 crate 頂層、不包在任何 `#[cfg(feature = ...)]` 底下,兩個 binary 都能無條件用。
- `dpm-server` 的 `config.rs` 裡,`load_or_init`/`gen_config` 這兩個「會被單元測試呼叫」的函式,一律吃明確的 `&Path` 參數(不是內部自己呼叫 `directories::ProjectDirs` 算真實路徑)——這是 `dpm-server` 既有慣例(`action.rs` 所有函式都吃明確路徑,`main.rs` 才是唯一算真實路徑、串起來呼叫的地方),沿用同一個慣例,否則單元測試會意外寫到開發者自己機器的真實 home 目錄。
- 這是破壞性改動,**不寫**任何 JSON→TOML 自動遷移邏輯——現有 `config.json`(如果存在)不會被讀取或轉檔,使用者升級後舊檔案就是孤兒檔案,新路徑/新格式從頭產生。這個專案版本號還在 `0.1.x`,既有前例(diesel→turso 遷移)也是乾淨換裝、沒寫轉換器,這次延續同一個慣例。
- `Setting.sources`(`Vec<Source>`)不支援環境變數覆寫——這不是額外要寫的排除邏輯,是 `config` crate 環境變數層天生的限制(它沒有合理語法可以表示一個 struct 陣列),不用刻意寫程式碼去擋,順其自然即可,不要試圖「補一個環境變數陣列解析」這種沒人要求的功能。
- 每個有程式碼變動的 task 結束前都要跑過 `cargo check`/`cargo clippy -- -D warnings`/相關 `cargo test`;commit message 用 Conventional Commits(`type(scope): description`)格式。

---

## Task 1: `dpm-core` — `config_layer::load_layered`(讀取三層合併設定)

**Files:**
- Modify: `crates/dpm-core/Cargo.toml`
- Modify: `crates/dpm-core/src/error.rs`
- Create: `crates/dpm-core/src/config_layer.rs`
- Modify: `crates/dpm-core/src/lib.rs`(加 `mod config_layer; pub use config_layer::*;`)

**Interfaces:**
- Consumes:無(純新增)。
- Produces:`pub fn load_layered<T: Default + Serialize + DeserializeOwned>(system_path: &Path, user_path: &Path, env_prefix: &str) -> CoreResult<T>`——Task 3、Task 5 都會直接呼叫這個函式。`CoreError::ConfigError(String)`——新的錯誤變體,Task 2 的 `TomlStorage` 也會用到。

- [ ] **Step 1: `Cargo.toml` 加新依賴**

編輯 `crates/dpm-core/Cargo.toml`,在 `[dependencies]` 區塊新增兩行(維持既有依賴按字母排序的習慣,插進 `clap.workspace = true` 之後、`ed25519-dalek` 之前):

```toml
clap.workspace = true
config = { version = "0.15", default-features = false, features = ["toml"] }
ed25519-dalek = "2.1"
```

再往下,`serde.workspace = true` 之後、`thiserror.workspace = true` 之前加:

```toml
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
toml = "1"
walkdir.workspace = true
```

（`toml = "1"` 是刻意的:`config` 0.15.25 自己 transitively 依賴 `toml ^1.0.6`,這裡對齊同一個 major 版本,讓 Cargo 把兩邊解析成同一份 `toml` crate,不會在同一個 binary 裡編兩份不同 major 版本。）

- [ ] **Step 2: 確認新依賴能解析**

Run: `cargo check -p DPM-Core`
Expected: 編譯成功(兩個新依賴目前都還沒被程式碼用到,只是依賴解析)。

- [ ] **Step 3: `CoreError` 加 `ConfigError` 變體**

編輯 `crates/dpm-core/src/error.rs`,在 `AmbiguousPackage` 那個變體之後加:

```rust
    #[error("Ambiguous package '{0}': exists in multiple sources, specify source/name")]
    AmbiguousPackage(String),

    #[error("Config error: {0}")]
    ConfigError(String),
}
```

- [ ] **Step 4: 寫失敗的測試(TDD——函式還不存在)**

建立 `crates/dpm-core/src/config_layer.rs`:

```rust
use crate::{CoreError, CoreResult};
use config::{Config, Environment, File, FileFormat};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::path::Path;

/// 依「系統層 < 使用者層 < 環境變數」三層優先權,把 `system_path`/
/// `user_path` 兩個 TOML 檔案(都可以不存在,不存在就當這層沒設定)跟
/// `env_prefix` 開頭、以 `__` 分隔的環境變數合併成一份有效設定。後加入的
/// 來源覆寫先加入的欄位——這裡的加入順序就是優先權順序。這個函式只負責
/// 「讀取+合併」,不負責寫檔;寫檔(例如 `dpm source add` 改使用者層那份
/// 檔案)一律走 [`crate::TomlStorage`],只碰使用者層那個實體檔案,系統層/
/// 環境變數不受影響。
pub fn load_layered<T>(system_path: &Path, user_path: &Path, env_prefix: &str) -> CoreResult<T>
where
    T: Default + Serialize + DeserializeOwned,
{
    let cfg = Config::builder()
        .add_source(
            File::from(system_path)
                .format(FileFormat::Toml)
                .required(false),
        )
        .add_source(
            File::from(user_path)
                .format(FileFormat::Toml)
                .required(false),
        )
        .add_source(Environment::with_prefix(env_prefix).separator("__"))
        .build()
        .map_err(|e| CoreError::ConfigError(e.to_string()))?;

    cfg.try_deserialize::<T>()
        .map_err(|e| CoreError::ConfigError(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Default, Serialize, Deserialize, PartialEq, Clone)]
    struct TestSettings {
        #[serde(default)]
        name: String,
        #[serde(default)]
        value: String,
    }

    #[test]
    fn system_layer_alone_is_used_when_user_and_env_absent() {
        let dir = tempfile::tempdir().unwrap();
        let system_path = dir.path().join("system.toml");
        let user_path = dir.path().join("user.toml"); // 故意不建立
        std::fs::write(&system_path, "name = \"from-system\"\nvalue = \"sys\"\n").unwrap();

        let result: TestSettings =
            load_layered(&system_path, &user_path, "DPM_TEST_SYSTEM_ONLY").unwrap();

        assert_eq!(result.name, "from-system");
        assert_eq!(result.value, "sys");
    }

    #[test]
    fn user_layer_alone_is_used_when_system_and_env_absent() {
        let dir = tempfile::tempdir().unwrap();
        let system_path = dir.path().join("system.toml"); // 故意不建立
        let user_path = dir.path().join("user.toml");
        std::fs::write(&user_path, "name = \"from-user\"\nvalue = \"usr\"\n").unwrap();

        let result: TestSettings =
            load_layered(&system_path, &user_path, "DPM_TEST_USER_ONLY").unwrap();

        assert_eq!(result.name, "from-user");
        assert_eq!(result.value, "usr");
    }

    #[test]
    fn user_layer_overrides_system_layer_when_both_present() {
        let dir = tempfile::tempdir().unwrap();
        let system_path = dir.path().join("system.toml");
        let user_path = dir.path().join("user.toml");
        std::fs::write(&system_path, "name = \"from-system\"\nvalue = \"sys\"\n").unwrap();
        // 使用者層只覆寫 name,沒提到 value。
        std::fs::write(&user_path, "name = \"from-user\"\n").unwrap();

        let result: TestSettings =
            load_layered(&system_path, &user_path, "DPM_TEST_USER_WINS").unwrap();

        assert_eq!(result.name, "from-user", "使用者層必須贏過系統層");
        assert_eq!(
            result.value, "sys",
            "使用者層沒設定的欄位,必須還是落回系統層的值(欄位級合併,不是整檔取代)"
        );
    }

    #[test]
    fn env_var_overrides_both_file_layers() {
        let dir = tempfile::tempdir().unwrap();
        let system_path = dir.path().join("system.toml");
        let user_path = dir.path().join("user.toml");
        std::fs::write(&system_path, "name = \"from-system\"\nvalue = \"sys\"\n").unwrap();
        std::fs::write(&user_path, "name = \"from-user\"\nvalue = \"usr\"\n").unwrap();

        std::env::set_var("DPM_TEST_ENV_WINS__NAME", "from-env");

        let result: TestSettings =
            load_layered(&system_path, &user_path, "DPM_TEST_ENV_WINS").unwrap();

        std::env::remove_var("DPM_TEST_ENV_WINS__NAME");

        assert_eq!(result.name, "from-env", "環境變數必須贏過兩個檔案層");
        assert_eq!(
            result.value, "usr",
            "沒有對應環境變數的欄位,還是要從使用者層拿"
        );
    }
}
```

（四個測試各自用自己專屬、明顯不會撞名的 `env_prefix` 字串——這樣即使 `cargo test` 平行跑,不同測試函式操作的是完全不同名字的環境變數,不需要額外的序列化/鎖機制。）

- [ ] **Step 5: 加 `tempfile` dev-dependency、`mod`/`pub use` 接進 `lib.rs`**

編輯 `crates/dpm-core/Cargo.toml`,`[dev-dependencies]` 區塊(目前只有 `tokio.workspace = true`)加一行:

```toml
[dev-dependencies]
tempfile = "3.10.1"
tokio.workspace = true
```

編輯 `crates/dpm-core/src/lib.rs`,把開頭的:

```rust
mod error;
mod zip_file;
```

改成:

```rust
mod config_layer;
mod error;
mod zip_file;
```

再把:

```rust
pub use error::*;
```

之後、`use serde::{Deserialize, Serialize};` 之前補一行:

```rust
pub use config_layer::*;
pub use error::*;
```

- [ ] **Step 6: 跑測試,確認通過**

Run: `cargo test -p DPM-Core config_layer`
Expected: 4 個測試全部 PASS(`system_layer_alone_is_used_when_user_and_env_absent`、`user_layer_alone_is_used_when_system_and_env_absent`、`user_layer_overrides_system_layer_when_both_present`、`env_var_overrides_both_file_layers`)。

- [ ] **Step 7: clippy**

Run: `cargo clippy -p DPM-Core --all-targets -- -D warnings`
Expected: 無警告無錯誤。

- [ ] **Step 8: Commit**

```bash
git add crates/dpm-core/Cargo.toml crates/dpm-core/src/error.rs crates/dpm-core/src/config_layer.rs crates/dpm-core/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(dpm-core): add load_layered() for system<user<env TOML config

New config_layer module wraps the `config` crate into a single generic
load_layered::<T>(system_path, user_path, env_prefix) that merges three
priority tiers (later sources override earlier fields, not whole
files). Both file sources are optional — a missing file just means
that tier contributes nothing, T::default() fills the rest. This is
the read-side half of the layered config system dpm/dpm-server will
both use; write-back (TomlStorage) is a separate task.

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: `dpm-core` — `TomlStorage`(寫回使用者層)

**Files:**
- Modify: `crates/dpm-core/src/lib.rs`

**Interfaces:**
- Consumes:`CoreError::ConfigError`(Task 1)。
- Produces:`pub struct TomlStorage<T>`,`impl<T> TomlStorage<T> { pub fn from_toml(path: &Path) -> CoreResult<T>; pub fn to_toml(data: &T, path: &Path) -> CoreResult<()>; }`——Task 3、Task 5、Task 6 都會直接呼叫。

- [ ] **Step 1: 寫失敗的測試(TDD——型別還不存在)**

編輯 `crates/dpm-core/src/lib.rs`,找到現有 `JsonStorage` 的 `impl` 區塊結束處(`to_json` 那個方法結束的 `}` 之後、`from_url` 那些 async 方法之前,或整個 `impl<T> JsonStorage<T> { ... }` 區塊結束後),在同一個檔案裡新增一個獨立的 `#[cfg(test)] mod toml_storage_tests`(放在檔案最底部,`Dependency` 的 `impl` 之後):

```rust
#[cfg(test)]
mod toml_storage_tests {
    use super::*;

    #[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
    struct Demo {
        name: String,
        count: i64,
    }

    #[test]
    fn to_toml_then_from_toml_round_trips_the_same_value() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("demo.toml");
        let original = Demo {
            name: "hello".to_string(),
            count: 3,
        };

        TomlStorage::to_toml(&original, &path).unwrap();
        assert!(path.exists());

        let reloaded: Demo = TomlStorage::from_toml(&path).unwrap();
        assert_eq!(reloaded, original);
    }

    #[test]
    fn from_toml_on_missing_file_is_an_io_error() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does-not-exist.toml");
        let err = TomlStorage::<Demo>::from_toml(&missing).unwrap_err();
        assert!(
            matches!(err, CoreError::IoError(_)),
            "missing file must surface as CoreError::IoError, got: {err:?}"
        );
    }
}
```

- [ ] **Step 2: 跑測試,確認因為型別不存在而編譯失敗**

Run: `cargo test -p DPM-Core toml_storage_tests`
Expected: 編譯失敗,錯誤訊息包含 `cannot find type `TomlStorage``。

- [ ] **Step 3: 實作 `TomlStorage`**

在 `crates/dpm-core/src/lib.rs` 裡,找到既有 `impl<T> JsonStorage<T> ... { ... }` 這個 impl 區塊(`to_json` 方法、再往下還有 `from_url`/`from_str_to` 等方法)結束的 `}` 之後,插入:

```rust
/// 跟 `JsonStorage` 同一個「整包讀出、整包寫回」的模式,只是格式換成
/// TOML——分層設定系統裡,唯一會被程式「寫入」的一層(使用者層那個實體
/// 檔案)透過這個型別讀寫;系統層/環境變數是唯讀的,不會經過這裡,合併讀取
/// 走 [`load_layered`]。
pub struct TomlStorage<T> {
    _marker: std::marker::PhantomData<T>,
}

impl<T> TomlStorage<T>
where
    T: Serialize + for<'de> Deserialize<'de>,
{
    pub fn from_toml(path: &Path) -> CoreResult<T> {
        let contents = std::fs::read_to_string(path)?;
        toml::from_str(&contents).map_err(|e| CoreError::ConfigError(e.to_string()))
    }

    pub fn to_toml(data: &T, path: &Path) -> CoreResult<()> {
        let contents =
            toml::to_string_pretty(data).map_err(|e| CoreError::ConfigError(e.to_string()))?;
        std::fs::write(path, contents)?;
        Ok(())
    }
}
```

- [ ] **Step 4: 跑測試,確認通過**

Run: `cargo test -p DPM-Core toml_storage_tests`
Expected: 2 個測試全部 PASS。

- [ ] **Step 5: clippy**

Run: `cargo clippy -p DPM-Core --all-targets -- -D warnings`
Expected: 無警告無錯誤。

- [ ] **Step 6: Commit**

```bash
git add crates/dpm-core/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(dpm-core): add TomlStorage for writing the user config tier

Mirrors the existing JsonStorage pattern (from_json/to_json) but for
TOML. This is the write-back half of the layered config system —
source add/remove, gen-config, and first-run bootstrap all write
through this, always to the user-tier file, never the system tier.

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: `dpm` client — `Setting` 換 TOML,`Context.config_dir` 跟 scope 脫鉤

**依賴 Task 1、Task 2。**

**Files:**
- Modify: `crates/dpm/src/context.rs`
- Modify: `crates/dpm/src/utils/system.rs`
- Modify: `crates/dpm/src/action.rs`
- Modify: `crates/dpm/tests/config_tests.rs`

**Interfaces:**
- Consumes:`dpm_core::load_layered`(Task 1)、`dpm_core::TomlStorage`(Task 2)。
- Produces:`Context::config_path(&self) -> PathBuf`(不變,行為改成一律指向使用者層、副檔名變 `.toml`)、新的 `Context::system_config_path() -> PathBuf`(machine-wide 系統層路徑,不吃 `&self`)。

- [ ] **Step 1: `Context::config_dir` 不再依 scope 分支**

編輯 `crates/dpm/src/context.rs`,把:

```rust
fn compute_paths(scope: Scope) -> ClientResult<Paths> {
    match scope {
        Scope::PerUser => {
            let proj_dirs = ProjectDirs::from("com", "duacodie", "dpm").ok_or_else(|| {
                ClientError::SystemError("no valid home directory found".to_string())
            })?;
            let data_dir = proj_dirs.data_dir().to_path_buf();
            Ok(Paths {
                main_dir: data_dir.clone(),
                bin_dir: data_dir.join("bin"),
                install_dir: data_dir.join("Software"),
                config_dir: proj_dirs.config_dir().to_path_buf(),
            })
        }
        Scope::System => {
            let root = PathBuf::from("/opt/com.duacodie/DPM");
            Ok(Paths {
                main_dir: root.clone(),
                bin_dir: root.join("bin"),
                install_dir: root.join("Software"),
                config_dir: root.join("Settings"),
            })
        }
    }
}
```

換成:

```rust
fn compute_paths(scope: Scope) -> ClientResult<Paths> {
    let proj_dirs = ProjectDirs::from("com", "duacodie", "dpm").ok_or_else(|| {
        ClientError::SystemError("no valid home directory found".to_string())
    })?;
    // 分層設定系統的「使用者層」一律是這個 OS 標準的個人 config 目錄,
    // 跟目前是哪個 scope 在裝套件完全無關(兩套獨立概念)——見
    // docs/superpowers/specs/2026-07-27-layered-toml-config-design.md。
    let config_dir = proj_dirs.config_dir().to_path_buf();
    match scope {
        Scope::PerUser => {
            let data_dir = proj_dirs.data_dir().to_path_buf();
            Ok(Paths {
                main_dir: data_dir.clone(),
                bin_dir: data_dir.join("bin"),
                install_dir: data_dir.join("Software"),
                config_dir,
            })
        }
        Scope::System => {
            let root = PathBuf::from("/opt/com.duacodie/DPM");
            Ok(Paths {
                main_dir: root.clone(),
                bin_dir: root.join("bin"),
                install_dir: root.join("Software"),
                config_dir,
            })
        }
    }
}
```

- [ ] **Step 2: `config_path()` 換副檔名,新增 `system_config_path()`**

同一個檔案,把:

```rust
    /// 這一處
    pub fn config_path(&self) -> PathBuf {
        self.config_dir.join("config.json")
    }
```

（連同上面既有的文件註解一起)換成:

```rust
    /// 分層設定系統的使用者層路徑——`dpm` 唯一會寫入的那一層。
    pub fn config_path(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }

    /// 分層設定系統的系統層路徑(machine-wide)——`dpm` 自己永遠不會寫入
    /// 這個路徑,只有系統管理員手動編輯。不吃 `&self`,因為這是跟目前
    /// scope/instance 無關的固定常數。
    pub fn system_config_path() -> PathBuf {
        if cfg!(target_os = "macos") {
            PathBuf::from("/Library/Application Support/com.duacodie.dpm/config.toml")
        } else {
            PathBuf::from("/etc/dpm/config.toml")
        }
    }
```

- [ ] **Step 3: 更新既有測試,反映「脫鉤」這個新行為**

同一個檔案,底部 `mod tests` 裡,把:

```rust
    #[test]
    fn per_user_and_system_scopes_produce_different_roots() {
        let per_user = compute_paths(Scope::PerUser).unwrap();
        let system = compute_paths(Scope::System).unwrap();
        assert_ne!(per_user.main_dir, system.main_dir);
        assert_eq!(system.main_dir, PathBuf::from("/opt/com.duacodie/DPM"));
        assert_eq!(system.bin_dir, PathBuf::from("/opt/com.duacodie/DPM/bin"));
        assert_eq!(
            system.install_dir,
            PathBuf::from("/opt/com.duacodie/DPM/Software")
        );
        assert_eq!(
            system.config_dir,
            PathBuf::from("/opt/com.duacodie/DPM/Settings")
        );
    }
```

換成:

```rust
    #[test]
    fn per_user_and_system_scopes_produce_different_roots() {
        let per_user = compute_paths(Scope::PerUser).unwrap();
        let system = compute_paths(Scope::System).unwrap();
        assert_ne!(per_user.main_dir, system.main_dir);
        assert_eq!(system.main_dir, PathBuf::from("/opt/com.duacodie/DPM"));
        assert_eq!(system.bin_dir, PathBuf::from("/opt/com.duacodie/DPM/bin"));
        assert_eq!(
            system.install_dir,
            PathBuf::from("/opt/com.duacodie/DPM/Software")
        );
        assert_eq!(
            per_user.config_dir, system.config_dir,
            "分層設定系統的使用者層路徑,必須跟 --system/per-user 安裝 scope 脫鉤——兩種 scope 讀到的是同一份設定"
        );
    }
```

- [ ] **Step 4: 確認 `dpm` crate 能編譯(這裡還沒改 `system.rs`/`action.rs`,預期會報錯)**

Run: `cargo check -p DPM`
Expected: 編譯失敗——`system.rs`/`action.rs` 還在用 `JsonStorage`+`config.json` 語意,`config_tests.rs` 也還有舊的 JSON 斷言,這是預期中的中繼狀態,下面幾步會修完。

- [ ] **Step 5: `system.rs` 的 `init_first_run`/`init_existing` 改用 `TomlStorage`/`load_layered`**

編輯 `crates/dpm/src/utils/system.rs`,把:

```rust
    pub async fn init_first_run(&self, ctx: &Context) -> ClientResult<Setting> {
        self.bootstrap_dirs(ctx)?;
        let config_path = ctx.config_path();
        let default_setting = Setting {
            sources: vec![Source {
                alias: "official".to_string(),
                repo_url: OFFICIAL_REPO_URL.to_string(),
                repo_info: official_repo_info_url(OFFICIAL_REPO_URL),
            }],
        };
        JsonStorage::to_json(&default_setting, &config_path)?;
        self.permision_check(&ctx.main_dir)?;
        Ok(JsonStorage::from_json(&config_path)?)
    }

    /// OS bootstrap for every run after the first: `config.json` already
    /// exists, so this only (re-)creates the scope's directories and reads
    /// the existing config back.
    pub async fn init_existing(&self, ctx: &Context) -> ClientResult<Setting> {
        self.bootstrap_dirs(ctx)?;
        let config_path = ctx.config_path();
        Ok(JsonStorage::from_json(&config_path)?)
    }
```

換成:

```rust
    pub async fn init_first_run(&self, ctx: &Context) -> ClientResult<Setting> {
        self.bootstrap_dirs(ctx)?;
        let config_path = ctx.config_path();
        let default_setting = Setting {
            sources: vec![Source {
                alias: "official".to_string(),
                repo_url: OFFICIAL_REPO_URL.to_string(),
                repo_info: official_repo_info_url(OFFICIAL_REPO_URL),
            }],
        };
        dpm_core::TomlStorage::to_toml(&default_setting, &config_path)?;
        self.permision_check(&ctx.main_dir)?;
        Ok(dpm_core::load_layered(
            &Context::system_config_path(),
            &config_path,
            "DPM",
        )?)
    }

    /// OS bootstrap for every run after the first: the user-tier
    /// `config.toml` already exists, so this only (re-)creates the scope's
    /// directories and re-reads the effective (system < user < env) config.
    pub async fn init_existing(&self, ctx: &Context) -> ClientResult<Setting> {
        self.bootstrap_dirs(ctx)?;
        let config_path = ctx.config_path();
        Ok(dpm_core::load_layered(
            &Context::system_config_path(),
            &config_path,
            "DPM",
        )?)
    }

    /// `gen-config` subcommand:把預設 `Setting` 寫進使用者層。使用者層
    /// 已存在且沒帶 `force` 就拒絕——那個檔案可能已經被手動改過,不能悄悄
    /// 蓋掉。永遠不會碰系統層(`Context::system_config_path()`)。
    pub async fn gen_config(&self, ctx: &Context, force: bool) -> ClientResult<PathBuf> {
        self.bootstrap_dirs(ctx)?;
        let config_path = ctx.config_path();
        if config_path.exists() && !force {
            return Err(ClientError::ConfigError(format!(
                "{} already exists — pass --force to overwrite",
                config_path.display()
            )));
        }
        dpm_core::TomlStorage::to_toml(&Setting::default(), &config_path)?;
        Ok(config_path)
    }
```

（`gen_config` 這個方法本次就寫進去,Task 4 只需要串 CLI/`entry()`,不用回頭改 `system.rs`。）

同一個檔案頂端,把:

```rust
use dpm_core::JsonStorage;
use libc::{getpwuid, getuid};
use std::{
    ffi::CStr,
    path::Path,
    process::{Command, Stdio},
};
```

換成(`JsonStorage` 這行整個刪掉——改用完整路徑 `dpm_core::TomlStorage`/`dpm_core::load_layered`,不需要額外 `use`;`path::Path` 改成 `path::{Path, PathBuf}`——給 `gen_config` 的回傳型別 `ClientResult<PathBuf>` 用):

```rust
use libc::{getpwuid, getuid};
use std::{
    ffi::CStr,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
```

- [ ] **Step 6: `action.rs::source()` 改用 `TomlStorage`**

編輯 `crates/dpm/src/action.rs`,把:

```rust
    pub async fn source(&self, action: SourceAction) -> ClientResult<()> {
        let config_path = self.ctx.config_path();
        let mut setting: Setting = JsonStorage::from_json(&config_path)?;
```

換成:

```rust
    pub async fn source(&self, action: SourceAction) -> ClientResult<()> {
        let config_path = self.ctx.config_path();
        let mut setting: Setting = dpm_core::TomlStorage::from_toml(&config_path)?;
```

同一個函式裡的兩處:

```rust
                JsonStorage::to_json(&setting, &config_path)?;
```

（`SourceAction::Add`、`SourceAction::Remove` 兩個分支各一次)都換成:

```rust
                dpm_core::TomlStorage::to_toml(&setting, &config_path)?;
```

檔案頂端把:

```rust
use dpm_core::{Dependency, JsonStorage, PackageKind, RepoInfo, VerifyingKey};
```

換成(拿掉 `JsonStorage`——這個檔案裡 `JsonStorage` 只有上面那 3 處 `Setting` 讀寫在用,拿掉之後沒有其他地方會報 unused import 以外的錯):

```rust
use dpm_core::{Dependency, PackageKind, RepoInfo, VerifyingKey};
```

- [ ] **Step 7: 更新 `config_tests.rs`,JSON→TOML**

編輯 `crates/dpm/tests/config_tests.rs`,整份改成:

```rust
#[cfg(test)]
mod config_tests {
    use DPM::{Context, Scope, Setting, Source, SystemController};

    #[test]
    fn setting_round_trips_through_toml() {
        let setting = Setting {
            sources: vec![Source {
                alias: "official".to_string(),
                repo_url: "https://github.com/Derrick-Program/DPM-Server".to_string(),
                repo_info:
                    "https://raw.githubusercontent.com/Derrick-Program/DPM-Server/main/RepoInfo.json"
                        .to_string(),
            }],
        };

        let toml = toml::to_string(&setting).unwrap();
        let parsed: Setting = toml::from_str(&toml).unwrap();

        assert_eq!(parsed.sources.len(), 1);
        assert_eq!(parsed.sources[0].alias, "official");
    }

    #[test]
    fn setting_defaults_to_empty_sources_when_missing() {
        let parsed: Setting = toml::from_str("").unwrap();
        assert!(parsed.sources.is_empty());
    }

    /// `init_first_run()` 本身還是沒有被這個測試整個跑過:第一次執行時,
    /// `init_first_run()` 一律會真的打網路去 seed "official" source
    /// (`ActionInfo::init_update` -> `RepoInfo::fetch_update_repo_info`)——
    /// 這是既有、跟這次改動無關的行為,不該讓單元測試依賴真實網路。
    ///
    /// 這裡驗證的是 `init_first_run()` 真正依賴的持久化機制本身:
    /// `TomlStorage::to_toml` 把一個 `Setting` 寫進真實檔案,再用
    /// `TomlStorage::from_toml` 讀回來,證明這條路徑真的能在真實檔案系統上
    /// 來回一致(不是只在記憶體裡的 `String` 打轉)。
    #[test]
    fn setting_persists_to_disk_and_reloads_via_toml_storage() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        assert!(!config_path.exists());

        let default_setting = Setting {
            sources: vec![Source {
                alias: "official".to_string(),
                repo_url: "https://github.com/Derrick-Program/DPM-Server".to_string(),
                repo_info:
                    "https://raw.githubusercontent.com/Derrick-Program/DPM-Server/main/RepoInfo.json"
                        .to_string(),
            }],
        };
        dpm_core::TomlStorage::to_toml(&default_setting, &config_path).unwrap();

        assert!(
            config_path.exists(),
            "config.toml must actually exist on disk after TomlStorage::to_toml, \
             not just live in an in-memory struct"
        );
        let reloaded: Setting = dpm_core::TomlStorage::from_toml(&config_path).unwrap();
        assert_eq!(reloaded.sources.len(), 1);
        assert_eq!(reloaded.sources[0].alias, "official");
    }

    /// `Context::for_test` 給每個路徑(包括 `config_dir`)一份隔離的
    /// tempdir,而不是真實的 per-user/`--system` 位置。證明 `Context` 給出
    /// 的 `config_dir` 是真的可寫、真的隔離的,用跟 `init_first_run()`
    /// 內部一樣的 `TomlStorage` 寫/讀循環。
    #[tokio::test]
    async fn context_for_test_gives_an_isolated_writable_config_dir() {
        let root = tempfile::tempdir().unwrap();
        let ctx = Context::for_test(root.path()).await.unwrap();
        assert!(
            ctx.config_dir.starts_with(root.path()),
            "config_dir must live under the caller's tempdir, not a real machine path"
        );

        let config_path = ctx.config_dir.join("config.toml");
        assert!(!config_path.exists());

        let default_setting = Setting {
            sources: vec![Source {
                alias: "official".to_string(),
                repo_url: "https://github.com/Derrick-Program/DPM-Server".to_string(),
                repo_info:
                    "https://raw.githubusercontent.com/Derrick-Program/DPM-Server/main/RepoInfo.json"
                        .to_string(),
            }],
        };
        dpm_core::TomlStorage::to_toml(&default_setting, &config_path).unwrap();
        assert!(config_path.exists());
        let reloaded: Setting = dpm_core::TomlStorage::from_toml(&config_path).unwrap();
        assert_eq!(reloaded.sources.len(), 1);
    }

    #[test]
    fn system_controllers_with_different_scopes_coexist_in_one_process() {
        let per_user = SystemController::new(Scope::PerUser);
        let system = SystemController::new(Scope::System);

        let bogus_path = std::path::Path::new("/nonexistent/for/this/test");
        assert!(
            per_user.permision_check(bogus_path).is_ok(),
            "PerUser scope must never attempt an ownership change"
        );
        assert!(format!("{system:?}").contains("System"));
    }
}
```

- [ ] **Step 8: 確認整個 crate 能編譯**

Run: `cargo check -p DPM`
Expected: 編譯成功。

- [ ] **Step 9: clippy**

Run: `cargo clippy -p DPM --all-targets -- -D warnings`
Expected: 無警告無錯誤。

- [ ] **Step 10: 既有測試沒有回歸**

Run: `cargo test -p DPM`
Expected: 全部通過,包含更新過的 `config_tests`。

- [ ] **Step 11: Commit**

```bash
git add crates/dpm/src/context.rs crates/dpm/src/utils/system.rs crates/dpm/src/action.rs crates/dpm/tests/config_tests.rs
git commit -m "$(cat <<'EOF'
feat(dpm): migrate Setting to layered TOML config

Setting now persists as config.toml instead of config.json, loaded
through dpm-core's load_layered (system < user < env) instead of a
flat JsonStorage::from_json. Context.config_dir no longer branches on
Scope::PerUser vs Scope::System — the config layer's user tier is
always the OS-standard per-user config dir regardless of which scope
is installing packages; a new Context::system_config_path() gives the
machine-wide (read-only) tier. init_first_run/init_existing/source()
all switch from JsonStorage to TomlStorage/load_layered accordingly.

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: `dpm` client — `gen-config` subcommand

**依賴 Task 3**(用到 Task 3 寫好的 `SystemController::gen_config`)。

**Files:**
- Modify: `crates/dpm/src/cli_parse.rs`
- Modify: `crates/dpm/src/lib.rs`
- Test: `crates/dpm/tests/config_tests.rs`

**Interfaces:**
- Consumes:`SystemController::gen_config(&self, ctx: &Context, force: bool) -> ClientResult<PathBuf>`(Task 3)。
- Produces:`Commands::GenConfig { force: bool }`。

- [ ] **Step 1: `cli_parse.rs` 加新 subcommand**

編輯 `crates/dpm/src/cli_parse.rs`,在 `Commands` enum 的 `Source { ... }` 變體之後加:

```rust
    /// Manage package sources
    #[command(subcommand_required = true, arg_required_else_help = true)]
    Source {
        #[command(subcommand)]
        action: SourceAction,
    },
    /// Generate a default config.toml at the user config layer
    #[command(visible_alias = "gc")]
    GenConfig {
        /// Overwrite an existing user-tier config.toml
        #[arg(long)]
        force: bool,
    },
}
```

- [ ] **Step 2: `lib.rs::entry()` 攔在最前面處理**

編輯 `crates/dpm/src/lib.rs`,把:

```rust
pub async fn entry(ctx: Context, config: Cli) -> ClientResult<()> {
    let system_controller = SystemController::new(ctx.scope);
    let config_path = ctx.config_path();
    let setting_config = if !config_path.exists() {
```

換成:

```rust
pub async fn entry(ctx: Context, config: Cli) -> ClientResult<()> {
    let system_controller = SystemController::new(ctx.scope);

    // `gen-config` 必須在「檔案不存在就自動 seed 預設值」那段邏輯之前處理
    // 完畢並直接回傳——不然全新安裝時,第一次呼叫 `gen-config` 會先被
    // 下面的 first-run 邏輯自動寫出預設檔案,`gen_config` 自己再看到「檔案
    // 已存在」而要求 `--force`,對使用者來說是很confusing 的雙重寫入。
    // 用 `&config.command` 借用、不是 `config.command` 移動所有權——下面
    // 第二個 `match config.command { ... }` 之後還要按值 match 同一個
    // 欄位(其他分支要把 `pn: Vec<String>` 這類欄位移進
    // `ActionInfo::new(...)`),這裡先用引用形式只是「偷看一眼是不是
    // GenConfig」,不能把它整個消耗掉,不然下面那個 match 會編譯失敗
    // (use of moved value)。`force` 因此綁定成 `&bool`,呼叫
    // `gen_config` 前用 `*force` 解引用成 `bool`(`bool` 是 `Copy`,解引用
    // 沒有所有權問題)。
    if let Some(Commands::GenConfig { force }) = &config.command {
        let path = system_controller.gen_config(&ctx, *force).await?;
        println!("wrote default config to {}", path.display());
        return Ok(());
    }

    let config_path = ctx.config_path();
    let setting_config = if !config_path.exists() {
```

再往下,把既有 `match config.command { ... }` 那個 match 式的結尾(`None => return Err(...)` 那個 arm 之前或之後)補上第 9 個 arm,讓這個 match 保持 exhaustive:

```rust
        Some(Commands::Source { action }) => {
            ActionInfo::new(ctx.clone(), vec![], false, setting_config)
                .source(action)
                .await?
        }
        // 已經在函式最前面攔截並提早回傳了,這裡理論上永遠不會執行到。
        Some(Commands::GenConfig { .. }) => unreachable!("GenConfig is handled earlier in entry()"),
        None => return Err(ClientError::ConfigError("no command given".to_string())),
```

- [ ] **Step 3: 確認整個 crate 能編譯**

Run: `cargo check -p DPM`
Expected: 編譯成功。

- [ ] **Step 4: 寫失敗的測試(TDD——`gen_config` 邏輯還沒被單元測試覆蓋過)**

在 `crates/dpm/tests/config_tests.rs` 的 `mod config_tests { ... }` 裡追加(`system_controllers_with_different_scopes_coexist_in_one_process` 之後):

```rust
    #[tokio::test]
    async fn gen_config_writes_default_setting_when_missing() {
        let root = tempfile::tempdir().unwrap();
        let ctx = Context::for_test(root.path()).await.unwrap();
        let controller = SystemController::new(Scope::PerUser);

        let path = controller.gen_config(&ctx, false).await.unwrap();

        assert!(path.exists());
        let reloaded: Setting = dpm_core::TomlStorage::from_toml(&path).unwrap();
        assert!(
            reloaded.sources.is_empty(),
            "gen-config writes Setting::default(), not the seeded 'official' source"
        );
    }

    #[tokio::test]
    async fn gen_config_refuses_to_overwrite_without_force() {
        let root = tempfile::tempdir().unwrap();
        let ctx = Context::for_test(root.path()).await.unwrap();
        let controller = SystemController::new(Scope::PerUser);

        controller.gen_config(&ctx, false).await.unwrap();
        let err = controller.gen_config(&ctx, false).await.unwrap_err();
        assert!(err.to_string().contains("already exists"));
    }

    #[tokio::test]
    async fn gen_config_overwrites_when_force_is_true() {
        let root = tempfile::tempdir().unwrap();
        let ctx = Context::for_test(root.path()).await.unwrap();
        let controller = SystemController::new(Scope::PerUser);

        let path = controller.gen_config(&ctx, false).await.unwrap();
        dpm_core::TomlStorage::to_toml(
            &Setting {
                sources: vec![Source {
                    alias: "hand-edited".to_string(),
                    repo_url: "https://example.com".to_string(),
                    repo_info: "https://example.com/RepoInfo.json".to_string(),
                }],
            },
            &path,
        )
        .unwrap();

        controller.gen_config(&ctx, true).await.unwrap();

        let reloaded: Setting = dpm_core::TomlStorage::from_toml(&path).unwrap();
        assert!(
            reloaded.sources.is_empty(),
            "--force must overwrite the hand-edited content back to defaults"
        );
    }
```

- [ ] **Step 5: 跑測試,確認通過**

Run: `cargo test -p DPM gen_config`
Expected: 3 個測試全部 PASS(`gen_config_writes_default_setting_when_missing`、`gen_config_refuses_to_overwrite_without_force`、`gen_config_overwrites_when_force_is_true`)。

- [ ] **Step 6: clippy**

Run: `cargo clippy -p DPM --all-targets -- -D warnings`
Expected: 無警告無錯誤。

- [ ] **Step 7: 手動 smoke test**

Run: `cargo run -p DPM -- gen-config --help`
Expected: 印出 `gen-config`/`gc` 的 help 文字,含 `--force` 說明,不 panic。

- [ ] **Step 8: Commit**

```bash
git add crates/dpm/src/cli_parse.rs crates/dpm/src/lib.rs crates/dpm/tests/config_tests.rs
git commit -m "$(cat <<'EOF'
feat(dpm): add gen-config subcommand

dpm gen-config [--force] writes a default Setting to the user config
tier, refusing to overwrite an existing file unless --force is given.
Handled as an early return in entry(), before the existing "does
config.toml exist yet" first-run bootstrap, so a fresh install's very
first `dpm gen-config` invocation doesn't get confused by that
bootstrap having just written the same file moments earlier.

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: `dpm-server` — `ServerConfig` + `config.rs`

**依賴 Task 1、Task 2。**

**Files:**
- Modify: `crates/dpm-server/Cargo.toml`
- Create: `crates/dpm-server/src/config.rs`
- Modify: `crates/dpm-server/src/main.rs`

**Interfaces:**
- Consumes:`dpm_core::load_layered`(Task 1)、`dpm_core::TomlStorage`(Task 2)。
- Produces:`pub struct ServerConfig { pub project_src: String, pub repo_dir: String, pub keys_dir: String, pub repo_info: String }`、`pub fn system_config_path() -> PathBuf`、`pub fn user_config_path() -> CoreResult<PathBuf>`、`pub fn load_or_init(system_path: &Path, user_path: &Path, env_prefix: &str) -> CoreResult<ServerConfig>`、`pub fn gen_config(user_path: &Path, force: bool) -> CoreResult<()>`——Task 6、`main.rs` 都會用。

- [ ] **Step 1: `Cargo.toml` 加 `directories` 依賴**

編輯 `crates/dpm-server/Cargo.toml`,在 `[dependencies]` 區塊、`DPM-Core` 之後加(維持字母排序,插在 `colored.workspace = true` 之後、`git2` 之前):

```toml
colored.workspace = true
directories = "6.0.0"
git2 = "0.18.1"
```

- [ ] **Step 2: 確認新依賴能解析**

Run: `cargo check -p DPM-Server`
Expected: 編譯成功。

- [ ] **Step 3: 寫失敗的測試(TDD——`ServerConfig`/相關函式都還不存在)**

建立 `crates/dpm-server/src/config.rs`:

```rust
use dpm_core::{CoreError, CoreResult};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// `dpm-server` 目前硬編碼相對 cwd 的四個路徑,搬進分層設定系統。所有
/// 欄位都是字串:填絕對路徑就是絕對路徑,填相對路徑就相對呼叫端的 cwd
/// (`main.rs` 用 `current_dir()?.join(&cfg.xxx)`——`Path::join` 遇到絕對
/// 路徑會直接取代,語意天然正確,不用額外判斷)。
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(default)]
pub struct ServerConfig {
    pub project_src: String,
    pub repo_dir: String,
    pub keys_dir: String,
    pub repo_info: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        ServerConfig {
            project_src: "packages".to_string(),
            repo_dir: "Repo".to_string(),
            keys_dir: "keys".to_string(),
            repo_info: "RepoInfo.json".to_string(),
        }
    }
}

/// 系統層路徑(machine-wide)——`dpm-server` 自己永遠不寫入,只有系統
/// 管理員手動編輯。
pub fn system_config_path() -> PathBuf {
    if cfg!(target_os = "macos") {
        PathBuf::from("/Library/Application Support/com.duacodie.dpm-server/config.toml")
    } else {
        PathBuf::from("/etc/dpm-server/config.toml")
    }
}

/// 使用者層路徑——`ProjectDirs` 算出來的個人 config 目錄下的
/// `config.toml`。這個函式本身只是路徑計算(不做任何檔案 I/O),測試裡
/// 呼叫它是安全的;真正的讀寫一律透過 [`load_or_init`]/[`gen_config`],
/// 兩者都吃明確的 `&Path`,不會自己重算一次真實路徑。
pub fn user_config_path() -> CoreResult<PathBuf> {
    let proj_dirs = directories::ProjectDirs::from("com", "duacodie", "dpm-server")
        .ok_or_else(|| CoreError::ConfigError("no valid home directory found".to_string()))?;
    Ok(proj_dirs.config_dir().join("config.toml"))
}

/// 讀出「有效」的三層合併設定;使用者層檔案不存在的話,先用預設值產生
/// 一份(冪等——之後每次執行都直接讀到)。
pub fn load_or_init(
    system_path: &Path,
    user_path: &Path,
    env_prefix: &str,
) -> CoreResult<ServerConfig> {
    if !user_path.exists() {
        if let Some(parent) = user_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        dpm_core::TomlStorage::to_toml(&ServerConfig::default(), user_path)?;
    }
    dpm_core::load_layered(system_path, user_path, env_prefix)
}

/// `gen-config` subcommand:把預設 `ServerConfig` 寫進使用者層路徑。
/// 已存在且沒帶 `force` 就拒絕。
pub fn gen_config(user_path: &Path, force: bool) -> CoreResult<()> {
    if user_path.exists() && !force {
        return Err(CoreError::ConfigError(format!(
            "{} already exists — pass --force to overwrite",
            user_path.display()
        )));
    }
    if let Some(parent) = user_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    dpm_core::TomlStorage::to_toml(&ServerConfig::default(), user_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_matches_the_previously_hardcoded_paths() {
        let cfg = ServerConfig::default();
        assert_eq!(cfg.project_src, "packages");
        assert_eq!(cfg.repo_dir, "Repo");
        assert_eq!(cfg.keys_dir, "keys");
        assert_eq!(cfg.repo_info, "RepoInfo.json");
    }

    #[test]
    fn load_or_init_creates_user_file_with_defaults_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let system_path = dir.path().join("system.toml");
        let user_path = dir.path().join("nested").join("user.toml");
        assert!(!user_path.exists());

        let cfg = load_or_init(&system_path, &user_path, "DPM_SERVER_TEST_INIT").unwrap();

        assert!(user_path.exists(), "user-tier file must be created");
        assert_eq!(cfg, ServerConfig::default());
    }

    #[test]
    fn load_or_init_merges_user_file_over_system_file() {
        let dir = tempfile::tempdir().unwrap();
        let system_path = dir.path().join("system.toml");
        let user_path = dir.path().join("user.toml");
        std::fs::write(&system_path, "repo_dir = \"/srv/from-system\"\n").unwrap();
        std::fs::write(&user_path, "repo_dir = \"/srv/from-user\"\n").unwrap();

        let cfg = load_or_init(&system_path, &user_path, "DPM_SERVER_TEST_MERGE").unwrap();

        assert_eq!(cfg.repo_dir, "/srv/from-user");
        assert_eq!(
            cfg.project_src, "packages",
            "fields neither layer sets must still fall back to ServerConfig::default()"
        );
    }

    #[test]
    fn env_var_overrides_repo_dir() {
        let dir = tempfile::tempdir().unwrap();
        let system_path = dir.path().join("system.toml");
        let user_path = dir.path().join("user.toml");
        std::fs::write(&user_path, "repo_dir = \"/srv/from-user\"\n").unwrap();

        std::env::set_var("DPM_SERVER_TEST_ENV__REPO_DIR", "/srv/from-env");
        let cfg = load_or_init(&system_path, &user_path, "DPM_SERVER_TEST_ENV").unwrap();
        std::env::remove_var("DPM_SERVER_TEST_ENV__REPO_DIR");

        assert_eq!(cfg.repo_dir, "/srv/from-env");
    }

    #[test]
    fn gen_config_refuses_to_overwrite_without_force() {
        let dir = tempfile::tempdir().unwrap();
        let user_path = dir.path().join("user.toml");

        gen_config(&user_path, false).unwrap();
        let err = gen_config(&user_path, false).unwrap_err();
        assert!(err.to_string().contains("already exists"));
    }

    #[test]
    fn gen_config_overwrites_when_force_is_true() {
        let dir = tempfile::tempdir().unwrap();
        let user_path = dir.path().join("user.toml");

        gen_config(&user_path, false).unwrap();
        std::fs::write(&user_path, "repo_dir = \"hand-edited\"\n").unwrap();

        gen_config(&user_path, true).unwrap();

        let reloaded: ServerConfig = dpm_core::TomlStorage::from_toml(&user_path).unwrap();
        assert_eq!(reloaded.repo_dir, "Repo");
    }
}
```

- [ ] **Step 4: 跑測試,確認因為模組還沒接進 `main.rs` 而編譯失敗（或者因為缺 `tempfile` dev-dependency 而失敗）**

Run: `cargo test -p DPM-Server config`
Expected: 編譯失敗——`tempfile` 還沒加進 `[dev-dependencies]`,或是 `config` 模組還沒被宣告(下一步補)。

- [ ] **Step 5: `Cargo.toml` 加 `tempfile` dev-dependency,`main.rs` 宣告 `mod config`**

編輯 `crates/dpm-server/Cargo.toml`,新增:

```toml
[dev-dependencies]
tempfile = "3.10.1"
```

編輯 `crates/dpm-server/src/main.rs`,把開頭的:

```rust
mod action;
mod cli_parse;
mod error;
pub use action::*;
```

換成:

```rust
mod action;
mod cli_parse;
mod config;
mod error;
pub use action::*;
pub use config::*;
```

- [ ] **Step 6: 跑測試,確認通過**

Run: `cargo test -p DPM-Server config`
Expected: 6 個測試全部 PASS(`default_matches_the_previously_hardcoded_paths`、`load_or_init_creates_user_file_with_defaults_when_missing`、`load_or_init_merges_user_file_over_system_file`、`env_var_overrides_repo_dir`、`gen_config_refuses_to_overwrite_without_force`、`gen_config_overwrites_when_force_is_true`)。

- [ ] **Step 7: `main.rs` 串接——把四個硬編碼路徑換成從 config 讀出來**

把 `crates/dpm-server/src/main.rs` 的:

```rust
fn main() -> Result<()> {
    let cli = Cli::parse();
    let project_src = current_dir()?.join("packages");
    let repo_dir = current_dir()?.join("Repo");
    let keys_dir = current_dir()?.join("keys");
    let software_repo_info = current_dir()?.join("RepoInfo.json");
    create_dir_all(&project_src)?;
    create_dir_all(&repo_dir)?;
    create_dir_all(&keys_dir)?;
```

換成:

```rust
fn main() -> Result<()> {
    let cli = Cli::parse();

    let cfg = load_or_init(&system_config_path(), &user_config_path()?, "DPM_SERVER")?;
    let project_src = current_dir()?.join(&cfg.project_src);
    let repo_dir = current_dir()?.join(&cfg.repo_dir);
    let keys_dir = current_dir()?.join(&cfg.keys_dir);
    let software_repo_info = current_dir()?.join(&cfg.repo_info);
    create_dir_all(&project_src)?;
    create_dir_all(&repo_dir)?;
    create_dir_all(&keys_dir)?;
```

- [ ] **Step 8: 確認整個 crate 能編譯**

Run: `cargo check -p DPM-Server`
Expected: 編譯成功。

- [ ] **Step 9: clippy**

Run: `cargo clippy -p DPM-Server --all-targets -- -D warnings`
Expected: 無警告無錯誤。

- [ ] **Step 10: 既有測試沒有回歸**

Run: `cargo test -p DPM-Server`
Expected: 全部通過,含 `config.rs` 新增的 6 個測試,以及 `action.rs` 底下既有的測試(那些測試直接呼叫 `hash`/`init`/`sign` 等函式並自帶 tempdir 路徑,不經過 `main.rs`,不受這次改動影響)。

- [ ] **Step 11: 手動 smoke test**

在一個乾淨的臨時目錄裡:

```bash
mkdir -p /tmp/dpm-server-config-smoke && cd /tmp/dpm-server-config-smoke
cargo run --manifest-path /path/to/DPM-Workspace/Cargo.toml -p DPM-Server -- keygen alice
ls packages Repo keys RepoInfo.json
```

Expected: 四個路徑都照預設值(`packages`/`Repo`/`keys`/`RepoInfo.json`)建在目前目錄下,行為跟改動前完全一樣(因為預設值就是原本的硬編碼值)。

- [ ] **Step 12: Commit**

```bash
git add crates/dpm-server/Cargo.toml crates/dpm-server/src/config.rs crates/dpm-server/src/main.rs
git commit -m "$(cat <<'EOF'
feat(dpm-server): move project_src/repo_dir/keys_dir/repo_info to config

New ServerConfig { project_src, repo_dir, keys_dir, repo_info } loaded
via dpm-core's load_layered (system < user < env, prefix DPM_SERVER)
instead of four paths hardcoded relative to current_dir(). Defaults
match the previous hardcoded values exactly, so behavior is unchanged
until someone actually sets a config.toml or DPM_SERVER__* env var.
load_or_init/gen_config take explicit &Path params (not real
ProjectDirs paths internally) so they stay unit-testable against
tempdirs, matching this crate's existing "functions take explicit
paths, main.rs wires the real ones" convention.

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: `dpm-server` — `gen-config` subcommand

**依賴 Task 5。**

**Files:**
- Modify: `crates/dpm-server/src/cli_parse.rs`
- Modify: `crates/dpm-server/src/main.rs`

**Interfaces:**
- Consumes:`config::gen_config`/`config::user_config_path`(Task 5)。
- Produces:`Commands::GenConfig(GenConfig)`,`struct GenConfig { pub force: bool }`。

- [ ] **Step 1: `cli_parse.rs` 加新 subcommand**

編輯 `crates/dpm-server/src/cli_parse.rs`,把:

```rust
#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Hash File or all in Project File
    Hash(Hash),
    /// Fix Packages.json
    Fix(Fix),
    /// Build Package
    Build(Build),
    ///Create Project
    Init(Init),
    /// Generate an ed25519 signing key pair for a package author
    Keygen(Keygen),
    /// Sign a package's packageInfo.json hash with its author's private key
    Sign(Sign),
}
```

換成:

```rust
#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Hash File or all in Project File
    Hash(Hash),
    /// Fix Packages.json
    Fix(Fix),
    /// Build Package
    Build(Build),
    ///Create Project
    Init(Init),
    /// Generate an ed25519 signing key pair for a package author
    Keygen(Keygen),
    /// Sign a package's packageInfo.json hash with its author's private key
    Sign(Sign),
    /// Generate a default config.toml at the user config layer
    GenConfig(GenConfig),
}
```

再於檔案底部(`Sign` struct 之後)加:

```rust
#[derive(Args, Debug)]
pub struct GenConfig {
    /// Overwrite an existing user-tier config.toml
    #[arg(long)]
    pub force: bool,
}
```

- [ ] **Step 2: `main.rs` 攔在最前面處理**

把 `crates/dpm-server/src/main.rs` 的:

```rust
fn main() -> Result<()> {
    let cli = Cli::parse();

    let cfg = load_or_init(&system_config_path(), &user_config_path()?, "DPM_SERVER")?;
```

換成:

```rust
fn main() -> Result<()> {
    let cli = Cli::parse();

    if let Commands::GenConfig(obj) = &cli.command {
        let path = user_config_path()?;
        gen_config(&path, obj.force)?;
        println!("wrote default config to {}", path.display());
        return Ok(());
    }

    let cfg = load_or_init(&system_config_path(), &user_config_path()?, "DPM_SERVER")?;
```

再把檔案底部的 match 式:

```rust
    match &cli.command {
        Commands::Hash(obj) => hash(obj, &project_src, &repo_dir)?,
        Commands::Fix(obj) => fix(obj, &mut repo_info, &project_src, &keys_dir)?,
        Commands::Build(obj) => build(obj, &project_src, &repo_dir)?,
        Commands::Init(obj) => init(obj, &project_src, &keys_dir)?,
        Commands::Keygen(obj) => keygen(obj, &keys_dir)?,
        Commands::Sign(obj) => sign(obj, &project_src, &keys_dir)?,
    }
```

換成:

```rust
    match &cli.command {
        Commands::Hash(obj) => hash(obj, &project_src, &repo_dir)?,
        Commands::Fix(obj) => fix(obj, &mut repo_info, &project_src, &keys_dir)?,
        Commands::Build(obj) => build(obj, &project_src, &repo_dir)?,
        Commands::Init(obj) => init(obj, &project_src, &keys_dir)?,
        Commands::Keygen(obj) => keygen(obj, &keys_dir)?,
        Commands::Sign(obj) => sign(obj, &project_src, &keys_dir)?,
        // 已經在函式最前面攔截並提早回傳了,這裡理論上永遠不會執行到。
        Commands::GenConfig(_) => unreachable!("GenConfig is handled earlier in main()"),
    }
```

- [ ] **Step 3: 確認整個 crate 能編譯**

Run: `cargo check -p DPM-Server`
Expected: 編譯成功。

- [ ] **Step 4: clippy**

Run: `cargo clippy -p DPM-Server --all-targets -- -D warnings`
Expected: 無警告無錯誤。

- [ ] **Step 5: 既有測試沒有回歸**

Run: `cargo test -p DPM-Server`
Expected: 全部通過。

- [ ] **Step 6: 手動 smoke test**

```bash
mkdir -p /tmp/dpm-server-genconfig-smoke && cd /tmp/dpm-server-genconfig-smoke
cargo run --manifest-path /path/to/DPM-Workspace/Cargo.toml -p DPM-Server -- gen-config
cargo run --manifest-path /path/to/DPM-Workspace/Cargo.toml -p DPM-Server -- gen-config
cargo run --manifest-path /path/to/DPM-Workspace/Cargo.toml -p DPM-Server -- gen-config --force
```

Expected:第一次印出 `wrote default config to ...` 並成功;第二次(沒帶 `--force`)報錯提到 `already exists`;第三次(帶 `--force`)成功覆寫。

- [ ] **Step 7: Commit**

```bash
git add crates/dpm-server/src/cli_parse.rs crates/dpm-server/src/main.rs
git commit -m "$(cat <<'EOF'
feat(dpm-server): add gen-config subcommand

dpm-server gen-config [--force] writes a default ServerConfig to the
user config tier, refusing to overwrite an existing file unless
--force is given — same UX as dpm's gen-config from the previous task.

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: 整個 workspace 收尾驗證 + 文件

**Files:**
- Modify: `README.md`
- Modify: `docs/CONTRIBUTE.MD`(視情況)

**Interfaces:** 無。

- [ ] **Step 1: `cargo check --workspace` 通過**

Run: `cargo check --workspace`
Expected: 編譯成功,無錯誤。

- [ ] **Step 2: 格式化檢查**

Run: `cargo fmt --all -- --check`
Expected: 無輸出。有輸出的話跑 `cargo fmt --all` 再重新檢查一次。

- [ ] **Step 3: clippy(整個 workspace)**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: 無警告無錯誤。

- [ ] **Step 4: 整個 workspace 測試**

Run: `cargo test --workspace`
Expected: 全部通過,包含這次新增的所有測試(`dpm-core` 的 `config_layer`/`toml_storage_tests` 共 6 個、`dpm` 的 `config_tests` 共 8 個、`dpm-server` 的 `config` 模組 6 個)。

- [ ] **Step 5: README.md 補一段設定檔說明**

編輯 `README.md`,在「### 子指令」表格後面(`install <name>` 支援單純套件名... 那段之後、「### 版本約束語法」之前),新增一個小節:

```markdown
### 設定檔(config.toml)

`dpm` 的設定分三層,後者覆寫前者:系統層(`/etc/dpm/config.toml`,Linux;`/Library/Application Support/com.duacodie.dpm/config.toml`,macOS,唯讀,系統管理員維護)< 使用者層(`dpm` 自動產生與寫入)< 環境變數。`dpm source add/remove` 只會改使用者層。

```bash
dpm gen-config          # 產生使用者層預設設定檔
dpm gen-config --force  # 已存在時強制覆寫
```
```

在「## Server(`dpm-server`)使用方式」段落、子指令表格之前,補一句(接在既有「`dpm-server` 目前沒有 prebuilt release...」那句之後):

```markdown
四個路徑(`project_src`/`repo_dir`/`keys_dir`/`repo_info`,預設 `packages`/`Repo`/`keys`/`RepoInfo.json`)可透過 `config.toml` 或 `DPM_SERVER__<FIELD>` 環境變數覆寫(例:`DPM_SERVER__REPO_DIR=/srv/dpm-repo`),同一套系統/使用者/環境變數三層規則。`dpm-server gen-config [--force]` 產生使用者層預設設定檔。
```

- [ ] **Step 6: 確認沒有漏 commit 的變動**

Run: `git status`
Expected: working tree clean(Task 1-6 每個都已經各自 commit 過)。

- [ ] **Step 7: Commit(文件)**

```bash
git add README.md
git commit -m "$(cat <<'EOF'
docs: document the layered TOML config system in README

Covers dpm's config.toml (system < user < env, gen-config subcommand)
and dpm-server's four configurable paths (project_src/repo_dir/
keys_dir/repo_info via config.toml or DPM_SERVER__* env vars).

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>
EOF
)"
```
