use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("{0}")]
    User(String),
    #[error("the zerdr Herdr session is not running; start bare `zerdr` first")]
    SessionUnavailable,
    #[error("no live zerdr client for this Herdr session; run bare `zerdr` first")]
    NoLiveLease,
    #[error("failed to access {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to decode {what}: {source}")]
    Json {
        what: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("Herdr client exited with status {0}")]
    ChildExit(i32),
    #[error("{program} failed with exit status {status}: {stderr}")]
    Process {
        program: String,
        status: i32,
        stderr: String,
    },
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }

    pub fn exit_code(&self) -> i32 {
        match self {
            Self::ChildExit(code) => *code,
            _ => 1,
        }
    }
}
