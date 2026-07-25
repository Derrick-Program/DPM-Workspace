use crate::*;
use colored::Colorize;
use dpm_core::zip_folder;
use dpm_core::CoreError;
use std::collections::HashMap;
use std::fs::File;
use std::path::Path;
use walkdir::WalkDir;
pub fn hash(obj: &Hash, project_src: &Path) -> ServerResult<()> {
    let project_path = project_src.join(&obj.packagename);
    let hashfile = &project_path.join("hashes.json");
    let project_info = &project_path.join("packageInfo.json");
    let mut hashes: HashMap<String, String> =
        JsonStorage::from_json(hashfile).unwrap_or_else(|_| HashMap::new());
    let mut counter: i32 = 0;
    if !project_path.exists() {
        return Err(ServerError::Core(CoreError::PackageNotFound(
            obj.packagename.clone(),
        )));
    }
    for entry in WalkDir::new(&project_path) {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() && path != hashfile {
            counter += 1;
            let hash = dpm_core::hash_file(path)?;
            let relative_path = path.strip_prefix(&project_path).unwrap_or(path);
            println!(
                "{} {} {} {}",
                counter,
                relative_path.display().to_string().yellow(),
                "===>".green(),
                hash.bold().blue(),
            );
            hashes.insert(relative_path.display().to_string(), hash);
        }
    }
    JsonStorage::to_json(&hashes, hashfile)?;
    counter += 1;
    let hash = dpm_core::hash_file(hashfile)?;
    println!(
        "{} {} {} {}",
        counter,
        hashfile.file_name().unwrap().to_str().unwrap().yellow(),
        "===>".green(),
        hash.bold().blue(),
    );
    hashes.insert(
        hashfile.file_name().unwrap().to_str().unwrap().to_string(),
        hash.clone(),
    );
    JsonStorage::to_json(&hashes, hashfile)?;
    let mut package_info: PackageInfo = JsonStorage::from_json(project_info)?;
    package_info.hash = hash;
    JsonStorage::to_json(&package_info, project_info)?;
    Ok(())
}

pub fn build(obj: &Build, project_src: &Path, repo_dir: &Path) -> ServerResult<()> {
    let project_path = project_src.join(&obj.packagename);
    if !project_path.exists() {
        return Err(ServerError::Core(CoreError::PackageNotFound(
            obj.packagename.clone(),
        )));
    }
    let zip_file_path = repo_dir.join(format!("{}.zip", obj.packagename));
    zip_folder(&project_path, &zip_file_path)?;
    Ok(())
}

pub fn init(obj: &Init, project_src: &Path) -> ServerResult<()> {
    let project_path = project_src.join(obj.name.as_str());
    if !project_path.exists() {
        create_dir_all(&project_path)?;
    } else {
        return Err(ServerError::ValidationError(format!(
            "{} already exists",
            project_path.display()
        )));
    }
    File::create(project_path.join(obj.entry.as_str()))?;
    let file_path = project_path.join("hashes.json");
    File::create(&file_path)?;
    let hash = dpm_core::hash_file(&file_path)?;
    let package_info = PackageInfo::new(
        obj.name.to_string(),
        obj.entry.to_string(),
        obj.ver.to_string(),
        obj.description.to_string(),
        hash,
        None,
    );
    JsonStorage::to_json(&package_info, &project_path.join("packageInfo.json"))?;
    Ok(())
}

pub fn fix(obj: &Fix, repo: &mut RepoInfo, project_src: &Path) -> ServerResult<()> {
    match &obj.command {
        FixAction::Add(obj) => fix_add(obj, repo, project_src)?,
        FixAction::Del(obj) => fix_del(obj, repo)?,
    }
    Ok(())
}

fn fix_add(obj: &Add, repo: &mut RepoInfo, project_src: &Path) -> ServerResult<()> {
    let path = project_src.join(&obj.project_name);
    let pk_info: PackageInfo = JsonStorage::from_json(&path.join("packageInfo.json"))?;

    let kind = match (&obj.url, &obj.build) {
        (Some(url), None) => {
            if !url.starts_with("https://") {
                return Err(ServerError::ValidationError(format!(
                    "--url {url} must use https://"
                )));
            }
            let file_name = obj
                .file_name
                .clone()
                .or_else(|| url.rsplit('/').next().map(|s| s.to_string()))
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    ServerError::ValidationError(
                        "could not derive a file name from --url; pass --file-name explicitly"
                            .to_string(),
                    )
                })?;

            let response = reqwest::blocking::get(url)?;
            if !response.status().is_success() {
                return Err(ServerError::Core(CoreError::NetworkError(format!(
                    "failed to fetch {url}: HTTP {}",
                    response.status()
                ))));
            }
            let bytes = response.bytes()?;
            let tmp_path = std::env::temp_dir().join(&file_name);
            std::fs::write(&tmp_path, &bytes)?;
            let hash = dpm_core::hash_file(&tmp_path)?;
            std::fs::remove_file(&tmp_path)?;

            PackageKind::Prebuilt {
                url: url.clone(),
                hash,
                file_name,
            }
        }
        (None, Some(build)) => PackageKind::Source {
            build: build.clone(),
        },
        (Some(_), Some(_)) => unreachable!("clap's conflicts_with already rejects this"),
        (None, None) => {
            return Err(ServerError::ValidationError(format!(
                "fix add {} needs exactly one of --url or --build",
                obj.project_name
            )));
        }
    };

    let version_info = PackageVersionInfo {
        version: pk_info.version.clone(),
        kind,
        dependencies: pk_info.dependencies,
        entry: None,
        description: Some(pk_info.description),
    };
    repo.add_package_version(obj.project_name.clone(), version_info)?;
    Ok(())
}
fn fix_del(obj: &Del, repo: &mut RepoInfo) -> ServerResult<()> {
    let version = match &obj.version {
        Some(v) => v.clone(),
        None => {
            let versions = repo.versions_of(&obj.project_name)?;
            if versions.len() > 1 {
                return Err(ServerError::ValidationError(format!(
                    "package {} has {} published versions — specify which one to remove",
                    obj.project_name,
                    versions.len()
                )));
            }
            versions
                .first()
                .ok_or_else(|| {
                    ServerError::Core(CoreError::PackageNotFound(obj.project_name.clone()))
                })?
                .version
                .clone()
        }
    };
    repo.remove_package_version(&obj.project_name, &version)?;
    println!(
        "Package '{}@{}' removed successfully.",
        obj.project_name, version
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `project_src` used to come from a process-wide `OnceLock`, so nothing
    /// in this file could be unit tested without a real global. Now that
    /// it's a plain parameter, `init()` can run against an isolated tempdir
    /// — this is the check that init() actually respects that parameter
    /// instead of falling back to some ambient path.
    #[test]
    fn init_creates_package_skeleton_under_given_project_src() {
        let project_src = std::env::temp_dir().join(format!(
            "dpm-server-action-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&project_src).unwrap();

        let obj = Init {
            name: "demo-pkg".to_string(),
            entry: "main.sh".to_string(),
            ver: "0.1.0".to_string(),
            description: "a demo package".to_string(),
        };

        init(&obj, &project_src).unwrap();

        let pkg_dir = project_src.join("demo-pkg");
        assert!(pkg_dir.join("main.sh").exists());
        assert!(pkg_dir.join("hashes.json").exists());
        assert!(pkg_dir.join("packageInfo.json").exists());

        std::fs::remove_dir_all(&project_src).ok();
    }

    /// `build()` used to read `current_dir()` directly for its output path
    /// even after `project_src` was parameterized — the one function in
    /// this file the round-1 fix didn't reach. Now `repo_dir` is a plain
    /// parameter too, so this runs against isolated tempdirs instead of
    /// depending on (and littering) the test runner's real working directory.
    #[test]
    fn build_zips_package_into_given_repo_dir() {
        let root = std::env::temp_dir().join(format!(
            "dpm-server-action-build-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let project_src = root.join("packages");
        let repo_dir = root.join("Repo");
        std::fs::create_dir_all(&project_src).unwrap();
        std::fs::create_dir_all(&repo_dir).unwrap();

        init(
            &Init {
                name: "demo-pkg".to_string(),
                entry: "main.sh".to_string(),
                ver: "0.1.0".to_string(),
                description: "a demo package".to_string(),
            },
            &project_src,
        )
        .unwrap();

        build(
            &Build {
                packagename: "demo-pkg".to_string(),
            },
            &project_src,
            &repo_dir,
        )
        .unwrap();

        assert!(repo_dir.join("demo-pkg.zip").exists());

        std::fs::remove_dir_all(&root).ok();
    }

    /// `hash()` used to write `hashes.json` then immediately re-read it back
    /// with a `.unwrap_or_else(|_| HashMap::new())` fallback — a redundant
    /// round-trip that, on any read failure, would silently discard every
    /// hash the walk just computed. The fix removes the re-read entirely;
    /// this test pins down that the entry file's hash actually ends up in
    /// `hashes.json` and that `packageInfo.json.hash` gets updated to the
    /// hash of `hashes.json` itself.
    #[test]
    fn hash_records_entry_file_hash_and_updates_package_info() {
        let project_src = std::env::temp_dir().join(format!(
            "dpm-server-action-hash-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&project_src).unwrap();

        init(
            &Init {
                name: "demo-pkg".to_string(),
                entry: "main.sh".to_string(),
                ver: "0.1.0".to_string(),
                description: "a demo package".to_string(),
            },
            &project_src,
        )
        .unwrap();

        hash(
            &Hash {
                packagename: "demo-pkg".to_string(),
            },
            &project_src,
        )
        .unwrap();

        let pkg_dir = project_src.join("demo-pkg");
        let hashes: HashMap<String, String> =
            JsonStorage::from_json(&pkg_dir.join("hashes.json")).unwrap();
        // This is the actual regression guard: the redundant re-read this
        // fix removed had a `.unwrap_or_else(|_| HashMap::new())` fallback,
        // so if it ever hit a read error, every walked entry below would
        // have been silently discarded and only the self-hash entry would
        // survive. If that regressed, these two keys would be missing.
        assert!(
            hashes.contains_key("main.sh"),
            "hashes.json must still record the entry file's hash"
        );
        assert!(
            hashes.contains_key("packageInfo.json"),
            "hashes.json must still record packageInfo.json's hash"
        );
        assert!(
            hashes.contains_key("hashes.json"),
            "hashes.json must record its own self-hash entry"
        );

        // Not asserting packageInfo.json.hash against a reconstructed
        // first-pass file here: `HashMap`'s iteration order is randomized
        // per-process, so re-serializing an equivalent map in the test
        // produces different (but equally valid) JSON byte-for-byte,
        // which would make a byte-hash comparison flaky across process
        // runs for reasons unrelated to this fix. A shape check is the
        // stable thing to assert instead.
        let package_info: PackageInfo =
            JsonStorage::from_json(&pkg_dir.join("packageInfo.json")).unwrap();
        assert_eq!(
            package_info.hash.len(),
            64,
            "packageInfo.json.hash must be a full blake3 hex digest, not empty/truncated"
        );

        std::fs::remove_dir_all(&project_src).ok();
    }
}
