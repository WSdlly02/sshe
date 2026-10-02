pub mod config;
pub mod history;
use config::Config;
use futures::future::join_all;
use history::History;
use iroh::{Endpoint, EndpointAddr, SecretKey, endpoint::presets};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sshe_protocol::{
    ALPN, Health, HistoryReport, MAX_EXEC_SECONDS, ProbeKind, ProbeReport, Record, Request,
    Response, VERSION, read_frame, write_frame,
};
use std::{collections::BTreeSet, path::Path, sync::Arc, time::Duration};
use tokio::{
    net::{UnixListener, UnixStream},
    sync::{RwLock, Semaphore},
    task::JoinSet,
    time::timeout,
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("config: {0}")]
    ConfigRead(#[from] toml::de::Error),
    #[error("config serialization: {0}")]
    ConfigWrite(#[from] toml::ser::Error),
    #[error(transparent)]
    Protocol(#[from] sshe_protocol::Error),
    #[error(transparent)]
    Core(#[from] sshe_core::Error),
    #[error("Iroh: {0}")]
    Transport(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("identity is already in use; use its running daemon: {0}")]
    IdentityBusy(#[source] std::fs::TryLockError),
    #[error("operation timed out")]
    Timeout,
    #[error("command outcome unknown after submission; not retried: {0}")]
    OutcomeUnknown(#[source] Box<Error>),
    #[error("{0}")]
    Invalid(String),
}
pub type Result<T> = std::result::Result<T, Error>;
fn transport(e: impl std::error::Error + Send + Sync + 'static) -> Error {
    Error::Transport(Box::new(e))
}

const CONNECT_SECS: u64 = 15;
/// Slack for framing and scheduling on top of bounded server work.
const MARGIN_SECS: u64 = 5;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(CONNECT_SECS);
/// The slowest server work is exec, which sshe-core bounds by MAX_EXEC_SECONDS.
const RPC_SECS: u64 = MAX_EXEC_SECONDS + MARGIN_SECS;
const RPC_TIMEOUT: Duration = Duration::from_secs(RPC_SECS);
/// CLI to daemon: covers the daemon's worst-case `call` (connect, open stream, RPC).
const LOCAL_TIMEOUT: Duration = Duration::from_secs(2 * CONNECT_SECS + RPC_SECS + MARGIN_SECS);
/// Relay paths can exceed probe.timeout_secs, so peer checks use the connect budget.
const PEER_PROBE_TIMEOUT: Duration = CONNECT_TIMEOUT;

#[derive(Serialize, Deserialize)]
struct LocalRequest {
    target: Option<String>,
    request: Request,
}

/// What a request may use on this side; absent parts make dependent requests fail.
struct Node<'a> {
    config: &'a Config,
    id: String,
    endpoint: Option<&'a Endpoint>,
    history: Option<&'a RwLock<History>>,
}

pub async fn endpoint(key: SecretKey) -> Result<Endpoint> {
    Endpoint::builder(presets::N0)
        .secret_key(key)
        .alpns(vec![ALPN.to_vec()])
        .bind()
        .await
        .map_err(transport)
}
async fn dispatch(node: &Node<'_>, request: Request) -> Result<Response> {
    Ok(match request {
        Request::Health => Response::Health(Health {
            endpoint_id: node.id.clone(),
            status: "ok".into(),
            version: VERSION.into(),
        }),
        Request::Probe { kind } => Response::Probe(ProbeReport {
            observer: node.id.clone(),
            kind,
            records: probe(node, kind).await?,
        }),
        Request::History { kind, about, limit } => {
            let history = node
                .history
                .ok_or_else(|| Error::Invalid("history requires a running daemon".into()))?;
            Response::History(HistoryReport {
                observer: node.id.clone(),
                queried_at: sshe_core::now(),
                records: history.read().await.query(kind, about.as_deref(), limit),
            })
        }
        Request::Exec {
            program,
            args,
            timeout_secs,
        } => Response::Exec(sshe_core::execute(&program, &args, timeout_secs).await?),
    })
}
fn response(result: Result<Response>) -> Response {
    result.unwrap_or_else(|e| Response::Error(e.to_string()))
}

async fn probe(node: &Node<'_>, kind: ProbeKind) -> Result<Vec<Record>> {
    let probe = &node.config.probe;
    Ok(match kind {
        ProbeKind::Host => sshe_core::host_probe(probe.timeout()).await,
        ProbeKind::Wan => sshe_core::network(&probe.wan, kind, probe.timeout()).await,
        ProbeKind::Lan => sshe_core::network(&probe.lan, kind, probe.timeout()).await,
        ProbeKind::Services => sshe_core::services(&probe.services, probe.timeout()).await,
        ProbeKind::Peers => {
            let ep = node
                .endpoint
                .ok_or_else(|| Error::Invalid("peer probe requires an endpoint".into()))?;
            join_all(
                node.config.peers.iter().map(|(alias, peer)| {
                    check_peer(ep, peer.id.to_string(), alias.clone(), peer.id)
                }),
            )
            .await
        }
    })
}
async fn check_peer(
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

pub async fn call(
    ep: &Endpoint,
    address: impl Into<EndpointAddr>,
    request: &Request,
) -> Result<Response> {
    let conn = timeout(CONNECT_TIMEOUT, ep.connect(address.into(), ALPN))
        .await
        .map_err(|_| Error::Timeout)?
        .map_err(transport)?;
    let result = async {
        let (mut send, mut recv) = timeout(CONNECT_TIMEOUT, conn.open_bi())
            .await
            .map_err(|_| Error::Timeout)?
            .map_err(transport)?;
        let operation = timeout(RPC_TIMEOUT, async {
            write_frame(&mut send, request).await?;
            send.finish().map_err(transport)?;
            Ok::<Response, Error>(read_frame(&mut recv).await?)
        })
        .await
        .map_err(|_| Error::Timeout)
        .and_then(|r| r);
        if matches!(request, Request::Exec { .. }) {
            operation.map_err(|e| Error::OutcomeUnknown(Box::new(e)))
        } else {
            operation
        }
    }
    .await;
    conn.close(0u8.into(), b"done");
    result
}

pub async fn invoke(path: &Path, target: Option<String>, request: Request) -> Result<Response> {
    let config = config::read(path)?;
    let target = target.filter(|t| t != "self");
    let needs_daemon = matches!(request, Request::History { .. });
    let needs_endpoint = matches!(
        request,
        Request::Probe {
            kind: ProbeKind::Peers
        }
    );
    // Local live probes and exec stay available even when the daemon is down.
    if target.is_none() && !needs_daemon && !needs_endpoint {
        let id = config::load_key(&config.identity)?.public().to_string();
        let node = Node {
            config: &config,
            id,
            endpoint: None,
            history: None,
        };
        return dispatch(&node, request).await;
    }
    match UnixStream::connect(config::socket_path(path)).await {
        Ok(mut stream) => {
            let is_exec = matches!(request, Request::Exec { .. });
            let result = timeout(LOCAL_TIMEOUT, async {
                write_frame(&mut stream, &LocalRequest { target, request }).await?;
                Ok::<Response, Error>(read_frame(&mut stream).await?)
            })
            .await
            .map_err(|_| Error::Timeout)
            .and_then(|r| r);
            if is_exec {
                result.map_err(|e| Error::OutcomeUnknown(Box::new(e)))
            } else {
                result
            }
        }
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            if target.is_none() && needs_daemon {
                return Err(Error::Invalid("history requires a running daemon".into()));
            }
            let peer = match &target {
                Some(alias) => Some(
                    config
                        .peers
                        .get(alias)
                        .ok_or_else(|| Error::Invalid(format!("unknown peer: {alias}")))?,
                ),
                None => None,
            };
            let _lock = config::lock_identity(&config)?;
            let ep = endpoint(config::load_key(&config.identity)?).await?;
            let result = match peer {
                Some(peer) => call(&ep, peer.id, &request).await,
                None => {
                    let node = Node {
                        config: &config,
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
        Err(e) => Err(e.into()),
    }
}

struct SocketGuard(std::path::PathBuf);
impl Drop for SocketGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Owned daemon state shared by request tasks and the scheduler.
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

pub async fn daemon(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let config = config::read(path)?;
    let _lock = config::lock_identity(&config)?;
    let socket = config::socket_path(path);
    let parent = socket
        .parent()
        .ok_or_else(|| Error::Invalid("socket parent missing".into()))?;
    if std::fs::metadata(parent)?.permissions().mode() & 0o077 != 0 {
        return Err(Error::Invalid(
            "daemon config directory must be private (chmod 700)".into(),
        ));
    }
    match std::fs::symlink_metadata(&socket) {
        Ok(meta) => {
            use std::os::unix::fs::FileTypeExt;
            if !meta.file_type().is_socket() {
                return Err(Error::Invalid(
                    "refusing to remove non-socket at socket path".into(),
                ));
            }
            match UnixStream::connect(&socket).await {
                Ok(_) => return Err(Error::Invalid("daemon socket is already in use".into())),
                Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => {}
                Err(e) => return Err(e.into()),
            }
            std::fs::remove_file(&socket)?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let listener = UnixListener::bind(&socket)?;
    let _socket_guard = SocketGuard(socket.clone());
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
    let ep = endpoint(config::load_key(&config.identity)?).await?;
    let permits = Arc::new(Semaphore::new(config.daemon.max_concurrent));
    let state = Arc::new(Daemon {
        history: RwLock::new(History::new(config.daemon.history_size)),
        config,
        ep: ep.clone(),
    });
    let mut tasks = JoinSet::new();
    tasks.spawn(schedule(state.clone()));
    eprintln!("sshe daemon {} (config changes require restart)", ep.id());
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let result = loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break Ok(()),
            _ = terminate.recv() => break Ok(()),
            Some(result) = tasks.join_next() => { if let Err(error) = result { eprintln!("task failed: {error}"); } },
            incoming = ep.accept() => {
                let Some(incoming) = incoming else { break Ok(()); };
                let Ok(permit) = permits.clone().try_acquire_owned() else { incoming.refuse(); continue; };
                let state = state.clone();
                tasks.spawn(async move {
                    let _permit = permit;
                    let result = async {
                        let conn = timeout(CONNECT_TIMEOUT, incoming).await.map_err(|_| Error::Timeout)?.map_err(transport)?;
                        serve_connection(conn, &state.node()).await
                    }.await;
                    if let Err(error) = result { eprintln!("RPC failed: {error}"); }
                });
            },
            accepted = listener.accept() => {
                let (mut stream, _) = match accepted { Ok(pair) => pair, Err(e) => break Err(e.into()) };
                let permit = permits.clone().try_acquire_owned().ok();
                let state = state.clone();
                tasks.spawn(async move {
                    let result = async {
                        let message: LocalRequest = timeout(CONNECT_TIMEOUT, read_frame(&mut stream)).await.map_err(|_| Error::Timeout)??;
                        // Reply instead of dropping, so a busy exec is not reported as outcome unknown.
                        let result = if permit.is_none() {
                            Err(Error::Invalid("daemon busy; request not executed".into()))
                        } else if let Some(alias) = message.target {
                            match state.config.peers.get(&alias) {
                                Some(peer) => call(&state.ep, peer.id, &message.request).await,
                                None => Err(Error::Invalid(format!("unknown peer: {alias}"))),
                            }
                        } else {
                            dispatch(&state.node(), message.request).await
                        };
                        timeout(CONNECT_TIMEOUT, write_frame(&mut stream, &response(result))).await.map_err(|_| Error::Timeout)??;
                        Ok::<(), Error>(())
                    }.await;
                    if let Err(error) = result { eprintln!("local RPC failed: {error}"); }
                });
            }
        }
    };
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    ep.close().await;
    result
}
async fn serve_connection(conn: iroh::endpoint::Connection, node: &Node<'_>) -> Result<()> {
    if !node.config.peers.values().any(|p| p.id == conn.remote_id()) {
        conn.close(1u8.into(), b"unauthorized");
        return Ok(());
    }
    let (mut send, mut recv) = timeout(CONNECT_TIMEOUT, conn.accept_bi())
        .await
        .map_err(|_| Error::Timeout)?
        .map_err(transport)?;
    let request: Request = timeout(CONNECT_TIMEOUT, read_frame(&mut recv))
        .await
        .map_err(|_| Error::Timeout)??;
    let reply = response(dispatch(node, request).await);
    timeout(CONNECT_TIMEOUT, write_frame(&mut send, &reply))
        .await
        .map_err(|_| Error::Timeout)??;
    send.finish().map_err(transport)?;
    let _ = timeout(CONNECT_TIMEOUT, conn.closed()).await;
    Ok::<(), Error>(())
}

/// Runs the configured probes each interval and records results; never serves requests.
async fn schedule(state: Arc<Daemon>) {
    let kinds: BTreeSet<ProbeKind> = state.config.daemon.probes.iter().copied().collect();
    let mut interval =
        tokio::time::interval(Duration::from_secs(state.config.daemon.interval_secs));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        let node = state.node();
        let rounds = join_all(kinds.iter().map(|&kind| probe(&node, kind))).await;
        let mut history = state.history.write().await;
        for round in rounds {
            match round {
                Ok(records) => records.into_iter().for_each(|r| history.insert(r)),
                Err(error) => eprintln!("scheduled probe failed: {error}"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    #[test]
    fn identity_persists_and_init_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let id = config::init(&path).unwrap();
        let cfg = config::read(&path).unwrap();
        assert_eq!(config::load_key(&cfg.identity).unwrap().public(), id);
        config::save(&path, &cfg).unwrap();
        let stored: Config = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(stored.identity.is_relative());
        assert_eq!(config::read(&path).unwrap().identity, cfg.identity);
        assert!(config::init(&path).is_err());
        assert_eq!(config::load_key(&cfg.identity).unwrap().public(), id);
        std::fs::write(&cfg.identity, b"broken").unwrap();
        assert!(config::load_key(&cfg.identity).is_err());
        assert!(config::validate_alias("self").is_err());
    }
    #[test]
    fn config_bounds_are_enforced() {
        let mut cfg: Config = toml::from_str("identity = \"k\"").unwrap();
        assert_eq!(cfg.daemon.interval_secs, 30);
        assert_eq!(cfg.probe.timeout_secs, 3);
        cfg.validate().unwrap();
        cfg.daemon.interval_secs = 1;
        assert!(cfg.validate().is_err());
        cfg.daemon.interval_secs = 30;
        cfg.probe.timeout_secs = 0;
        assert!(cfg.validate().is_err());
        assert!(
            toml::from_str::<Config>("identity = \"k\"\n[daemon]\nprobes = [\"bogus\"]").is_err()
        );
    }
    #[test]
    fn identity_lock_excludes_second_endpoint() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        config::init(&path).unwrap();
        let cfg = config::read(&path).unwrap();
        let guard = config::lock_identity(&cfg).unwrap();
        assert!(matches!(
            config::lock_identity(&cfg),
            Err(Error::IdentityBusy(_))
        ));
        drop(guard);
        assert!(config::lock_identity(&cfg).is_ok());
    }
    async fn test_endpoint() -> Endpoint {
        Endpoint::builder(presets::Minimal)
            .alpns(vec![ALPN.to_vec()])
            .bind()
            .await
            .unwrap()
    }
    #[tokio::test]
    async fn real_quic_health_exec_peer_probe_and_whitelist() {
        let server = test_endpoint().await;
        let client = test_endpoint().await;
        let stranger = test_endpoint().await;
        let config = Config {
            identity: "unused".into(),
            peers: BTreeMap::from([(
                "different-local-alias".into(),
                config::Peer { id: client.id() },
            )]),
            probe: Default::default(),
            daemon: Default::default(),
        };
        let incoming_ep = server.clone();
        let task = tokio::spawn(async move {
            let node = Node {
                config: &config,
                id: incoming_ep.id().to_string(),
                endpoint: Some(&incoming_ep),
                history: None,
            };
            for _ in 0..5 {
                let conn = incoming_ep.accept().await.unwrap().await.unwrap();
                serve_connection(conn, &node).await.unwrap();
            }
        });
        match call(&client, server.addr(), &Request::Health)
            .await
            .unwrap()
        {
            Response::Health(h) => {
                assert_eq!(h.endpoint_id, server.id().to_string());
                assert_eq!((h.status.as_str(), h.version.as_str()), ("ok", VERSION));
            }
            _ => panic!("wrong response"),
        }
        let response = call(
            &client,
            server.addr(),
            &Request::Exec {
                program: "printf".into(),
                args: vec!["%s".into(), "literal; $(false)".into()],
                timeout_secs: 2,
            },
        )
        .await
        .unwrap();
        match response {
            Response::Exec(r) => assert_eq!(r.stdout, b"literal; $(false)"),
            _ => panic!("wrong response"),
        }
        let history = Request::History {
            kind: None,
            about: None,
            limit: 10,
        };
        match call(&client, server.addr(), &history).await.unwrap() {
            Response::Error(e) => assert!(e.contains("running daemon")),
            _ => panic!("history without a store must be refused"),
        }
        assert!(
            call(&stranger, server.addr(), &Request::Health)
                .await
                .is_err()
        );
        let record = check_peer(&client, "server".into(), "srv".into(), server.addr()).await;
        assert!(record.success, "{:?}", record.error);
        assert_eq!(record.data.unwrap()["version"], VERSION);
        task.await.unwrap();
        client.close().await;
        stranger.close().await;
        server.close().await;
    }
    #[tokio::test]
    async fn disconnect_after_submission_is_unknown_and_not_replayed() {
        let server = test_endpoint().await;
        let client = test_endpoint().await;
        let ep = server.clone();
        let task = tokio::spawn(async move {
            let conn = ep.accept().await.unwrap().await.unwrap();
            let (_send, mut recv) = conn.accept_bi().await.unwrap();
            let _: Request = read_frame(&mut recv).await.unwrap();
            conn.close(2u8.into(), b"simulated loss after submission");
        });
        let result = call(
            &client,
            server.addr(),
            &Request::Exec {
                program: "true".into(),
                args: vec![],
                timeout_secs: 1,
            },
        )
        .await;
        assert!(matches!(result, Err(Error::OutcomeUnknown(_))));
        task.await.unwrap();
        assert!(
            timeout(Duration::from_millis(100), server.accept())
                .await
                .is_err()
        );
        client.close().await;
        server.close().await;
    }
}
