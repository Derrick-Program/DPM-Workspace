use crate::{
    entry_is_safe, entry_resolves_inside_install_dir, swap_into_install_dir, ClientError,
    ClientResult, SystemController,
};
use dpm_core::CoreError;
use std::{
    fs::{self, Permissions},
    os::unix::fs::PermissionsExt,
    path::Path,
};

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
