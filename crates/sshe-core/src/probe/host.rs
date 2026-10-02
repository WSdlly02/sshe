use super::measure;
use crate::{Error, Result};
use nix::sys::statvfs::statvfs;
use serde::Serialize;
use sshe_protocol::{ProbeKind, Record};
use std::{path::Path, time::Duration};

#[derive(Debug, Serialize)]
struct HostInfo {
    hostname: String,
    uptime_seconds: f64,
    loadavg: LoadAvg,
    memory: Memory,
    root_disk: Disk,
}

#[derive(Debug, PartialEq, Serialize)]
struct LoadAvg {
    #[serde(rename = "1m")]
    one: f64,
    #[serde(rename = "5m")]
    five: f64,
    #[serde(rename = "15m")]
    fifteen: f64,
}

#[derive(Debug, PartialEq, Serialize)]
struct Memory {
    total_bytes: u64,
    available_bytes: u64,
    swap_total_bytes: u64,
    swap_free_bytes: u64,
}

#[derive(Debug, Serialize)]
struct Disk {
    total_bytes: u64,
    available_bytes: u64,
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

async fn host() -> Result<HostInfo> {
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

#[cfg(test)]
mod tests {
    use super::*;
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
