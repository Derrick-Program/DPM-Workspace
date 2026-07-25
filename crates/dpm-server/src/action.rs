use crate::*;
use colored::Colorize;
use dpm_core::CoreError;
use std::collections::HashMap;
use std::env::current_dir;
use std::fs::File;
use walkdir::WalkDir;
pub fn hash(obj: &Hash) -> ServerResult<()> {
    let project_path = PROJECT_SRC.get().unwrap().join(&obj.packagename);
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

pub fn build(obj: &Build) -> ServerResult<()> {
    let project_path = PROJECT_SRC.get().unwrap().join(&obj.packagename);
    if !project_path.exists() {
        return Err(ServerError::Core(CoreError::PackageNotFound(
            obj.packagename.clone(),
        )));
    }
    let zip_file_path = current_dir()?
        .join("Repo")
        .join(format!("{}.zip", obj.packagename));
    zip_folder(&project_path, &zip_file_path)?;
    Ok(())
}

pub fn init(obj: &Init) -> ServerResult<()> {
    let project_path = PROJECT_SRC.get().unwrap().join(obj.name.as_str());
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

pub fn fix(obj: &Fix, repo: &mut RepoInfo) -> ServerResult<()> {
    match &obj.command {
        FixAction::Add(obj) => fix_add(obj, repo)?,
        FixAction::Del(obj) => fix_del(obj, repo)?,
    }
    Ok(())
}

fn fix_add(obj: &Add, repo: &mut RepoInfo) -> ServerResult<()> {
    let path = PROJECT_SRC.get().unwrap().join(&obj.project_name);
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
