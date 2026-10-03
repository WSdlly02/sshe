use super::*;
use sshe_core::Classify;
use sshe_protocol::FailureKind;
use tokio::time::{advance, sleep};

fn sampler() -> Sampler {
    Sampler::new(&DaemonConfig::default())
}

fn record(kind: ProbeKind, at: u64, success: bool) -> Record {
    Record {
        kind,
        target: "target".into(),
        label: None,
        method: "test".into(),
        observed_at: at,
        duration_ms: 1,
        success,
        error: (!success).then(|| "timeout".into()),
        error_kind: (!success).then_some(FailureKind::Timeout),
        last_success_at: success.then_some(at),
        data: None,
    }
}

#[tokio::test(start_paused = true)]
async fn manual_and_scheduled_callers_share_one_round_and_one_history_write() {
    let sampler = sampler();
    let start = Instant::now();
    let work = async {
        sleep(Duration::from_secs(5)).await;
        Ok(vec![record(ProbeKind::Wan, 1, true)])
    };
    let (first, scheduled, manual) = tokio::join!(
        sampler.sample(ProbeKind::Wan, false, work),
        sampler.sample(ProbeKind::Wan, true, async {
            panic!("duplicate scheduled probe")
        }),
        sampler.sample(ProbeKind::Wan, false, async {
            panic!("duplicate manual probe")
        }),
    );
    let expected = serde_json::to_value(first.unwrap()).unwrap();
    assert_eq!(serde_json::to_value(scheduled.unwrap()).unwrap(), expected);
    assert_eq!(serde_json::to_value(manual.unwrap()).unwrap(), expected);
    let history = sampler.history.read().await.query(None, None, 10);
    assert_eq!(serde_json::to_value(history).unwrap(), expected);
    assert_eq!(
        *sampler.deadline(ProbeKind::Wan).borrow(),
        Some(start + Duration::from_secs(65))
    );
}

#[tokio::test(start_paused = true)]
async fn manual_completion_resets_only_its_kind_and_failed_checks_are_recorded() {
    let sampler = sampler();
    let start = Instant::now();
    for kind in [ProbeKind::Wan, ProbeKind::Peers] {
        sampler
            .sample(kind, false, async { Ok(vec![record(kind, 1, true)]) })
            .await
            .unwrap();
    }
    advance(Duration::from_secs(45)).await;
    let failed = sampler
        .sample(ProbeKind::Wan, false, async {
            Ok(vec![record(ProbeKind::Wan, 46, false)])
        })
        .await
        .unwrap();
    assert_eq!(failed[0].last_success_at, Some(1));
    assert_eq!(
        *sampler.deadline(ProbeKind::Wan).borrow(),
        Some(start + Duration::from_secs(105))
    );
    assert_eq!(
        *sampler.deadline(ProbeKind::Peers).borrow(),
        Some(start + Duration::from_secs(60))
    );
    advance(Duration::from_secs(15)).await;
    let skipped = sampler
        .sample(ProbeKind::Wan, true, async { panic!("old timer fired") })
        .await
        .unwrap();
    assert!(skipped.is_empty());
    sampler
        .sample(ProbeKind::Peers, true, async {
            Ok(vec![record(ProbeKind::Peers, 61, true)])
        })
        .await
        .unwrap();
    assert_eq!(
        sampler
            .history
            .read()
            .await
            .query(Some(ProbeKind::Wan), None, 10)
            .len(),
        2
    );
    assert_eq!(
        sampler
            .history
            .read()
            .await
            .query(Some(ProbeKind::Peers), None, 10)
            .len(),
        2
    );
}

#[tokio::test(start_paused = true)]
async fn different_kinds_run_concurrently() {
    let sampler = sampler();
    let start = Instant::now();
    let (slow, fast) = tokio::join!(
        sampler.sample(ProbeKind::Peers, false, async {
            sleep(Duration::from_secs(15)).await;
            Ok(vec![record(ProbeKind::Peers, 15, false)])
        }),
        sampler.sample(ProbeKind::Wan, false, async {
            Ok(vec![record(ProbeKind::Wan, 1, true)])
        }),
    );
    slow.unwrap();
    fast.unwrap();
    assert_eq!(
        *sampler.deadline(ProbeKind::Wan).borrow(),
        Some(start + Duration::from_secs(60))
    );
    assert_eq!(
        *sampler.deadline(ProbeKind::Peers).borrow(),
        Some(start + Duration::from_secs(75))
    );
}

#[tokio::test(start_paused = true)]
async fn errors_are_shared_and_rescheduled_without_a_busy_loop() {
    let sampler = sampler();
    let (a, b) = tokio::join!(
        sampler.sample(ProbeKind::Wan, true, async {
            sleep(Duration::from_secs(1)).await;
            Err(Error::Timeout)
        }),
        sampler.sample(ProbeKind::Wan, false, async { panic!("duplicate probe") }),
    );
    assert_eq!(a.unwrap_err().failure_kind(), FailureKind::Timeout);
    assert_eq!(b.unwrap_err().failure_kind(), FailureKind::Timeout);
    assert!(sampler.deadline(ProbeKind::Wan).borrow().unwrap() > Instant::now());
}

#[tokio::test]
async fn cancelled_leader_releases_waiters_and_allows_a_new_round() {
    let sampler = sampler();
    let mut leader = Box::pin(sampler.sample(ProbeKind::Wan, false, std::future::pending()));
    assert!(futures::poll!(&mut leader).is_pending());
    let mut follower =
        Box::pin(sampler.sample(ProbeKind::Wan, false, async { panic!("duplicate probe") }));
    assert!(futures::poll!(&mut follower).is_pending());
    drop(leader);
    assert!(matches!(follower.await, Err(Error::ProbeCancelled)));
    let records = sampler
        .sample(ProbeKind::Wan, false, async {
            Ok(vec![record(ProbeKind::Wan, 1, true)])
        })
        .await
        .unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(sampler.history.read().await.query(None, None, 10).len(), 1);
}
