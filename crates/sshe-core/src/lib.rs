//! Local probes and bounded command execution, independent of the transport.
mod config;
mod error;
mod exec;
mod probe;
mod time;

pub use config::{MAX_PROBE_TIMEOUT_SECS, NetworkTargets, ProbeConfig};
pub use error::{Error, Result};
pub use exec::execute;
pub use probe::{host_probe, measure, network, services};
pub use time::now;
