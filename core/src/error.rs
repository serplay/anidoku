use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),

    #[error("provider error: {0}")]
    Provider(String),

    #[error("decrypt error: {0}")]
    Decrypt(String),

    #[error("subtitle error: {0}")]
    Subtitle(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
