use crate::ssher::config::{FinalHostConfig, SelectionMode};
use crate::{Error, Result};
use futures::stream::{FuturesUnordered, StreamExt};
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use tokio::net::{TcpStream, lookup_host};
use tokio::process::Command;
use tokio::time;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeSource {
    Cache,
    Probe,
}

#[derive(Debug, Clone)]
pub struct ProbeResult {
    pub endpoint: String,
    pub latency_ms: u128,
    pub source: ProbeSource,
}

pub async fn select_best_endpoint(host: &FinalHostConfig, port: u16) -> Result<ProbeResult> {
    let timeout = Duration::from_millis(host.probe_timeout_ms);
    let selection_mode = host.selection_mode;
    let mut tasks = FuturesUnordered::new();
    let mut errors: Vec<Error> = Vec::new();

    for endpoint in host.endpoints.iter().cloned() {
        tasks.push(probe_endpoint(endpoint, port, timeout, selection_mode));
    }

    while let Some(result) = tasks.next().await {
        match result {
            Ok(best) => return Ok(best),
            Err(err) => errors.push(err),
        }
    }

    Err(Error::Unreachable(errors))
}

async fn probe_endpoint(
    endpoint: String,
    port: u16,
    timeout: Duration,
    selection_mode: SelectionMode,
) -> Result<ProbeResult> {
    let latency_ms = match selection_mode {
        SelectionMode::LowestTcpLatency => probe_tcp(&endpoint, port, timeout).await,
        SelectionMode::LowestIcmpLatency => probe_icmp(&endpoint, timeout).await,
    }
    .map_err(|source| Error::Probe {
        endpoint: endpoint.clone(),
        port,
        source: Box::new(source),
    })?;

    Ok(ProbeResult {
        endpoint,
        latency_ms,
        source: ProbeSource::Probe,
    })
}

async fn probe_tcp(host: &str, port: u16, timeout: Duration) -> Result<u128> {
    let addr = resolve_socket_addr(host, port).await?;
    let start = Instant::now();

    time::timeout(timeout, TcpStream::connect(addr))
        .await
        .map_err(|_| Error::Invalid("connect timeout".into()))?
        .map_err(|e| Error::io("connect failed", e))?;

    Ok(start.elapsed().as_millis())
}

async fn resolve_socket_addr(host: &str, port: u16) -> Result<SocketAddr> {
    let addr_text = format!("{host}:{port}");
    let mut addrs = lookup_host(&addr_text)
        .await
        .map_err(|e| Error::io(format!("resolve failed for {addr_text}"), e))?;

    addrs
        .next()
        .ok_or_else(|| Error::Invalid("no socket address resolved".to_string()))
}

async fn probe_icmp(host: &str, timeout: Duration) -> Result<u128> {
    let timeout_sec = timeout.as_secs().max(1);
    let output = Command::new("ping")
        .args(["-c", "1", "-W", &timeout_sec.to_string(), host])
        .output()
        .await
        .map_err(|e| Error::io(format!("failed to execute ping for {host}"), e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let message = stderr.trim();
        if message.is_empty() {
            return Err(Error::Invalid("ping failed".to_string()));
        }
        return Err(Error::Invalid(format!("ping failed: {message}")));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_ping_latency(&stdout)
        .ok_or_else(|| Error::Invalid("unable to parse ping latency".to_string()))
}

fn parse_ping_latency(stdout: &str) -> Option<u128> {
    let marker = "time=";
    let start = stdout.find(marker)? + marker.len();
    let tail = &stdout[start..];
    let end = tail.find(" ms").or_else(|| tail.find("ms"))?;
    let value = tail[..end].trim().parse::<f64>().ok()?;
    Some(value.round() as u128)
}

#[cfg(test)]
mod tests {
    use super::parse_ping_latency;

    #[test]
    fn parses_ping_output() {
        let sample = "64 bytes from 1.1.1.1: icmp_seq=1 ttl=57 time=12.7 ms\n\n--- 1.1.1.1 ping statistics ---";
        assert_eq!(parse_ping_latency(sample), Some(13));
    }
}
