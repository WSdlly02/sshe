#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("config: {0}")]
    ConfigRead(#[from] toml::de::Error),
    #[error("config serialization: {0}")]
    ConfigWrite(#[from] toml::ser::Error),
    #[error(transparent)]
    Protocol(#[from] sshe_protocol::Error),
    #[error(transparent)]
    Core(#[from] sshe_core::Error),
    #[error("Iroh: {0}")]
    Transport(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("identity is already in use; use its running daemon: {0}")]
    IdentityBusy(#[source] std::fs::TryLockError),
    #[error("operation timed out")]
    Timeout,
    #[error("command outcome unknown after submission; not retried: {0}")]
    OutcomeUnknown(#[source] Box<Error>),
    #[error("{0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn transport(e: impl std::error::Error + Send + Sync + 'static) -> Error {
    Error::Transport(Box::new(e))
}
