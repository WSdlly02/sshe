//! CLI-side routing: in process, through the daemon, or on a temporary Endpoint.
use crate::{
    Error, Result, config,
    config::Config,
    dispatch::{Node, dispatch},
    identity,
    ipc::LocalRequest,
    layout::Layout,
    transport::{Dialer, LOCAL_TIMEOUT, endpoint, unknown_if_exec, within},
};
use sshe_protocol::{ProbeKind, Request, Response, read_frame, write_frame};
use std::{io::ErrorKind, path::Path};
use tokio::net::UnixStream;

enum Route {
    /// Local exec needs neither the daemon nor an Endpoint.
    InProcess,
    /// Prefers the daemon, falling back to local probes without an Endpoint.
    DaemonOrInProcess,
    /// Prefers the daemon's Endpoint; without a daemon, binds a temporary one.
    DaemonOrEndpoint,
    /// Local history lives only in the daemon.
    DaemonOnly,
}

fn route(target: Option<&str>, request: &Request) -> Route {
    match (target, request) {
        (Some(_), _) => Route::DaemonOrEndpoint,
        (None, Request::History { .. }) => Route::DaemonOnly,
        (
            None,
            Request::Probe {
                kind: ProbeKind::Peers,
            },
        ) => Route::DaemonOrEndpoint,
        (None, Request::Probe { .. }) => Route::DaemonOrInProcess,
        (None, _) => Route::InProcess,
    }
}

/// Runs `request` on `target` (an alias; None or "self" means this node).
pub async fn invoke(path: &Path, target: Option<String>, request: Request) -> Result<Response> {
    let config = config::read(path)?;
    let target = target.filter(|t| t != "self");
    let route = route(target.as_deref(), &request);
    if let Route::InProcess = route {
        return in_process(&config, request).await;
    }
    if let Some(response) = via_daemon(&Layout::new(path), target.clone(), &request).await? {
        return Ok(response);
    }
    match route {
        Route::DaemonOnly => Err(Error::Invalid("history requires a running daemon".into())),
        Route::DaemonOrInProcess | Route::InProcess => in_process(&config, request).await,
        Route::DaemonOrEndpoint => with_endpoint(&config, target.as_deref(), request).await,
    }
}

async fn in_process(config: &Config, request: Request) -> Result<Response> {
    let node = Node {
        config,
        id: crate::endpoint_id(config)?.to_string(),
        dialer: None,
        sampler: None,
    };
    dispatch(&node, request).await
}

/// None when no daemon is listening, or none could (socket path too long to bind).
async fn via_daemon(
    layout: &Layout,
    target: Option<String>,
    request: &Request,
) -> Result<Option<Response>> {
    if !layout.socket_fits() {
        return Ok(None);
    }
    let mut stream = match UnixStream::connect(&layout.socket).await {
        Ok(stream) => stream,
        Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::ConnectionRefused) => {
            return Ok(None);
        }
        Err(e) => return Err(e.into()),
    };
    let message = LocalRequest {
        target,
        request: request.clone(),
    };
    let reply = within(LOCAL_TIMEOUT, async {
        write_frame(&mut stream, &message).await?;
        Ok::<_, Error>(read_frame(&mut stream).await?)
    })
    .await;
    unknown_if_exec(request, reply).map(Some)
}

/// Holds the identity lock for the Endpoint's lifetime, so it never races a daemon.
async fn with_endpoint(
    config: &Config,
    target: Option<&str>,
    request: Request,
) -> Result<Response> {
    let peer = target.map(|alias| config.peer(alias)).transpose()?;
    let _lock = identity::lock(&config.identity)?;
    // A temporary endpoint never needs a fixed port; that is the daemon's.
    let ep = endpoint(identity::load_key(&config.identity)?, None).await?;
    let dialer = Dialer::new(ep.clone(), config.peers.values().map(|p| p.id));
    let result = match peer {
        Some(peer) => dialer.call(peer.addr(), &request).await,
        None => {
            let node = Node {
                config,
                id: ep.id().to_string(),
                dialer: Some(&dialer),
                sampler: None,
            };
            dispatch(&node, request).await
        }
    };
    ep.close().await;
    result
}
