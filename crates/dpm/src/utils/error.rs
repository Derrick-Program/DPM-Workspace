use dpm_core::CoreError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("Core error: {0}")]
    Core(#[from] CoreError),

    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    #[error("Configuration error: {0}")]
    ConfigError(String),

    #[error("System error: {0}")]
    SystemError(String),

    #[error("Lock error: {0}")]
    LockError(String),
}

pub type ClientResult<T> = Result<T, ClientError>;
