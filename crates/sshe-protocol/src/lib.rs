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
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const ALPN: &[u8] = b"sshe/1";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const MAX_FRAME: usize = 4 * 1024 * 1024;
pub const OUTPUT_LIMIT: usize = 256 * 1024;
pub const MAX_EXEC_SECONDS: u64 = 60;
/// Upper bound on records returned by one history query; keeps replies under MAX_FRAME.
pub const MAX_HISTORY_QUERY: usize = 1000;

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

pub async fn read_frame<R: AsyncRead + Unpin, T: DeserializeOwned>(r: &mut R) -> Result<T> {
    let len = r.read_u32().await? as usize;
    if len > MAX_FRAME {
        return Err(Error::FrameTooLarge);
    }
    let mut bytes = vec![0; len];
    r.read_exact(&mut bytes).await?;
    Ok(serde_json::from_slice(&bytes)?)
}
pub async fn write_frame<W: AsyncWrite + Unpin, T: Serialize>(w: &mut W, value: &T) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > MAX_FRAME {
        return Err(Error::FrameTooLarge);
    }
    w.write_u32(bytes.len() as u32).await?;
    w.write_all(&bytes).await?;
    w.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn refuses_oversized_frame_before_payload() {
        let mut bytes = ((MAX_FRAME + 1) as u32).to_be_bytes().as_slice().to_vec();
        assert!(
            read_frame::<_, Request>(&mut bytes.as_slice())
                .await
                .is_err()
        );
        bytes.clear();
    }
}
