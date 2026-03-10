use crate::config::{FinalHostConfig, SelectionMode};
use std::net::{TcpStream, ToSocketAddrs};
use std::process::Command;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct ProbeResult {
    pub endpoint: String,
    pub latency_ms: u128,
}

pub fn select_best_endpoint(host: &FinalHostConfig) -> Result<ProbeResult, String> {
    let port = host.port;
    let timeout = Duration::from_millis(host.probe_timeout_ms);

    let mut best: Option<ProbeResult> = None;
    let mut last_err: Option<String> = None;

    for endpoint in &host.endpoints {
        let probe = match host.selection_mode {
            SelectionMode::LowestTcpLatency => probe_tcp(endpoint, port, timeout),
            SelectionMode::LowestIcmpLatency => probe_icmp(endpoint, timeout),
        };

        match probe {
            Ok(latency_ms) => {
                let current = ProbeResult {
                    endpoint: endpoint.clone(),
                    latency_ms,
                };

                match &best {
                    Some(prev) if prev.latency_ms <= current.latency_ms => {}
                    _ => best = Some(current),
                }
            }
            Err(err) => {
                last_err = Some(format!("{endpoint}:{port} -> {err}"));
            }
        }
    }

    best.ok_or_else(|| last_err.unwrap_or_else(|| "no reachable endpoint found".to_string()))
}

fn probe_tcp(host: &str, port: u16, timeout: Duration) -> Result<u128, String> {
    let addr_text = format!("{host}:{port}");
    let mut addrs = addr_text
        .to_socket_addrs()
        .map_err(|e| format!("resolve failed: {e}"))?;

    let addr = addrs
        .next()
        .ok_or_else(|| "no socket address resolved".to_string())?;

    let start = Instant::now();
    TcpStream::connect_timeout(&addr, timeout).map_err(|e| format!("connect failed: {e}"))?;
    let elapsed = start.elapsed().as_millis();

    Ok(elapsed)
}

fn probe_icmp(host: &str, timeout: Duration) -> Result<u128, String> {
    let timeout_sec = timeout.as_secs().max(1);
    let output = Command::new("ping")
        .args(["-c", "1", "-W", &timeout_sec.to_string(), host])
        .output()
        .map_err(|e| format!("failed to execute ping: {e}"))?;

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
