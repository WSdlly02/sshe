use sshe_protocol::{FailureKind, MAX_EXEC_SECONDS, OUTPUT_LIMIT};
use std::{fmt::Display, io};

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
    #[error("DNS: {0}")]
    Dns(String),
    #[error("{0}")]
    Inactive(String),
    #[error("{0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Errors that can say why a check failed; `measure` records the answer.
pub trait Classify: Display {
    fn failure_kind(&self) -> FailureKind;
}

impl Classify for Error {
    fn failure_kind(&self) -> FailureKind {
        match self {
            Error::Io(e) => io_failure_kind(e),
            Error::Timeout => FailureKind::Timeout,
            Error::Dns(_) => FailureKind::Dns,
            Error::Inactive(_) => FailureKind::Remote,
            _ => FailureKind::Other,
        }
    }
}

pub fn io_failure_kind(e: &io::Error) -> FailureKind {
    use io::ErrorKind::*;
    match e.kind() {
        ConnectionRefused => FailureKind::Refused,
        TimedOut => FailureKind::Timeout,
        HostUnreachable | NetworkUnreachable => FailureKind::Unreachable,
        ConnectionReset | ConnectionAborted | NotConnected | BrokenPipe => {
            FailureKind::ConnectionLost
        }
        _ => FailureKind::Other,
    }
}
