mod error;
mod zip_file;
pub use error::*;
pub use ed25519_dalek::{Signature, SigningKey, VerifyingKey};
use ed25519_dalek::{Signer, Verifier};
use serde::{Deserialize, Serialize};
use serde_json::to_writer_pretty;
use std::{collections::HashMap, io::Read, path::Path};
pub use zip_file::*;

/// 對檔案內容算 blake3 hash,回傳小寫十六進位字串。
/// client(安裝驗證)、server(發布時算 hash)共用同一份實作。
pub fn hash_file(path: &Path) -> CoreResult<String> {
    let mut file = std::fs::File::open(path)?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;
    Ok(blake3::hash(&buffer).to_hex().to_string())
}

/// 對任意 bytes 算 blake3 hash,回傳小寫十六進位字串——跟 [`hash_file`] 同一個
/// 演算法,只是輸入不是檔案而是記憶體內容(`kind: source` 套件的
/// `build_command` + commit hash 組合就是這樣算的,沒有對應的實體檔案可以餵
/// 給 `hash_file`)。
pub fn hash_bytes(data: &[u8]) -> String {
    blake3::hash(data).to_hex().to_string()
}

/// 產生一把新的 ed25519 簽章金鑰對——`dpm-server keygen` 用。簽章本身
/// (`sign_hash`)是確定性的,不需要隨機性,只有「產生新金鑰」這一步需要
/// CSPRNG,直接用 `getrandom` 填 32 bytes seed 建構 `SigningKey`,不透過
/// `SigningKey::generate`(那需要額外開 `ed25519-dalek` 的 `rand_core`
/// feature 並拉近一個版本可能跟其他依賴打架的 `rand_core` crate——這裡不需要)。
pub fn generate_signing_key() -> CoreResult<SigningKey> {
    let mut secret = [0u8; 32];
    getrandom::getrandom(&mut secret)
        .map_err(|e| CoreError::SecurityError(format!("failed to generate random key: {e}")))?;
    Ok(SigningKey::from_bytes(&secret))
}

/// 把讀出來的 32 bytes 私鑰檔案內容還原成 `SigningKey`。
pub fn signing_key_from_bytes(bytes: &[u8]) -> CoreResult<SigningKey> {
    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|_| CoreError::SignatureInvalid("private key must be 32 bytes".to_string()))?;
    Ok(SigningKey::from_bytes(&arr))
}

/// 把讀出來的 32 bytes 公鑰檔案內容(`keys/<author_id>.pub`)還原成
/// `VerifyingKey`。`dpm-server fix add`(讀本機 `keys/` 目錄)、`dpm`
/// client(讀從官方 repo 抓下來的 raw bytes)共用同一份實作。
pub fn verifying_key_from_bytes(bytes: &[u8]) -> CoreResult<VerifyingKey> {
    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|_| CoreError::SignatureInvalid("public key must be 32 bytes".to_string()))?;
    VerifyingKey::from_bytes(&arr).map_err(|e| CoreError::SignatureInvalid(e.to_string()))
}

/// 對一個 hex 編碼的 hash 字串(`packageInfo.json.hash`/`PackageKind` 的
/// hash 欄位本身,不是重新雜湊一次)簽章,回傳 hex 編碼的簽章字串。
pub fn sign_hash(signing_key: &SigningKey, hash_hex: &str) -> String {
    let sig: Signature = signing_key.sign(hash_hex.as_bytes());
    hex::encode(sig.to_bytes())
}

/// 驗證 `signature_hex` 是否是 `verifying_key` 對 `hash_hex` 的合法簽章。
/// hex 格式錯誤、簽章長度不對、驗證不過——任何一步失敗都回傳同一種
/// `CoreError::SignatureInvalid`,呼叫端不需要分辨失敗原因,一律視為
/// 「這個簽章不可信」。
pub fn verify_hash_signature(
    verifying_key: &VerifyingKey,
    hash_hex: &str,
    signature_hex: &str,
) -> CoreResult<()> {
    let sig_bytes = hex::decode(signature_hex)
        .map_err(|e| CoreError::SignatureInvalid(format!("signature is not valid hex: {e}")))?;
    let sig_bytes: [u8; 64] = sig_bytes
        .try_into()
        .map_err(|_| CoreError::SignatureInvalid("signature must be 64 bytes".to_string()))?;
    let signature = Signature::from_bytes(&sig_bytes);
    verifying_key
        .verify(hash_hex.as_bytes(), &signature)
        .map_err(|e| CoreError::SignatureInvalid(e.to_string()))
}

/// `dpm`、`dpm-server` 兩個 CLI 的 clap 配色主題,共用同一份實作。
pub fn get_styles() -> clap::builder::Styles {
    clap::builder::Styles::styled()
        .usage(
            anstyle::Style::new()
                .bold()
                .underline()
                .fg_color(Some(anstyle::Color::Ansi(anstyle::AnsiColor::Yellow))),
        )
        .header(
            anstyle::Style::new()
                .bold()
                .underline()
                .fg_color(Some(anstyle::Color::Ansi(anstyle::AnsiColor::Yellow))),
        )
        .literal(
            anstyle::Style::new().fg_color(Some(anstyle::Color::Ansi(anstyle::AnsiColor::Green))),
        )
        .invalid(
            anstyle::Style::new()
                .bold()
                .fg_color(Some(anstyle::Color::Ansi(anstyle::AnsiColor::Red))),
        )
        .error(
            anstyle::Style::new()
                .bold()
                .fg_color(Some(anstyle::Color::Ansi(anstyle::AnsiColor::Red))),
        )
        .valid(
            anstyle::Style::new()
                .bold()
                .underline()
                .fg_color(Some(anstyle::Color::Ansi(anstyle::AnsiColor::Green))),
        )
        .placeholder(
            anstyle::Style::new().fg_color(Some(anstyle::Color::Ansi(anstyle::AnsiColor::White))),
        )
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
    /// 發布這個版本的作者 id(`keys/<author_id>.pub` 的檔名)。`Option` 是為了
    /// 讓舊格式(這次改動之前產生)的 `packageInfo.json` 還能被解析——
    /// `dpm-server init --author` 之後一律會填。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// `dpm-server sign` 對 `hash` 欄位簽出來的 hex 簽章。`init` 建立時是
    /// `None`,只有 `sign` 這一個指令會寫入。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
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
    /// - `author`: 發布這個版本的作者 id
    ///
    /// # 回傳
    /// 回傳一個新的 `PackageInfo` 結構體(`signature` 一律從 `None` 開始)
    pub fn new(
        package_name: String,
        file_name: String,
        version: String,
        description: String,
        hash: String,
        dependencies: Option<Vec<Dependency>>,
        author: Option<String>,
    ) -> PackageInfo {
        PackageInfo {
            package_name,
            file_name,
            version,
            description,
            hash,
            dependencies,
            author,
            signature: None,
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
    /// 只提供原始碼 + build 指令,client 在本機執行 build。`hash` 是
    /// `blake3(build_command + commit hash)`(`dpm-server hash --build`
    /// 算出來的),`Option` 是因為還沒被 `hash`+`sign` 過的草稿狀態下沒有值。
    Source {
        build: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hash: Option<String>,
    },
}

impl PackageKind {
    /// 供本地 SQLite `LocalRepo` 表使用的扁平欄位("prebuilt" | "source" +
    /// 該 variant 專屬欄位)。跟 [`Self::from_db_fields`] 成對——variant 名稱
    /// 只在這兩個函式裡出現一次,呼叫端不需要自己重複 "prebuilt"/"source"
    /// 字面值。
    pub fn to_db_fields(
        &self,
    ) -> (
        &'static str,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    ) {
        match self {
            PackageKind::Prebuilt {
                url,
                hash,
                file_name,
            } => (
                "prebuilt",
                Some(url.clone()),
                Some(hash.clone()),
                Some(file_name.clone()),
                None,
            ),
            PackageKind::Source { build, hash } => (
                "source",
                None,
                hash.clone(),
                None,
                Some(build.clone()),
            ),
        }
    }

    /// [`Self::to_db_fields`] 的反向操作,把 `LocalRepo` 讀出來的扁平欄位還原
    /// 成 `PackageKind`,而不是讓呼叫端各自比對 `kind == "source"` 字串。
    pub fn from_db_fields(
        kind: &str,
        url: Option<String>,
        hash: Option<String>,
        filename: Option<String>,
        build_command: Option<String>,
    ) -> CoreResult<Self> {
        match kind {
            "prebuilt" => Ok(PackageKind::Prebuilt {
                url: url.ok_or_else(|| {
                    CoreError::InvalidPackage("prebuilt package missing url".to_string())
                })?,
                hash: hash.ok_or_else(|| {
                    CoreError::InvalidPackage("prebuilt package missing hash".to_string())
                })?,
                file_name: filename.ok_or_else(|| {
                    CoreError::InvalidPackage("prebuilt package missing filename".to_string())
                })?,
            }),
            "source" => Ok(PackageKind::Source {
                build: build_command.ok_or_else(|| {
                    CoreError::InvalidPackage("source package missing build command".to_string())
                })?,
                hash,
            }),
            other => Err(CoreError::InvalidPackage(format!(
                "unknown package kind: {other}"
            ))),
        }
    }
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
    /// 發布這個版本的作者 id。只有 `source.repo_url == OFFICIAL_REPO_URL`
    /// 的來源會被 client 拿來做簽章驗證,其他來源忽略這個欄位。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// `dpm-server sign` 簽出來的 hex 簽章,簽的是 `kind` 裡的 hash 欄位。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
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
