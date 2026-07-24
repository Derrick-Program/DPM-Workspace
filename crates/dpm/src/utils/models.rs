use super::ClientError;
use super::ClientResult;
use dpm_core::CoreError::*;
use dpm_core::Dependency;
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
    pub entry: String,
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
        entry: &str,
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
            entry: entry.to_owned(),
            dependencies,
        }
    }

    /// 將結構轉為 JSON 字串
    pub fn to_json_string(&self) -> ClientResult<String> {
        serde_json::to_string(self).map_err(|e| ClientError::Core(JsonError(e)))
    }

    /// 從 JSON 字串解析為結構
    pub fn from_json_string(json: &str) -> ClientResult<Self> {
        serde_json::from_str(json).map_err(|e| ClientError::Core(JsonError(e)))
    }
}
