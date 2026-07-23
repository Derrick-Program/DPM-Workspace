use super::ClientError;
use super::ClientResult;
use crate::schema::LocalRepo;
use diesel::prelude::*;
use dpm_core::CoreError::*;
use dpm_core::Dependency;
use serde::{Deserialize, Serialize};
#[derive(Debug, Queryable, Selectable, Serialize, Deserialize, Clone)]
#[diesel(table_name = LocalRepo)]
pub struct DbPackage {
    pub name: String,
    pub version: String,
    pub url: String,
    pub description: String,
    pub filename: String,
    pub hash: String,
    pub entry: String,
    pub dependencies: Option<Vec<Dependency>>,
}

#[derive(Insertable)]
#[diesel(table_name = LocalRepo)]
pub struct NewDbPackage {
    pub name: String,
    pub version: String,
    pub url: String,
    pub description: String,
    pub filename: String,
    pub hash: String,
    pub entry: String,
    pub dependencies: Option<String>,
}
#[allow(clippy::too_many_arguments)]
impl DbPackage {
    pub fn new(
        name: &str,
        version: &str,
        url: &str,
        description: &str,
        filename: &str,
        hash: &str,
        entry: &str,
        dependencies: Option<Vec<Dependency>>,
    ) -> Self {
        DbPackage {
            name: name.to_owned(),
            version: version.to_owned(),
            url: url.to_owned(),
            description: description.to_owned(),
            filename: filename.to_owned(),
            hash: hash.to_owned(),
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
