//! Unified error types used across the crate.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ScalperError {
    #[error("configuration error: {0}")]
    Config(String),

    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),

    #[error("exchange error: {0}")]
    Exchange(String),

    #[error("risk gate blocked: {0}")]
    RiskBlocked(String),

    #[error("llm error: {0}")]
    Llm(String),

    #[error("parse error: {0}")]
    Parse(String),

    #[error("unsupported: {0}")]
    Unsupported(String),
}

pub type Result<T> = std::result::Result<T, ScalperError>;
