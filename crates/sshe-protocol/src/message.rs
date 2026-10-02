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
    pub last_success_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}
