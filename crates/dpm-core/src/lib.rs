mod error;
pub use error::*;
use serde::{Deserialize, Serialize};
use serde_json::to_writer_pretty;
use std::{collections::HashMap, io::Read, path::Path};

/// 對檔案內容算 blake3 hash,回傳小寫十六進位字串。
/// client(安裝驗證)、server(發布時算 hash)共用同一份實作。
pub fn hash_file(path: &Path) -> CoreResult<String> {
    let mut file = std::fs::File::open(path)?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;
    Ok(blake3::hash(&buffer).to_hex().to_string())
}

/// 代表套件的依賴資訊
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Dependency {
    pub name: String,
    pub version: String,
}
/// 儲存套件的完整資訊
#[derive(Debug, Serialize, Deserialize)]
pub struct PackageInfo {
    pub package_name: String,
    pub file_name: String,
    pub version: String,
    pub description: String,
    pub hash: String,
    pub dependencies: Option<Vec<Dependency>>,
}

impl PackageInfo {
    /// 建立一個新的 `PackageInfo` 實例
    ///
    /// # 參數
    /// - `package_name`: 套件名稱
    /// - `file_name`: 套件檔案名稱
    /// - `version`: 套件版本
    /// - `description`: 套件描述
    /// - `hash`: 套件檔案的雜湊值
    /// - `dependencies`: 可選的依賴列表
    ///
    /// # 回傳
    /// 回傳一個新的 `PackageInfo` 結構體
    pub fn new(
        package_name: String,
        file_name: String,
        version: String,
        description: String,
        hash: String,
        dependencies: Option<Vec<Dependency>>,
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
/// 用於處理 JSON 的存儲模組
pub struct JsonStorage<T> {
    _marker: std::marker::PhantomData<T>,
}
impl<T> JsonStorage<T>
where
    T: Serialize + for<'de> Deserialize<'de>,
{
    /// 從 JSON 檔案載入資料
    ///
    /// # 參數
    /// - `path`: JSON 檔案的路徑
    ///
    /// # 回傳
    /// 回傳載入的資料或錯誤
    pub fn from_json(path: &Path) -> CoreResult<T> {
        let mut file_contents = String::new();
        let mut file = std::fs::File::open(path)?;
        file.read_to_string(&mut file_contents)?;
        let data: T = serde_json::from_str(&file_contents).map_err(CoreError::JsonError)?;
        Ok(data)
    }

    /// 將資料存儲為 JSON 檔案
    ///
    /// # 參數
    /// - `data`: 要儲存的資料
    /// - `path`: 儲存檔案的路徑
    pub fn to_json(data: &T, path: &Path) -> CoreResult<()> {
        let file = std::fs::File::create(path)?;
        to_writer_pretty(file, &data)?;
        Ok(())
    }

    /// 從 URL 獲取並反序列化 JSON 資料（異步）
    ///
    /// # 參數
    /// - `url`: JSON 資料的 URL
    ///
    /// # 回傳
    /// 回傳載入的資料或錯誤
    pub async fn from_url(url: &str) -> CoreResult<T> {
        let response = reqwest::get(url)
            .await
            .map_err(|e| CoreError::NetworkError(e.to_string()))?
            .text()
            .await
            .map_err(|e| CoreError::NetworkError(e.to_string()))?;
        let repo_info: T = serde_json::from_str(&response)?;
        Ok(repo_info)
    }
    /// 從字串反序列化 JSON 資料
    ///
    /// # 參數
    /// - `file_contents`: JSON 格式的字串
    ///
    /// # 回傳
    /// 回傳反序列化的資料或錯誤
    pub fn from_str_to(file_contents: &str) -> CoreResult<T> {
        let data: T = serde_json::from_str(file_contents)?;
        Ok(data)
    }
}

/// 套件在某個來源索引裡的一個具體版本條目。
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum PackageKind {
    /// 已預先打包好的二進位/壓縮檔,client 直接下載解壓。
    Prebuilt {
        url: String,
        hash: String,
        file_name: String,
    },
    /// 只提供原始碼 + build 指令,client 在本機執行 build(Phase 4 才會真的走這條路)。
    Source { build: String },
}

/// 套件的一個發布版本。已發布的版本視為不可變——要變更只能發布新版本或撤下整個版本。
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PackageVersionInfo {
    pub version: String,
    #[serde(flatten)]
    pub kind: PackageKind,
    pub dependencies: Option<Vec<Dependency>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// 儲存庫的資訊管理模組——代表「一個來源」自己的索引,不含來源名稱本身
/// (來源是 client 端 config 的概念,見 `dpm` crate 的 `Source`/`Setting`)。
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct RepoInfo {
    /// 套件名稱 -> 該套件所有已發布版本(依發布順序,不排序)
    packages: HashMap<String, Vec<PackageVersionInfo>>,
}
impl RepoInfo {
    /// 建立一個新的 `RepoInfo` 實例
    pub fn new() -> Self {
        RepoInfo {
            packages: HashMap::new(),
        }
    }
    /// 檢查是否存在指定名稱的套件(任何版本)
    pub fn has_package(&self, package_name: &str) -> bool {
        self.packages.contains_key(package_name)
    }
    /// 取得某套件的所有已發布版本
    pub fn versions_of(&self, package_name: &str) -> CoreResult<&Vec<PackageVersionInfo>> {
        self.packages
            .get(package_name)
            .ok_or_else(|| CoreError::PackageNotFound(package_name.to_string()))
    }
    /// 取得某套件「最新」的版本——依發布順序(`Vec` 最後一筆),不比較 semver。
    pub fn latest_version(&self, package_name: &str) -> CoreResult<&PackageVersionInfo> {
        self.versions_of(package_name)?
            .last()
            .ok_or_else(|| CoreError::PackageNotFound(package_name.to_string()))
    }
    pub fn get_package_handler(&self) -> &HashMap<String, Vec<PackageVersionInfo>> {
        &self.packages
    }
}

#[cfg(feature = "server")]
impl RepoInfo {
    /// 新增一個套件版本。同一個套件名稱下,`info.version` 不能跟既有版本重複
    /// (已發布版本不可變——要換內容是撤下重發,不是原地覆寫)。
    pub fn add_package_version(
        &mut self,
        name: String,
        info: PackageVersionInfo,
    ) -> CoreResult<()> {
        let versions = self.packages.entry(name).or_default();
        if versions.iter().any(|v| v.version == info.version) {
            return Err(CoreError::VersionMismatch(format!(
                "version {} is already published",
                info.version
            )));
        }
        versions.push(info);
        Ok(())
    }

    /// 移除某套件的特定版本。移除後若該套件已無任何版本,連同套件名稱一起移除。
    pub fn remove_package_version(
        &mut self,
        package_name: &str,
        version: &str,
    ) -> CoreResult<PackageVersionInfo> {
        let versions = self
            .packages
            .get_mut(package_name)
            .ok_or_else(|| CoreError::PackageNotFound(package_name.to_string()))?;
        let idx = versions
            .iter()
            .position(|v| v.version == version)
            .ok_or_else(|| CoreError::PackageNotFound(format!("{package_name}@{version}")))?;
        let removed = versions.remove(idx);
        if versions.is_empty() {
            self.packages.remove(package_name);
        }
        Ok(removed)
    }
}

#[cfg(feature = "client")]
impl RepoInfo {
    /// 從遠端抓某個來源的完整索引,整包覆蓋 `self`(這個 `RepoInfo` 實例代表
    /// 單一來源;多來源合併是呼叫端——`dpm` crate——的責任,每個來源各自呼叫
    /// 一次這個方法在自己的 `RepoInfo` 實例上)。
    pub async fn fetch_update_repo_info(&mut self, url: &str) -> CoreResult<()> {
        let repo_info: RepoInfo = JsonStorage::from_url(url).await?;
        self.packages = repo_info.packages;
        Ok(())
    }

    /// 取得某套件某個特定版本的完整 `packageInfo.json`(只有 `Prebuilt` 版本
    /// 有 URL 可抓;`Source` 版本目前回傳 `InvalidPackage` 錯誤,Phase 4 client
    /// 端 source 安裝路徑落地後才會補上對應處理)。
    pub async fn get_package_info(
        &self,
        package_name: &str,
        version: &str,
    ) -> CoreResult<PackageInfo> {
        let versions = self.versions_of(package_name)?;
        let entry = versions
            .iter()
            .find(|v| v.version == version)
            .ok_or_else(|| CoreError::PackageNotFound(format!("{package_name}@{version}")))?;
        match &entry.kind {
            PackageKind::Prebuilt { url, file_name, .. } => {
                let new_url = url.replace(
                    file_name,
                    format!("src/{package_name}/packageInfo.json").as_str(),
                );
                JsonStorage::from_url(&new_url).await
            }
            PackageKind::Source { .. } => Err(CoreError::InvalidPackage(format!(
                "{package_name}@{version} is a source package, not yet installable"
            ))),
        }
    }
}
impl Dependency {
    pub fn new(name: &str, version: &str) -> Self {
        Dependency {
            name: name.to_owned(),
            version: version.to_owned(),
        }
    }
}

// "rust-analyzer.cargo.features": ["client", "server"]
