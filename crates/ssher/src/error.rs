#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{operation}: {source}")]
    Io {
        operation: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{operation}: {source}")]
    Parse {
        operation: String,
        #[source]
        source: toml::de::Error,
    },
    #[error("cache serialization: {0}")]
    Serialize(#[from] toml::ser::Error),
    #[error("environment: {0}")]
    Environment(#[from] std::env::VarError),
    #[error("system clock error: {0}")]
    Clock(#[from] std::time::SystemTimeError),
    #[error("invalid user ID: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
    #[error("{0}")]
    Invalid(String),
    #[error("{endpoint}:{port} -> {source}")]
    Probe {
        endpoint: String,
        port: u16,
        #[source]
        source: Box<Error>,
    },
    #[error("no reachable endpoint: {0:?}")]
    Unreachable(Vec<Error>),
}

pub type Result<T> = std::result::Result<T, Error>;
impl Error {
    pub fn io(operation: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io {
            operation: operation.into(),
            source,
        }
    }
}
