//! Iroh endpoint, one-shot RPC calls and the timeouts every caller shares.
use crate::{Error, Result, error::transport};
use iroh::{Endpoint, EndpointAddr, SecretKey, endpoint::presets};
use sshe_protocol::{ALPN, MAX_EXEC_SECONDS, Request, Response, read_frame, write_frame};
use std::time::Duration;

const CONNECT_SECS: u64 = 15;
/// Slack for framing and scheduling on top of bounded server work.
const MARGIN_SECS: u64 = 5;
/// The slowest server work is exec, which sshe-core bounds by MAX_EXEC_SECONDS.
const RPC_SECS: u64 = MAX_EXEC_SECONDS + MARGIN_SECS;

pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(CONNECT_SECS);
const RPC_TIMEOUT: Duration = Duration::from_secs(RPC_SECS);
/// CLI to daemon: covers the daemon's worst-case `call` (connect, open stream, RPC).
pub(crate) const LOCAL_TIMEOUT: Duration =
    Duration::from_secs(2 * CONNECT_SECS + RPC_SECS + MARGIN_SECS);

/// Bounds `f` by `limit`, reporting expiry as `Error::Timeout`.
pub(crate) async fn within<T, E: Into<Error>>(
    limit: Duration,
    f: impl Future<Output = std::result::Result<T, E>>,
) -> Result<T> {
    tokio::time::timeout(limit, f)
        .await
        .map_err(|_| Error::Timeout)?
        .map_err(Into::into)
}

/// An exec that failed after submission may still have run, so it must not look retryable.
pub(crate) fn unknown_if_exec(request: &Request, result: Result<Response>) -> Result<Response> {
    if matches!(request, Request::Exec { .. }) {
        result.map_err(|e| Error::OutcomeUnknown(Box::new(e)))
    } else {
        result
    }
}

pub(crate) async fn endpoint(key: SecretKey) -> Result<Endpoint> {
    Endpoint::builder(presets::N0)
        .secret_key(key)
        .alpns(vec![ALPN.to_vec()])
        .bind()
        .await
        .map_err(transport)
}

/// One RPC on a fresh connection; the connection is closed afterwards.
pub(crate) async fn call(
    ep: &Endpoint,
    address: impl Into<EndpointAddr>,
    request: &Request,
) -> Result<Response> {
    let conn = within(CONNECT_TIMEOUT, async {
        ep.connect(address.into(), ALPN).await.map_err(transport)
    })
    .await?;
    let result = async {
        let (mut send, mut recv) = within(CONNECT_TIMEOUT, async {
            conn.open_bi().await.map_err(transport)
        })
        .await?;
        let reply = within(RPC_TIMEOUT, async {
            write_frame(&mut send, request).await?;
            send.finish().map_err(transport)?;
            Ok::<_, Error>(read_frame(&mut recv).await?)
        })
        .await;
        unknown_if_exec(request, reply)
    }
    .await;
    conn.close(0u8.into(), b"done");
    result
}
