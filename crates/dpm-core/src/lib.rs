mod config_layer;
mod error;
mod zip_file;
pub use config_layer::*;
pub use ed25519_dalek::{Signature, SigningKey, VerifyingKey};
use ed25519_dalek::{Signer, Verifier};
pub use error::*;
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

/// 檢查 `author` 是否只由 `[A-Za-z0-9_-]` 組成且非空——多個呼叫端
/// (`dpm-server` 的 `init`/`sign`/`verify_publish_authorization`、`dpm` 的
/// `verify_official_signature`)都會把 `author` 直接當路徑片段組出
/// `keys_dir.join(...)`,而 `author` 的來源(`packageInfo.json`/
/// `RepoInfo.json`/CLI 參數)都是攻擊者可控的資料。沒有這層檢查,
/// 像 `"../../../mallory/evil-keys/main/keys/mallory"` 這樣的值可以逃出
/// `keys_dir`,讀到任意檔案,或把「官方」金鑰抓取重導向到攻擊者控制的位置。
/// 之前這個檢查在 `dpm-server`、`dpm` 各自複製了一份;這裡集中成一份共用實作。
pub fn validate_author_id(author: &str) -> CoreResult<()> {
    if author.is_empty()
        || !author
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(CoreError::SignatureInvalid(format!(
            "author id '{author}' contains invalid characters — must match [A-Za-z0-9_-]+"
        )));
    }
    Ok(())
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

/// 跟 `JsonStorage` 同一個「整包讀出、整包寫回」的模式,只是格式換成
/// TOML——分層設定系統裡,唯一會被程式「寫入」的一層(使用者層那個實體
/// 檔案)透過這個型別讀寫;系統層/環境變數是唯讀的,不會經過這裡,合併讀取
/// 走 [`load_layered`]。
pub struct TomlStorage<T> {
    _marker: std::marker::PhantomData<T>,
}

impl<T> TomlStorage<T>
where
    T: Serialize + for<'de> Deserialize<'de>,
{
    pub fn from_toml(path: &Path) -> CoreResult<T> {
        let contents = std::fs::read_to_string(path)?;
        toml::from_str(&contents).map_err(|e| CoreError::ConfigError(e.to_string()))
    }

    pub fn to_toml(data: &T, path: &Path) -> CoreResult<()> {
        let contents =
            toml::to_string_pretty(data).map_err(|e| CoreError::ConfigError(e.to_string()))?;
        std::fs::write(path, contents)?;
        Ok(())
    }
}

/// `PackageKind::Prebuilt` 的其中一組 target-specific build——apt/dnf 風格,
/// 同一個套件版本可以有多組,`target` 用 Rust target triple(`self_update::
/// get_target()` 那套字串),`None` 代表任何平台通用(對應 apt 的
/// `Architecture: all`)。
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PrebuiltBuild {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    pub url: String,
    pub hash: String,
    pub file_name: String,
}

/// 套件在某個來源索引裡的一個具體版本條目。
#[derive(Debug, Serialize, Clone)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum PackageKind {
    /// 已預先打包好的二進位/壓縮檔,client 直接下載解壓。一個版本可以登記
    /// 多組 target-specific build(apt/dnf 風格),`to_db_fields` 依呼叫端
    /// 傳入的本機 target 挑一組。舊格式(單一 `url`/`hash`/`file_name`,沒有
    /// `builds` 陣列)透過下面的 `Deserialize` 手動實作相容,視為一組
    /// `target: None` 的通用 build。
    Prebuilt { builds: Vec<PrebuiltBuild> },
    /// 只提供原始碼 + build 指令,client 在本機執行 build。`hash` 是
    /// `blake3(build_command + commit hash)`(`dpm-server hash --build`
    /// 算出來的),`Option` 是因為還沒被 `hash`+`sign` 過的草稿狀態下沒有值。
    /// `supported_targets` 是安裝前的允許清單(`None` = 任何平台)——build
    /// 指令本身不分平台,只有一組,這個欄位純粹用來擋「這台機器不支援」的
    /// 情況,不是挑選邏輯。
    ///
    /// 已知、刻意延後處理的缺口(非本次功能涵蓋範圍,不要當成已解決):這個
    /// hash 綁定的是 `build_command` 字串加上發布當下的 commit,但 commit
    /// 本身沒有被發布到任何 client 端可以驗證的地方,client 目前也完全沒有
    /// 重算這個 hash 並跟簽章比對——`build` 欄位是直接從 `RepoInfo.json`
    /// 讀出來就拿去執行(見 `dpm/src/action.rs::install_source_package`)。
    /// 也就是說,對 `kind: source` 套件而言,簽章目前**不提供任何**對
    /// `build_command` 或原始碼樹的保護:只要能改 `RepoInfo.json`(不需要
    /// 簽名金鑰),就能把 `build` 換成任意指令,client 端的驗證閘門不會擋下
    /// 來。`kind: Prebuilt` 不受影響(下載內容有獨立 hash 比對,見
    /// `fetch_and_verify_prebuilt`)。
    Source {
        build: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hash: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        supported_targets: Option<Vec<String>>,
    },
}

impl<'de> Deserialize<'de> for PackageKind {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error as _;
        use serde_json::Value;

        let mut value = Value::deserialize(deserializer)?;
        let obj = value
            .as_object_mut()
            .ok_or_else(|| D::Error::custom("PackageKind must be a JSON object"))?;
        let kind = obj
            .get("kind")
            .and_then(Value::as_str)
            .ok_or_else(|| D::Error::custom("PackageKind is missing a \"kind\" tag"))?
            .to_string();

        match kind.as_str() {
            "prebuilt" => {
                if !obj.contains_key("builds") {
                    // 舊格式:單一 url/hash/file_name,沒有 builds 陣列——
                    // 包成一組 target: None(通用)的 build,相容既有已發布
                    // 資料,不需要重新發布。
                    let url = obj
                        .get("url")
                        .and_then(Value::as_str)
                        .ok_or_else(|| D::Error::custom("prebuilt package missing url"))?
                        .to_string();
                    let hash = obj
                        .get("hash")
                        .and_then(Value::as_str)
                        .ok_or_else(|| D::Error::custom("prebuilt package missing hash"))?
                        .to_string();
                    let file_name = obj
                        .get("file_name")
                        .and_then(Value::as_str)
                        .ok_or_else(|| D::Error::custom("prebuilt package missing file_name"))?
                        .to_string();
                    return Ok(PackageKind::Prebuilt {
                        builds: vec![PrebuiltBuild {
                            target: None,
                            url,
                            hash,
                            file_name,
                        }],
                    });
                }
                #[derive(Deserialize)]
                struct Shape {
                    builds: Vec<PrebuiltBuild>,
                }
                let shape: Shape =
                    serde_json::from_value(value.clone()).map_err(D::Error::custom)?;
                Ok(PackageKind::Prebuilt {
                    builds: shape.builds,
                })
            }
            "source" => {
                #[derive(Deserialize)]
                struct Shape {
                    build: String,
                    #[serde(default)]
                    hash: Option<String>,
                    #[serde(default)]
                    supported_targets: Option<Vec<String>>,
                }
                let shape: Shape =
                    serde_json::from_value(value.clone()).map_err(D::Error::custom)?;
                Ok(PackageKind::Source {
                    build: shape.build,
                    hash: shape.hash,
                    supported_targets: shape.supported_targets,
                })
            }
            other => Err(D::Error::custom(format!("unknown package kind: {other}"))),
        }
    }
}

impl PackageKind {
    /// 依呼叫端傳入的本機 target(`self_update::get_target()` 那套字串)
    /// 挑一組能用的 build,再扁平化成 `LocalRepo` 表的欄位。`Prebuilt`:先找
    /// 完全匹配的 target,找不到就退回 `target: None` 的通用 build,兩者都
    /// 沒有就回傳 `Err`(訊息列出這個版本實際登記的所有 target)。`Source`:
    /// 檢查 `supported_targets`(`None` 或包含這個 target 才算支援),不支援
    /// 就回傳 `Err`(訊息列出 `supported_targets` 內容)。呼叫端
    /// (`sync_source_inner`)對 `Err` 的處理是印警告、跳過這個版本、不寫進
    /// 本機 DB——target 匹配只在 sync 時做一次,本機 DB 存的永遠已經是「這台
    /// 機器裝得下」的單一 build,`LocalRepo` 表結構不用為多 target 改動。
    #[allow(clippy::type_complexity)]
    pub fn to_db_fields(
        &self,
        target: &str,
    ) -> CoreResult<(
        &'static str,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    )> {
        match self {
            PackageKind::Prebuilt { builds } => {
                let chosen = builds
                    .iter()
                    .find(|b| b.target.as_deref() == Some(target))
                    .or_else(|| builds.iter().find(|b| b.target.is_none()))
                    .ok_or_else(|| {
                        let registered: Vec<&str> = builds
                            .iter()
                            .map(|b| b.target.as_deref().unwrap_or("<universal>"))
                            .collect();
                        CoreError::InvalidPackage(format!(
                            "no build available for target {target}; registered targets: {registered:?}"
                        ))
                    })?;
                Ok((
                    "prebuilt",
                    Some(chosen.url.clone()),
                    Some(chosen.hash.clone()),
                    Some(chosen.file_name.clone()),
                    None,
                ))
            }
            PackageKind::Source {
                build,
                hash,
                supported_targets,
            } => {
                if let Some(supported) = supported_targets {
                    if !supported.iter().any(|t| t == target) {
                        return Err(CoreError::InvalidPackage(format!(
                            "package does not support target {target}; supported targets: {supported:?}"
                        )));
                    }
                }
                Ok(("source", None, hash.clone(), None, Some(build.clone())))
            }
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
            "prebuilt" => {
                let url = url.ok_or_else(|| {
                    CoreError::InvalidPackage("prebuilt package missing url".to_string())
                })?;
                let hash = hash.ok_or_else(|| {
                    CoreError::InvalidPackage("prebuilt package missing hash".to_string())
                })?;
                let file_name = filename.ok_or_else(|| {
                    CoreError::InvalidPackage("prebuilt package missing filename".to_string())
                })?;
                Ok(PackageKind::Prebuilt {
                    builds: vec![PrebuiltBuild {
                        target: None,
                        url,
                        hash,
                        file_name,
                    }],
                })
            }
            "source" => Ok(PackageKind::Source {
                build: build_command.ok_or_else(|| {
                    CoreError::InvalidPackage("source package missing build command".to_string())
                })?,
                hash,
                supported_targets: None,
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
    /// 同一個 `(name, version)` 已經發布過時,唯一允許的例外是「幫既有
    /// `Prebuilt` 版本追加一組新 target 的 build」(`dpm-server fix add
    /// ... url --target <T>` 對同版本連續呼叫多次,`AddKind::Build`/
    /// `Source` 套件沒有這個例外,同版本第二次發布一律拒絕)。追加的
    /// target 如果已經存在(含兩邊都是 `None`,即兩次都沒帶 `--target`),
    /// 一樣拒絕。
    pub fn add_package_version(
        &mut self,
        name: String,
        info: PackageVersionInfo,
    ) -> CoreResult<()> {
        let versions = self.packages.entry(name).or_default();
        if let Some(existing) = versions.iter_mut().find(|v| v.version == info.version) {
            return match (&mut existing.kind, &info.kind) {
                (
                    PackageKind::Prebuilt {
                        builds: existing_builds,
                    },
                    PackageKind::Prebuilt { builds: new_builds },
                ) => {
                    for nb in new_builds {
                        if existing_builds.iter().any(|b| b.target == nb.target) {
                            return Err(CoreError::VersionMismatch(format!(
                                "version {} already has a build for target {}",
                                info.version,
                                nb.target.as_deref().unwrap_or("<universal>")
                            )));
                        }
                    }
                    existing_builds.extend(new_builds.iter().cloned());
                    Ok(())
                }
                _ => Err(CoreError::VersionMismatch(format!(
                    "version {} is already published",
                    info.version
                ))),
            };
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

#[cfg(test)]
mod toml_storage_tests {
    use super::*;

    #[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
    struct Demo {
        name: String,
        count: i64,
    }

    #[test]
    fn to_toml_then_from_toml_round_trips_the_same_value() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("demo.toml");
        let original = Demo {
            name: "hello".to_string(),
            count: 3,
        };

        TomlStorage::to_toml(&original, &path).unwrap();
        assert!(path.exists());

        let reloaded: Demo = TomlStorage::from_toml(&path).unwrap();
        assert_eq!(reloaded, original);
    }

    #[test]
    fn from_toml_on_missing_file_is_an_io_error() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does-not-exist.toml");
        let err = TomlStorage::<Demo>::from_toml(&missing).unwrap_err();
        assert!(
            matches!(err, CoreError::IoError(_)),
            "missing file must surface as CoreError::IoError, got: {err:?}"
        );
    }
}

#[cfg(test)]
mod package_kind_target_tests {
    use super::*;

    fn build(target: Option<&str>, url: &str) -> PrebuiltBuild {
        PrebuiltBuild {
            target: target.map(|s| s.to_string()),
            url: url.to_string(),
            hash: "a".repeat(64),
            file_name: "pkg.zip".to_string(),
        }
    }

    #[test]
    fn to_db_fields_picks_the_exact_matching_target() {
        let kind = PackageKind::Prebuilt {
            builds: vec![
                build(Some("x86_64-unknown-linux-gnu"), "https://example.com/linux.zip"),
                build(Some("aarch64-apple-darwin"), "https://example.com/mac.zip"),
            ],
        };
        let (kind_str, url, _, _, _) = kind.to_db_fields("aarch64-apple-darwin").unwrap();
        assert_eq!(kind_str, "prebuilt");
        assert_eq!(url.unwrap(), "https://example.com/mac.zip");
    }

    #[test]
    fn to_db_fields_falls_back_to_the_universal_build_when_no_exact_match() {
        let kind = PackageKind::Prebuilt {
            builds: vec![
                build(Some("x86_64-unknown-linux-gnu"), "https://example.com/linux.zip"),
                build(None, "https://example.com/universal.zip"),
            ],
        };
        let (_, url, _, _, _) = kind.to_db_fields("aarch64-apple-darwin").unwrap();
        assert_eq!(url.unwrap(), "https://example.com/universal.zip");
    }

    #[test]
    fn to_db_fields_errors_when_no_match_and_no_universal_build() {
        let kind = PackageKind::Prebuilt {
            builds: vec![build(
                Some("x86_64-unknown-linux-gnu"),
                "https://example.com/linux.zip",
            )],
        };
        let err = kind.to_db_fields("aarch64-apple-darwin").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("x86_64-unknown-linux-gnu"),
            "error must list the registered targets so the user knows what IS available: {msg}"
        );
    }

    #[test]
    fn to_db_fields_accepts_source_package_with_matching_supported_target() {
        let kind = PackageKind::Source {
            build: "cc -shared -o lib.so lib.c".to_string(),
            hash: Some("a".repeat(64)),
            supported_targets: Some(vec!["aarch64-apple-darwin".to_string()]),
        };
        let (kind_str, _, _, _, build_command) = kind.to_db_fields("aarch64-apple-darwin").unwrap();
        assert_eq!(kind_str, "source");
        assert_eq!(build_command.unwrap(), "cc -shared -o lib.so lib.c");
    }

    #[test]
    fn to_db_fields_rejects_source_package_on_unsupported_target() {
        let kind = PackageKind::Source {
            build: "cc -shared -o lib.so lib.c".to_string(),
            hash: Some("a".repeat(64)),
            supported_targets: Some(vec!["x86_64-unknown-linux-gnu".to_string()]),
        };
        let err = kind.to_db_fields("aarch64-apple-darwin").unwrap_err();
        assert!(err.to_string().contains("x86_64-unknown-linux-gnu"));
    }

    #[test]
    fn to_db_fields_accepts_source_package_with_no_supported_targets_declared() {
        let kind = PackageKind::Source {
            build: "cc -shared -o lib.so lib.c".to_string(),
            hash: Some("a".repeat(64)),
            supported_targets: None,
        };
        assert!(kind.to_db_fields("any-target-at-all").is_ok());
    }

    #[test]
    fn old_format_prebuilt_without_builds_array_deserializes_as_one_universal_build() {
        let json = r#"{"kind":"prebuilt","url":"https://example.com/old.zip","hash":"abc","file_name":"old.zip"}"#;
        let kind: PackageKind = serde_json::from_str(json).unwrap();
        match kind {
            PackageKind::Prebuilt { builds } => {
                assert_eq!(builds.len(), 1);
                assert_eq!(builds[0].target, None);
                assert_eq!(builds[0].url, "https://example.com/old.zip");
            }
            _ => panic!("expected Prebuilt"),
        }
    }

    #[test]
    fn old_format_source_without_supported_targets_deserializes_as_none() {
        let json = r#"{"kind":"source","build":"make"}"#;
        let kind: PackageKind = serde_json::from_str(json).unwrap();
        match kind {
            PackageKind::Source {
                supported_targets, ..
            } => assert_eq!(supported_targets, None),
            _ => panic!("expected Source"),
        }
    }
}

// "rust-analyzer.cargo.features": ["client", "server"]
