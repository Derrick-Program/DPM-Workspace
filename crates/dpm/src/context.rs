use crate::{ClientError, ClientResult, Db, Scope, SystemController};
use directories::ProjectDirs;
use dpm_core::CoreError::DatabaseError;
use std::path::PathBuf;
use std::sync::Arc;

/// Everything `ActionInfo`/`SystemController` need to know about *where*
/// this run of `dpm` lives: which scope it's operating in, the four
/// directories that scope resolves to, and a handle to the local package
/// database opened under `main_dir`.
///
/// This replaces what used to be six process-wide `OnceLock` statics
/// (`MAIN_DIR`/`BIN_DIR`/`INSTALL_DIR`/`CONFIG`/`SCOPE`/`DB_INSTANCE` in
/// `lib.rs`) set once by `set_globle_var` and read from anywhere via
/// `.get().unwrap()`. A `OnceLock` can only be set once per process and
/// always resolves to the real per-user/`--system` paths — there was no way
/// for a test to point `install()`/`init()` at a throwaway directory instead
/// of the real machine's config/data directories (`config_tests.rs`
/// documented this gap directly). `Context` is passed explicitly instead:
/// `Context::for_scope` is the production adapter (real `ProjectDirs`/
/// `/opt/com.duacodie/DPM` paths), `Context::for_test` is the test adapter
/// (everything under a caller-supplied tempdir) — two adapters at the same
/// seam.
#[derive(Debug, Clone)]
pub struct Context {
    pub scope: Scope,
    pub main_dir: PathBuf,
    pub bin_dir: PathBuf,
    pub install_dir: PathBuf,
    pub config_dir: PathBuf,
    pub db: Arc<Db>,
    pub info_db: Arc<Db>,
}

struct Paths {
    main_dir: PathBuf,
    bin_dir: PathBuf,
    install_dir: PathBuf,
    config_dir: PathBuf,
}

fn compute_paths(scope: Scope) -> ClientResult<Paths> {
    let proj_dirs = ProjectDirs::from("com", "duacodie", "dpm")
        .ok_or_else(|| ClientError::SystemError("no valid home directory found".to_string()))?;
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

async fn open_db(main_dir: &std::path::Path, is_info: bool) -> ClientResult<Db> {
    let name = if is_info { "LocalRepoInfo" } else { "LocalRepo" };
    let db_path = main_dir.join(format!("{name}.db"));
    let lock_path = main_dir.join(format!("{name}.lock"));
    let db = Db::new(db_path.to_str().unwrap(), lock_path.to_str().unwrap())
        .await
        .map_err(|e| ClientError::Core(DatabaseError(e.to_string())))?;
    db.run_migrations(is_info).await?;
    Ok(db)
}

impl Context {
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
            // Unix-only crate (libc/sudo deps, `system_command_runner` rejects
            // anything that isn't Linux or macOS), so this branch means "Linux"
            // in practice, not literally "every non-macOS OS".
            PathBuf::from("/etc/dpm/config.toml")
        }
    }

    pub fn sbin_dir(&self) -> PathBuf { self.main_dir.join("sbin") }
    pub fn lib_dir(&self) -> PathBuf { self.main_dir.join("lib") }
    pub fn include_dir(&self) -> PathBuf { self.main_dir.join("include") }
    pub fn share_dir(&self) -> PathBuf { self.main_dir.join("share") }
    pub fn completions_dir(&self) -> PathBuf { self.main_dir.join("completions") }
    pub fn docs_dir(&self) -> PathBuf { self.main_dir.join("docs") }
    pub fn opt_dir(&self) -> PathBuf { self.main_dir.join("opt") }
    pub fn etc_dir(&self) -> PathBuf { self.main_dir.join("etc") }
    pub fn var_dir(&self) -> PathBuf { self.main_dir.join("var") }

    /// Production constructor. Resolves `scope`'s real paths, creates
    /// `main_dir` (fixing ownership under `--system`) if it doesn't exist
    /// yet, and opens the real local package database under it.
    pub async fn for_scope(scope: Scope) -> ClientResult<Self> {
        let paths = compute_paths(scope)?;
        if !paths.main_dir.exists() {
            let controller = SystemController::new(scope);
            controller.system_command_runner(
                "mkdir",
                vec!["-p", paths.main_dir.to_str().unwrap()],
                "Can't create DPM main dir",
            )?;
            controller.permision_check(&paths.main_dir)?;
        }
        let db = open_db(&paths.main_dir, false).await?;
        let info_db = open_db(&paths.main_dir, true).await?;
        Ok(Context {
            scope,
            main_dir: paths.main_dir,
            bin_dir: paths.bin_dir,
            install_dir: paths.install_dir,
            config_dir: paths.config_dir,
            db: Arc::new(db),
            info_db: Arc::new(info_db),
        })
    }

    pub async fn open_info_db(&self) -> ClientResult<Db> {
        open_db(&self.main_dir, true).await
    }

    /// Test constructor: every path lives under `root` (a caller-owned
    /// tempdir) instead of the real per-user/`--system` locations, and scope
    /// is always `PerUser` (no root/sudo, no `--system` chown behavior to
    /// simulate — tests exercising `SystemController`'s scope-specific
    /// branches do so directly against `Scope`, not through this).
    ///
    /// Deliberately not `#[cfg(test)]`: integration tests under
    /// `crates/dpm/tests/` link against the library built *without*
    /// `--cfg test` (only the crate's own unit tests get that), so a
    /// `#[cfg(test)]` item here would be invisible to them — same reason
    /// `Db::new` (also test-consumed) is a plain `pub fn`.
    pub async fn for_test(root: &std::path::Path) -> ClientResult<Self> {
        let main_dir = root.to_path_buf();
        let bin_dir = main_dir.join("bin");
        let install_dir = main_dir.join("Software");
        let config_dir = main_dir.join("Settings");
        let subdirs = [
            &main_dir,
            &bin_dir,
            &install_dir,
            &config_dir,
            &main_dir.join("sbin"),
            &main_dir.join("lib"),
            &main_dir.join("include"),
            &main_dir.join("share"),
            &main_dir.join("completions"),
            &main_dir.join("docs"),
            &main_dir.join("opt"),
            &main_dir.join("etc"),
            &main_dir.join("var"),
        ];
        for dir in subdirs {
            std::fs::create_dir_all(dir)
                .map_err(|e| ClientError::Core(dpm_core::CoreError::IoError(e)))?;
        }
        let db = open_db(&main_dir, false).await?;
        let info_db = open_db(&main_dir, true).await?;
        Ok(Context {
            scope: Scope::PerUser,
            main_dir,
            bin_dir,
            install_dir,
            config_dir,
            db: Arc::new(db),
            info_db: Arc::new(info_db),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[tokio::test]
    async fn for_test_creates_isolated_directory_tree_and_working_db() {
        let root = tempfile::tempdir().unwrap();
        let ctx = Context::for_test(root.path()).await.unwrap();

        assert_eq!(ctx.main_dir, root.path());
        assert!(ctx.bin_dir.starts_with(root.path()));
        assert!(ctx.install_dir.starts_with(root.path()));
        assert!(ctx.config_dir.starts_with(root.path()));
        assert!(ctx.bin_dir.exists());
        assert!(ctx.install_dir.exists());
        assert!(ctx.config_dir.exists());

        // The DB is real and usable, not a stub — proves the seam gives a
        // fully working Context, not just plausible-looking paths.
        let all = ctx.db.read_all().await.unwrap();
        assert!(all.is_empty());
    }

    #[tokio::test]
    async fn two_for_test_contexts_are_fully_isolated_from_each_other() {
        let root_a = tempfile::tempdir().unwrap();
        let root_b = tempfile::tempdir().unwrap();
        let ctx_a = Context::for_test(root_a.path()).await.unwrap();
        let ctx_b = Context::for_test(root_b.path()).await.unwrap();

        ctx_a
            .db
            .insert(crate::DbPackage::new(
                "official", "foo", "1.0.0", "prebuilt", None, None, None, None, "", None, None,
                None, None,
            ))
            .await
            .unwrap();

        assert_eq!(ctx_a.db.read_all().await.unwrap().len(), 1);
        assert_eq!(
            ctx_b.db.read_all().await.unwrap().len(),
            0,
            "a second Context::for_test must not see the first one's data"
        );
    }
}
