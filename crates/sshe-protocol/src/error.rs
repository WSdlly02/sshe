use crate::MAX_FRAME;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("frame exceeds {MAX_FRAME} bytes")]
    FrameTooLarge,
    #[error("frame I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid message: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
