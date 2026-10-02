//! Serves one request from a whitelisted peer or from the local CLI.
use crate::{
    Error, Result,
    dispatch::{Node, dispatch, response},
    error::transport,
    ipc::LocalRequest,
    transport::{CONNECT_TIMEOUT, call, within},
};
use iroh::endpoint::Connection;
use sshe_protocol::{Request, Response, read_frame, write_frame};
use tokio::net::UnixStream;

/// Drops peers outside `[peers]`; otherwise answers one RPC on one stream.
pub(crate) async fn serve_peer(conn: Connection, node: &Node<'_>) -> Result<()> {
    if !node.config.peers.values().any(|p| p.id == conn.remote_id()) {
        conn.close(1u8.into(), b"unauthorized");
        return Ok(());
    }
    let (mut send, mut recv) = within(CONNECT_TIMEOUT, async {
        conn.accept_bi().await.map_err(transport)
    })
    .await?;
    let request: Request = within(CONNECT_TIMEOUT, read_frame(&mut recv)).await?;
    let reply = response(dispatch(node, request).await);
    within(CONNECT_TIMEOUT, write_frame(&mut send, &reply)).await?;
    send.finish().map_err(transport)?;
    // Let the caller read the reply and close first; give up quietly if it never does.
    let _ = tokio::time::timeout(CONNECT_TIMEOUT, conn.closed()).await;
    Ok(())
}

/// Answers even when not admitted, so a refused exec is not reported as outcome unknown.
pub(crate) async fn serve_local(
    mut stream: UnixStream,
    node: &Node<'_>,
    admitted: bool,
) -> Result<()> {
    let message: LocalRequest = within(CONNECT_TIMEOUT, read_frame(&mut stream)).await?;
    let result = if !admitted {
        Err(Error::Invalid("daemon busy; request not executed".into()))
    } else if let Some(alias) = message.target {
        forward(node, &alias, &message.request).await
    } else {
        dispatch(node, message.request).await
    };
    within(CONNECT_TIMEOUT, write_frame(&mut stream, &response(result))).await
}

async fn forward(node: &Node<'_>, alias: &str, request: &Request) -> Result<Response> {
    let peer = node.config.peer(alias)?;
    let ep = node
        .endpoint
        .ok_or_else(|| Error::Invalid("forwarding requires an endpoint".into()))?;
    call(ep, peer.id, request).await
}
