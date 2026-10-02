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
    #[error("{0}")]
    Invalid(String),
}
pub type Result<T> = std::result::Result<T, Error>;
use futures::future::join_all;
use nix::{
    sys::{
        signal::{Signal, killpg},
        statvfs::statvfs,
    },
    unistd::Pid,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sshe_protocol::{ExecResult, MAX_EXEC_SECONDS, OUTPUT_LIMIT, ProbeKind, Record};
use std::os::unix::process::ExitStatusExt;
use std::{
    collections::BTreeMap,
    fmt::Display,
    path::Path,
    process::Stdio,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    net::{TcpStream, lookup_host},
    process::Command,
    time::timeout,
};

pub const MAX_PROBE_TIMEOUT_SECS: u64 = 10;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProbeConfig {
    /// Per-check limit for DNS, TCP and service checks; checks in a group run concurrently.
    pub timeout_secs: u64,
    pub wan: NetworkTargets,
    pub lan: NetworkTargets,
    pub services: BTreeMap<String, String>,
}
impl Default for ProbeConfig {
    fn default() -> Self {
        Self {
            timeout_secs: 3,
            wan: Default::default(),
            lan: Default::default(),
            services: Default::default(),
        }
    }
}
impl ProbeConfig {
    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_secs)
    }
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NetworkTargets {
    pub domains: Vec<String>,
    /// Explicit host:port targets; IP addresses avoid DNS dependence.
    pub tcp: Vec<String>,
}
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[derive(Debug, Serialize)]
pub struct HostInfo {
    pub hostname: String,
    pub uptime_seconds: f64,
    pub loadavg: LoadAvg,
    pub memory: Memory,
    pub root_disk: Disk,
}
#[derive(Debug, PartialEq, Serialize)]
pub struct LoadAvg {
    #[serde(rename = "1m")]
    pub one: f64,
    #[serde(rename = "5m")]
    pub five: f64,
    #[serde(rename = "15m")]
    pub fifteen: f64,
}
#[derive(Debug, PartialEq, Serialize)]
pub struct Memory {
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub swap_total_bytes: u64,
    pub swap_free_bytes: u64,
}
#[derive(Debug, Serialize)]
pub struct Disk {
    pub total_bytes: u64,
    pub available_bytes: u64,
}

fn invalid(what: &str) -> Error {
    Error::Invalid(format!("unexpected {what} format"))
}
fn parse_uptime(text: &str) -> Result<f64> {
    text.split_whitespace()
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| invalid("/proc/uptime"))
}
fn parse_loadavg(text: &str) -> Result<LoadAvg> {
    let mut fields = text.split_whitespace().map(str::parse::<f64>);
    let mut next = || {
        fields
            .next()
            .and_then(|v| v.ok())
            .ok_or_else(|| invalid("/proc/loadavg"))
    };
    Ok(LoadAvg {
        one: next()?,
        five: next()?,
        fifteen: next()?,
    })
}
fn parse_meminfo(text: &str) -> Result<Memory> {
    let field = |name: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(name)?.strip_prefix(':'))
            .and_then(|v| v.trim().strip_suffix("kB")?.trim().parse::<u64>().ok())
            .map(|kib| kib * 1024)
            .ok_or_else(|| Error::Invalid(format!("/proc/meminfo lacks {name}")))
    };
    Ok(Memory {
        total_bytes: field("MemTotal")?,
        available_bytes: field("MemAvailable")?,
        swap_total_bytes: field("SwapTotal")?,
        swap_free_bytes: field("SwapFree")?,
    })
}
pub async fn host() -> Result<HostInfo> {
    let hostname = tokio::fs::read_to_string("/proc/sys/kernel/hostname").await?;
    let uptime = tokio::fs::read_to_string("/proc/uptime").await?;
    let load = tokio::fs::read_to_string("/proc/loadavg").await?;
    let memory = tokio::fs::read_to_string("/proc/meminfo").await?;
    let disk = statvfs(Path::new("/"))?;
    Ok(HostInfo {
        hostname: hostname.trim().into(),
        uptime_seconds: parse_uptime(&uptime)?,
        loadavg: parse_loadavg(&load)?,
        memory: parse_meminfo(&memory)?,
        root_disk: Disk {
            total_bytes: disk.blocks() * disk.fragment_size(),
            available_bytes: disk.blocks_available() * disk.fragment_size(),
        },
    })
}
pub async fn host_probe(limit: Duration) -> Vec<Record> {
    vec![
        measure(
            ProbeKind::Host,
            "host".into(),
            None,
            "procfs",
            limit,
            async { Ok::<_, Error>(Some(serde_json::to_value(host().await?)?)) },
        )
        .await,
    ]
}

pub async fn network(targets: &NetworkTargets, kind: ProbeKind, limit: Duration) -> Vec<Record> {
    let dns = targets.domains.iter().map(|domain| {
        measure(kind, domain.clone(), None, "dns", limit, async move {
            let addresses: Vec<String> = lookup_host((domain.as_str(), 0))
                .await?
                .map(|a| a.ip().to_string())
                .collect();
            if addresses.is_empty() {
                return Err(Error::Invalid("no addresses returned".into()));
            }
            Ok(Some(Value::from(addresses)))
        })
    });
    let tcp = targets.tcp.iter().map(|target| {
        measure(kind, target.clone(), None, "tcp", limit, async move {
            TcpStream::connect(target).await?;
            Ok::<_, Error>(None)
        })
    });
    let mut results = join_all(dns).await;
    results.extend(join_all(tcp).await);
    results
}
pub async fn services(services: &BTreeMap<String, String>, limit: Duration) -> Vec<Record> {
    join_all(services.iter().map(|(name, target)| {
        measure(
            ProbeKind::Services,
            target.clone(),
            Some(name.clone()),
            "service",
            limit,
            async move {
                if let Some(address) = target.strip_prefix("tcp:") {
                    TcpStream::connect(address).await?;
                } else if let Some(unit) = target.strip_prefix("systemd:") {
                    let output = execute(
                        "systemctl",
                        &["is-active".into(), "--".into(), unit.into()],
                        limit.as_secs().max(1),
                    )
                    .await?;
                    if output.exit_code != Some(0) {
                        return Err(Error::Invalid(format!(
                            "unit inactive: {}",
                            String::from_utf8_lossy(&output.stdout).trim()
                        )));
                    }
                } else {
                    return Err(Error::Invalid(format!(
                        "unsupported service check: {target}"
                    )));
                }
                Ok(None)
            },
        )
    }))
    .await
}

/// Runs one bounded check; failure and timeout become part of the record.
pub async fn measure<E: Display>(
    kind: ProbeKind,
    target: String,
    label: Option<String>,
    method: &str,
    limit: Duration,
    f: impl Future<Output = std::result::Result<Option<Value>, E>>,
) -> Record {
    let start = Instant::now();
    let (error, data) = match timeout(limit, f).await {
        Ok(Ok(data)) => (None, data),
        Ok(Err(e)) => (Some(format!("{e:#}")), None),
        Err(_) => (Some("timeout".into()), None),
    };
    let at = now();
    Record {
        kind,
        target,
        label,
        method: method.into(),
        observed_at: at,
        duration_ms: start.elapsed().as_millis() as u64,
        success: error.is_none(),
        last_success_at: error.is_none().then_some(at),
        error,
        data,
    }
}

struct ProcessGroup(u32);
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        let _ = killpg(Pid::from_raw(self.0 as i32), Signal::SIGKILL);
    }
}
async fn read_output(reader: impl AsyncRead + Unpin) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .take((OUTPUT_LIMIT + 1) as u64)
        .read_to_end(&mut bytes)
        .await?;
    if bytes.len() > OUTPUT_LIMIT {
        return Err(Error::OutputLimit);
    }
    Ok(bytes)
}
/// No shell, no replay. Dropping this future also kills the process group.
pub async fn execute(program: &str, args: &[String], timeout_secs: u64) -> Result<ExecResult> {
    if !(1..=MAX_EXEC_SECONDS).contains(&timeout_secs) {
        return Err(Error::InvalidTimeout);
    }
    let start = Instant::now();
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true)
        .spawn()?;
    let _group = ProcessGroup(child.id().expect("newly spawned child has a pid"));
    let stdout = child.stdout.take().expect("stdout is piped");
    let stderr = child.stderr.take().expect("stderr is piped");
    let result = timeout(Duration::from_secs(timeout_secs), async {
        tokio::try_join!(read_output(stdout), read_output(stderr), async {
            Ok::<_, Error>(child.wait().await?)
        })
    })
    .await
    .map_err(|_| Error::Timeout)??;
    Ok(ExecResult {
        stdout: result.0,
        stderr: result.1,
        exit_code: result.2.code(),
        signal: result.2.signal(),
        duration_ms: start.elapsed().as_millis() as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn argv_is_literal_and_exit_status_preserved() {
        let result = execute("printf", &["%s".into(), "$(false); hello world".into()], 2)
            .await
            .unwrap();
        assert_eq!(result.stdout, b"$(false); hello world");
        assert_eq!(result.exit_code, Some(0));
        assert_eq!(
            execute("sh", &["-c".into(), "exit 7".into()], 2)
                .await
                .unwrap()
                .exit_code,
            Some(7)
        );
    }
    #[tokio::test]
    async fn output_and_runtime_are_bounded() {
        assert!(
            execute("yes", &[], 2)
                .await
                .unwrap_err()
                .to_string()
                .contains("output exceeded")
        );
        assert!(
            execute("sleep", &["5".into()], 1)
                .await
                .unwrap_err()
                .to_string()
                .contains("timed out")
        );
    }
    #[tokio::test]
    async fn tcp_probe_records_real_success_and_failure() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let targets = NetworkTargets {
            tcp: vec![listener.local_addr().unwrap().to_string()],
            ..Default::default()
        };
        let limit = Duration::from_secs(3);
        assert!(network(&targets, ProbeKind::Wan, limit).await[0].success);
        drop(listener);
        let failed = &network(&targets, ProbeKind::Wan, limit).await[0];
        assert!(!failed.success && failed.last_success_at.is_none());
    }
    #[test]
    fn procfs_text_becomes_structured_fields() {
        let meminfo = "MemTotal:       16000 kB\nMemFree:  1 kB\nMemAvailable:    8000 kB\nSwapTotal:  0 kB\nSwapFree:  0 kB\n";
        assert_eq!(
            parse_meminfo(meminfo).unwrap(),
            Memory {
                total_bytes: 16000 * 1024,
                available_bytes: 8000 * 1024,
                swap_total_bytes: 0,
                swap_free_bytes: 0,
            }
        );
        assert!(parse_meminfo("MemTotalX: 1 kB\n").is_err());
        assert_eq!(
            parse_loadavg("0.12 0.08 0.05 1/234 5678\n").unwrap(),
            LoadAvg {
                one: 0.12,
                five: 0.08,
                fifteen: 0.05
            }
        );
        assert_eq!(parse_uptime("12345.67 999.00\n").unwrap(), 12345.67);
    }
    #[tokio::test]
    async fn host_probe_reads_this_machine() {
        let record = &host_probe(Duration::from_secs(3)).await[0];
        assert!(record.success, "{:?}", record.error);
        assert!(record.data.as_ref().unwrap()["memory"]["total_bytes"].is_u64());
    }
}
