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

    #[error("download error: {0}")]
    Download(String),

    #[error("anilist error: {0}")]
    AniList(String),

    /// AniList rejected the token (401). Surfaced to the UI as a re-login
    /// prompt rather than a hard error — see the sync worker.
    #[error("anilist authentication expired")]
    Unauthorized,

    /// AniList rate limit hit (429). Carries the retry-after in seconds.
    #[error("anilist rate limited; retry after {0}s")]
    RateLimited(u64),
}

pub type Result<T> = std::result::Result<T, Error>;
