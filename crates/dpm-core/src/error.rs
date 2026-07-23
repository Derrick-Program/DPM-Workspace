use thiserror::Error;
#[allow(dead_code)]
#[derive(Debug, Error)]
pub enum CoreError {
    #[error("Package not found: {0}")]
    PackageNotFound(String),

    #[error("Version mismatch: {0}")]
    VersionMismatch(String),

    #[error("Invalid package format: {0}")]
    InvalidPackage(String),

    #[error("Dependency error: {0}")]
    DependencyError(String),

    #[error("Network error: {0}")]
    NetworkError(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("Database error: {0}")]
    DatabaseError(String),

    #[error("Hash verification failed: {expected} != {actual}")]
    HashMismatch { expected: String, actual: String },

    #[error("Security error: {0}")]
    SecurityError(String),
}
#[allow(dead_code)]
pub type CoreResult<T> = Result<T, CoreError>;
