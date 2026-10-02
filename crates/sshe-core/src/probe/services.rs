use super::measure;
use crate::{Error, execute};
use futures::future::join_all;
use sshe_protocol::{ProbeKind, Record};
use std::{collections::BTreeMap, time::Duration};
use tokio::net::TcpStream;
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
