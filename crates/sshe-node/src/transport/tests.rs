use super::*;
use std::sync::Arc;
use tokio::{sync::oneshot, time::timeout};

async fn endpoint() -> Endpoint {
    Endpoint::builder(presets::Minimal)
        .alpns(vec![ALPN.to_vec()])
        .bind()
        .await
        .unwrap()
}

#[tokio::test]
async fn queued_exec_expires_without_dialing_or_submitting() {
    let server = endpoint().await;
    let client = endpoint().await;
    let dialer = Dialer::new(client.clone(), [server.id()]);
    let guard = dialer.peers[&server.id()].lock().await;
    let request = Request::Exec {
        program: "true".into(),
        args: vec![],
        timeout_secs: 1,
    };
    let result = timeout(
        Duration::from_secs(2),
        dialer.call_until(
            server.addr(),
            &request,
            Instant::now() + Duration::from_millis(50),
        ),
    )
    .await
    .expect("waiting for the peer lock must honor the call deadline");
    assert!(matches!(result, Err(Error::Timeout)), "{result:?}");
    drop(guard);
    assert!(
        timeout(Duration::from_millis(50), server.accept())
            .await
            .is_err()
    );
    client.close().await;
    server.close().await;
}

#[tokio::test]
async fn expired_deadline_does_not_poll_ready_work() {
    let mut started = false;
    let result = within_deadline(Instant::now(), IO_TIMEOUT, async {
        started = true;
        Ok::<_, Error>(())
    })
    .await;
    assert!(matches!(result, Err(Error::Timeout)));
    assert!(!started);
}

#[tokio::test]
async fn submitted_exec_deadline_is_unknown_and_not_retried() {
    let server = endpoint().await;
    let client = endpoint().await;
    let conn = client.connect(server.addr(), ALPN);
    let incoming = async { server.accept().await.unwrap().await.unwrap() };
    let (conn, remote) = tokio::join!(conn, incoming);
    let dialer = Arc::new(Dialer::new(client.clone(), [server.id()]));
    *dialer.peers[&server.id()].lock().await = Some(conn.unwrap());
    let caller = dialer.clone();
    let addr = server.addr();
    let call = tokio::spawn(async move {
        caller
            .call_until(
                addr,
                &Request::Exec {
                    program: "true".into(),
                    args: vec![],
                    timeout_secs: 1,
                },
                Instant::now() + Duration::from_secs(1),
            )
            .await
    });
    let (_send, mut recv) = remote.accept_bi().await.unwrap();
    let request: Request = read_frame(&mut recv).await.unwrap();
    assert!(matches!(request, Request::Exec { .. }));
    let result = timeout(Duration::from_secs(3), call)
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(result, Err(Error::OutcomeUnknown(ref e)) if matches!(**e, Error::Timeout)),
        "{result:?}"
    );
    assert!(remote.close_reason().is_none());
    assert!(
        timeout(Duration::from_millis(50), remote.accept_bi())
            .await
            .is_err()
    );
    client.close().await;
    server.close().await;
}

#[tokio::test]
async fn reconnect_keeps_original_deadline() {
    let server = endpoint().await;
    let client = endpoint().await;
    let (conn, remote) = tokio::join!(client.connect(server.addr(), ALPN), async {
        server.accept().await.unwrap().await.unwrap()
    },);
    let dialer = Arc::new(Dialer::new(client.clone(), [server.id()]));
    *dialer.peers[&server.id()].lock().await = Some(conn.unwrap());
    let deadline = Instant::now() + Duration::from_secs(1);
    let caller = dialer.clone();
    let addr = server.addr();
    let call =
        tokio::spawn(async move { caller.call_until(addr, &Request::Health, deadline).await });
    let (_send, mut recv) = remote.accept_bi().await.unwrap();
    let _: Request = read_frame(&mut recv).await.unwrap();
    // Consume part of the original budget before forcing the safe retry.
    tokio::time::sleep(Duration::from_millis(300)).await;
    remote.close(0u8.into(), b"restarting");
    let replacement = timeout(Duration::from_secs(2), async {
        server.accept().await.unwrap().await.unwrap()
    })
    .await
    .unwrap();
    let (_send, mut recv) = replacement.accept_bi().await.unwrap();
    let _: Request = read_frame(&mut recv).await.unwrap();
    let result = tokio::time::timeout_at(deadline + Duration::from_millis(150), call)
        .await
        .expect("retry must not receive a fresh budget")
        .unwrap();
    assert!(matches!(result, Err(Error::Timeout)), "{result:?}");
    client.close().await;
    server.close().await;
}

#[tokio::test]
async fn stream_reset_preserves_other_rpcs_and_cached_connection() {
    let server = endpoint().await;
    let client = endpoint().await;
    let ep = server.clone();
    let (started_tx, started_rx) = oneshot::channel();
    let (resume_tx, resume_rx) = oneshot::channel();
    let task = tokio::spawn(async move {
        let conn = ep.accept().await.unwrap().await.unwrap();
        let (mut exec_send, mut exec_recv) = conn.accept_bi().await.unwrap();
        let request: Request = read_frame(&mut exec_recv).await.unwrap();
        assert!(matches!(request, Request::Exec { .. }));
        started_tx.send(()).unwrap();
        let (mut probe_send, mut probe_recv) = conn.accept_bi().await.unwrap();
        let _: Request = read_frame(&mut probe_recv).await.unwrap();
        probe_send.reset(77u8.into()).unwrap();
        resume_rx.await.unwrap();
        assert!(conn.close_reason().is_none());
        write_frame(
            &mut exec_send,
            &Response::Exec(sshe_protocol::ExecResult {
                stdout: b"done".to_vec(),
                stderr: vec![],
                exit_code: Some(0),
                signal: None,
                duration_ms: 1,
            }),
        )
        .await
        .unwrap();
        exec_send.finish().unwrap();
        let (mut send, mut recv) = conn.accept_bi().await.unwrap();
        let _: Request = read_frame(&mut recv).await.unwrap();
        write_frame(&mut send, &Response::Error("same connection".into()))
            .await
            .unwrap();
        send.finish().unwrap();
        send.stopped().await.ok();
    });
    let dialer = Arc::new(Dialer::new(client.clone(), [server.id()]));
    let caller = dialer.clone();
    let addr = server.addr();
    let exec = tokio::spawn(async move {
        caller
            .call(
                addr,
                &Request::Exec {
                    program: "true".into(),
                    args: vec![],
                    timeout_secs: 1,
                },
            )
            .await
    });
    started_rx.await.unwrap();
    let result = timeout(
        Duration::from_secs(2),
        dialer.call(server.addr(), &Request::Health),
    )
    .await
    .expect("a reset stream must not cause a redial");
    assert!(matches!(result, Err(ref e) if e.failure_kind() == FailureKind::ConnectionLost));
    resume_tx.send(()).unwrap();
    assert!(matches!(exec.await.unwrap(), Ok(Response::Exec(r)) if r.stdout == b"done"));
    assert!(matches!(dialer.call(server.addr(), &Request::Health).await,
        Ok(Response::Error(e)) if e == "same connection"));
    task.await.unwrap();
    client.close().await;
    server.close().await;
}
