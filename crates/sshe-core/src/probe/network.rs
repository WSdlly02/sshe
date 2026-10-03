use super::measure;
use crate::{Error, NetworkTargets};
use futures::future::{join, join_all};
use serde_json::Value;
use sshe_protocol::{ProbeKind, Record};
use std::time::Duration;
use tokio::net::{TcpStream, lookup_host};

/// DNS and TCP checks all run at once, so a group takes about one `limit`.
pub async fn network(targets: &NetworkTargets, kind: ProbeKind, limit: Duration) -> Vec<Record> {
    let dns = targets.domains.iter().map(|domain| {
        measure(kind, domain.clone(), None, "dns", limit, async move {
            let addresses: Vec<String> = lookup_host((domain.as_str(), 0))
                .await
                .map_err(|e| Error::Dns(e.to_string()))?
                .map(|a| a.ip().to_string())
                .collect();
            if addresses.is_empty() {
                return Err(Error::Dns("no addresses returned".into()));
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
    let (mut results, tcp) = join(join_all(dns), join_all(tcp)).await;
    results.extend(tcp);
    results
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn tcp_probe_records_real_success_and_failure() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        // Bound but not listening: connections are refused, and the reserved port can
        // never be picked as a source port, which made a dropped listener flaky.
        let closed = tokio::net::TcpSocket::new_v4().unwrap();
        closed.bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let targets = NetworkTargets {
            tcp: vec![
                listener.local_addr().unwrap().to_string(),
                closed.local_addr().unwrap().to_string(),
            ],
            ..Default::default()
        };
        let records = network(&targets, ProbeKind::Wan, Duration::from_secs(3)).await;
        assert!(records[0].success);
        assert!(!records[1].success && records[1].last_success_at.is_none());
        assert_eq!(
            records[1].error_kind,
            Some(sshe_protocol::FailureKind::Refused)
        );
    }
}
