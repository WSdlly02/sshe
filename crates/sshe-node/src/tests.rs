use crate::{
    Error,
    config::{self, Config},
    dispatch::Node,
    identity::{load_key, lock},
    probe::check_peer,
    server::serve_peer,
    transport::{Dialer, fixed_port},
};
use futures::{StreamExt, stream::FuturesUnordered};
use iroh::{Endpoint, EndpointAddr, SecretKey, endpoint::presets};
use sshe_core::Classify;
use sshe_protocol::{ALPN, FailureKind, Request, Response, VERSION, read_frame, write_frame};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering::SeqCst},
    },
    time::Duration,
};
use tokio::{sync::Semaphore, task::JoinHandle, time::timeout};

#[test]
fn identity_persists_and_init_never_overwrites() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let id = config::init(&path).unwrap();
    let cfg = config::read(&path).unwrap();
    assert_eq!(load_key(&cfg.identity).unwrap().public(), id);
    config::save(&path, &cfg).unwrap();
    let stored: Config = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert!(stored.identity.is_relative());
    assert_eq!(config::read(&path).unwrap().identity, cfg.identity);
    assert!(config::init(&path).is_err());
    assert_eq!(load_key(&cfg.identity).unwrap().public(), id);
    std::fs::write(&cfg.identity, b"broken").unwrap();
    assert!(load_key(&cfg.identity).is_err());
    assert!(config::validate_alias("self").is_err());
}

#[test]
fn config_bounds_are_enforced() {
    let mut cfg: Config = toml::from_str("identity = \"k\"").unwrap();
    assert_eq!(cfg.daemon.interval_secs, 60);
    assert_eq!(cfg.probe.timeout_secs, 5);
    cfg.validate().unwrap();
    cfg.daemon.interval_secs = 1;
    assert!(cfg.validate().is_err());
    cfg.daemon.interval_secs = 30;
    cfg.probe.timeout_secs = 0;
    assert!(cfg.validate().is_err());
    assert!(toml::from_str::<Config>("identity = \"k\"\n[daemon]\nprobes = [\"bogus\"]").is_err());
    let peer: Config = toml::from_str(
        "identity = \"k\"\n[daemon]\nbind_port = 7777\n[peers.vps]\nid = \"d5976448ad557264453bc81d49987d8a4f9497f701f5d74c78277446c147b5eb\"\naddrs = [\"203.0.113.7:7777\"]",
    )
    .unwrap();
    assert_eq!(peer.daemon.bind_port, Some(7777));
    assert_eq!(peer.peers["vps"].addr().ip_addrs().count(), 1);
}

#[test]
fn identity_lock_excludes_second_endpoint() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    config::init(&path).unwrap();
    let cfg = config::read(&path).unwrap();
    let guard = lock(&cfg.identity).unwrap();
    assert!(matches!(lock(&cfg.identity), Err(Error::IdentityBusy(_))));
    drop(guard);
    assert!(lock(&cfg.identity).is_ok());
}

async fn test_endpoint() -> Endpoint {
    Endpoint::builder(presets::Minimal)
        .alpns(vec![ALPN.to_vec()])
        .bind()
        .await
        .unwrap()
}

#[tokio::test]
async fn fixed_port_binds_both_families() {
    let port = std::net::UdpSocket::bind("0.0.0.0:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let ep = fixed_port(Endpoint::builder(presets::Minimal), port)
        .unwrap()
        .bind()
        .await
        .unwrap();
    let sockets = ep.bound_sockets();
    assert!(sockets.iter().all(|s| s.port() == port), "{sockets:?}");
    assert!(sockets.iter().any(|s| s.is_ipv4()), "{sockets:?}");
    if std::net::UdpSocket::bind(("::", 0)).is_ok() {
        assert!(sockets.iter().any(|s| s.is_ipv6()), "{sockets:?}");
    }
    ep.close().await;
}

fn config_trusting(id: iroh::EndpointId) -> Config {
    Config {
        identity: "unused".into(),
        peers: BTreeMap::from([(
            "different-local-alias".into(),
            config::Peer { id, addrs: vec![] },
        )]),
        probe: Default::default(),
        daemon: Default::default(),
    }
}

/// Serves every incoming connection the way the daemon does; counts handshakes.
fn spawn_server(
    ep: Endpoint,
    config: Config,
    permits: usize,
) -> (Arc<AtomicUsize>, JoinHandle<()>) {
    let accepted = Arc::new(AtomicUsize::new(0));
    let count = accepted.clone();
    let task = tokio::spawn(async move {
        let permits = Semaphore::new(permits);
        let dialer = Dialer::new(ep.clone(), config.peers.values().map(|p| p.id));
        let node = Node {
            config: &config,
            id: ep.id().to_string(),
            dialer: Some(&dialer),
            sampler: None,
        };
        let mut conns = FuturesUnordered::new();
        loop {
            tokio::select! {
                incoming = ep.accept() => {
                    let Some(incoming) = incoming else { break };
                    count.fetch_add(1, SeqCst);
                    if let Ok(conn) = incoming.await {
                        conns.push(serve_peer(conn, &node, &permits));
                    }
                }
                Some(()) = conns.next(), if !conns.is_empty() => {}
            }
        }
    });
    (accepted, task)
}

#[tokio::test]
async fn rpcs_reuse_one_connection_and_whitelist_holds() {
    let server = test_endpoint().await;
    let client = test_endpoint().await;
    let stranger = test_endpoint().await;
    let (accepted, task) = spawn_server(server.clone(), config_trusting(client.id()), 16);
    let dialer = Dialer::new(client.clone(), [server.id()]);
    match dialer.call(server.addr(), &Request::Health).await.unwrap() {
        Response::Health(h) => {
            assert_eq!(h.endpoint_id, server.id().to_string());
            assert_eq!((h.status.as_str(), h.version.as_str()), ("ok", VERSION));
        }
        _ => panic!("wrong response"),
    }
    let exec = Request::Exec {
        program: "printf".into(),
        args: vec!["%s".into(), "literal; $(false)".into()],
        timeout_secs: 2,
    };
    match dialer.call(server.addr(), &exec).await.unwrap() {
        Response::Exec(r) => assert_eq!(r.stdout, b"literal; $(false)"),
        _ => panic!("wrong response"),
    }
    let history = Request::History {
        kind: None,
        about: None,
        limit: 10,
    };
    match dialer.call(server.addr(), &history).await.unwrap() {
        Response::Error(e) => assert!(e.contains("running daemon")),
        _ => panic!("history without a store must be refused"),
    }
    let record = check_peer(
        &dialer,
        "srv".into(),
        server.addr(),
        Duration::from_secs(15),
    )
    .await;
    assert!(record.success, "{:?}", record.error);
    assert_eq!(record.data.unwrap()["version"], VERSION);
    assert_eq!(accepted.load(SeqCst), 1, "all RPCs share one connection");

    let error = Dialer::new(stranger.clone(), [server.id()])
        .call(server.addr(), &Request::Health)
        .await
        .unwrap_err();
    assert_eq!(error.failure_kind(), FailureKind::Unauthorized, "{error}");
    for ep in [client, stranger, server] {
        ep.close().await;
    }
    task.await.unwrap();
}

#[tokio::test]
async fn saturated_server_replies_busy_instead_of_dropping() {
    let server = test_endpoint().await;
    let client = test_endpoint().await;
    let (_, task) = spawn_server(server.clone(), config_trusting(client.id()), 0);
    match Dialer::new(client.clone(), [server.id()])
        .call(server.addr(), &Request::Health)
        .await
        .unwrap()
    {
        Response::Error(e) => assert!(e.contains("busy"), "{e}"),
        _ => panic!("expected a busy reply"),
    }
    client.close().await;
    server.close().await;
    task.await.unwrap();
}

#[tokio::test]
async fn unknown_address_is_classified() {
    let client = test_endpoint().await;
    let nowhere = EndpointAddr::from_parts(SecretKey::generate().public(), []);
    let record = check_peer(
        &Dialer::new(client.clone(), [nowhere.id]),
        "ghost".into(),
        nowhere,
        Duration::from_secs(15),
    )
    .await;
    assert!(!record.success);
    assert_eq!(
        record.error_kind,
        Some(FailureKind::NoAddress),
        "{:?}",
        record.error
    );
    client.close().await;
}

#[tokio::test]
async fn closed_cached_connection_is_replaced() {
    let server = test_endpoint().await;
    let client = test_endpoint().await;
    let ep = server.clone();
    // First connection answers once and closes; the second is served normally.
    let task = tokio::spawn(async move {
        let conn = ep.accept().await.unwrap().await.unwrap();
        let (mut send, mut recv) = conn.accept_bi().await.unwrap();
        let _: Request = read_frame(&mut recv).await.unwrap();
        let health = Response::Error("first connection".into());
        write_frame(&mut send, &health).await.unwrap();
        send.finish().unwrap();
        // Simulate a peer restart: let the reply land, then drop the connection.
        send.stopped().await.ok();
        conn.close(0u8.into(), b"restarting");
        let config = config_trusting(conn.remote_id());
        let node = Node {
            config: &config,
            id: ep.id().to_string(),
            dialer: None,
            sampler: None,
        };
        let conn = ep.accept().await.unwrap().await.unwrap();
        serve_peer(conn, &node, &Semaphore::new(1)).await;
    });
    let dialer = Dialer::new(client.clone(), [server.id()]);
    assert!(matches!(
        dialer.call(server.addr(), &Request::Health).await,
        Ok(Response::Error(_))
    ));
    // The server is gone from that connection; give the close a moment to arrive.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(matches!(
        dialer.call(server.addr(), &Request::Health).await,
        Ok(Response::Health(_))
    ));
    client.close().await;
    task.await.unwrap();
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
    let result = Dialer::new(client.clone(), [server.id()])
        .call(
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

#[tokio::test]
async fn local_probe_uses_daemon_history_and_health_does_not_sample() {
    use crate::{dispatch::dispatch, sampling::Sampler, server::serve_local};
    use sshe_protocol::ProbeKind;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    config::init(&path).unwrap();
    let config = config::read(&path).unwrap();
    let ep = test_endpoint().await;
    let dialer = Dialer::new(ep.clone(), []);
    let sampler = Sampler::new(&config.daemon);
    let node = Node {
        config: &config,
        id: ep.id().to_string(),
        dialer: Some(&dialer),
        sampler: Some(&sampler),
    };
    let listener = tokio::net::UnixListener::bind(path.with_extension("sock")).unwrap();
    let server = async {
        let (stream, _) = listener.accept().await.unwrap();
        serve_local(stream, &node, &Semaphore::new(1))
            .await
            .unwrap();
    };
    let client = crate::invoke(
        &path,
        None,
        Request::Probe {
            kind: ProbeKind::Host,
        },
    );
    let (response, ()) = timeout(Duration::from_secs(5), async {
        tokio::join!(client, server)
    })
    .await
    .unwrap();
    let Response::Probe(report) = response.unwrap() else {
        panic!("expected probe")
    };
    assert!(!report.records.is_empty());
    // Host isn't scheduled by default, but manual results still belong to this daemon.
    assert!(!config.daemon.probes.contains(&ProbeKind::Host));
    let history = sampler
        .history
        .read()
        .await
        .query(Some(ProbeKind::Host), None, 100);
    assert_eq!(
        serde_json::to_value(&report.records).unwrap(),
        serde_json::to_value(history).unwrap()
    );
    let next = *sampler.deadline(ProbeKind::Host).borrow();
    dispatch(&node, Request::Health).await.unwrap();
    assert_eq!(*sampler.deadline(ProbeKind::Host).borrow(), next);
    assert!(sampler.deadline(ProbeKind::Peers).borrow().is_none());
    drop(listener);
    std::fs::remove_file(path.with_extension("sock")).unwrap();
    // Without a daemon, local probes still work without acquiring an Endpoint lock.
    let _lock = lock(&config.identity).unwrap();
    assert!(matches!(
        crate::invoke(
            &path,
            None,
            Request::Probe {
                kind: ProbeKind::Host
            }
        )
        .await,
        Ok(Response::Probe(_))
    ));
    assert!(
        crate::invoke(
            &path,
            None,
            Request::History {
                kind: None,
                about: None,
                limit: 10
            }
        )
        .await
        .is_err()
    );
    ep.close().await;
}

#[tokio::test]
async fn remote_manual_probe_is_recorded_by_the_observer() {
    use crate::sampling::Sampler;
    use sshe_protocol::ProbeKind;
    let server = test_endpoint().await;
    let client = test_endpoint().await;
    let incoming = server.clone();
    let client_id = client.id();
    let task = tokio::spawn(async move {
        let config = config_trusting(client_id);
        let sampler = Sampler::new(&config.daemon);
        let dialer = Dialer::new(incoming.clone(), [client_id]);
        let node = Node {
            config: &config,
            id: incoming.id().to_string(),
            dialer: Some(&dialer),
            sampler: Some(&sampler),
        };
        let conn = incoming.accept().await.unwrap().await.unwrap();
        serve_peer(conn, &node, &Semaphore::new(16)).await;
    });
    let dialer = Dialer::new(client.clone(), [server.id()]);
    let Response::Probe(probe) = dialer
        .call(
            server.addr(),
            &Request::Probe {
                kind: ProbeKind::Host,
            },
        )
        .await
        .unwrap()
    else {
        panic!("expected probe")
    };
    let Response::History(history) = dialer
        .call(
            server.addr(),
            &Request::History {
                kind: Some(ProbeKind::Host),
                about: None,
                limit: 100,
            },
        )
        .await
        .unwrap()
    else {
        panic!("expected history")
    };
    assert_eq!(probe.observer, server.id().to_string());
    assert_eq!(history.observer, probe.observer);
    assert!(!history.records.is_empty());
    assert_eq!(
        serde_json::to_value(probe.records).unwrap(),
        serde_json::to_value(history.records).unwrap()
    );
    client.close().await;
    task.await.unwrap();
    server.close().await;
}
