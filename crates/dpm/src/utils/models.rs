use super::ClientError;
use super::ClientResult;
use dpm_core::{Dependency, PackageKind};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DbPackage {
    pub source: String,
    pub name: String,
    pub version: String,
    /// "prebuilt" | "source"
    pub kind: String,
    pub url: Option<String>,
    pub hash: Option<String>,
    pub filename: Option<String>,
    pub build_command: Option<String>,
    pub description: String,
    pub entry: Option<String>,
    pub dependencies: Option<Vec<Dependency>>,
    /// 發布這個版本的作者 id——只有官方來源(`repo_url == OFFICIAL_REPO_URL`)
    /// 的套件才會有值,第三方來源永遠是 `None`。
    pub author: Option<String>,
    /// `dpm-server sign` 簽出來的 hex 簽章,簽的是 `hash` 欄位。
    pub signature: Option<String>,
    /// 使用者是否在命令列直接指名安裝這個套件(`true`),還是被別的套件的
    /// `dependencies` 拉進來裝的(`false`)。只對 `InstalledPackages` 有意義
    /// ——`AvailablePackages`(遠端索引快取)裡的列這個欄位恆為 `true`,沒有
    /// 實際語意,純粹因為 `DbPackage` 是兩張表共用的資料結構。`dpm autoremove`
    /// 靠這個欄位判斷哪些已裝套件可以被當成孤兒依賴清掉。
    pub explicit: bool,
}

#[allow(clippy::too_many_arguments)]
impl DbPackage {
    pub fn new(
        source: &str,
        name: &str,
        version: &str,
        kind: &str,
        url: Option<String>,
        hash: Option<String>,
        filename: Option<String>,
        build_command: Option<String>,
        description: &str,
        entry: Option<String>,
        dependencies: Option<Vec<Dependency>>,
        author: Option<String>,
        signature: Option<String>,
        explicit: bool,
    ) -> Self {
        DbPackage {
            source: source.to_owned(),
            name: name.to_owned(),
            version: version.to_owned(),
            kind: kind.to_owned(),
            url,
            hash,
            filename,
            build_command,
            description: description.to_owned(),
            entry,
            dependencies,
            author,
            signature,
            explicit,
        }
    }

    /// 把扁平化存進 `LocalRepo` 的 `kind`/`url`/`hash`/`filename`/`build_command`
    /// 欄位還原成 `PackageKind`,呼叫端不需要自己比對 `"source"`/`"prebuilt"`
    /// 字面值。
    pub fn kind(&self) -> ClientResult<PackageKind> {
        PackageKind::from_db_fields(
            &self.kind,
            self.url.clone(),
            self.hash.clone(),
            self.filename.clone(),
            self.build_command.clone(),
        )
        .map_err(ClientError::Core)
    }
}
