use crate::{Error, Result, config::Config, transport::Dialer};
use futures::future::join_all;
use iroh::EndpointAddr;
use serde_json::json;
use sshe_protocol::{ProbeKind, Record, Request, Response};
use std::time::Duration;

pub(crate) async fn probe(
    config: &Config,
    dialer: Option<&Dialer>,
    kind: ProbeKind,
) -> Result<Vec<Record>> {
    let probe = &config.probe;
    Ok(match kind {
        ProbeKind::Host => sshe_core::host_probe(probe.timeout()).await,
        ProbeKind::Wan => sshe_core::network(&probe.wan, kind, probe.timeout()).await,
        ProbeKind::Lan => sshe_core::network(&probe.lan, kind, probe.timeout()).await,
        ProbeKind::Services => sshe_core::services(&probe.services, probe.timeout()).await,
        ProbeKind::Peers => {
            let dialer =
                dialer.ok_or_else(|| Error::Invalid("peer probe requires an endpoint".into()))?;
            join_all(config.peers.iter().map(|(alias, peer)| {
                check_peer(dialer, alias.clone(), peer.addr(), probe.peer_timeout())
            }))
            .await
        }
    })
}

/// Health over the cached connection; a cold dial counts toward `limit`.
pub(crate) async fn check_peer(
    dialer: &Dialer,
    alias: String,
    addr: EndpointAddr,
    limit: Duration,
) -> Record {
    sshe_core::measure(
        ProbeKind::Peers,
        addr.id.to_string(),
        Some(alias),
        "iroh_health",
        limit,
        async move {
            match dialer.call(addr, &Request::Health).await? {
                Response::Health(h) if h.status == "ok" => Ok(Some(json!({"version": h.version}))),
                Response::Health(h) => Err(Error::Remote(format!("unhealthy: {}", h.status))),
                Response::Error(e) => Err(Error::Remote(e)),
                _ => Err(Error::Invalid("unexpected response".into())),
            }
        },
    )
    .await
}
