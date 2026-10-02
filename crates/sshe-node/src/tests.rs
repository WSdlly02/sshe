use crate::{
    Error,
    config::{self, Config},
    dispatch::Node,
    identity::{load_key, lock},
    probe::check_peer,
    server::serve_peer,
    transport::call,
};
use iroh::{Endpoint, endpoint::presets};
use sshe_protocol::{ALPN, Request, Response, VERSION, read_frame};
use std::collections::BTreeMap;
use std::time::Duration;
use tokio::time::timeout;

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
    assert_eq!(cfg.daemon.interval_secs, 30);
    assert_eq!(cfg.probe.timeout_secs, 3);
    cfg.validate().unwrap();
    cfg.daemon.interval_secs = 1;
    assert!(cfg.validate().is_err());
    cfg.daemon.interval_secs = 30;
    cfg.probe.timeout_secs = 0;
    assert!(cfg.validate().is_err());
    assert!(toml::from_str::<Config>("identity = \"k\"\n[daemon]\nprobes = [\"bogus\"]").is_err());
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
            serve_peer(conn, &node).await.unwrap();
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
