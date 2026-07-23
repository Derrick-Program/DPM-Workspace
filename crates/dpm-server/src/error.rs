use dpm_core::CoreError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ServerError {
    #[error("Core error: {0}")]
    Core(#[from] CoreError),

    #[error("Storage error: {0}")]
    StorageError(String),

    #[error("Authentication error: {0}")]
    AuthError(String),

    #[error("Package validation error: {0}")]
    ValidationError(String),
}

pub type ServerResult<T> = Result<T, ServerError>;
