//! Serves requests from whitelisted peers and from the local CLI.
use crate::{
    Error, Result,
    dispatch::{Node, dispatch, response},
    error::{UNAUTHORIZED, transport},
    ipc::LocalRequest,
    transport::{IO_TIMEOUT, within},
};
use futures::{StreamExt, stream::FuturesUnordered};
use iroh::endpoint::{Connection, RecvStream, SendStream};
use sshe_protocol::{Request, Response, read_frame, write_frame};
use tokio::{net::UnixStream, sync::Semaphore};

const BUSY: &str = "daemon busy; request not executed";

/// Drops peers outside `[peers]`; otherwise answers each stream as one RPC until
/// the peer closes the connection. Streams run concurrently, so a long exec does
/// not delay a health check on the same connection.
pub(crate) async fn serve_peer(conn: Connection, node: &Node<'_>, permits: &Semaphore) {
    let remote = conn.remote_id();
    let Some(alias) = node
        .config
        .peers
        .iter()
        .find_map(|(alias, peer)| (peer.id == remote).then_some(alias))
    else {
        tracing::info!(%remote, "rejected connection from a peer not in [peers]");
        conn.close(UNAUTHORIZED, b"unauthorized");
        return;
    };
    tracing::debug!(peer = %alias, "peer connected");
    let mut inflight = FuturesUnordered::new();
    loop {
        tokio::select! {
            accepted = conn.accept_bi() => match accepted {
                Ok((send, recv)) => inflight.push(serve_stream(send, recv, node, permits)),
                Err(reason) => {
                    tracing::debug!(peer = %alias, %reason, "peer connection closed");
                    break;
                }
            },
            Some(result) = inflight.next(), if !inflight.is_empty() => {
                if let Err(error) = result {
                    tracing::warn!(peer = %alias, %error, "peer request failed");
                }
            }
        }
    }
    // Requests already received still complete; their replies may have nowhere to go.
    while inflight.next().await.is_some() {}
}

async fn serve_stream(
    mut send: SendStream,
    mut recv: RecvStream,
    node: &Node<'_>,
    permits: &Semaphore,
) -> Result<()> {
    let request: Request = within(IO_TIMEOUT, read_frame(&mut recv)).await?;
    let reply = match permits.try_acquire() {
        Ok(_permit) => response(dispatch(node, request).await),
        Err(_) => Response::Error(BUSY.into()),
    };
    within(IO_TIMEOUT, write_frame(&mut send, &reply)).await?;
    send.finish().map_err(transport)
}

/// Answers even when saturated, so a refused exec is not reported as outcome unknown.
pub(crate) async fn serve_local(
    mut stream: UnixStream,
    node: &Node<'_>,
    permits: &Semaphore,
) -> Result<()> {
    let message: LocalRequest = within(IO_TIMEOUT, read_frame(&mut stream)).await?;
    let result = match permits.try_acquire() {
        Ok(_permit) => match message.target {
            Some(alias) => forward(node, &alias, &message.request).await,
            None => dispatch(node, message.request).await,
        },
        Err(_) => Err(Error::Invalid(BUSY.into())),
    };
    within(IO_TIMEOUT, write_frame(&mut stream, &response(result))).await
}

async fn forward(node: &Node<'_>, alias: &str, request: &Request) -> Result<Response> {
    let peer = node.config.peer(alias)?;
    let dialer = node
        .dialer
        .ok_or_else(|| Error::Invalid("forwarding requires an endpoint".into()))?;
    dialer.call(peer.addr(), request).await
}
