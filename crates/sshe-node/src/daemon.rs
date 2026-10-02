//! Daemon lifecycle: socket setup, admission, task supervision and shutdown.
use crate::{
    Error, Result, config,
    config::Config,
    dispatch::Node,
    error::transport,
    history::History,
    identity,
    layout::Layout,
    scheduler, server,
    transport::{CONNECT_TIMEOUT, endpoint, within},
};
use iroh::{Endpoint, endpoint::Incoming};
use std::{
    os::unix::fs::{FileTypeExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::{
    net::{UnixListener, UnixStream},
    signal::unix::{SignalKind, signal},
    sync::{OwnedSemaphorePermit, RwLock, Semaphore},
    task::JoinSet,
};

/// Owned state shared by request tasks and the scheduler.
struct Daemon {
    config: Config,
    ep: Endpoint,
    history: RwLock<History>,
}

impl Daemon {
    fn node(&self) -> Node<'_> {
        Node {
            config: &self.config,
            id: self.ep.id().to_string(),
            endpoint: Some(&self.ep),
            history: Some(&self.history),
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
    let (listener, _socket_guard) = bind_socket(&Layout::new(path).socket).await?;
    let ep = endpoint(identity::load_key(&config.identity)?).await?;
    let permits = Arc::new(Semaphore::new(config.daemon.max_concurrent));
    let state = Arc::new(Daemon {
        history: RwLock::new(History::new(config.daemon.history_size)),
        config,
        ep: ep.clone(),
    });
    let mut tasks = JoinSet::new();
    let scheduled = state.clone();
    tasks.spawn(async move { scheduler::run(&scheduled.node(), &scheduled.history).await });
    eprintln!("sshe daemon {} (config changes require restart)", ep.id());
    let mut terminate = signal(SignalKind::terminate())?;
    let result = loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break Ok(()),
            _ = terminate.recv() => break Ok(()),
            Some(joined) = tasks.join_next() => {
                if let Err(error) = joined {
                    eprintln!("task failed: {error}");
                }
            }
            incoming = ep.accept() => {
                let Some(incoming) = incoming else { break Ok(()) };
                match permits.clone().try_acquire_owned() {
                    Ok(permit) => { tasks.spawn(handle_peer(state.clone(), incoming, permit)); }
                    Err(_) => incoming.refuse(),
                }
            }
            accepted = listener.accept() => {
                let (stream, _) = match accepted {
                    Ok(pair) => pair,
                    Err(e) => break Err(e.into()),
                };
                let permit = permits.clone().try_acquire_owned().ok();
                tasks.spawn(handle_local(state.clone(), stream, permit));
            }
        }
    };
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    ep.close().await;
    result
}

/// Binds a private socket, replacing only a stale socket no daemon is listening on.
async fn bind_socket(socket: &Path) -> Result<(UnixListener, SocketGuard)> {
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

async fn handle_peer(state: Arc<Daemon>, incoming: Incoming, _permit: OwnedSemaphorePermit) {
    let result = async {
        let conn = within(CONNECT_TIMEOUT, async { incoming.await.map_err(transport) }).await?;
        server::serve_peer(conn, &state.node()).await
    }
    .await;
    if let Err(error) = result {
        eprintln!("RPC failed: {error}");
    }
}

/// `permit` is None when saturated; the request is then refused with a reply.
async fn handle_local(
    state: Arc<Daemon>,
    stream: UnixStream,
    permit: Option<OwnedSemaphorePermit>,
) {
    if let Err(error) = server::serve_local(stream, &state.node(), permit.is_some()).await {
        eprintln!("local RPC failed: {error}");
    }
}
