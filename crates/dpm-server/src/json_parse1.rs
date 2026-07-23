use std::{
    collections::HashMap,
    fs::File,
    io::{self, Read},
    path::Path,
};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::to_writer_pretty;
#[derive(Debug, Serialize, Deserialize)]
pub struct PackageInfo {
    pub package_name: String,
    pub file_name: String,
    pub version: String,
    pub description: String,
    pub hash: String,
    pub dependencies: Option<Vec<String>>,
}

impl PackageInfo {
    pub fn new(
        package_name: String,
        file_name: String,
        version: String,
        description: String,
        hash: String,
        dependencies: Option<Vec<String>>,
    ) -> PackageInfo {
        PackageInfo {
            package_name,
            file_name,
            version,
            description,
            hash,
            dependencies,
        }
    }
}

// #[derive(Debug, Serialize, Deserialize)]
// pub struct HashInfo {
//     pub file_name: String,
//     pub hash: String,
// }
// impl HashInfo {
//     pub fn new(file_name: String, hash: String) -> HashInfo {
//         HashInfo { file_name, hash }
//     }
// }

pub struct JsonStorage<T> {
    _marker: std::marker::PhantomData<T>,
}

impl<T> JsonStorage<T>
where
    T: Serialize + for<'de> Deserialize<'de>,
{
    pub fn from_json(path: &Path) -> io::Result<T> {
        let mut file_contents = String::new();
        let mut file = File::open(path)?;
        file.read_to_string(&mut file_contents)?;
        let data: T = serde_json::from_str(&file_contents)?;
        Ok(data)
    }

    pub fn to_json(data: &T, path: &Path) -> io::Result<()> {
        let file = File::create(path)?;
        to_writer_pretty(file, &data)?;
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct RepoInfo {
    packages: HashMap<String, PackageBasicInfo>,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct PackageBasicInfo {
    pub url: String,
    pub file_name: String,
    pub version: String,
    pub hash: String,
    pub dependencies: Option<Vec<String>>,
}

impl RepoInfo {
    pub fn new() -> Self {
        RepoInfo {
            packages: HashMap::new(),
        }
    }
    pub fn has_package(&self, package_name: &str) -> bool {
        self.packages.contains_key(package_name)
    }
    pub fn add_package(
        &mut self,
        name: String,
        url: String,
        file_name: String,
        version: String,
        hash: String,
        dependencies: Option<Vec<String>>,
    ) {
        let package = PackageBasicInfo {
            url,
            file_name,
            version,
            hash,
            dependencies,
        };
        self.packages.insert(name, package);
    }
    pub fn add_package_with_info(&mut self, name: String, info: PackageBasicInfo) {
        self.packages.insert(name, info);
    }
    pub fn get_package(&self, package_name: &str) -> Result<&PackageBasicInfo> {
        match self.packages.get(package_name) {
            Some(package) => Ok(package),
            None => Err(anyhow::anyhow!("Package '{}' not found.", package_name)),
        }
    }
    pub fn remove_package(&mut self, package_name: &str) -> Result<PackageBasicInfo> {
        match self.packages.remove(package_name) {
            Some(package) => Ok(package),
            None => Err(anyhow::anyhow!("Package '{}' not found.", package_name)),
        }
    }
    pub fn update_package(
        &mut self,
        package_name: &str,
        url: Option<String>,
        file_name: Option<String>,
        version: Option<String>,
        hash: Option<String>,
        dependencies: Option<Vec<String>>,
    ) {
        if let Some(existing_package) = self.packages.get_mut(package_name) {
            if let Some(new_url) = url {
                existing_package.url = new_url;
            }
            if let Some(new_file_name) = file_name {
                existing_package.file_name = new_file_name;
            }
            if let Some(new_version) = version {
                existing_package.version = new_version;
            }
            if let Some(new_hash) = hash {
                existing_package.hash = new_hash;
            }
            if let Some(new_dependencies) = dependencies {
                existing_package.dependencies = Some(new_dependencies);
            }
        } else {
            self.packages.insert(
                package_name.to_string(),
                PackageBasicInfo {
                    url: url.unwrap_or_default(),
                    file_name: file_name.unwrap_or_default(),
                    version: version.unwrap_or_default(),
                    hash: hash.unwrap_or_default(),
                    dependencies: None,
                },
            );
        }
    }
}
