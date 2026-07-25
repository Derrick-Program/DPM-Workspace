use crate::{ClientError, ClientResult, SystemController};
use dpm_core::CoreError;
use std::{
    fs::{self, Permissions},
    os::unix::fs::PermissionsExt,
    path::{Component, Path},
};

/// 把 staging 目錄裡已經驗證好的內容原子性換裝進最終安裝路徑。
/// 若 install_path 已存在(升級情境),先把舊的搬進 staging_root/previous
/// (同檔案系統 rename,不是複製),新內容才搬進最終路徑——任何一步失敗,
/// install_path 都維持在「舊版本完整存在」或「還沒開始換裝」其中一種完好
/// 狀態,不會出現半殘目錄。呼叫端的 staging TempDir drop 時會把搬出來的
/// 舊版本一併清掉。
fn swap_into_install_dir(
    new_dir: &Path,
    install_path: &Path,
    staging_root: &Path,
) -> ClientResult<()> {
    if install_path.exists() {
        let backup = staging_root.join("previous");
        std::fs::rename(install_path, &backup)
            .map_err(|e| ClientError::Core(CoreError::IoError(e)))?;
        if let Err(e) = std::fs::rename(new_dir, install_path) {
            // Roll back: put the old version back so install_path is never
            // left missing/half-installed if the final rename fails. If the
            // rollback itself fails (double fault), the old install may be
            // gone for good once the caller's staging TempDir is dropped, so
            // this must never fail silently.
            if let Err(rollback_err) = std::fs::rename(&backup, install_path) {
                eprintln!(
                    "CRITICAL: failed to restore backup after failed install \
                     (install error: {e}; rollback error: {rollback_err}); \
                     the previous install at {} may be lost",
                    install_path.display()
                );
            }
            return Err(ClientError::Core(CoreError::IoError(e)));
        }
    } else {
        std::fs::rename(new_dir, install_path)
            .map_err(|e| ClientError::Core(CoreError::IoError(e)))?;
    }
    Ok(())
}

/// 判斷一個從(可能未受信任的)套件索引拿到的 entry/file_name 相對路徑,
/// 是否會在跟 `install_path` join 之後逃逸出 install_path 之外。
/// `PathBuf::join` 遇到絕對路徑會直接整個取代 base(這是文件化行為,不是
/// bug),所以只要 entry 是絕對路徑,或含有 `..` component,就一律視為不安全
/// ——不需要等到檔案系統上真的存在才能判斷,也不需要 canonicalize。
fn entry_is_safe(entry: &str) -> bool {
    let path = Path::new(entry);
    !path.is_absolute() && !path.components().any(|c| matches!(c, Component::ParentDir))
}

/// `entry_is_safe` 只擋得住路徑「字串」裡的絕對路徑/`..`,擋不住
/// `install_path` 底下真的有個 symlink 指到外面——`out_dir`(source 安裝)或
/// 解壓出來的內容(prebuilt 安裝)都是 build_command/發佈者可控的,可以放一個
/// `entry` 指向的 symlink 指到任意路徑(例如 `/etc/shadow`)。呼叫這個函式時
/// `swap_into_install_dir` 一定已經跑完,`install_path` 已經在檔案系統上存在,
/// 所以在把 `main_file` 交給 `fs::set_permissions`(等同 `chmod`,會跟隨
/// symlink)之前,canonicalize 兩邊、確認解完符號連結後 `main_file` 仍然落在
/// `install_path` 底下——這是額外的一層檢查,不是取代 `entry_is_safe`。
fn entry_resolves_inside_install_dir(
    pkg: &str,
    main_file: &Path,
    install_path: &Path,
) -> ClientResult<()> {
    let canonical_install = std::fs::canonicalize(install_path)
        .map_err(|e| ClientError::Core(CoreError::IoError(e)))?;
    let canonical_main_file =
        std::fs::canonicalize(main_file).map_err(|e| ClientError::Core(CoreError::IoError(e)))?;
    if !canonical_main_file.starts_with(&canonical_install) {
        return Err(ClientError::Core(CoreError::InvalidPackage(format!(
            "{pkg}'s entry resolves outside the install directory"
        ))));
    }
    Ok(())
}

/// Installs already-ready content (an extracted zip for a `Prebuilt`
/// package, a build's `$OUT` directory for a `Source` package) into
/// `install_dir/pkg` atomically via `swap_into_install_dir`, then — if
/// `entry` is non-empty — makes the entry executable and symlinks it into
/// `bin_dir`.
///
/// This is the "Placer" half of `install()`'s previously-fused six
/// concerns, and also collapses what used to be two near-identical
/// entry-safety-check + chmod + symlink blocks — one in the `Prebuilt`
/// path (unconditional on `package_info.file_name`), one in
/// `install_source_package` (conditional on `repo_package_info.entry` being
/// non-empty). In practice `dpm-server`'s publish-side validation never
/// lets a `Prebuilt` package's `file_name` be empty, so treating both
/// uniformly as "conditional on non-empty" changes no real-world behavior
/// — it only makes the never-actually-hit empty case skip cleanly instead
/// of chmod'ing/symlinking the whole install directory as if it were the
/// entry.
pub fn place_package(
    pkg: &str,
    content_dir: &Path,
    entry: &str,
    install_dir: &Path,
    bin_dir: &Path,
    staging_root: &Path,
    system_controller: &SystemController,
) -> ClientResult<()> {
    let install_path = install_dir.join(pkg);
    swap_into_install_dir(content_dir, &install_path, staging_root)?;

    if entry.is_empty() {
        return Ok(());
    }
    if !entry_is_safe(entry) {
        return Err(ClientError::Core(CoreError::InvalidPackage(format!(
            "{pkg} has an unsafe entry path: {entry}"
        ))));
    }
    let main_file = install_path.join(entry);
    entry_resolves_inside_install_dir(pkg, &main_file, &install_path)?;
    fs::set_permissions(&main_file, Permissions::from_mode(0o755))
        .map_err(|e| ClientError::SystemError(e.to_string()))?;
    let ln_path = bin_dir.join(pkg);
    system_controller.system_command_runner(
        "ln",
        vec![
            "-s",
            main_file.display().to_string().as_str(),
            ln_path.display().to_string().as_str(),
        ],
        "Can't create link",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Scope;
    use tempfile::tempdir;

    #[test]
    fn places_content_and_links_a_non_empty_entry() {
        let root = tempdir().unwrap();
        let content_dir = root.path().join("content");
        fs::create_dir_all(&content_dir).unwrap();
        fs::write(content_dir.join("main"), b"#!/bin/sh\necho hi\n").unwrap();
        let install_dir = root.path().join("install");
        let bin_dir = root.path().join("bin");
        fs::create_dir_all(&install_dir).unwrap();
        fs::create_dir_all(&bin_dir).unwrap();

        let controller = SystemController::new(Scope::PerUser);
        place_package(
            "pkg",
            &content_dir,
            "main",
            &install_dir,
            &bin_dir,
            root.path(),
            &controller,
        )
        .unwrap();

        let installed_main = install_dir.join("pkg").join("main");
        assert!(installed_main.exists());
        let perms = fs::metadata(&installed_main).unwrap().permissions();
        assert_eq!(perms.mode() & 0o777, 0o755);

        let link = bin_dir.join("pkg");
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(fs::read_link(&link).unwrap(), installed_main);
    }

    #[test]
    fn empty_entry_places_content_but_skips_linking() {
        let root = tempdir().unwrap();
        let content_dir = root.path().join("content");
        fs::create_dir_all(&content_dir).unwrap();
        fs::write(content_dir.join("data"), b"just data").unwrap();
        let install_dir = root.path().join("install");
        let bin_dir = root.path().join("bin");
        fs::create_dir_all(&install_dir).unwrap();
        fs::create_dir_all(&bin_dir).unwrap();

        let controller = SystemController::new(Scope::PerUser);
        place_package(
            "pkg",
            &content_dir,
            "",
            &install_dir,
            &bin_dir,
            root.path(),
            &controller,
        )
        .unwrap();

        assert!(install_dir.join("pkg").join("data").exists());
        assert!(
            !bin_dir.join("pkg").exists(),
            "no entry means no symlink should be created"
        );
    }

    #[test]
    fn rejects_unsafe_entry_path() {
        let root = tempdir().unwrap();
        let content_dir = root.path().join("content");
        fs::create_dir_all(&content_dir).unwrap();
        let install_dir = root.path().join("install");
        let bin_dir = root.path().join("bin");
        fs::create_dir_all(&install_dir).unwrap();

        let controller = SystemController::new(Scope::PerUser);
        let result = place_package(
            "pkg",
            &content_dir,
            "../../etc/passwd",
            &install_dir,
            &bin_dir,
            root.path(),
            &controller,
        );
        assert!(result.is_err());
    }
}

#[cfg(test)]
mod atomic_install_tests {
    use super::swap_into_install_dir;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn fresh_install_moves_new_dir_into_place() {
        let root = tempdir().unwrap();
        let new_dir = root.path().join("new");
        let install_path = root.path().join("install");
        fs::create_dir_all(&new_dir).unwrap();
        fs::write(new_dir.join("marker.txt"), b"v2").unwrap();

        swap_into_install_dir(&new_dir, &install_path, root.path()).unwrap();

        assert!(install_path.join("marker.txt").exists());
        assert_eq!(
            fs::read_to_string(install_path.join("marker.txt")).unwrap(),
            "v2"
        );
        assert!(
            !new_dir.exists(),
            "new_dir should have been moved, not copied"
        );
    }

    #[test]
    fn upgrade_replaces_old_content_with_new() {
        let root = tempdir().unwrap();
        let new_dir = root.path().join("new");
        let install_path = root.path().join("install");
        fs::create_dir_all(&new_dir).unwrap();
        fs::write(new_dir.join("marker.txt"), b"v2").unwrap();
        fs::create_dir_all(&install_path).unwrap();
        fs::write(install_path.join("marker.txt"), b"v1").unwrap();

        swap_into_install_dir(&new_dir, &install_path, root.path()).unwrap();

        assert_eq!(
            fs::read_to_string(install_path.join("marker.txt")).unwrap(),
            "v2",
            "install_path must contain the new version's content after swap"
        );
    }

    #[test]
    fn old_install_survives_if_new_dir_is_missing() {
        let root = tempdir().unwrap();
        let missing_new_dir = root.path().join("does-not-exist");
        let install_path = root.path().join("install");
        fs::create_dir_all(&install_path).unwrap();
        fs::write(install_path.join("marker.txt"), b"v1").unwrap();

        let result = swap_into_install_dir(&missing_new_dir, &install_path, root.path());

        assert!(result.is_err(), "swap must fail if new_dir doesn't exist");
        assert_eq!(
            fs::read_to_string(install_path.join("marker.txt")).unwrap(),
            "v1",
            "old install must be untouched when the swap fails before completion"
        );
    }

    /// Forces the double-fault path: the backup rename succeeds, the
    /// new-content rename fails (missing `new_dir`), and then the rollback
    /// rename is *also* made to fail by stripping write permission from
    /// `root` the instant the backup lands. A background thread races the
    /// call to `swap_into_install_dir` to flip that permission bit between
    /// the first and second renames — both operations share `root` as their
    /// parent directory, so there is no way to set this up with a static
    /// permission change alone.
    ///
    /// Timing note: the watcher/main-thread synchronization here is real OS
    /// thread scheduling (spin-poll on `backup.exists()`), not a guaranteed
    /// ordering — there is no deterministic seam in `swap_into_install_dir`
    /// to hook into, and adding one just for this test would be more
    /// production-code surface than this test-only race is worth. On a
    /// busy/throttled/virtualized CI runner the watcher can lose the race
    /// (chmod lands after both renames already completed). If that happens,
    /// this test does **not** pass vacuously — the `!install_path.exists()`
    /// / `backup.exists()` assertions below would fail loudly, since the
    /// rollback would have succeeded normally. So a lost race shows up as an
    /// intermittent, non-actionable CI failure rather than a silent false
    /// pass. If this test is ever observed flaking in CI, quarantine it with
    /// `#[ignore]` rather than trying to chase the scheduling further.
    #[test]
    #[cfg(unix)]
    fn rollback_failure_still_returns_err_without_panicking() {
        use std::os::unix::fs::PermissionsExt;

        // chmod 0o555 below only blocks the rollback rename for a
        // non-root user; root can write through it regardless, which would
        // make the rollback always succeed and this test deterministically
        // fail for a reason unrelated to the code under test. Skip
        // gracefully when running as root (e.g. some containerized CI).
        if unsafe { libc::geteuid() } == 0 {
            eprintln!(
                "skipping rollback_failure_still_returns_err_without_panicking: \
                 running as root, chmod 0o555 does not block root's renames"
            );
            return;
        }

        let root = tempdir().unwrap();
        let missing_new_dir = root.path().join("does-not-exist");
        let install_path = root.path().join("install");
        fs::create_dir_all(&install_path).unwrap();
        fs::write(install_path.join("marker.txt"), b"v1").unwrap();

        let backup = root.path().join("previous");
        let root_path = root.path().to_path_buf();

        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let watcher_root = root_path.clone();
        let watcher_backup = backup.clone();
        let watcher = std::thread::spawn(move || {
            ready_tx.send(()).unwrap();
            while !watcher_backup.exists() {
                std::hint::spin_loop();
            }
            // The backup just landed: make root read-only so neither the
            // new-content rename nor the rollback rename can (re)create the
            // "install" entry inside it.
            let mut perms = fs::metadata(&watcher_root).unwrap().permissions();
            perms.set_mode(0o555);
            fs::set_permissions(&watcher_root, perms).unwrap();
        });
        // Make sure the watcher is already spinning before we start the race.
        ready_rx.recv().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));

        let result = swap_into_install_dir(&missing_new_dir, &install_path, root.path());
        watcher.join().unwrap();

        // Restore permissions so the TempDir can clean itself up on drop.
        let mut perms = fs::metadata(&root_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&root_path, perms).unwrap();

        assert!(
            result.is_err(),
            "swap must still return Err (not panic, not silently succeed) \
             when the rollback rename also fails"
        );
        assert!(
            !install_path.exists(),
            "when the rollback also fails, install_path must not silently end up \
             restored or half-installed"
        );
        assert!(
            backup.exists(),
            "the backed-up old install must still be sitting in staging_root/previous \
             (unrestored) since the rollback rename failed"
        );
    }
}

#[cfg(test)]
mod entry_is_safe_tests {
    use super::entry_is_safe;

    #[test]
    fn rejects_absolute_path() {
        assert!(!entry_is_safe("/etc/cron.d/x"));
    }

    #[test]
    fn rejects_parent_dir_traversal() {
        assert!(!entry_is_safe("../../../etc/passwd"));
        assert!(!entry_is_safe("bin/../../escape"));
    }

    #[test]
    fn accepts_normal_relative_filename() {
        assert!(entry_is_safe("bin/main"));
        assert!(entry_is_safe("main"));
        assert!(entry_is_safe("./main"));
    }

    #[test]
    fn rejects_empty_string_is_still_safe_but_caller_checks_empty_separately() {
        // entry_is_safe itself doesn't special-case "": the `.is_empty()`
        // check happens at the call sites before this is even invoked.
        // Document that here so nobody "fixes" this later.
        assert!(entry_is_safe(""));
    }
}

#[cfg(test)]
mod entry_resolves_inside_install_dir_tests {
    use super::entry_resolves_inside_install_dir;
    use std::os::unix::fs::symlink;
    use tempfile::tempdir;

    #[test]
    fn accepts_normal_file_inside_install_dir() {
        let install_dir = tempdir().unwrap();
        let main_file = install_dir.path().join("main");
        std::fs::write(&main_file, b"bin").unwrap();

        assert!(entry_resolves_inside_install_dir("pkg", &main_file, install_dir.path()).is_ok());
    }

    #[test]
    fn rejects_symlink_escaping_install_dir() {
        let install_dir = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let secret = outside.path().join("secret");
        std::fs::write(&secret, b"root-owned").unwrap();

        // `out_dir`/extracted content is build_command/publisher-controlled;
        // this simulates `entry = "evil"` resolving to a symlink planted
        // there that points outside install_path.
        let evil = install_dir.path().join("evil");
        symlink(&secret, &evil).unwrap();

        let result = entry_resolves_inside_install_dir("pkg", &evil, install_dir.path());
        assert!(
            result.is_err(),
            "symlink escaping install_dir must be rejected"
        );
    }
}
