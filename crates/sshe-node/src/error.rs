use iroh::endpoint::{
    ConnectError, ConnectWithOptsError, ConnectingError, ConnectionError, ReadError, VarInt,
    WriteError,
};
use sshe_core::{Classify, io_failure_kind};
use sshe_protocol::FailureKind;

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
    #[error("Iroh: {source}")]
    Transport {
        kind: FailureKind,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error("identity is in use by a running daemon or another sshe command: {0}")]
    IdentityBusy(#[source] std::fs::TryLockError),
    #[error("operation timed out")]
    Timeout,
    #[error("command outcome unknown after submission; not retried: {0}")]
    OutcomeUnknown(#[source] Box<Error>),
    #[error(transparent)]
    ProbeFailed(#[from] std::sync::Arc<Error>),
    #[error("probe cancelled before completion")]
    ProbeCancelled,
    /// The remote answered with an error or an unhealthy status.
    #[error("{0}")]
    Remote(String),
    #[error("{0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Application close code a daemon uses for peers outside its `[peers]`.
pub(crate) const UNAUTHORIZED: VarInt = VarInt::from_u32(1);

pub(crate) fn connect_failed(e: ConnectError) -> Error {
    let kind = match &e {
        ConnectError::Connect {
            source: ConnectWithOptsError::NoAddress { .. },
            ..
        } => FailureKind::NoAddress,
        ConnectError::Connecting {
            source: ConnectingError::ConnectionError { source, .. },
            ..
        }
        | ConnectError::Connection { source, .. } => connection_kind(source),
        _ => FailureKind::Other,
    };
    Error::Transport {
        kind,
        source: Box::new(e),
    }
}

pub(crate) fn connection_lost(e: ConnectionError) -> Error {
    Error::Transport {
        kind: connection_kind(&e),
        source: Box::new(e),
    }
}

pub(crate) fn transport(e: impl std::error::Error + Send + Sync + 'static) -> Error {
    Error::Transport {
        kind: FailureKind::Other,
        source: Box::new(e),
    }
}

fn connection_kind(e: &ConnectionError) -> FailureKind {
    match e {
        ConnectionError::ApplicationClosed(close) if close.error_code == UNAUTHORIZED => {
            FailureKind::Unauthorized
        }
        // Includes a refused handshake from a daemon at its connection limit.
        ConnectionError::ConnectionClosed(_) => FailureKind::Refused,
        ConnectionError::TimedOut => FailureKind::Timeout,
        ConnectionError::ApplicationClosed(_)
        | ConnectionError::Reset
        | ConnectionError::LocallyClosed => FailureKind::ConnectionLost,
        _ => FailureKind::Other,
    }
}

/// Stream I/O surfaces as io::Error; the QUIC cause, if any, sits inside it.
fn stream_io_kind(e: &std::io::Error) -> FailureKind {
    let inner = e.get_ref();
    let lost = inner
        .and_then(|i| i.downcast_ref::<ReadError>())
        .and_then(|r| match r {
            ReadError::ConnectionLost(c) => Some(c),
            _ => None,
        })
        .or_else(|| {
            inner
                .and_then(|i| i.downcast_ref::<WriteError>())
                .and_then(|w| match w {
                    WriteError::ConnectionLost(c) => Some(c),
                    _ => None,
                })
        });
    match lost {
        Some(c) => connection_kind(c),
        None => io_failure_kind(e),
    }
}

impl Classify for Error {
    fn failure_kind(&self) -> FailureKind {
        match self {
            Error::Transport { kind, .. } => *kind,
            Error::Timeout => FailureKind::Timeout,
            Error::Protocol(sshe_protocol::Error::Io(e)) | Error::Io(e) => stream_io_kind(e),
            Error::Core(e) => e.failure_kind(),
            Error::OutcomeUnknown(e) => e.failure_kind(),
            Error::ProbeFailed(e) => e.failure_kind(),
            Error::Remote(_) => FailureKind::Remote,
            _ => FailureKind::Other,
        }
    }
}
