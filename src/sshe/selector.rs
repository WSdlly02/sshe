use crate::sshe::config::{FinalHostConfig, SelectionMode};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::{TcpStream, lookup_host};
use tokio::process::Command;
use tokio::sync::Semaphore;
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

pub async fn select_best_endpoint(host: &FinalHostConfig) -> Result<ProbeResult, String> {
    let timeout = Duration::from_millis(host.probe_timeout_ms);
    let semaphore = Arc::new(Semaphore::new(host.probe_concurrency.max(1)));
    let mut tasks = Vec::with_capacity(host.endpoints.len());

    for endpoint in host.endpoints.iter().cloned() {
        let semaphore = Arc::clone(&semaphore);
        let selection_mode = host.selection_mode;
        let port = host.port;

        tasks.push(tokio::spawn(async move {
            let _permit = semaphore
                .acquire_owned()
                .await
                .map_err(|_| format!("{endpoint}:{port} -> semaphore closed"))?;

            let latency_ms = match selection_mode {
                SelectionMode::LowestTcpLatency => probe_tcp(&endpoint, port, timeout).await,
                SelectionMode::LowestIcmpLatency => probe_icmp(&endpoint, timeout).await,
            }
            .map_err(|err| format!("{endpoint}:{port} -> {err}"))?;

            Ok::<ProbeResult, String>(ProbeResult {
                endpoint,
                latency_ms,
                source: ProbeSource::Probe,
            })
        }));
    }

    let mut best: Option<ProbeResult> = None;
    let mut errors = Vec::new();

    for task in tasks {
        match task.await {
            Ok(Ok(result)) => match &best {
                Some(current) if current.latency_ms <= result.latency_ms => {}
                _ => best = Some(result),
            },
            Ok(Err(err)) => errors.push(err),
            Err(err) => errors.push(format!("probe task failed: {err}")),
        }
    }

    best.ok_or_else(|| {
        if errors.is_empty() {
            "no reachable endpoint found".to_string()
        } else {
            errors.join("; ")
        }
    })
}

async fn probe_tcp(host: &str, port: u16, timeout: Duration) -> Result<u128, String> {
    let addr = resolve_socket_addr(host, port).await?;
    let start = Instant::now();

    time::timeout(timeout, TcpStream::connect(addr))
        .await
        .map_err(|_| "connect timeout".to_string())?
        .map_err(|err| format!("connect failed: {err}"))?;

    Ok(start.elapsed().as_millis())
}

async fn resolve_socket_addr(host: &str, port: u16) -> Result<SocketAddr, String> {
    let addr_text = format!("{host}:{port}");
    let mut addrs = lookup_host(&addr_text)
        .await
        .map_err(|err| format!("resolve failed: {err}"))?;

    addrs
        .next()
        .ok_or_else(|| "no socket address resolved".to_string())
}

async fn probe_icmp(host: &str, timeout: Duration) -> Result<u128, String> {
    let timeout_sec = timeout.as_secs().max(1);
    let output = Command::new("ping")
        .args(["-c", "1", "-W", &timeout_sec.to_string(), host])
        .output()
        .await
        .map_err(|err| format!("failed to execute ping: {err}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let message = stderr.trim();
        if message.is_empty() {
            return Err("ping failed".to_string());
        }
        return Err(format!("ping failed: {message}"));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_ping_latency(&stdout).ok_or_else(|| "unable to parse ping latency".to_string())
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
