//! Local probes and bounded command execution, independent of the transport.
mod config;
mod error;
mod exec;
mod probe;
mod time;

pub use config::{MAX_PEER_TIMEOUT_SECS, MAX_PROBE_TIMEOUT_SECS, NetworkTargets, ProbeConfig};
pub use error::{Classify, Error, Result, io_failure_kind};
pub use exec::execute;
pub use probe::{host_probe, measure, network, services};
pub use time::now;
