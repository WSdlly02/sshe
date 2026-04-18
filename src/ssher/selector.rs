use crate::ssher::config::{FinalHostConfig, SelectionMode};
use anyhow::{Context, Result, anyhow};
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
    let mut errors: Vec<anyhow::Error> = Vec::new();

    for endpoint in host.endpoints.iter().cloned() {
        tasks.push(probe_endpoint(endpoint, port, timeout, selection_mode));
    }

    while let Some(result) = tasks.next().await {
        match result {
            Ok(best) => return Ok(best),
            Err(err) => errors.push(err),
        }
    }

    if errors.is_empty() {
        Err(anyhow!("no reachable endpoint found"))
    } else {
        Err(anyhow!(
            "{}",
            errors
                .into_iter()
                .map(|err| err.to_string())
                .collect::<Vec<String>>()
                .join("; ")
        ))
    }
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
    .map_err(|err| anyhow!("{endpoint}:{port} -> {err}"))?;

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
        .map_err(|_| anyhow!("connect timeout"))?
        .context("connect failed")?;

    Ok(start.elapsed().as_millis())
}

async fn resolve_socket_addr(host: &str, port: u16) -> Result<SocketAddr> {
    let addr_text = format!("{host}:{port}");
    let mut addrs = lookup_host(&addr_text)
        .await
        .with_context(|| format!("resolve failed for {addr_text}"))?;

    addrs
        .next()
        .ok_or_else(|| anyhow!("no socket address resolved"))
}

async fn probe_icmp(host: &str, timeout: Duration) -> Result<u128> {
    let timeout_sec = timeout.as_secs().max(1);
    let output = Command::new("ping")
        .args(["-c", "1", "-W", &timeout_sec.to_string(), host])
        .output()
        .await
        .with_context(|| format!("failed to execute ping for {host}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let message = stderr.trim();
        if message.is_empty() {
            return Err(anyhow!("ping failed"));
        }
        return Err(anyhow!("ping failed: {message}"));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_ping_latency(&stdout).ok_or_else(|| anyhow!("unable to parse ping latency"))
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
