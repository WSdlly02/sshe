//! Wire messages and bounded JSON framing for sshe RPC.
mod codec;
mod error;
mod limits;
mod message;

pub use codec::{read_frame, write_frame};
pub use error::{Error, Result};
pub use limits::{ALPN, MAX_EXEC_SECONDS, MAX_FRAME, MAX_HISTORY_QUERY, OUTPUT_LIMIT, VERSION};
pub use message::{
    ExecResult, FailureKind, Health, HistoryReport, ProbeKind, ProbeReport, Record, Request,
    Response,
};
