use crate::{
    Error, Result,
    config::Config,
    transport::{CONNECT_TIMEOUT, call},
};
use futures::future::join_all;
use iroh::{Endpoint, EndpointAddr};
use serde_json::json;
use sshe_protocol::{ProbeKind, Record, Request, Response};
use std::time::Duration;

/// Relay paths can exceed the local probe budget.
const PEER_PROBE_TIMEOUT: Duration = CONNECT_TIMEOUT;

pub(crate) async fn probe(
    config: &Config,
    endpoint: Option<&Endpoint>,
    kind: ProbeKind,
) -> Result<Vec<Record>> {
    let probe = &config.probe;
    Ok(match kind {
        ProbeKind::Host => sshe_core::host_probe(probe.timeout()).await,
        ProbeKind::Wan => sshe_core::network(&probe.wan, kind, probe.timeout()).await,
        ProbeKind::Lan => sshe_core::network(&probe.lan, kind, probe.timeout()).await,
        ProbeKind::Services => sshe_core::services(&probe.services, probe.timeout()).await,
        ProbeKind::Peers => {
            let ep =
                endpoint.ok_or_else(|| Error::Invalid("peer probe requires an endpoint".into()))?;
            join_all(
                config.peers.iter().map(|(alias, peer)| {
                    check_peer(ep, peer.id.to_string(), alias.clone(), peer.id)
                }),
            )
            .await
        }
    })
}
pub(crate) async fn check_peer(
    ep: &Endpoint,
    target: String,
    alias: String,
    address: impl Into<EndpointAddr>,
) -> Record {
    let address = address.into();
    sshe_core::measure(
        ProbeKind::Peers,
        target,
        Some(alias),
        "iroh_health",
        PEER_PROBE_TIMEOUT,
        async move {
            match call(ep, address, &Request::Health).await? {
                Response::Health(h) if h.status == "ok" => Ok(Some(json!({"version": h.version}))),
                Response::Health(h) => Err(Error::Invalid(format!("unhealthy: {}", h.status))),
                Response::Error(e) => Err(Error::Invalid(e)),
                _ => Err(Error::Invalid("unexpected response".into())),
            }
        },
    )
    .await
}
