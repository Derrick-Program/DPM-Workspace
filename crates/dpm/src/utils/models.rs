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
