use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("terminal error: {0}")]
    Terminal(String),

    #[error("scan error: {0}")]
    Scan(String),

    #[error("delete error for {path:?}: {reason}")]
    Delete { path: PathBuf, reason: String },
}
