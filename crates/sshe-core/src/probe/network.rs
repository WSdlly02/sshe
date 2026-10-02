use super::measure;
use crate::{Error, NetworkTargets};
use futures::future::join_all;
use serde_json::Value;
use sshe_protocol::{ProbeKind, Record};
use std::time::Duration;
use tokio::net::{TcpStream, lookup_host};
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

#[cfg(test)]
mod tests {
    use super::*;
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
}
