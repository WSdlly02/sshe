use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    /// Liveness for peer probes; not exposed by the CLI.
    Health,
    Probe {
        kind: ProbeKind,
    },
    /// Read recorded probe results; never triggers a probe.
    History {
        kind: Option<ProbeKind>,
        /// Matches a record's target or label exactly, as named on the queried node.
        about: Option<String>,
        limit: usize,
    },
    Exec {
        program: String,
        args: Vec<String>,
        timeout_secs: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeKind {
    Host,
    Lan,
    Wan,
    Services,
    Peers,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum Response {
    Health(Health),
    Probe(ProbeReport),
    History(HistoryReport),
    Exec(ExecResult),
    Error(String),
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Health {
    pub endpoint_id: String,
    pub status: String,
    pub version: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProbeReport {
    pub observer: String,
    pub kind: ProbeKind,
    pub records: Vec<Record>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HistoryReport {
    pub observer: String,
    pub queried_at: u64,
    /// Oldest first.
    pub records: Vec<Record>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ExecResult {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub duration_ms: u64,
}
/// Why a check failed, so history and logs separate network faults from refusals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    /// No answer within the check's limit.
    Timeout,
    /// The peer's address could not be found: address lookup or relay unavailable.
    NoAddress,
    /// The remote actively refused: TCP reset, a refused QUIC handshake or a full daemon.
    Refused,
    /// No route to the host or network.
    Unreachable,
    /// The peer does not list this node in its `[peers]`.
    Unauthorized,
    /// An established connection or stream was lost.
    ConnectionLost,
    /// DNS resolution failed or returned nothing.
    Dns,
    /// The remote answered but reported an error or an unhealthy state.
    Remote,
    /// Anything else, including local errors.
    Other,
}

impl ProbeKind {
    /// Every kind, so per-kind state can be built up front.
    pub const ALL: [ProbeKind; 5] = [
        ProbeKind::Host,
        ProbeKind::Lan,
        ProbeKind::Wan,
        ProbeKind::Services,
        ProbeKind::Peers,
    ];

    /// The wire name, also used in logs.
    pub fn as_str(self) -> &'static str {
        match self {
            ProbeKind::Host => "host",
            ProbeKind::Lan => "lan",
            ProbeKind::Wan => "wan",
            ProbeKind::Services => "services",
            ProbeKind::Peers => "peers",
        }
    }
}

// Exhaustive on purpose: a new kind fails to compile here until it is added to `ALL`.
const _: fn(ProbeKind) = |kind| match kind {
    ProbeKind::Host | ProbeKind::Lan | ProbeKind::Wan | ProbeKind::Services | ProbeKind::Peers => {}
};

impl FailureKind {
    /// The wire name, also used in logs.
    pub fn as_str(self) -> &'static str {
        match self {
            FailureKind::Timeout => "timeout",
            FailureKind::NoAddress => "no_address",
            FailureKind::Refused => "refused",
            FailureKind::Unreachable => "unreachable",
            FailureKind::Unauthorized => "unauthorized",
            FailureKind::ConnectionLost => "connection_lost",
            FailureKind::Dns => "dns",
            FailureKind::Remote => "remote",
            FailureKind::Other => "other",
        }
    }
}

/// One check result, as seen by the report's observer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    pub kind: ProbeKind,
    /// Stable identity: domain, host:port, service spec, EndpointId or "host".
    pub target: String,
    /// Observer-local name such as a peer alias or service name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub method: String,
    pub observed_at: u64,
    pub duration_ms: u64,
    pub success: bool,
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<FailureKind>,
    pub last_success_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn log_names_match_wire_names() {
        for kind in [ProbeKind::Host, ProbeKind::Peers] {
            assert_eq!(serde_json::to_value(kind).unwrap(), kind.as_str());
        }
        for kind in [FailureKind::NoAddress, FailureKind::ConnectionLost] {
            assert_eq!(serde_json::to_value(kind).unwrap(), kind.as_str());
        }
    }
}
