use crate::*;
use anyhow::Result as AnyhowResult;
use colored::Colorize;
use std::collections::HashMap;
use std::env::current_dir;
use std::fs::{read_dir, File};
use std::path::PathBuf;
use walkdir::WalkDir;
pub fn hash(obj: &Hash) -> AnyhowResult<()> {
    let project_path = PROJECT_SRC.get().unwrap().join(&obj.packagename);
    let hashfile = &project_path.join("hashes.json");
    let project_info = &project_path.join("packageInfo.json");
    let mut hashes: HashMap<String, String> =
        JsonStorage::from_json(hashfile).unwrap_or_else(|_| HashMap::new());
    let mut counter: i32 = 0;
    if !project_path.exists() {
        return Err(anyhow::anyhow!(
            "\nPackage: {} {}",
            obj.packagename.yellow(),
            "Not found!".red()
        ));
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
    let mut hashes: HashMap<String, String> =
        JsonStorage::from_json(hashfile).unwrap_or_else(|_| HashMap::new());
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

pub fn build(obj: &Build) -> Result<()> {
    let project_path = PROJECT_SRC.get().unwrap().join(&obj.packagename);
    if !project_path.exists() {
        return Err(anyhow::anyhow!(
            "\nPackage: {} {}",
            obj.packagename.yellow(),
            "Not found!".red()
        ));
    }
    let zip_file_path = current_dir()?
        .join("Repo")
        .join(format!("{}.zip", obj.packagename));
    zip_folder(&project_path, &zip_file_path)?;
    Ok(())
}

pub fn init(obj: &Init) -> Result<()> {
    let project_path = PROJECT_SRC.get().unwrap().join(obj.name.as_str());
    if !project_path.exists() {
        create_dir_all(&project_path)?;
    } else {
        return Err(anyhow::anyhow!(
            "\n{} {}",
            format!("{}", project_path.display()).yellow(),
            "exists!".red()
        ));
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

pub fn fix(obj: &Fix, repo: &mut RepoInfo) -> Result<()> {
    match &obj.command {
        FixAction::Add(obj) => fix_add(obj, repo)?,
        FixAction::Del(obj) => fix_del(obj, repo)?,
    }
    Ok(())
}

fn fix_add(obj: &Add, repo: &mut RepoInfo) -> Result<()> {
    let path = PROJECT_SRC.get().unwrap().join(&obj.project_name);
    let package = current_dir()?
        .join("Repo")
        .join(format!("{}.zip", obj.project_name));
    if !package.exists() {
        return Err(anyhow::anyhow!(
            "\nPackage: {} {}",
            format!("{}", package.display()).yellow(),
            "Not found!".red()
        ));
    }
    let pk_info: PackageInfo = JsonStorage::from_json(&path.join("packageInfo.json"))?;

    let version_info = PackageVersionInfo {
        version: pk_info.version.clone(),
        kind: PackageKind::Prebuilt {
            url: format!(
                "https://github.com/Derrick-Program/DPM-Server/raw/main/Repo/{}.zip",
                obj.project_name
            ),
            hash: dpm_core::hash_file(&package)?,
            file_name: format!("{}.zip", pk_info.package_name),
        },
        dependencies: pk_info.dependencies,
        entry: None,
        description: Some(pk_info.description),
    };
    repo.add_package_version(obj.project_name.clone(), version_info)?;
    Ok(())
}
fn fix_del(obj: &Del, repo: &mut RepoInfo) -> Result<()> {
    let version = match &obj.version {
        Some(v) => v.clone(),
        None => {
            let versions = repo.versions_of(&obj.project_name)?;
            if versions.len() > 1 {
                return Err(anyhow::anyhow!(
                    "\nPackage {} has {} published versions — specify which one to remove",
                    obj.project_name.yellow(),
                    versions.len()
                ));
            }
            versions
                .first()
                .ok_or_else(|| {
                    anyhow::anyhow!("\nPackage {} not found", obj.project_name.yellow())
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

pub fn repo_init(repo: &mut RepoInfo) -> Result<()> {
    println!("Initializing Repo...");
    let ret = find_zip_files_and_names_in_repo()?;
    for (_, name) in ret {
        let name_witout_zip = name.trim_end_matches(".zip");
        let project = PROJECT_SRC.get().unwrap().join(name_witout_zip);
        if !project.exists() {
            return Err(anyhow::anyhow!(
                "\nPackage: {} {}",
                name_witout_zip.yellow(),
                "Not found!".red()
            ));
        }
        let pk_info: PackageInfo = JsonStorage::from_json(&project.join("packageInfo.json"))?;
        let version_info = PackageVersionInfo {
            version: pk_info.version.clone(),
            kind: PackageKind::Prebuilt {
                url: format!(
                    "https://github.com/Derrick-Program/DPM-Server/raw/main/Repo/{}.zip",
                    pk_info.package_name
                ),
                hash: pk_info.hash.clone(),
                file_name: name.clone(),
            },
            dependencies: pk_info.dependencies,
            entry: None,
            description: Some(pk_info.description),
        };
        repo.add_package_version(name_witout_zip.to_string(), version_info)?;
        println!("Done...");
    }

    Ok(())
}

fn find_zip_files_and_names_in_repo() -> Result<Vec<(PathBuf, String)>> {
    let repo_dir = std::env::current_dir()?.join("Repo");
    let mut zip_files = Vec::new();
    if repo_dir.is_dir() {
        for entry in read_dir(repo_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(std::ffi::OsStr::to_str) == Some("zip") {
                if let Some(file_name) = path.file_name().and_then(std::ffi::OsStr::to_str) {
                    zip_files.push((path.clone(), file_name.to_string()));
                }
            }
        }
    }

    Ok(zip_files)
}

// fn resolve_dependencies_with_install(
//     package_name: &str,
//     repo: &mut RepoInfo, // RepoInfo 的所有數據
//     resolved: &mut HashSet<String>,
//     seen: &mut HashSet<String>,
// ) -> Result<()> {
//     if resolved.contains(package_name) {
//         return Ok(());
//     }
//     if seen.contains(package_name) {
//         return Err(anyhow::anyhow!(
//             "Circular dependency detected: {}",
//             package_name
//         )); // 檢測到循環依賴
//     }

//     seen.insert(package_name.to_string());
//     let package_info = repo.get_package(package_name)?;
//     if let Some(dependencies) = &package_info.dependencies {
//         for dependency in dependencies {
//             resolve_dependencies_with_install(dependency, repo, resolved, seen)?;
//         }
//     }
//     install_package(package_name, repo_info)?;

//     resolved.insert(package_name.to_string());
//     println!("Resolved and installed: {}", package_name);

//     Ok(())
// }
