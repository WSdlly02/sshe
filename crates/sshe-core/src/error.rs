use sshe_protocol::{MAX_EXEC_SECONDS, OUTPUT_LIMIT};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("system call: {0}")]
    System(#[from] nix::errno::Errno),
    #[error("serialization: {0}")]
    Json(#[from] serde_json::Error),
    #[error("command timed out; process group terminated")]
    Timeout,
    #[error("command output exceeded {OUTPUT_LIMIT} bytes per stream; process group terminated")]
    OutputLimit,
    #[error("invalid timeout: must be 1..={MAX_EXEC_SECONDS} seconds")]
    InvalidTimeout,
    #[error("{0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, Error>;
