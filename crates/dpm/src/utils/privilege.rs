use crate::ClientResult;
use std::path::Path;

/// 把即將執行 build_command 的子行程,在 fork 之後、exec 之前從 root drop
/// 回原本呼叫 `sudo`(Linux `--system`)的那個使用者,避免「整個 process 已經
/// 是 root,build_command 就跟著是 root」——`system_command_runner` 沒被
/// 呼叫只能防住額外的 `sudo` 前綴,防不了本來就是 root 的父行程。
/// 只有 Linux 需要:macOS 的 `--system` 是逐指令 sudo,呼叫這個函式的
/// process 本身從來不會是 root。
#[cfg(target_os = "linux")]
pub(crate) fn drop_privileges_for_build(cmd: &mut std::process::Command) -> ClientResult<()> {
    use crate::ClientError;
    use std::os::unix::process::CommandExt;

    if unsafe { libc::getuid() } != 0 {
        // 不是 root(PerUser scope,或非 sudo 直接以一般使用者執行),
        // build_command 本來就不會以 root 執行,不需要 drop。
        return Ok(());
    }

    let uid: libc::uid_t = std::env::var("SUDO_UID")
        .ok()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| {
            ClientError::SystemError(
                "refusing to run an untrusted build command as root: process is running as \
                 root but SUDO_UID is not set/parseable, so there is no non-root user to drop \
                 privileges to"
                    .to_string(),
            )
        })?;
    let gid: libc::gid_t = std::env::var("SUDO_GID")
        .ok()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| {
            ClientError::SystemError(
                "refusing to run an untrusted build command as root: process is running as \
                 root but SUDO_GID is not set/parseable, so there is no non-root group to drop \
                 privileges to"
                    .to_string(),
            )
        })?;

    // Safety: the closure only calls async-signal-safe libc functions
    // (setgroups/setgid/setuid) and returns before doing anything else; this
    // matches the documented safety contract of `pre_exec`.
    unsafe {
        cmd.pre_exec(move || {
            // Clear root's supplementary groups (typically includes gid 0)
            // FIRST — otherwise the "de-privileged" child stays a member of
            // any group-0-writable file on the system even after
            // setgid/setuid change the primary/effective/saved ids. Must run
            // before setgid/setuid: dropping the primary uid/gid first can
            // remove the privilege needed to call setgroups at all.
            if libc::setgroups(0, std::ptr::null()) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            // GID before UID: dropping UID first can leave us unprivileged to
            // change GID afterward (standard Unix privilege-drop ordering).
            if libc::setgid(gid) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::setuid(uid) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn drop_privileges_for_build(_cmd: &mut std::process::Command) -> ClientResult<()> {
    // ponytail: macOS `--system` is per-command sudo, never whole-process
    // elevation, so the parent (and thus this build child) is never root
    // here — nothing to drop. Revisit only if macOS ever gains a
    // whole-process elevation mode.
    Ok(())
}

/// 在 Linux `--system` 下,`main.rs` 已經把整個 `dpm` process 提權成 root,
/// 所以呼叫這個函式的當下(parent process)建立出來的 `clone_dir`/`out_dir`
/// 都是 root 所有、mode ~0755。`drop_privileges_for_build` 之後只會把「build
/// 子行程」降回 `SUDO_UID`/`SUDO_GID`,降權後的子行程沒辦法寫進 root 專屬的
/// 目錄——build 只要寫自己的 working tree 或寫 `$OUT` 就會直接 `EACCES`。這裡
/// 把該目錄整棵樹 `chown` 給同一個 `SUDO_UID`/`SUDO_GID`,讓降權後的 build
/// 子行程能真的寫得進去;不需要之後再 chown 回 root——`swap_into_install_dir`
/// 的 `rename` 呼叫方(parent process)本身還是 root,不受來源內容 ownership
/// 影響,任何殘留的非 root ownership 會被既有的 `permision_check()`(下次
/// `dpm` 呼叫時對整個 `MAIN_DIR` 做 `chown -R root:root`)自動收斂。
/// 只有 Linux root 需要:條件跟 `drop_privileges_for_build` 一致——per-user
/// scope 與 macOS 下,建立這些目錄的行程本來就不是 root,不需要 chown。
#[cfg(target_os = "linux")]
pub(crate) fn chown_dir_to_sudo_user(dir: &Path) -> ClientResult<()> {
    use crate::ClientError;

    if unsafe { libc::getuid() } != 0 {
        return Ok(());
    }

    let uid: libc::uid_t = std::env::var("SUDO_UID")
        .ok()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| {
            ClientError::SystemError(
                "couldn't prepare build directory for the target user: process is running as \
                 root but SUDO_UID is not set/parseable"
                    .to_string(),
            )
        })?;
    let gid: libc::gid_t = std::env::var("SUDO_GID")
        .ok()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| {
            ClientError::SystemError(
                "couldn't prepare build directory for the target user: process is running as \
                 root but SUDO_GID is not set/parseable"
                    .to_string(),
            )
        })?;

    let status = std::process::Command::new("chown")
        .arg("-R")
        .arg(format!("{uid}:{gid}"))
        .arg(dir)
        .status()
        .map_err(|e| {
            ClientError::SystemError(format!(
                "couldn't prepare build directory for the target user: failed to run chown: {e}"
            ))
        })?;
    if !status.success() {
        return Err(ClientError::SystemError(format!(
            "couldn't prepare build directory for the target user: chown exited with {status}"
        )));
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn chown_dir_to_sudo_user(_dir: &Path) -> ClientResult<()> {
    // ponytail: macOS `--system` is per-command sudo, never whole-process
    // elevation, so the parent creating these dirs is never root here —
    // there's no ownership mismatch to fix. Mirrors drop_privileges_for_build's
    // own OS gating.
    Ok(())
}
