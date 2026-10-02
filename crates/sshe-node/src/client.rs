//! CLI-side routing: in process, through the daemon, or on a temporary Endpoint.
use crate::{
    Error, Result, config,
    config::Config,
    dispatch::{Node, dispatch},
    identity,
    ipc::LocalRequest,
    layout::Layout,
    transport::{LOCAL_TIMEOUT, call, endpoint, unknown_if_exec, within},
};
use sshe_protocol::{ProbeKind, Request, Response, read_frame, write_frame};
use std::{io::ErrorKind, path::Path};
use tokio::net::UnixStream;

enum Route {
    /// Local probes and exec need neither the daemon nor an Endpoint.
    InProcess,
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
        _ => with_endpoint(&config, target.as_deref(), request).await,
    }
}

async fn in_process(config: &Config, request: Request) -> Result<Response> {
    let node = Node {
        config,
        id: crate::endpoint_id(config)?.to_string(),
        endpoint: None,
        history: None,
    };
    dispatch(&node, request).await
}

/// None when no daemon is listening.
async fn via_daemon(
    layout: &Layout,
    target: Option<String>,
    request: &Request,
) -> Result<Option<Response>> {
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
    let ep = endpoint(identity::load_key(&config.identity)?).await?;
    let result = match peer {
        Some(peer) => call(&ep, peer.id, &request).await,
        None => {
            let node = Node {
                config,
                id: ep.id().to_string(),
                endpoint: Some(&ep),
                history: None,
            };
            dispatch(&node, request).await
        }
    };
    ep.close().await;
    result
}
