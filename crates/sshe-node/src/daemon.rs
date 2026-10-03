//! Daemon lifecycle: socket setup, admission, task supervision and shutdown.
//!
//! Admission has three separate limits, so neither strangers nor a busy peer can
//! lock the local CLI out:
//! - connection slots, held from an incoming handshake until the connection ends;
//! - peer request permits and local request permits, held while a request runs.
use crate::{
    Error, Result, config,
    config::Config,
    dispatch::Node,
    error::transport,
    identity,
    layout::Layout,
    sampling::Sampler,
    scheduler, server,
    transport::{Dialer, HANDSHAKE_TIMEOUT, endpoint, within},
};
use iroh::endpoint::Incoming;
use std::{
    os::unix::fs::{FileTypeExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::{
    net::{UnixListener, UnixStream},
    signal::unix::{SignalKind, signal},
    sync::{OwnedSemaphorePermit, Semaphore},
    task::JoinSet,
};

/// Concurrent incoming connections, handshakes included.
const MAX_CONNECTIONS: usize = 64;

/// Owned state shared by request tasks and the scheduler.
struct Daemon {
    config: Config,
    dialer: Dialer,
    sampler: Sampler,
    peer_permits: Semaphore,
    local_permits: Semaphore,
}

impl Daemon {
    fn node(&self) -> Node<'_> {
        Node {
            config: &self.config,
            id: self.dialer.endpoint().id().to_string(),
            dialer: Some(&self.dialer),
            sampler: Some(&self.sampler),
        }
    }
}

struct SocketGuard(PathBuf);
impl Drop for SocketGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

pub async fn daemon(path: &Path) -> Result<()> {
    let config = config::read(path)?;
    let _lock = identity::lock(&config.identity)?;
    let (listener, _socket_guard) = bind_socket(&Layout::new(path)).await?;
    let ep = endpoint(
        identity::load_key(&config.identity)?,
        config.daemon.bind_port,
    )
    .await?;
    let max = config.daemon.max_concurrent;
    let state = Arc::new(Daemon {
        sampler: Sampler::new(&config.daemon),
        peer_permits: Semaphore::new(max),
        local_permits: Semaphore::new(max),
        dialer: Dialer::new(ep.clone(), config.peers.values().map(|p| p.id)),
        config,
    });
    let connection_slots = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    // Local connections beyond this are dropped unread instead of queueing.
    let local_slots = Arc::new(Semaphore::new(2 * max));
    let mut tasks = JoinSet::new();
    let scheduled = state.clone();
    tasks.spawn(async move {
        scheduler::run(&scheduled.config, &scheduled.dialer, &scheduled.sampler).await
    });
    tracing::info!(id = %ep.id(), bind_port = ?state.config.daemon.bind_port,
        "daemon started; config changes require restart");
    let mut terminate = signal(SignalKind::terminate())?;
    let result = loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break Ok(()),
            _ = terminate.recv() => break Ok(()),
            Some(joined) = tasks.join_next() => {
                if let Err(error) = joined {
                    tracing::error!(%error, "daemon task failed");
                }
            }
            incoming = ep.accept() => {
                let Some(incoming) = incoming else { break Ok(()) };
                match connection_slots.clone().try_acquire_owned() {
                    Ok(slot) => { tasks.spawn(handle_peer(state.clone(), incoming, slot)); }
                    Err(_) => {
                        tracing::warn!("connection limit reached; refusing incoming handshake");
                        incoming.refuse();
                    }
                }
            }
            accepted = listener.accept() => {
                let (stream, _) = match accepted {
                    Ok(pair) => pair,
                    Err(e) => break Err(e.into()),
                };
                match local_slots.clone().try_acquire_owned() {
                    Ok(slot) => { tasks.spawn(handle_local(state.clone(), stream, slot)); }
                    Err(_) => tracing::warn!("too many local connections; dropping one unread"),
                }
            }
        }
    };
    tracing::info!("daemon stopping");
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    ep.close().await;
    result
}

/// Binds a private socket, replacing only a stale socket no daemon is listening on.
async fn bind_socket(layout: &Layout) -> Result<(UnixListener, SocketGuard)> {
    let socket = layout.socket.as_path();
    if !layout.socket_fits() {
        return Err(Error::Invalid(format!(
            "socket path {} is longer than 107 bytes; use a shorter config path",
            socket.display()
        )));
    }
    let parent = socket
        .parent()
        .ok_or_else(|| Error::Invalid("socket parent missing".into()))?;
    if std::fs::metadata(parent)?.permissions().mode() & 0o077 != 0 {
        return Err(Error::Invalid(
            "daemon config directory must be private (chmod 700)".into(),
        ));
    }
    match std::fs::symlink_metadata(socket) {
        Ok(meta) => {
            if !meta.file_type().is_socket() {
                return Err(Error::Invalid(
                    "refusing to remove non-socket at socket path".into(),
                ));
            }
            match UnixStream::connect(socket).await {
                Ok(_) => return Err(Error::Invalid("daemon socket is already in use".into())),
                Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => {}
                Err(e) => return Err(e.into()),
            }
            std::fs::remove_file(socket)?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let listener = UnixListener::bind(socket)?;
    let guard = SocketGuard(socket.to_path_buf());
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))?;
    Ok((listener, guard))
}

/// Failed handshakes are routine (stale or duplicated packets via a relay), so they
/// are logged at debug only.
async fn handle_peer(state: Arc<Daemon>, incoming: Incoming, _slot: OwnedSemaphorePermit) {
    let conn = match within(HANDSHAKE_TIMEOUT, async {
        incoming.await.map_err(transport)
    })
    .await
    {
        Ok(conn) => conn,
        Err(error) => {
            tracing::debug!(%error, "incoming handshake did not complete");
            return;
        }
    };
    server::serve_peer(conn, &state.node(), &state.peer_permits).await;
}

async fn handle_local(state: Arc<Daemon>, stream: UnixStream, _slot: OwnedSemaphorePermit) {
    if let Err(error) = server::serve_local(stream, &state.node(), &state.local_permits).await {
        tracing::warn!(%error, "local request failed");
    }
}
