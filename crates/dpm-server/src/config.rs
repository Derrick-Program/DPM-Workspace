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
        // Unix-only crate (libc/sudo deps, `system_command_runner` rejects
        // anything that isn't Linux or macOS), so this branch means "Linux"
        // in practice, not literally "every non-macOS OS".
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
        // Empty file, not a serialized ServerConfig::default() — see the
        // equivalent comment in dpm's SystemController::gen_config for why a
        // fully-populated default file would permanently shadow the system tier.
        std::fs::write(user_path, "")?;
    }
    dpm_core::load_layered(system_path, user_path, env_prefix)
}

/// `gen-config` subcommand:在使用者層路徑建立一個「空的」`config.toml`
/// (零個 key,不是序列化後的 `ServerConfig::default()`——理由見函式內註解)。
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
    // Empty file, not a serialized ServerConfig::default() — a present-but-
    // default key still wins over the system tier in config-crate's
    // key-presence-based merge, which would make the system tier unreachable.
    std::fs::write(user_path, "")?;
    Ok(())
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
    fn system_tier_field_survives_a_fully_populated_user_tier_write() {
        let dir = tempfile::tempdir().unwrap();
        let system_path = dir.path().join("system.toml");
        let user_path = dir.path().join("user.toml");
        std::fs::write(&system_path, "repo_dir = \"/srv/from-system\"\n").unwrap();

        // Simulates what load_or_init now does on first run: write nothing,
        // not a fully-populated default struct.
        let cfg = load_or_init(&system_path, &user_path, "DPM_SERVER_TEST_SURVIVES").unwrap();

        assert_eq!(
            cfg.repo_dir, "/srv/from-system",
            "system tier must reach through when the user tier writes no keys at all"
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
