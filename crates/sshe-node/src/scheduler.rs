//! Runs only configured probe kinds; each has an independent completion-based timer.
use crate::{config::Config, sampling::Sampler, transport::Dialer};
use futures::future::join_all;
use iroh::Endpoint;
use sshe_protocol::ProbeKind;
use std::{collections::BTreeSet, time::Duration};
use tokio::time::{Instant, sleep_until};

/// iroh suggests waiting about one net report (10s) for `online`.
const ONLINE_WAIT: Duration = Duration::from_secs(10);

pub(crate) async fn run(config: &Config, dialer: &Dialer, sampler: &Sampler) {
    wait_online(dialer.endpoint()).await;
    let kinds: BTreeSet<_> = config.daemon.probes.iter().copied().collect();
    join_all(
        kinds
            .into_iter()
            .map(|kind| run_kind(config, dialer, sampler, kind)),
    )
    .await;
}

async fn run_kind(config: &Config, dialer: &Dialer, sampler: &Sampler, kind: ProbeKind) {
    let mut next = sampler.deadline(kind);
    loop {
        let due = next.borrow_and_update().unwrap_or_else(Instant::now);
        tokio::select! {
            changed = next.changed() => {
                if changed.is_err() { return; }
            }
            _ = sleep_until(due) => {
                // Recheck under the sampling lock: a manual probe may have just
                // completed while this timer was becoming ready.
                if let Err(error) = sampler.run_due(config, dialer, kind).await {
                    tracing::error!(kind = kind.as_str(), %error, "scheduled probe failed");
                }
            }
        }
    }
}

/// Without a relay, peers may not find this node yet; probing still goes ahead,
/// since direct addresses and local checks need no relay. Manual requests are
/// served meanwhile, and the deadlines they set are kept.
async fn wait_online(ep: &Endpoint) {
    match tokio::time::timeout(ONLINE_WAIT, ep.online()).await {
        Ok(()) => tracing::info!("endpoint online"),
        Err(_) => tracing::warn!(
            wait_secs = ONLINE_WAIT.as_secs(),
            "no relay reachable; probing anyway, peers may not find this node yet"
        ),
    }
}

#[cfg(test)]
mod tests;
