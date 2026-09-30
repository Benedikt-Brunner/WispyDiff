use std::fmt;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not a pull request reference: {0}")]
    InvalidPrRef(String),
    #[error("`git {command}` failed: {stderr}")]
    Git { command: String, stderr: String },
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("GitHub API error: {0}")]
    GitHub(String),
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error("could not decode cached data: {0}")]
    Codec(String),
    #[error("could not parse git diff output: {0}")]
    DiffParse(String),
    #[error("no GitHub token available: {0}")]
    NoToken(String),
    #[error("{0}")]
    Assistant(String),
}

impl Error {
    pub(crate) fn codec(err: impl fmt::Display) -> Self {
        Error::Codec(err.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
