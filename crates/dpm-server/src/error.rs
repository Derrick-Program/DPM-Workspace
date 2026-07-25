use dpm_core::CoreError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ServerError {
    #[error("Core error: {0}")]
    Core(#[from] CoreError),

    #[error("Package validation error: {0}")]
    ValidationError(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Directory walk error: {0}")]
    WalkError(#[from] walkdir::Error),

    #[error("Network error: {0}")]
    NetworkError(#[from] reqwest::Error),
}

pub type ServerResult<T> = Result<T, ServerError>;
