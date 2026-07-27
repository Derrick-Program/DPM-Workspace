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
