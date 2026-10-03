use super::*;
use iroh::endpoint::presets;
use std::sync::Arc;

#[tokio::test]
async fn scheduler_observes_manual_resets_without_enrolling_unscheduled_kinds() {
    let ep = Endpoint::builder(presets::Minimal).bind().await.unwrap();
    let mut config: Config = toml::from_str("identity = 'unused'").unwrap();
    config.daemon.probes = vec![ProbeKind::Wan];
    config.probe.wan.domains.clear();
    config.probe.wan.tcp.clear();
    let config = Arc::new(config);
    let sampler = Arc::new(Sampler::new(&config.daemon));
    let dialer = Arc::new(Dialer::new(ep.clone(), []));
    tokio::time::pause();
    let mut updates = sampler.deadline(ProbeKind::Wan);
    let task = {
        let config = config.clone();
        let sampler = sampler.clone();
        let dialer = dialer.clone();
        tokio::spawn(async move { run_kind(&config, &dialer, &sampler, ProbeKind::Wan).await })
    };
    updates.changed().await.unwrap();
    let start = updates.borrow_and_update().unwrap() - Duration::from_secs(60);
    assert_eq!(
        *sampler.deadline(ProbeKind::Wan).borrow(),
        Some(start + Duration::from_secs(60))
    );
    tokio::time::advance(Duration::from_secs(45)).await;
    sampler.run(&config, &dialer, ProbeKind::Wan).await.unwrap();
    sampler
        .run(&config, &dialer, ProbeKind::Services)
        .await
        .unwrap();
    let services_due = *sampler.deadline(ProbeKind::Services).borrow();
    updates.borrow_and_update();
    tokio::time::advance(Duration::from_secs(15)).await;
    tokio::task::yield_now().await;
    assert_eq!(
        *sampler.deadline(ProbeKind::Wan).borrow(),
        Some(start + Duration::from_secs(105))
    );
    tokio::time::advance(Duration::from_secs(45)).await;
    updates.changed().await.unwrap();
    // Tokio rounds sleep deadlines to its millisecond timer resolution.
    let due = sampler.deadline(ProbeKind::Wan).borrow().unwrap();
    let expected = start + Duration::from_secs(165);
    assert!(
        due >= expected && due <= expected + Duration::from_millis(5),
        "{due:?}"
    );
    assert_eq!(
        *sampler.deadline(ProbeKind::Services).borrow(),
        services_due
    );
    task.abort();
    let _ = task.await;
    tokio::time::resume();
    ep.close().await;
}
