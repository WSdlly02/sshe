//! Iroh endpoint, RPC over reused connections, and the timeouts every caller shares.
use crate::{
    Error, Result,
    error::{connect_failed, connection_lost, transport},
};
use iroh::{
    Endpoint, EndpointAddr, EndpointId, SecretKey,
    endpoint::{BindOpts, Builder, Connection, presets},
};
use sshe_core::Classify;
use sshe_protocol::{
    ALPN, FailureKind, MAX_EXEC_SECONDS, Request, Response, read_frame, write_frame,
};
use std::{
    collections::HashMap,
    net::{Ipv4Addr, Ipv6Addr},
    time::Duration,
};
use tokio::{sync::Mutex, time::Instant};

/// iroh's own bound on a handshake is the 30s QUIC idle timeout; match it.
const DIAL_SECS: u64 = 30;
/// Opening a stream, or reading or writing a request frame.
const IO_SECS: u64 = 15;
/// Slack for framing and scheduling on top of bounded server work.
const MARGIN_SECS: u64 = 5;
/// The slowest server work is exec, which sshe-core bounds by MAX_EXEC_SECONDS.
const RPC_SECS: u64 = MAX_EXEC_SECONDS + MARGIN_SECS;

pub(crate) const DIAL_TIMEOUT: Duration = Duration::from_secs(DIAL_SECS);
pub(crate) const IO_TIMEOUT: Duration = Duration::from_secs(IO_SECS);
/// Server side: an incoming handshake must finish within this.
pub(crate) const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);
const RPC_TIMEOUT: Duration = Duration::from_secs(RPC_SECS);
/// One deadline for queueing, dialing, opening streams, RPC and any retry.
const CALL_TIMEOUT: Duration = Duration::from_secs(DIAL_SECS + IO_SECS + RPC_SECS);
/// Leave the daemon time to return a response before the CLI gives up.
pub(crate) const LOCAL_TIMEOUT: Duration =
    CALL_TIMEOUT.saturating_add(Duration::from_secs(MARGIN_SECS));

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

/// Each phase keeps its own limit but cannot extend the original call deadline.
async fn within_deadline<T, E: Into<Error>>(
    deadline: Instant,
    limit: Duration,
    f: impl Future<Output = std::result::Result<T, E>>,
) -> Result<T> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    // Tokio polls a ready future before checking its timer. Do not start work
    // at all when the budget has already expired.
    if remaining.is_zero() {
        return Err(Error::Timeout);
    }
    within(limit.min(remaining), f).await
}

/// An exec that failed after submission may still have run, so it must not look retryable.
pub(crate) fn unknown_if_exec(request: &Request, result: Result<Response>) -> Result<Response> {
    if is_exec(request) {
        result.map_err(|e| Error::OutcomeUnknown(Box::new(e)))
    } else {
        result
    }
}

fn is_exec(request: &Request) -> bool {
    matches!(request, Request::Exec { .. })
}

pub(crate) async fn endpoint(key: SecretKey, bind_port: Option<u16>) -> Result<Endpoint> {
    let mut builder = Endpoint::builder(presets::N0)
        .secret_key(key)
        .alpns(vec![ALPN.to_vec()]);
    if let Some(port) = bind_port {
        builder = fixed_port(builder, port)?;
    }
    builder.bind().await.map_err(transport)
}

/// Replaces both default sockets. The IPv6 one is v6-only, so they share the port;
/// like iroh's default, a host without IPv6 still starts.
pub(crate) fn fixed_port(builder: Builder, port: u16) -> Result<Builder> {
    builder
        .bind_addr((Ipv4Addr::UNSPECIFIED, port))
        .and_then(|b| {
            b.bind_addr_with_opts(
                (Ipv6Addr::UNSPECIFIED, port),
                BindOpts::default().set_is_required(false),
            )
        })
        .map_err(transport)
}

/// Keeps one outgoing connection per configured peer and reuses it while it lives.
/// Dials only when a request needs it; nothing reconnects in the background.
pub(crate) struct Dialer {
    ep: Endpoint,
    /// Empty until the first request to that peer. Each peer has its own lock, so a
    /// slow dial to one never holds up another, and concurrent callers for the same
    /// peer wait for a single dial.
    peers: HashMap<EndpointId, Mutex<Option<Connection>>>,
}

/// Where an exchange failed; only failures before submission are safe to retry for exec.
struct Failed {
    submitted: bool,
    error: Error,
}

impl Dialer {
    pub(crate) fn new(ep: Endpoint, peers: impl IntoIterator<Item = EndpointId>) -> Self {
        Self {
            ep,
            peers: peers.into_iter().map(|id| (id, Mutex::default())).collect(),
        }
    }

    pub(crate) fn endpoint(&self) -> &Endpoint {
        &self.ep
    }

    /// One RPC. A reused connection that turns out to be dead is replaced once,
    /// unless that could run an exec twice.
    pub(crate) async fn call(&self, addr: EndpointAddr, request: &Request) -> Result<Response> {
        self.call_until(addr, request, Instant::now() + CALL_TIMEOUT)
            .await
    }

    async fn call_until(
        &self,
        addr: EndpointAddr,
        request: &Request,
        deadline: Instant,
    ) -> Result<Response> {
        let (conn, reused) = self.connection(addr.clone(), deadline).await?;
        let result = match exchange(&conn, request, deadline).await {
            Err(failed)
                if reused
                    && (!failed.submitted || !is_exec(request))
                    && failed.error.failure_kind() == FailureKind::ConnectionLost
                    && conn.close_reason().is_some() =>
            {
                // A stream reset alone must never tear down other RPCs.
                let (conn, _) = self.connection(addr, deadline).await?;
                exchange(&conn, request, deadline).await
            }
            other => other,
        };
        match result {
            Ok(response) => Ok(response),
            Err(Failed {
                submitted: true,
                error,
            }) => unknown_if_exec(request, Err(error)),
            Err(Failed { error, .. }) => Err(error),
        }
    }

    /// Queueing for this peer and dialing share the same phase budget.
    async fn connection(
        &self,
        addr: EndpointAddr,
        deadline: Instant,
    ) -> Result<(Connection, bool)> {
        within_deadline(deadline, DIAL_TIMEOUT, async {
            let mut cached = self
                .peers
                .get(&addr.id)
                .ok_or_else(|| Error::Invalid(format!("{} is not a configured peer", addr.id)))?
                .lock()
                .await;
            if let Some(conn) = cached.as_ref().filter(|c| c.close_reason().is_none()) {
                return Ok((conn.clone(), true));
            }
            let conn = self.ep.connect(addr, ALPN).await.map_err(connect_failed)?;
            *cached = Some(conn.clone());
            Ok::<_, Error>((conn, false))
        })
        .await
    }
}

async fn exchange(
    conn: &Connection,
    request: &Request,
    deadline: Instant,
) -> std::result::Result<Response, Failed> {
    let (mut send, mut recv) = within_deadline(deadline, IO_TIMEOUT, async {
        conn.open_bi().await.map_err(connection_lost)
    })
    .await
    .map_err(|error| Failed {
        submitted: false,
        error,
    })?;
    let mut submitted = false;
    within_deadline(deadline, RPC_TIMEOUT, async {
        submitted = true;
        write_frame(&mut send, request).await?;
        send.finish().map_err(transport)?;
        Ok::<_, Error>(read_frame(&mut recv).await?)
    })
    .await
    .map_err(|error| Failed { submitted, error })
}

#[cfg(test)]
mod tests;
